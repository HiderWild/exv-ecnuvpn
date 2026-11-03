
//! engine 数据面真实组装（R1b A1a）：认证 → CSTP → 特权初始化 → 数据面，**全部在
//! engine 进程内**，StatusPublisher 推真实状态里程碑。
//!
//! 本模块是 acceptance `engine::RealNativeOps`（阶段 3b）的产品移植——复用
//! `exv-vpn-cstp`（WebvpnLogin / CstpSession）+ `crate::platform_tunnel`（特权初始化/
//! teardown）+ `crate::data_plane`（ring→CSTP→TLS worker），不依赖 acceptance crate
//! （test-only）。
//!
//! **R0 归因铁律**：三个验收组装大等待（6s/8s sleep、10s DAD poll）不带入；apply 段
//! 降到真实 API 值（≈0.3s，R0 §4）。
//!
//! [`TunnelRuntime`] 是注入 seam（契约测试注入 fake，生产注入 [`RealTunnelRuntime`]）：
//! - [`TunnelRuntime::start_apply`]：A1b 异步形态——后台组装逐段执行后返回；逐段经
//!   [`StatusEvent`] 推真实状态里程碑（ConnectingControl → ApplyingPlatformTunnel →
//!   StartingDataPlane → Connected / Failed）。
//! - [`TunnelRuntime::disconnect`]：**减负断开**（D13）——session owner 断 VPN 连接 +
//!   route owner 清路由（含地址/on-link）+ **Paused 暂停标记**（网卡保留，D12）。
//! - [`TunnelRuntime::teardown`]：**退出清理**——断开 + NIC owner 关 adapter（0 网卡
//!   残留兜底；有界 join + 超时兜底由调用方/协调者编排，D15）。
//!
//! ## S1.5 三 owner + 单协调者（D11）
//!
//! - **session owner**（`LiveTunnel`）：持 CstpSession + 登录/CSTP/TLS 通道；join 既有
//!   双数据面线程（reader/writer 保持独立）。连接序负责协商 offer。
//! - **NIC owner**（`crate::platform_tunnel::NicOwner`）：adapter 句柄 + WintunLibrary +
//!   session 生命周期；**首次连接建、断开不拆、退出清理阶段 close**（D12）。
//! - **route owner**（`crate::platform_tunnel::RouteOwner`）：路由 + DNS + 地址族 apply，
//!   经 NIC 交出的 LUID 驱动（D17）；断开序清路由/地址/DNS（D13）。
//! - **单协调者** = 本模块 [`RealTunnelRuntime`]：三张顺序表落纸——
//!   连接序：session 协商 offer → NIC 确保/复用 adapter → route 应用地址/DNS/路由 →
//!   **屏障**（all-or-nothing，D16）→ 数据面启动；
//!   断开序：协调者编排 session-end → route 清 → NIC 暂停（保留 adapter）；
//!   退出序：协调者收三 owner → 有界 join（D15）→ adapter close。
//!
//! **Connected 必须来自真实数据面就绪**：`data_plane::spawn_data_plane` 成功返回后
//! 才推 Connected——记账式 set_phase 已退役（grpc_server 不再凭空推占位信号）。

pub(crate) mod lifecycle;

use crate::stop_report::{StopAction, StopActionState, StopObligation, StopReport};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_cstp::connector::{BootstrapConfig, TrustPolicy};
use exv_vpn_cstp::session::{CstpControlEvent, CstpSession, SessionError};
use exv_vpn_cstp::webvpn::{LoginError, WebvpnLogin};
use exv_vpn_win32_config::{ConnectionMode, Protocol};
use exv_vpn_wire::generated;
use generated::ConnectPhase;
use tokio::sync::mpsc;

use crate::connect_ledger::{AttemptEnd, LedgerDomain, TraceAdmission};
use crate::data_plane::{
    DataPlaneAux, EngineDataPlane, EngineDataPlaneThreads, LatencyProbeConfig, WINTUN_RING_CAPACITY,
};
use crate::log_sink::{LogLevel, LogSink};
use crate::platform_tunnel::{NicOwner, PlatformFacts, RouteCleanupObligations, RouteOwner};
use crate::secret_payload::ConnectionCredentials;
use crate::status::{StatusEvent, StatusPublisher};
use crate::system_proxy_journal::SystemProxyJournal;
use crate::vgdc_connect::{
    GatewayResolutionObserver, connected_socket_fields, production_gateway_resolver_observed,
    socket_binder_for_mode,
};

/// CSTP 控制面端口（标准 HTTPS/CSTP 端口；协议帧未携带端口字段）。
const CSTP_GATEWAY_PORT: u16 = 443;

// ---------------------------------------------------------------------------
// T1 延迟探测（latency design v2）：DPD RTT 探索性优先 → ping fallback。
// ---------------------------------------------------------------------------
// 探测循环在数据面 writer 线程内（`data_plane::ProbeState`），本模块在拿到真实
// CSTP offer 后构建 [`LatencyProbeConfig`] 并随 `DataPlaneAux` 注入。
//   * DPD：探索性，开关 [`DPD_PROBE_ENABLED`]（默认关）；学校 ASA 是否应答未验证，
//     真机验证属 S5。
//   * ping fallback：每 [`LATENCY_PING_INTERVAL_SECS`]（3 分钟）1 次，目标 = 客户端
//     隧道子网网络基地址（`network_base`；真实学校网关地址 S5 校准）。
//   * 手动立即刷新：前端写 `config_dir()/latency_refresh`（[`LATENCY_REFRESH_FILE`]，
//     内容 = epoch 毫秒），探测循环每秒轮询一次，发现新值立即 ping。
// wire/UI 零变更：延迟经既有 `StatsRegistry.latency_ms` → `StatsEvent` →
// `RuntimeSnapshot.stats` 到达前端。

/// T1：DPD 探测开关（探索性，默认关——学校 ASA 应答未验证）。
const DPD_PROBE_ENABLED: bool = false;
/// T1：ping fallback 探测周期（秒；每 3 分钟 1 次，服务端负荷最小）。
const LATENCY_PING_INTERVAL_SECS: u64 = 180;
/// T1：手动刷新标记文件名。tauri 前端「立即刷新延迟」写 `config_dir()/latency_refresh`
/// （内容 = epoch 毫秒），engine 探测循环轮询到新值即立即 ping。名字与 tauri 侧
/// `app/src/kernel/latency.rs` 的常量保持一致（两侧独立硬编码，因 tauri 不依赖
/// engine/config crate）。
pub const LATENCY_REFRESH_FILE: &str = "latency_refresh";

/// 客户端隧道子网的网络基地址（探测目标默认值；`addr` 按 `prefix` 掩码清零主机位）。
#[must_use]
fn network_base(addr: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = if prefix >= 32 {
        u32::MAX
    } else {
        u32::MAX << (32 - prefix)
    };
    Ipv4Addr::from(u32::from(addr) & mask)
}

/// 组装请求上下文（一次连接的一次性数据；凭据消费后由调用方/运行时零化）。
pub struct ApplyContext {
    /// Core 冻结的本次逻辑会话模式；自动重连沿用，不读取实时设置来覆盖。
    pub connection_mode: ConnectionMode,
    /// wire 传入的 domain 隧道计划（`opaque_intent` 用于关联；真实 apply 以 CSTP
    /// offer 为准——address/prefix/mtu/dns/routes 来自真实协商）。
    pub plan: exv_vpn_domain::ports::TunnelPlan,
    /// 一次性协议认证材料；Cisco 密码或 aTrust 浏览器候选会话。
    /// `None` = 未提供凭据（真实运行时 fail closed；fake 容忍）。
    pub credentials: Option<ConnectionCredentials>,
    /// apply operation_id（status 事件关联）。
    pub operation_id: Vec<u8>,
    /// 状态推送端点（服务共享的 StatusPublisher）。
    pub status: Arc<StatusPublisher>,
    /// 统计发布器（粗粒度阶段由真实结果驱动；A1b 终态经此设置）。
    pub stats: Arc<crate::stats::StatsPublisher>,
    /// 结构化日志 sink（诊断文案；状态语义一律走 status 通道，D3 铁律）。
    pub log: Arc<LogSink>,
}

/// 用实际 CSTP socket 的端点查询当前出口，不以临时登录连接或另一张物理网卡代替。
fn observed_cstp_bypass(
    gateway: std::net::SocketAddr,
    endpoints: (std::net::SocketAddr, std::net::SocketAddr),
    own_ifindex: Option<u32>,
) -> Result<(exv_vpn_win32_resource::routes::RouteRow, u32), TunnelError> {
    let (local, peer) = endpoints;
    let error = |detail| TunnelError::plain(ConnectPhase::ConnectingControl, detail);
    if peer != gateway {
        return Err(error(
            "engine: CSTP peer differs from resolved gateway".to_string(),
        ));
    }
    let (std::net::IpAddr::V4(local_ip), std::net::IpAddr::V4(gateway_ip)) =
        (local.ip(), peer.ip())
    else {
        return Err(error(
            "engine: CSTP egress requires IPv4 endpoints".to_string(),
        ));
    };
    let (row, ifindex) =
        exv_vpn_win32_resource::routes::observe_gateway_egress(gateway_ip, local_ip)
            .map_err(|native| error(format!("engine: CSTP egress query:{native:?}")))?;
    if own_ifindex == Some(ifindex) {
        return Err(error(
            "engine: gateway route points into EXV's own tunnel; disconnect and retry".to_string(),
        ));
    }
    Ok((row, ifindex))
}

/// 组装失败的受控错误：携带失败阶段 + domain 错误码 + 可读 detail，直接映射为
/// status 通道的 Failed 事件（`VpnError`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelError {
    /// 语义分类：决定 wire error code 映射。
    pub kind: TunnelErrorKind,
    /// 失败发生的细粒度连接阶段（status 事件携带）。
    pub connect_phase: ConnectPhase,
    /// SAML domain 错误码；Platform/PlatformDependency 时为 Win32 原始错误码。
    pub code: u32,
    /// 网关 `a0` 结果码（登录被拒时：`a0=15` 真凭据错；非登录失败恒 0）。
    pub a0: u32,
    /// 可读错误详情（typed 变体名，绝不含 secret/cookie）。
    pub detail: String,
    /// 稳定失败原因 id（阶段② reason 透传；`<域>/<细分>` 小写，空 = 缺省，落
    /// T-22 泛化文案）。词表与文案单一来源见前端 `lib/copy-tables.ts` 与壳
    /// `wire.rs` 的 reason 文案表（两表注释互指 + 一致性测试钉住）。
    /// ConnectingControl 段的 `plain()` 群与出口探查（`observed_cstp_bypass`）
    /// 刻意维持空 reason——细分不入 v1 词表，泛化句已可行动。
    pub reason: &'static str,
}

/// SAML 要求的内部标记码（仅存于 `TunnelError.code`，供日志与测试判别；与 wire
/// 判别值无关——wire SAML 码自 EXV_UNFREEZE 2026-10-09 起为 20，见
/// `tunnel_error_to_wire` 的 `WIRE_SAML_REQUIRED`）。MVP 不支持交互式 SAML，
/// engine 诚实标记。
const SAML_MARKER_CODE: u32 = 1;
/// wire `ErrorStage::Ingress` 判别值。
const ERROR_STAGE_INGRESS: i32 = 1;
/// wire `RetryAdvice::DoNotRetry` 判别值。
const RETRY_ADVICE_DO_NOT_RETRY: i32 = 1;

/// `TunnelError` 语义分类：精确映射到 wire error code，避免所有非 SAML 失败
/// 一律映射为 `Unauthorized(15)` 的误导性行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelErrorKind {
    /// 重复连接（已有活跃组装/隧道，`assembling||is_connected` 拒绝）。
    DuplicateConnect,
    /// 凭据缺失（`credentials` 为 `None`）。
    MissingCredentials,
    /// 登录被网关拒绝（`a0` 非 0，唯一真实的认证拒绝）。
    LoginRejected,
    /// 登录返回 SAML 要求（MVP 不支持交互式 SAML）。
    LoginSaml,
    /// 传输层失败（CSTP session/TLS/网关不可达）。
    Transport,
    /// 平台/配置失败（特权初始化、路由、数据面、config 读取等）。
    Platform,
    /// 登录前发现本地网络组件不可用，且安装副本恢复失败。
    PlatformDependency,
}

/// `TunnelError` → wire `VpnError`（status 通道 Failed 事件）。可读 detail 仅本地
/// 记录，绝不落 wire（spec §10：wire 只带结构化字段）；稳定原因以 `reason` id
/// 透传（EXV_UNFREEZE 2026-10-09，tag 8），壳侧按 id 映射文案。
///
/// 精确映射表（按 `TunnelErrorKind` 判别）：
/// - `DuplicateConnect` → `CONNECT_IN_PROGRESS(3)`
/// - `MissingCredentials` → `INVALID_INPUT(1)`
/// - `LoginRejected` → `UNAUTHORIZED(15)`（唯一真实的认证拒绝）
/// - `LoginSaml` → `SAML_REQUIRED(20)`（EXV_UNFREEZE 2026-10-09 修撞码：旧值 1
///   与 `INVALID_INPUT` 同值，SAML 场景误显「输入无效」）
/// - `Transport` → `DEADLINE_EXCEEDED(16)`
/// - `Platform` → `EFFECT_UNKNOWN(13)`
pub(crate) fn tunnel_error_to_wire(err: &TunnelError) -> generated::VpnError {
    // wire code 判别值（common.proto 定义）。
    const WIRE_CONNECT_IN_PROGRESS: i32 = 3;
    const WIRE_INVALID_INPUT: i32 = 1;
    const WIRE_UNAUTHORIZED: i32 = 15;
    const WIRE_SAML_REQUIRED: i32 = 20;
    const WIRE_DEADLINE_EXCEEDED: i32 = 16;
    const WIRE_EFFECT_UNKNOWN: i32 = 13;

    let code = match err.kind {
        TunnelErrorKind::DuplicateConnect => WIRE_CONNECT_IN_PROGRESS,
        TunnelErrorKind::MissingCredentials => WIRE_INVALID_INPUT,
        TunnelErrorKind::LoginRejected => {
            if err.a0 != 0 {
                WIRE_UNAUTHORIZED
            } else {
                WIRE_EFFECT_UNKNOWN
            }
        }
        TunnelErrorKind::LoginSaml => WIRE_SAML_REQUIRED,
        TunnelErrorKind::Transport => WIRE_DEADLINE_EXCEEDED,
        TunnelErrorKind::Platform => WIRE_EFFECT_UNKNOWN,
        TunnelErrorKind::PlatformDependency => {
            generated::ErrorCode::PlatformDependencyUnavailable as i32
        }
    };
    tracing::warn!(
        phase = ?err.connect_phase,
        kind = ?err.kind,
        code,
        a0 = err.a0,
        reason = err.reason,
        "tunnel assembly failed"
    );
    generated::VpnError {
        code,
        // 稳定 reason id（tag 8，EXV_UNFREEZE 2026-10-09）：空串 = 缺省。
        reason: err.reason.to_string(),
        stage: if matches!(
            err.kind,
            TunnelErrorKind::PlatformDependency | TunnelErrorKind::Platform
        ) {
            if err.connect_phase == ConnectPhase::ObservingOwnedState {
                generated::ErrorStage::ObservingOwnedState as i32
            } else if err.connect_phase == ConnectPhase::ConnectingControl {
                generated::ErrorStage::ConnectingControl as i32
            } else if err.connect_phase == ConnectPhase::StartingDataPlane {
                generated::ErrorStage::StartingDataPlane as i32
            } else {
                generated::ErrorStage::ApplyingPlatformTunnel as i32
            }
        } else {
            ERROR_STAGE_INGRESS
        },
        certainty: if err.kind == TunnelErrorKind::PlatformDependency {
            if err.connect_phase == ConnectPhase::ObservingOwnedState {
                generated::EffectCertainty::NoEffect as i32
            } else {
                generated::EffectCertainty::Unknown as i32
            }
        } else {
            0
        },
        retry: RETRY_ADVICE_DO_NOT_RETRY,
        subject: None,
        resource: None,
        // 本机平台失败保留系统码；认证等既有映射仍携带网关 a0。
        native: Some(generated::RedactedNativeError {
            category: if err.kind == TunnelErrorKind::PlatformDependency {
                if err.code == 5 {
                    generated::NativeErrorCategory::Permission as i32
                } else {
                    generated::NativeErrorCategory::Storage as i32
                }
            } else if err.kind == TunnelErrorKind::Platform {
                generated::NativeErrorCategory::Resource as i32
            } else {
                generated::NativeErrorCategory::Transport as i32
            },
            namespace: if err.kind == TunnelErrorKind::PlatformDependency
                && err.code & 0xffff_0000 != 0
            {
                generated::NativeErrorNamespace::Hresult as i32
            } else {
                generated::NativeErrorNamespace::Win32 as i32
            },
            code: i64::from(
                if matches!(
                    err.kind,
                    TunnelErrorKind::PlatformDependency | TunnelErrorKind::Platform
                ) {
                    err.code
                } else {
                    err.a0
                },
            ),
        }),
        operation_detail: None,
    }
}

impl TunnelError {
    /// 认证阶段失败：从 `LoginError` 读网关 `a0` 结果码与 SAML 标记。
    /// reason 挂载条件（复审 2.1-5d）：`login_rejected` 仅在 `a0≠0`（网关明确
    /// 拒绝凭据）时挂——`a0=0` 时 wire code 为 EFFECT_UNKNOWN，若仍挂「登录被拒」
    /// 会与 code 语义相悖、误导文案。
    fn login(err: &LoginError) -> Self {
        let a0 = err
            .a0_result()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let (kind, code) = if matches!(err, LoginError::SamlRequired) {
            (TunnelErrorKind::LoginSaml, SAML_MARKER_CODE)
        } else {
            (TunnelErrorKind::LoginRejected, 0)
        };
        let reason = if matches!(err, LoginError::SamlRequired) {
            "auth/saml_required"
        } else if a0 != 0 {
            "auth/login_rejected"
        } else {
            ""
        };
        Self {
            kind,
            connect_phase: ConnectPhase::ConnectingControl,
            code,
            a0,
            detail: format!("engine: login:{err:?}"),
            reason,
        }
    }

    /// CSTP 会话阶段失败（无 a0）。
    fn session(err: SessionError) -> Self {
        Self {
            kind: TunnelErrorKind::Transport,
            connect_phase: ConnectPhase::ConnectingControl,
            code: 0,
            a0: 0,
            detail: format!("engine: connect-tunnel:{err:?}"),
            reason: "transport/session_failed",
        }
    }

    /// 其它失败（特权初始化 / 数据面；无 a0）。
    fn plain(connect_phase: ConnectPhase, detail: impl Into<String>) -> Self {
        Self {
            kind: TunnelErrorKind::Platform,
            connect_phase,
            code: 0,
            a0: 0,
            detail: detail.into(),
            reason: "",
        }
    }

    /// 带稳定 reason id 的受控失败（阶段② reason 透传；词表见
    /// `frontend/src/lib/copy-tables.ts`）。`platform/route_apply` 仅在无原始
    /// native 码时挂载（T-05 裁定：有 native 码时壳侧 1168/5 分档文案更具体，
    /// reason 让路——engine 侧条件挂载使壳侧优先级判定保持统一）。
    fn plain_with_reason(
        connect_phase: ConnectPhase,
        detail: impl Into<String>,
        reason: &'static str,
    ) -> Self {
        Self {
            reason,
            ..Self::plain(connect_phase, detail)
        }
    }
}

/// `start_apply` 启动决策（P2：已连接时采纳复用，不拒绝）。
///
/// 纯函数——按组装状态、live 状态与本次是否携带新凭据作出启动决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartDecision {
    /// 有在途组装（assembling=true）→ 拒绝，返回 `DuplicateConnect`（CONNECT_IN_PROGRESS）。
    RefuseDuplicate,
    /// 有完整 live 隧道且不在组装中（is_connected=true, assembling=false）→ 采纳：
    /// 不重建隧道，直接为本次 operation 发布 Connected 状态。
    Adopt,
    /// 无在途组装且无活跃隧道 → 正常进入组装流程。
    Proceed,
}

/// 决策逻辑（纯函数，可单测）。携带新凭据表示用户明确发起新连接，即使旧 live
/// 协议和连接模式相同也必须进入组装；没有新凭据时才允许接管现有会话。
#[inline]
fn decide_start_apply(
    is_assembling: bool,
    is_connected: bool,
    has_new_credentials: bool,
) -> StartDecision {
    if is_assembling {
        StartDecision::RefuseDuplicate
    } else if is_connected && !has_new_credentials {
        StartDecision::Adopt
    } else {
        StartDecision::Proceed
    }
}

/// 单次同步受理的事实回执，不存储为连接状态；Adopted 仅由实际 rebind 点产生。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyAcceptance {
    Started,
    Adopted,
}

/// 数据面组装 seam（注入点；契约测试注入 fake，生产注入 [`RealTunnelRuntime`]）。
///
/// A1b 异步形态：`start_apply` **受理即回**（后台组装 task 跑，逐段状态实时 post），
/// `cancel` 触发取消令牌令后台组装在段边界中断并自清理；`disconnect` 减负断开
/// （session-end + route 清 + NIC 暂停，网卡保留）；`teardown` 退出清理（断开 +
/// adapter close）。
pub trait TunnelRuntime: Send + Sync {
    /// 后台启动真实组装（A1b）：受理即回 pending，组装在后台 task 内逐段执行并
    /// 实时 post 状态（ConnectingControl → ApplyingPlatformTunnel → StartingDataPlane
    /// → Connected / Failed）。
    ///
    /// `self: Arc<Self>`——后台 thread 需持有运行时引用（组装跨调用存活）。
    ///
    /// # Errors
    ///
    /// 重复组装（已有活跃组装/隧道）→ [`TunnelError`]（duplicate connect）。组装
    /// **过程**中的失败不在此返回——经 `ctx.status` 以 Failed 事件上报。
    fn start_apply(self: Arc<Self>, ctx: ApplyContext) -> Result<(), TunnelError>;
    /// R1 受理返回发布游标；实现必须在自己的受理串行窗口内、任何发布前取得。
    /// 未提供此窗口的运行时不能启动一个无法关联完成的自愈尝试。
    fn start_reconnect(
        self: Arc<Self>,
        _ctx: ApplyContext,
        _attempt: u32,
    ) -> Result<u64, TunnelError> {
        Err(TunnelError {
            kind: TunnelErrorKind::Platform,
            connect_phase: ConnectPhase::ConnectingControl,
            code: 0,
            a0: 0,
            detail: "runtime does not expose reconnect admission".into(),
            reason: "",
        })
    }
    /// 传递门受理事实；默认替身沿用原入口，真实运行时在最终裁决后开户。
    fn start_apply_admitted(
        self: Arc<Self>,
        ctx: ApplyContext,
        _trace: TraceAdmission,
    ) -> Result<(), TunnelError> {
        self.start_apply(ctx)
    }
    /// 默认实现只承诺启动；不得根据调用后的瞬时 is_connected 猜测采纳。
    fn start_apply_with_receipt(
        self: Arc<Self>,
        ctx: ApplyContext,
        trace: TraceAdmission,
    ) -> Result<ApplyAcceptance, TunnelError> {
        self.start_apply_admitted(ctx, trace)
            .map(|()| ApplyAcceptance::Started)
    }
    /// 取消令牌：请求中止在途组装（StopTunnel）。协作式——后台组装在段边界检查并
    /// 自清理（登录后/CSTP 后/apply 后）。
    fn cancel(&self);
    /// **减负断开（D13）**：session owner 断 VPN 连接（stop_and_join + 结束 session +
    /// 关 CSTP 通道）+ route owner 清路由（地址/on-link/DNS）+ **Paused 暂停标记**
    /// （`is_paused()` 为 true）；NIC owner 保留 adapter（D12，网卡无地址惰性存续）。
    ///
    /// 流量门控复用既有 `stop_and_join`/session-end 机制，**不另造旗标门**（M13）——
    /// 新流量随 ring session 结束被拒、旧连接随 TLS 任务退出终止、路由清空。
    ///
    /// # Errors
    ///
    /// route 清硬失败 → `String`（调用方据情兜底；adapter 保留，退出清理仍会 close）。
    fn disconnect(&self) -> Result<(), String>;
    /// 本次实际停止及剩余资源义务；默认适配只根据运行时可观察事实收口。
    fn disconnect_report(&self) -> StopReport {
        let result = self.disconnect();
        let mut report = StopReport {
            stopped: !self.is_connected() && !self.is_assembling(),
            ..Default::default()
        };
        report.action(LedgerDomain::SessionLease, "runtime_disconnect", &result);
        report
    }
    /// **退出清理（D12/D15）**：断开 + NIC owner 关 adapter（adapter creator close
    /// 移除网卡，0 残留兜底）。有界 join 由调用方在断开后确保（worker 已 join）。
    ///
    /// # Errors
    ///
    /// teardown 硬失败 → `String`（调用方据情兜底）。
    fn teardown(&self) -> Result<(), String>;
    /// 当前是否已连接（数据面存活）。
    fn is_connected(&self) -> bool;
    /// 是否有在途组装（`start_apply` 已受理、未完成/未取消）。
    fn is_assembling(&self) -> bool;
    /// 是否处于 **Paused**（D13：减负断开后、下次连接前）。engine 运行时状态；host
    /// 侧映射见 kernel_control_service（Stopped + fine-phase）。
    fn is_paused(&self) -> bool;
}

/// 单次真实连接的日志身份；世代只在原有安装点分配，不提前预测。
#[derive(Clone)]
struct RuntimeDiagnostics {
    log: Arc<LogSink>,
    operation_id: String,
    generation: Option<u64>,
}

impl RuntimeDiagnostics {
    fn new(log: Arc<LogSink>, operation_id: &[u8]) -> Self {
        Self {
            log,
            operation_id: uuid::Uuid::from_slice(operation_id)
                .map_or_else(|_| "unknown".into(), |id| id.to_string()),
            generation: None,
        }
    }

    fn emit(
        &self,
        code: &str,
        reason: &str,
        outcome: &str,
        extra: impl IntoIterator<Item = (String, String)>,
    ) {
        let mut fields = crate::build_identity::fields();
        fields.extend([
            ("operation_id".into(), self.operation_id.clone()),
            (
                "tunnel_generation".into(),
                self.generation
                    .map_or_else(|| "unassigned".into(), |generation| generation.to_string()),
            ),
            ("reason".into(), reason.into()),
            ("outcome".into(), outcome.into()),
            (
                "event_observed_ms".into(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .to_string(),
            ),
        ]);
        fields.extend(extra);
        let refs: Vec<_> = fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        self.log.emit(
            LogLevel::Debug,
            "engine",
            code,
            "连接与清理路径的实际观测",
            &refs,
        );
    }
}

/// engine 持有的 CSTP 会话通道（保持 TLS 数据任务存活：`write_channel` sender +
/// `read_channel` receiver 任一 drop 都会让 `CstpSession::open` 内 spawn 的
/// TLS 读/写任务退出）。
///
/// 字段只写不读——本结构的作用是**持有**通道句柄以保持 TLS 任务存活，读取由数据面
/// 线程经克隆完成；`dead_code` 警告是有意的 lifetime-holding 模式。
#[allow(dead_code)]
struct EngineCstp {
    write_channel: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    read_channel: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>>,
    /// T1 latency probe：CSTP 控制面事件接收端（DPD response）。持有它保持通道
    /// 存活；drop（断开）后 cstp 读任务的控制发送静默失败（非致命）。
    control_rx: Option<Arc<Mutex<mpsc::UnboundedReceiver<CstpControlEvent>>>>,
    /// T3（F6/P1-1）：数据面掉线信号发送端的 **LiveTunnel 持有副本**——LiveTunnel
    /// drop 即关停 sweeper（覆盖 writer 线程卡死、其闭包副本永不释放的泄漏边界）。
    /// 与 writer 闭包内的发送端副本共同构成 sweeper 的终止条件（双份安放）。
    lost_tx: Option<std::sync::mpsc::Sender<DataPlaneLost>>,
    tasks: exv_vpn_cstp::session::CstpTasks,
    runtime: tokio::runtime::Handle,
}

impl EngineCstp {
    fn close(self) -> Result<(), String> {
        let Self {
            mut tasks, runtime, ..
        } = self;
        // RPC 经 spawn_blocking 调用；专用 runtime 驱动 TLS 任务，当前线程只等待回执。
        std::thread::spawn(move || runtime.block_on(tasks.close()))
            .join()
            .map_err(|_| "cstp_cleanup_worker_join".to_owned())?
    }
}

enum PreparedPackets {
    Cisco {
        write_channel: mpsc::UnboundedSender<Vec<u8>>,
        read_channel: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
        control_rx: Arc<Mutex<mpsc::UnboundedReceiver<CstpControlEvent>>>,
        latency_cfg: LatencyProbeConfig,
    },
    Atrust {
        channels: crate::atrust_tunnel::RawPacketChannels,
    },
}

/// 控制段取得结果；数据包通道与既有 per-connection owner 分开移交。
struct PreparedConnection {
    offer: exv_vpn_cstp::session::TunnelOffer,
    campus_dns_servers: Vec<String>,
    bypasses: Vec<exv_vpn_win32_resource::routes::RouteRow>,
    // 默认载体析构也必须先沿原 owner graceful 回滚，再释放宿主通道。
    control: AttemptOwner,
    packets: PreparedPackets,
}

/// 每个序号对应下面 LiveTunnel 的唯一资源槽，不复制句柄或清理权限。
#[derive(Default)]
struct AttemptBindings {
    id: Vec<u8>,
    trace: Option<(usize, String)>,
    session: Option<usize>,
    // bool 只表示本轮新取得 owner，不等同创建者 owned。
    nic: Option<(Option<usize>, bool)>,
    route: Option<usize>,
    data_plane: Option<usize>,
    nrpt: Option<usize>,
    registration_error: Option<TunnelError>,
    prior_results: crate::connect_ledger::RollbackOutcome,
}

fn settle_nic_attempt(
    acquired_this_attempt: bool,
    dependencies_pending: bool,
    action: impl FnOnce(bool) -> Result<(), String>,
) -> Result<(), String> {
    if acquired_this_attempt && dependencies_pending {
        return Err("engine: NIC retained because dependent cleanup failed".into());
    }
    action(acquired_this_attempt)
}

fn record_nic_creation(count: &std::sync::atomic::AtomicUsize, acquired: bool, created: bool) {
    if acquired && created {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 仅携带原资源边界已经观察的结果及访问失败来源，不推断未访问资源的状态。
#[derive(Default)]
struct PreviousCleanupObservation {
    outcome: crate::connect_ledger::RollbackOutcome,
    access_failure: Option<&'static str>,
}

impl PreviousCleanupObservation {
    fn access_failed(&mut self, source: &'static str) -> String {
        self.access_failure = Some(source);
        source.to_owned()
    }

    fn held(&mut self, domain: LedgerDomain, resource: &str) {
        self.outcome.observe(
            None,
            domain,
            Err(format!(
                "engine: previous {resource} retained; cleanup not started"
            )),
            true,
            "previous_resource_retained",
        );
    }
}

/// 所有退出（含 unwind）都保留同一个 owner，直到提交或统一回滚。
struct AttemptOwner {
    runtime: Arc<RealTunnelRuntime>,
    live: Option<LiveTunnel>,
}

impl AttemptOwner {
    fn new(runtime: &Arc<RealTunnelRuntime>, mut live: LiveTunnel, id: &[u8]) -> Self {
        let trace = runtime
            .attempt_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .as_ref()
            .filter(|ledger| ledger.attempt_id == id)
            .and_then(|ledger| {
                let connection_id = match &ledger.trace {
                    TraceAdmission::Created { connection_id }
                    | TraceAdmission::Replaced { connection_id }
                    | TraceAdmission::Existing { connection_id }
                    | TraceAdmission::Skipped {
                        connection_id: Some(connection_id),
                    } => connection_id,
                    TraceAdmission::Unwired
                    | TraceAdmission::Skipped {
                        connection_id: None,
                    } => return None,
                };
                ledger
                    .entries
                    .iter()
                    .find(|e| e.domain == LedgerDomain::Trace)
                    .map(|e| (e.sequence, connection_id.clone()))
            });
        live.attempt.get_or_insert_with(|| AttemptBindings {
            id: id.to_vec(),
            trace,
            ..Default::default()
        });
        Self {
            runtime: runtime.clone(),
            live: Some(live),
        }
    }
    fn into_live(mut self) -> LiveTunnel {
        self.live.take().expect("attempt owner held")
    }
    fn register(&mut self, domain: LedgerDomain, persistent: bool) -> Result<usize, TunnelError> {
        let id = self.attempt.as_ref().expect("attempt bindings").id.clone();
        self.runtime.record_acquired(&id, domain, persistent)
    }
    fn check_registration(&mut self) -> Result<(), TunnelError> {
        self.attempt
            .as_mut()
            .expect("attempt bindings")
            .registration_error
            .take()
            .map_or(Ok(()), Err)
    }
    fn hold_nrpt_result(
        &mut self,
        result: &mut Result<
            Option<crate::nrpt_worker::NrptWorkerLease>,
            crate::nrpt_worker::NrptStartError,
        >,
    ) -> Result<(), TunnelError> {
        let lease = match result {
            Ok(worker) => worker.take(),
            Err(error) => error.worker.take().map(|lease| *lease),
        };
        if let Some(lease) = lease {
            self.campus_dns = Some(lease);
            self.attempt.as_mut().unwrap().nrpt = None;
            let sequence = self.register(LedgerDomain::Nrpt, false)?;
            self.attempt.as_mut().unwrap().nrpt = Some(sequence);
        }
        Ok(())
    }
    fn release_first_nrpt(&mut self) {
        if let Some(mut lease) = self.campus_dns.take() {
            let binding = self.attempt.as_mut().unwrap();
            let sequence = binding.nrpt.take();
            let result = lease.shutdown();
            let mut observed = crate::connect_ledger::RollbackOutcome::default();
            observed.observe(
                sequence,
                LedgerDomain::Nrpt,
                result.clone(),
                false,
                "nrpt_shutdown",
            );
            if let Some(sequence) = sequence {
                self.runtime
                    .attempt_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record_result(
                        &self.attempt.as_ref().unwrap().id,
                        sequence,
                        observed.entries[0].state.clone(),
                    );
            }
            if result.is_ok() {
                self.attempt
                    .as_mut()
                    .unwrap()
                    .prior_results
                    .entries
                    .extend(observed.entries);
            } else {
                self.retired_nrpt.push((sequence, lease));
            }
        }
    }
}

impl std::ops::Deref for AttemptOwner {
    type Target = LiveTunnel;
    fn deref(&self) -> &LiveTunnel {
        self.live.as_ref().expect("attempt owner held")
    }
}
impl std::ops::DerefMut for AttemptOwner {
    fn deref_mut(&mut self) -> &mut LiveTunnel {
        self.live.as_mut().expect("attempt owner held")
    }
}
impl Drop for AttemptOwner {
    fn drop(&mut self) {
        if let Some(live) = self.live.take() {
            self.runtime.rollback_attempt(
                live,
                if std::thread::panicking() {
                    "assembly_panicked"
                } else {
                    "assembly_failed_or_cancelled"
                },
            );
        }
    }
}

/// aTrust 节点连接及驱动该连接的运行时句柄。
struct EngineAtrust {
    owner: crate::atrust_tunnel::AtrustPacketTunnel,
    runtime: tokio::runtime::Handle,
    _lost_tx: Option<std::sync::mpsc::Sender<DataPlaneLost>>,
}

impl EngineAtrust {
    // LiveTunnel 的同步清理也可能由 RPC 的 Tokio 上下文调用。
    // 在独立线程驱动 close，避免嵌套 block_on；返回前等待节点任务释放连接。
    fn close(mut self) -> Result<Option<crate::atrust_tunnel::AtrustNormalEnd>, String> {
        std::thread::Builder::new()
            .name("atrust-close".into())
            .spawn(move || self.runtime.block_on(self.owner.close_observed()))
            .map_err(|_| "engine: cannot start aTrust cleanup worker".to_owned())?
            .join()
            .map_err(|_| "engine: aTrust cleanup worker stopped".to_owned())?
    }
}

/// 每次连接的协议、数据面与路由持有者；NIC 独立常驻于 RealTunnelRuntime。
/// 显式断开和 Drop 均先标记 aTrust 正常退出，再 join 网卡线程、结束网卡 session、
/// 关闭协议连接，最后清理路由。aTrust 的异步关闭由 EngineAtrust 桥接。
struct LiveTunnel {
    protocol: Protocol,
    connection_mode: ConnectionMode,
    data_plane_threads: Option<EngineDataPlaneThreads>,
    data_plane: Option<EngineDataPlane>,
    cstp: Option<EngineCstp>,
    atrust: Option<EngineAtrust>,
    route: Option<RouteOwner>,
    campus_dns: Option<crate::nrpt_worker::NrptWorkerLease>,
    diagnostics: Option<RuntimeDiagnostics>,
    attempt: Option<AttemptBindings>,
    pending_route_cleanup: Option<RouteCleanupObligations>,
    retired_nrpt: Vec<(Option<usize>, crate::nrpt_worker::NrptWorkerLease)>,
}

impl LiveTunnel {
    fn connecting_control(
        connection_mode: ConnectionMode,
        diagnostics: RuntimeDiagnostics,
    ) -> Self {
        Self {
            protocol: Protocol::Cisco,
            connection_mode,
            data_plane_threads: None,
            data_plane: None,
            cstp: None,
            atrust: None,
            route: None,
            campus_dns: None,
            diagnostics: Some(diagnostics),
            attempt: None,
            pending_route_cleanup: None,
            retired_nrpt: Vec::new(),
        }
    }

    /// **drop_live——F6 共用清理主体**（`TunnelRuntime::disconnect`（S7）与掉线
    /// sweeper（S4）共用；区别只在调用方的 paused 置位时机——两个路径最终都置
    /// `paused=true`）：停数据面线程（先 join）→ 结束 session → 关闭 CSTP 通道 →
    /// 逆序清 route（地址/路由/DNS，**adapter 保留**）。逐项尝试后汇总本机资源错误，
    /// 协议与 join 结果另外如实记录，不跳过后续清理。
    ///
    /// 调用纪律（F6）：sweeper 侧**锁内仅 take、锁外调用**本函数——绝不持 live 锁
    /// 执行（内含 join，持锁 join 有死锁风险）。
    fn disconnect(&mut self) -> Result<(), String> {
        self.disconnect_for("runtime_disconnect")
    }

    fn disconnect_for(&mut self, reason: &str) -> Result<(), String> {
        self.disconnect_for_observed(reason).0
    }

    /// 与正常断开同一原语，逐项观察；一项失败仍按既定安全序尝试后续资源。
    fn disconnect_for_observed(
        &mut self,
        reason: &str,
    ) -> (Result<(), String>, crate::connect_ledger::RollbackOutcome) {
        let (report, outcome) = self.disconnect_for_report(reason);
        (report.result(), outcome)
    }

    fn disconnect_for_report(
        &mut self,
        reason: &str,
    ) -> (StopReport, crate::connect_ledger::RollbackOutcome) {
        let started = Instant::now();
        let mut outcome = self
            .attempt
            .as_mut()
            .map(|b| std::mem::take(&mut b.prior_results))
            .unwrap_or_default();
        let mut report = StopReport {
            stopped: false,
            ..Default::default()
        };
        if let Some(diag) = &self.diagnostics {
            diag.emit("tunnel.runtime.live_cleanup", reason, "started", []);
        }
        if let Some(atrust) = &self.atrust {
            atrust.owner.begin_host_shutdown();
        }
        // 线程 join -> session end，两个取得边界共用一条 DataPlane 义务。
        if self.data_plane_threads.is_some() || self.data_plane.is_some() {
            let result = self
                .data_plane_threads
                .take()
                .map_or(Ok(()), |mut threads| threads.stop_and_join_result());
            self.data_plane = None;
            report.action(LedgerDomain::DataPlane, "data_plane_stop", &result);
            outcome.observe(
                self.attempt.as_ref().and_then(|b| b.data_plane),
                LedgerDomain::DataPlane,
                result,
                false,
                "data_plane_stop",
            );
        }
        if self.cstp.is_some() || self.atrust.is_some() {
            let cstp = self.cstp.take().map_or(Ok(()), EngineCstp::close);
            let atrust = self.atrust.take().map_or(Ok(None), EngineAtrust::close);
            if let Ok(Some(ended)) = &atrust {
                if let Some(diag) = &self.diagnostics {
                    diag.emit(
                        "tunnel.protocol.termination",
                        "packet_worker_terminated",
                        "completed",
                        [("termination_category".into(), ended.category().to_owned())],
                    );
                }
            }
            let atrust = atrust.map(|_| ());
            let result = match (cstp, atrust) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
                (Err(a), Err(b)) => Err(format!("{a}; {b}")),
            };
            if let Err(category) = &result {
                if let Some(diag) = &self.diagnostics {
                    diag.emit(
                        "tunnel.protocol.termination",
                        "packet_worker_terminated",
                        "failed",
                        [("failure_category".into(), category.clone())],
                    );
                }
            }
            report.action(LedgerDomain::SessionLease, "protocol_close", &result);
            // 协议终止错误如实进入 rollback，仍不阻止本机资源清理或正常新连接。
            outcome.observe(
                self.attempt.as_ref().and_then(|b| b.session),
                LedgerDomain::SessionLease,
                result,
                false,
                "protocol_close",
            );
        }
        // NRPT 先于校园路由；失败的 lease 保持原 owner，不能丢弃其重试权限。
        if let Some(worker) = self.campus_dns.as_mut() {
            let result = worker.shutdown();
            report.action(LedgerDomain::Nrpt, "nrpt_shutdown", &result);
            if result.is_ok() {
                self.campus_dns = None;
            }
            outcome.observe(
                self.attempt.as_ref().and_then(|b| b.nrpt),
                LedgerDomain::Nrpt,
                result,
                false,
                "nrpt_shutdown",
            );
        }
        self.retired_nrpt.retain_mut(|(sequence, worker)| {
            let result = worker.shutdown();
            let keep = result.is_err();
            report.action(LedgerDomain::Nrpt, "retired_nrpt_shutdown", &result);
            outcome.observe(
                *sequence,
                LedgerDomain::Nrpt,
                result,
                false,
                "nrpt_shutdown",
            );
            keep
        });
        // RouteOwner 的地址/MTU/路由/DNS/代理内部逆序保持不变。
        if let Some(route) = self.route.as_mut() {
            let route_report = route.clear_report();
            let result = route_report.result();
            let released = route_report.remaining.is_empty();
            report.merge(route_report);
            if released {
                self.route = None;
            }
            outcome.observe(
                self.attempt.as_ref().and_then(|b| b.route),
                LedgerDomain::RouteFamily,
                result,
                false,
                "route_clear",
            );
        }
        if let Some(routes) = self.pending_route_cleanup.as_mut() {
            let result = routes.retry_clear();
            report.action(LedgerDomain::RouteFamily, "pending_route_clear", &result);
            if routes.is_empty() {
                self.pending_route_cleanup = None;
            }
            outcome.observe(
                self.attempt.as_ref().and_then(|b| b.route),
                LedgerDomain::RouteFamily,
                result,
                false,
                "pending_route_clear",
            );
        }
        let facts = self.stop_report_for_outcome(&outcome);
        report.stopped = facts.stopped;
        for obligation in facts.remaining {
            if obligation.step != "route_clear"
                || !report.remaining.iter().any(|o| o.owner == obligation.owner)
            {
                report.remaining.push(obligation);
            }
        }
        let result = report.result();
        if let Some(diag) = &self.diagnostics {
            diag.emit(
                "tunnel.runtime.live_cleanup",
                reason,
                if result.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
                [(
                    "elapsed_ms".into(),
                    started.elapsed().as_millis().to_string(),
                )],
            );
        }
        (report, outcome)
    }
    fn stop_report_for_outcome(
        &self,
        outcome: &crate::connect_ledger::RollbackOutcome,
    ) -> StopReport {
        let mut report = StopReport::from_rollback(outcome);
        report.stopped = self.data_plane_threads.is_none()
            && self.data_plane.is_none()
            && self.cstp.is_none()
            && self.atrust.is_none();
        if let Some(worker) = &self.campus_dns {
            report.remaining.push(StopObligation {
                domain: LedgerDomain::Nrpt,
                owner: worker.owner_guid().unwrap_or("live:nrpt").to_owned(),
                step: "nrpt_shutdown",
                blocks_new_connect: true,
            });
        }
        for (sequence, worker) in &self.retired_nrpt {
            report.remaining.push(StopObligation {
                domain: LedgerDomain::Nrpt,
                owner: worker
                    .owner_guid()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("retired:nrpt:{sequence:?}")),
                step: "retired_nrpt_shutdown",
                blocks_new_connect: true,
            });
        }
        if let Some(route) = &self.route {
            report.remaining.push(StopObligation {
                domain: LedgerDomain::RouteFamily,
                owner: route.owner_identity(),
                step: "route_clear",
                blocks_new_connect: true,
            });
        }
        if let Some(routes) = &self.pending_route_cleanup {
            for owner in routes.owner_identities() {
                report.remaining.push(StopObligation {
                    domain: LedgerDomain::RouteFamily,
                    owner,
                    step: "pending_route_clear",
                    blocks_new_connect: true,
                });
            }
        }
        report
    }
}

impl Drop for LiveTunnel {
    fn drop(&mut self) {
        // 正常断开、装配回滚、Drop 兜底共用同一清理主体及安全依赖序。
        if self.data_plane_threads.is_some()
            || self.data_plane.is_some()
            || self.cstp.is_some()
            || self.atrust.is_some()
            || self.campus_dns.is_some()
            || !self.retired_nrpt.is_empty()
            || self.route.is_some()
            || self.pending_route_cleanup.is_some()
        {
            let _ = self.disconnect_for("live_drop_fallback");
        }
    }
}

// ---------------------------------------------------------------------------
// T3（2026-09-05 cstp-keepalive 计划 F6）：掉线清 live sweeper——数据面真掉线后
// engine `live` 必清（S4），使 `is_connected()`=false、C3a 重连走全量重建（S5），
// 修复前「死隧道 Adopt 补发假 Connected」（S9）的路径不复现。
// ---------------------------------------------------------------------------

/// 数据面掉线信号（writer 线程 → sweeper 线程；F6）。
///
/// 携带信号所属隧道的**世代号**（复审 P2-5）：sweeper 收到信号后与运行时当前世代
/// 比对，不匹配即忽略——防 sweeper 病态迟醒（调度饿死数秒）后拆毁重连建立的新
/// 隧道 live。世代号在 assemble 安装 live 时递增分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DataPlaneLost {
    generation: u64,
}

/// sweeper 空转轮询间隔（`recv_timeout`；同时是运行时 drop 后 sweeper 的最大残留
/// 存活时长——弱引用在下一次唤醒即失效，P2-1）。
const LIVE_SWEEPER_IDLE_POLL: Duration = Duration::from_millis(500);

/// sweeper 单步结果（可观测；P2-7 单测断言用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweeperStep {
    /// 世代匹配的掉线信号 → 已清 live（session-end + 路由清 + 有界 join）+ paused=true。
    ClearedLive,
    /// 信号到达时 live 已空（用户 disconnect/teardown 先到）→ no-op（先到先得，
    /// 不动 paused——清理方自会置位；避免误暂停在途重连）。
    LiveAlreadyGone,
    /// 信号世代 ≠ 当前世代（旧隧道迟醒信号）→ 忽略，不动 live（P2-5 世代守卫）。
    StaleGenerationIgnored,
    /// 空转超时且运行时仍存活 → 继续循环。
    Idle,
    /// 发送端全部 drop / 运行时已 drop → sweeper 终止。
    Exit,
}

/// sweeper 单步（F6 循环体提取；P2-7 可单测——LiveTunnel 字段可 None 构造，无需
/// 真实 Wintun/NIC）。
///
/// 设计约束（复审 P2-1）：持 `Weak<RealTunnelRuntime>`——sweeper 不延长运行时生命
/// 周期，`RealTunnelRuntime::Drop` 兜底（含 writer 线程卡死的泄漏边界）不被 sweeper
/// 持有的强引用阻止；运行时 drop 后 sweeper 在下一次唤醒（≤ 1 个轮询间隔）内自终止。
fn live_sweeper_step(
    rt: &Weak<RealTunnelRuntime>,
    lost_rx: &std::sync::mpsc::Receiver<DataPlaneLost>,
    idle_poll: Duration,
) -> SweeperStep {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::RecvTimeoutError;
    match lost_rx.recv_timeout(idle_poll) {
        Ok(lost) => {
            let Some(rt) = rt.upgrade() else {
                // 运行时已 drop：Drop 兜底已清 live，直接退出。
                return SweeperStep::Exit;
            };
            // 原清理占用与新装配串行；take(None) 不代表旧 owner 已完成。
            // sweeper 不丢弃已收到的掉线信号，等待原装配收口后再核对 generation。
            let _cleanup = rt
                .begin_cleanup_until(false, None)
                .expect("sweeper waits for original owner");
            let _resources = rt.resources.lock().expect("runtime resources lock");
            if lost.generation != rt.generation.load(Ordering::SeqCst) {
                // 世代守卫（P2-5）：信号属于已拆除的旧隧道，绝不拆当前 live。
                if let Some(diag) = rt.diagnostic_context() {
                    diag.emit(
                        "tunnel.runtime.sweeper",
                        "stale_generation",
                        "ignored",
                        [("signal_generation".into(), lost.generation.to_string())],
                    );
                }
                return SweeperStep::StaleGenerationIgnored;
            }
            // 锁内仅 take（`Mutex<Option>` take 先到先得，与用户 disconnect/teardown
            // 竞态时后到 no-op）；**锁外**执行清 live——绝不持锁 disconnect
            // （LiveTunnel 清理会 join 数据面线程，持锁 join 有死锁风险）。
            let taken = {
                let mut guard = rt.live.lock().expect("live lock");
                // 锁内复检世代（TOCTOU 封堵）：assemble **先** bump 世代、**后**落
                // live（同线程程序序 + live 锁建立 happens-before）——若锁内已见
                // 更新世代，说明新 live 正在/已经安装，本信号绝不 take。
                if lost.generation != rt.generation.load(Ordering::SeqCst) {
                    return SweeperStep::StaleGenerationIgnored;
                }
                guard.take()
            };
            drop(_resources);
            match taken {
                Some(mut tunnel) => {
                    let diagnostics = tunnel
                        .diagnostics
                        .clone()
                        .or_else(|| rt.diagnostic_context());
                    let started = Instant::now();
                    if let Some(diag) = &diagnostics {
                        diag.emit(
                            "tunnel.runtime.sweeper",
                            "data_plane_lost",
                            "started",
                            [("signal_generation".into(), lost.generation.to_string())],
                        );
                    }
                    let result = tunnel.disconnect_for("data_plane_lost");
                    if let Some(diag) = &diagnostics {
                        diag.emit(
                            "tunnel.runtime.sweeper",
                            "data_plane_lost",
                            if result.is_ok() {
                                "completed"
                            } else {
                                "failed"
                            },
                            [
                                ("signal_generation".into(), lost.generation.to_string()),
                                (
                                    "elapsed_ms".into(),
                                    started.elapsed().as_millis().to_string(),
                                ),
                            ],
                        );
                    }
                    if let Err(e) = result {
                        // 路由清硬失败：stderr 单行诊断（掉线本身已由 C2 事件上报；
                        // W17 SAFETY-ORDER 由 LiveTunnel 字段 Drop 兜底）。
                        eprintln!("[info] engine live-sweeper: drop_live route clear failed: {e}");
                        // 数据面已停，保留清理所有者供下一次 disconnect/connect 重试。
                        if tunnel
                            .stop_report_for_outcome(&Default::default())
                            .blocks_new_connect()
                        {
                            *rt.live.lock().expect("live lock") = Some(tunnel);
                        }
                    }
                    // S4：掉线清 live 与减负断开同语义——置 Paused（下次连接
                    // start_apply 清）。
                    rt.paused.store(true, Ordering::SeqCst);
                    // T6a：R1 触发点——隧道已死且 live 已清，engine 自愈接管
                    //（门控在驱动内：材料在/无在途循环/隧道确死/reconnect.enabled；
                    // 无材料时驱动转 T5 第五情形善后——worker 死亡不再静默，
                    // 计划 v6.1 §8.2）。同步非阻塞（驱动只做判定并 spawn）。
                    if let Some(driver) = rt
                        .reconnect
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade)
                    {
                        driver.on_tunnel_lost();
                    }
                    SweeperStep::ClearedLive
                }
                None => {
                    if let Some(diag) = rt.diagnostic_context() {
                        diag.emit(
                            "tunnel.runtime.sweeper",
                            "live_already_gone",
                            "no_op",
                            [("signal_generation".into(), lost.generation.to_string())],
                        );
                    }
                    SweeperStep::LiveAlreadyGone
                }
            }
        }
        Err(RecvTimeoutError::Timeout) => {
            if rt.upgrade().is_none() {
                SweeperStep::Exit // 运行时已 drop：不再等发送端，自终止（P2-1）。
            } else {
                SweeperStep::Idle
            }
        }
        Err(RecvTimeoutError::Disconnected) => {
            // 发送端全部 drop（writer 线程退出 + LiveTunnel drop，P1-1 双份安放）：
            // sweeper 自终止（不 join——发送端 drop 即关停）。
            SweeperStep::Exit
        }
    }
}

/// 掉线 sweeper 线程主体（F6）：recv 掉线信号 → 世代守卫 → 清 live，直至终止条件。
fn live_sweeper_loop(
    rt: Weak<RealTunnelRuntime>,
    lost_rx: std::sync::mpsc::Receiver<DataPlaneLost>,
) {
    while live_sweeper_step(&rt, &lost_rx, LIVE_SWEEPER_IDLE_POLL) != SweeperStep::Exit {}
}

/// 唯一已批准的本地原生验收故障位点；不进入 RPC 或正式配置契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptanceFaultOnce {
    AfterControlBeforeNic,
}

/// 真实组装运行时（A1b 异步形态）：`start_apply` 受理即回，组装在后台 thread 内
/// 逐段执行（登录/CSTP 经专用 tokio runtime `block_on`；特权/Wintun 段为阻塞调用），
/// 逐段状态实时 post；`cancel` 触发取消令牌令组装在段边界中断并自清理。
///
/// **单飞**：任一时刻至多一个活跃组装/隧道（`live` 为 `Mutex<Option<LiveTunnel>>`；
/// `assembling` 标记在途组装；`cancel_flag` 协作式取消令牌）。
///
/// **S1.5 三 owner**：`nic`（Mutex<Option<NicOwner>>）为 NIC owner，跨连接存续（D12）；
/// `live`（session owner + route owner）为 per-connection 存活状态。
pub struct RealTunnelRuntime {
    /// 显式隔离验收输入；每个 engine runtime 在统一装配边界最多消费一次。
    acceptance_fault_once: std::sync::atomic::AtomicBool,
    /// 专用异步运行时（登录/CSTP；`block_on`，不阻塞调用方 async worker）。
    rt: Option<tokio::runtime::Runtime>,
    /// wintun.dll 路径（engine 启动参数）。
    wintun_dll: PathBuf,
    /// 创建的 adapter 名称（engine 启动参数）。
    adapter_name: String,
    /// 用户配置目录（服务形态由安装参数显式传入，避免使用 LocalSystem 的用户目录）。
    config_dir: PathBuf,
    /// **发起用户 SID**（系统代理豁免写入必须落在该用户 HKU；engine 以服务身份
    /// 运行时不能写自己的 HKCU）。来自 `--user-sid` 或本进程用户 SID。
    core_user_sid: Option<String>,
    /// **系统代理账本**（2026-09-05 账本计划 J4 接线）：apply 记账 / clear 清账 /
    /// 启动回放。与 `grpc_server::with_real_tunnel` 的 `open_and_replay` 共享同一
    /// store 句柄；禁用态（open 失败/测试旧构造器）= 无账本保护，best-effort 降级。
    journal: SystemProxyJournal,
    /// **NIC owner**（adapter 句柄 + lib）：首次连接建、断开不拆、退出清理 close（D12）。
    nic: Mutex<Option<NicOwner>>,
    /// 当前活跃隧道的 per-connection 状态（session owner + route owner；无 = 未连接）。
    live: Mutex<Option<LiveTunnel>>,
    /// 平台 apply 回滚后仍失败的外层路由；后续断开/连接必须继续持有并重试。
    pending_routes: Mutex<Vec<RouteCleanupObligations>>,
    /// 原资源槽访问检查；装配/清理占用负责全段串行，必要 join 不持此锁。
    resources: Mutex<()>,
    /// 取消与最终 Connected 提交的短临界区；不覆盖登录或 join。
    lifecycle_commit: Mutex<lifecycle::CommitState>,
    assembly_done: std::sync::Condvar,
    /// 同一把锁管理活动记录和最近一条结束记录，不持有资源。
    attempt_ledger: Mutex<crate::connect_ledger::AttemptLedger>,
    /// 在途组装标记（`start_apply` 已受理、未完成/未取消）。
    assembling: std::sync::atomic::AtomicBool,
    /// 协作式取消令牌（StopTunnel 置位；组装在段边界检查并自清理）。
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    /// **Paused 暂停标记（D13）**：减负断开后置位，下次连接清。engine 运行时状态。
    paused: std::sync::atomic::AtomicBool,
    /// **NIC 创建计数**（D12 复用证据：首次连接建一次）。
    nic_created_count: std::sync::atomic::AtomicUsize,
    /// **NIC 复用计数**（D12 复用证据：断开后重连 adapter 句柄未重建）。
    nic_reused_count: std::sync::atomic::AtomicUsize,
    /// **live 世代号**（T3/F6，复审 P2-5）：assemble 安装 live 时递增分配；数据面
    /// 掉线信号携带其所属世代，sweeper 比对运行时当前世代——不匹配即忽略，防
    /// sweeper 病态迟醒拆毁重连建立的新隧道。
    generation: std::sync::atomic::AtomicU64,
    /// 保存最近受理的操作身份，供没有 ApplyContext 的取消/退出入口记录。
    diagnostics: Mutex<Option<RuntimeDiagnostics>>,
    /// 连接门（T4）：装配成功（install_live 之后）经它把 trace 推 Live 并回写
    /// 装配字段（网卡/NRPT owner/engine PID）。`None` = 未接线（测试构造器）。
    trace_gate: Option<Arc<crate::connect_gate::ConnectGate>>,
    /// R1 重连驱动（T6a）：sweeper 清除死亡隧道的 live 之后回调其触发入口。
    /// **Weak**——强引用由 `HelperControlService` 持有，防止 运行时↔驱动 强引用
    /// 环阻止 `RealTunnelRuntime::Drop` 的兜底清理。`None` = 未接线（测试构造器）。
    reconnect: Mutex<Option<std::sync::Weak<crate::reconnect::ReconnectDriver>>>,
}

/// W1：预检网卡归属事实——本进程 NicOwner 持有即"设计内持久网卡"，
/// 预检四象限判定据此放行（不拒绝实际可建立的连接）。
impl crate::connect_gate::AdapterOwnershipProbe for RealTunnelRuntime {
    fn held_adapter_name(&self) -> Option<String> {
        self.nic
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|owner| owner.adapter_name().to_owned()))
    }
}

impl RealTunnelRuntime {
    /// 建真实运行时（wintun.dll / adapter_name 来自 engine 启动参数，配置目录使用当前
    /// 进程解析结果）。测试与 oneshot 可使用此便捷构造；服务入口使用显式目录构造器。
    ///
    /// # Panics
    /// tokio 运行时创建失败（infallible）→ panic。
    #[must_use]
    pub fn new(wintun_dll: PathBuf, adapter_name: String) -> Self {
        Self::new_with_config_dir(wintun_dll, adapter_name, exv_vpn_win32_config::config_dir())
    }

    /// 建使用显式用户配置目录的真实运行时。
    #[must_use]
    pub fn new_with_config_dir(
        wintun_dll: PathBuf,
        adapter_name: String,
        config_dir: PathBuf,
    ) -> Self {
        Self::new_with_config_dir_and_sid(wintun_dll, adapter_name, config_dir, None)
    }

    /// 建使用显式用户配置目录 + 发起用户 SID 的真实运行时（系统代理 family 所需）。
    #[must_use]
    pub fn new_with_config_dir_and_sid(
        wintun_dll: PathBuf,
        adapter_name: String,
        config_dir: PathBuf,
        core_user_sid: Option<String>,
    ) -> Self {
        // 旧构造器委托（§4.7）：禁用账本态——测试/过渡路径无账本保护，best-effort。
        Self::new_with_config_dir_and_sid_and_journal(
            wintun_dll,
            adapter_name,
            config_dir,
            core_user_sid,
            SystemProxyJournal::disabled(),
        )
    }

    /// 建使用显式用户配置目录 + 发起用户 SID + **系统代理账本**的真实运行时
    /// （§4.7 冻结签名；生产路径由 `grpc_server::with_real_tunnel` 在 `open_and_replay`
    /// 之后把已回放的账本句柄交进来——「回放完成后，打开/复用的 store 句柄交给
    /// `RealTunnelRuntime` 供运行期记账」）。
    #[must_use]
    pub fn new_with_config_dir_and_sid_and_journal(
        wintun_dll: PathBuf,
        adapter_name: String,
        config_dir: PathBuf,
        core_user_sid: Option<String>,
        journal: SystemProxyJournal,
    ) -> Self {
        Self {
            acceptance_fault_once: std::sync::atomic::AtomicBool::new(false),
            rt: Some(
                tokio::runtime::Runtime::new()
                    .expect("engine: tokio runtime（登录/CSTP 需要异步运行时）"),
            ),
            wintun_dll,
            adapter_name,
            config_dir,
            core_user_sid,
            journal,
            nic: Mutex::new(None),
            live: Mutex::new(None),
            pending_routes: Mutex::new(Vec::new()),
            resources: Mutex::new(()),
            lifecycle_commit: Mutex::new(lifecycle::CommitState::default()),
            assembly_done: std::sync::Condvar::new(),
            attempt_ledger: Mutex::new(crate::connect_ledger::AttemptLedger::default()),
            assembling: std::sync::atomic::AtomicBool::new(false),
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            paused: std::sync::atomic::AtomicBool::new(false),
            nic_created_count: std::sync::atomic::AtomicUsize::new(0),
            nic_reused_count: std::sync::atomic::AtomicUsize::new(0),
            generation: std::sync::atomic::AtomicU64::new(0),
            diagnostics: Mutex::new(None),
            trace_gate: None,
            reconnect: Mutex::new(None),
        }
    }

    /// 注入连接门（T4）：`grpc_server::with_real_tunnel` 生产组装用；测试构造器
    /// 保持 `None`（装配路径维持既有行为，无痕回写）。
    #[must_use]
    pub fn with_trace_gate(mut self, gate: Arc<crate::connect_gate::ConnectGate>) -> Self {
        self.trace_gate = Some(gate);
        self
    }

    /// 仅由启动前已验证的隔离验收输入启用，不读环境或配置文件。
    #[must_use]
    pub fn with_acceptance_fault_once(self, fault: Option<AcceptanceFaultOnce>) -> Self {
        self.acceptance_fault_once
            .store(fault.is_some(), std::sync::atomic::Ordering::SeqCst);
        self
    }

    /// 注入 R1 重连驱动（T6a）：`grpc_server::with_real_tunnel` 在驱动构造后调用
    /// （驱动需先持有运行时 `Arc`，故此处只存 `Weak`）。sweeper 的 `ClearedLive`
    /// 分支经它触发 engine 自愈；未注入（测试构造器）时掉线维持既有行为（清
    /// live + Paused，不自愈）。
    pub fn attach_reconnect(&self, driver: &Arc<crate::reconnect::ReconnectDriver>) {
        *self
            .reconnect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(driver));
    }

    fn inner_runtime(&self) -> &tokio::runtime::Runtime {
        self.rt
            .as_ref()
            .expect("inner runtime exists until RealTunnelRuntime::drop")
    }

    fn current_trace_id(&self, log: &LogSink) -> Option<String> {
        match self.trace_gate.as_ref()?.read_current_trace(log)? {
            crate::connection_trace::TraceReadOutcome::Present(record) => {
                Some(record.connection_id)
            }
            _ => None,
        }
    }

    fn diagnostic_context(&self) -> Option<RuntimeDiagnostics> {
        self.diagnostics.lock().ok().and_then(|value| value.clone())
    }

    /// **NIC 创建次数**（D12 复用证据：首次连接建一次）。
    #[must_use]
    pub fn nic_created_count(&self) -> usize {
        self.nic_created_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// **NIC 复用次数**（D12 复用证据：断开后重连 adapter 句柄未重建）。
    #[must_use]
    pub fn nic_reused_count(&self) -> usize {
        self.nic_reused_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 读用户 config（server/user_agent/校园路由）。服务形态使用安装时记录的用户目录，
    /// 不依赖服务进程（LocalSystem）的 `%USERPROFILE%`。
    ///
    /// # Errors
    /// config 读取失败（文件系统错误）→ `String`（fail closed）。
    fn load_config(&self) -> Result<exv_vpn_win32_config::ExvConfig, String> {
        // 连接阶段必须读取实际配置，不能沿用首次启动时缺文件即生成默认值的语义。
        let path = self.config_dir.join("config.json");
        let text = std::fs::read_to_string(&path).map_err(|error| {
            format!(
                "config-load: read {}: kind={:?}, os={:?}",
                path.display(),
                error.kind(),
                error.raw_os_error()
            )
        })?;
        let parse_error = |error: serde_json::Error| {
            // serde 的完整错误可能含字段值；这里只记录类别与位置。
            format!(
                "config-load: parse category={:?}, line={}, column={}",
                error.classify(),
                error.line(),
                error.column()
            )
        };
        let raw: serde_json::Value = serde_json::from_str(&text).map_err(&parse_error)?;
        if let Some(protocol) = raw.get("protocol")
            && !matches!(protocol.as_str(), Some("cisco" | "atrust"))
        {
            return Err("config-load: unsupported explicit protocol".into());
        }
        serde_json::from_value(raw).map_err(parse_error)
    }

    fn load_connection_config(
        &self,
        credentials: &ConnectionCredentials,
    ) -> Result<exv_vpn_win32_config::ExvConfig, String> {
        let config = self.load_config()?;
        let protocol = match credentials {
            ConnectionCredentials::Cisco(_) => Protocol::Cisco,
            ConnectionCredentials::Atrust(_) => Protocol::Atrust,
        };
        if config.protocol != protocol {
            return Err("config-load: credential protocol/config mismatch".into());
        }
        Ok(config)
    }

    /// 立即延迟刷新标记与服务安装时固定的用户配置目录保持一致。
    fn refresh_marker_path(&self) -> PathBuf {
        self.config_dir.join(LATENCY_REFRESH_FILE)
    }

    /// 调用方持有 resources 锁；硬失败的清理义务保留到下一次操作。
    fn retry_pending_routes(&self) -> Result<(), String> {
        self.retry_pending_routes_observed(&mut Default::default())
    }

    fn retry_pending_routes_observed(
        &self,
        observed: &mut PreviousCleanupObservation,
    ) -> Result<(), String> {
        let mut pending = self
            .pending_routes
            .lock()
            .map_err(|_| observed.access_failed("pending routes lock"))?;
        let groups_before = pending.len();
        let mut errors = Vec::new();
        for routes in pending.iter_mut() {
            let result = routes.retry_clear();
            if let Err(error) = &result {
                errors.push(error.clone());
            }
            // 旧义务的真实重试结果，不冒记为本轮新取得的登记序号。
            observed.outcome.observe(
                None,
                LedgerDomain::RouteFamily,
                result,
                false,
                "pending_route_clear",
            );
        }
        pending.retain(|routes| !routes.is_empty());
        if groups_before > 0
            && let Some(diag) = self.diagnostic_context()
        {
            diag.emit(
                "tunnel.route.cleanup",
                "pending_rollback_recovery",
                if errors.is_empty() {
                    "completed"
                } else {
                    "failed"
                },
                [
                    ("pending_groups_before".into(), groups_before.to_string()),
                    ("pending_groups_after".into(), pending.len().to_string()),
                    ("detail".into(), errors.join("; ")),
                ],
            );
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// 调用方持原装配/清理占用；清理动作和保留 owner 仍只有原路径。
    fn clear_previous_connection_observed(
        &self,
        observed: &mut PreviousCleanupObservation,
    ) -> Result<(), String> {
        let previous = self
            .live
            .lock()
            .map_err(|_| observed.access_failed("live lock"))?
            .take();
        if let Some(mut previous) = previous {
            let (result, mut previous_outcome) =
                previous.disconnect_for_observed("before_new_connection");
            // 结果仍属旧 holder；旧序号不能落到本轮新账本的同号条目。
            for entry in &mut previous_outcome.entries {
                entry.sequence = None;
            }
            observed
                .outcome
                .entries
                .append(&mut previous_outcome.entries);
            observed
                .outcome
                .unresolved
                .append(&mut previous_outcome.unresolved);
            if let Err(error) = result
                && previous
                    .stop_report_for_outcome(&previous_outcome)
                    .blocks_new_connect()
            {
                match self.live.lock() {
                    Ok(mut live) => *live = Some(previous),
                    Err(poisoned) => {
                        // 只归还原 owner；访问失败不能触发其 Drop 再清一次。
                        *poisoned.into_inner() = Some(previous);
                        observed.access_failed("live lock");
                        return Err(error);
                    }
                }
                // 原路径在此早退。其它 runtime pending 组仍真实持有，不能把未试当成功。
                let pending = match self.pending_routes.lock() {
                    Ok(pending) => pending,
                    Err(_) => {
                        observed.access_failed("pending routes lock");
                        return Err(error);
                    }
                };
                for routes in pending.iter() {
                    if !routes.is_empty() {
                        observed.outcome.observe(
                            None,
                            LedgerDomain::RouteFamily,
                            Err(format!(
                                "engine: pending routes retained after previous cleanup failure; remaining={}",
                                routes.len()
                            )),
                            true,
                            "pending_route_retained",
                        );
                    }
                }
                return Err(error);
            }
        }
        self.retry_pending_routes_observed(observed)
    }

    fn clear_previous_connection_for_attempt(
        self: &Arc<Self>,
        operation_id: &[u8],
        connection_mode: ConnectionMode,
        diagnostics: RuntimeDiagnostics,
    ) -> Result<(), String> {
        let mut observed = PreviousCleanupObservation::default();
        let (_resources, access_error) = match self.resources.lock() {
            Ok(guard) => (guard, None),
            Err(poisoned) => (
                poisoned.into_inner(),
                Some(observed.access_failed("runtime resources lock")),
            ),
        };
        // 原 assembling 占用排斥 Stop/sweeper；互斥锁不跨旧 owner 的必要 join。
        drop(_resources);
        let result = match access_error {
            Some(error) => Err(error),
            None => self.clear_previous_connection_observed(&mut observed),
        };
        if result.is_err() {
            self.finish_previous_observation_for_attempt(
                operation_id,
                connection_mode,
                diagnostics,
                observed,
            );
        }
        result
    }

    /// 未开始取得资源的失败只观察仍持有的旧槽，不改变最早取消的旧连接行为。
    fn rollback_unstarted_attempt(
        self: &Arc<Self>,
        operation_id: &[u8],
        diagnostics: RuntimeDiagnostics,
    ) {
        let mut observed = PreviousCleanupObservation::default();
        let _resources = match self.resources.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                observed.access_failed("runtime resources lock");
                poisoned.into_inner()
            }
        };
        if observed.access_failure.is_none() {
            let _ = (|| -> Result<(), String> {
                let live = self
                    .live
                    .lock()
                    .map_err(|_| observed.access_failed("live lock"))?;
                if let Some(live) = live.as_ref() {
                    // plane 与 threads 是同组义务；观察一次，不接管或清理句柄。
                    if live.data_plane.is_some() || live.data_plane_threads.is_some() {
                        observed.held(LedgerDomain::DataPlane, "data plane group");
                    }
                    if live.cstp.is_some() || live.atrust.is_some() {
                        observed.held(LedgerDomain::SessionLease, "protocol session");
                    }
                    if live.campus_dns.is_some() {
                        observed.held(LedgerDomain::Nrpt, "NRPT worker");
                    }
                    for _ in &live.retired_nrpt {
                        observed.held(LedgerDomain::Nrpt, "retired NRPT worker");
                    }
                    if live.route.is_some() {
                        observed.held(LedgerDomain::RouteFamily, "route owner");
                    }
                    if live
                        .pending_route_cleanup
                        .as_ref()
                        .is_some_and(|r| !r.is_empty())
                    {
                        observed.held(LedgerDomain::RouteFamily, "live pending routes");
                    }
                }
                let pending = self
                    .pending_routes
                    .lock()
                    .map_err(|_| observed.access_failed("pending routes lock"))?;
                for routes in pending.iter().filter(|r| !r.is_empty()) {
                    observed.held(
                        LedgerDomain::RouteFamily,
                        &format!("pending routes (remaining={})", routes.len()),
                    );
                }
                Ok(())
            })();
        }
        // 相同观察收口接缝；owner Drop 明确发生在原 resources Guard 释放之前。
        self.finish_previous_observation_for_attempt(
            operation_id,
            ConnectionMode::Standard,
            diagnostics,
            observed,
        );
    }

    /// 调用方持原 resources 锁；只消费观察结果，不再清理旧 owner / pending groups。
    fn finish_previous_observation_for_attempt(
        self: &Arc<Self>,
        operation_id: &[u8],
        connection_mode: ConnectionMode,
        diagnostics: RuntimeDiagnostics,
        mut observed: PreviousCleanupObservation,
    ) {
        if let Some(source) = observed.access_failure {
            diagnostics.emit(
                "tunnel.previous_cleanup.access",
                "previous_cleanup_access",
                "failed",
                [("detail".into(), source.to_owned())],
            );
        }
        let live = LiveTunnel::connecting_control(connection_mode, diagnostics);
        let mut owner = AttemptOwner::new(self, live, operation_id);
        if let Some(source) = observed.access_failure {
            if owner
                .attempt
                .as_ref()
                .is_some_and(|binding| binding.trace.is_some())
            {
                observed.outcome.observe(
                    None,
                    LedgerDomain::Trace,
                    Err(format!("engine: previous cleanup access failed: {source}")),
                    true,
                    "previous_cleanup_access",
                );
            } else {
                // 无关联 UUID 无写删权限，也不把未访问的旧资源记成空回滚成功。
                // 上方 connecting_control 全部资源槽为空，new 只绑定实际账本，
                // 尚未执行本轮任何资源取得；仅此空 owner 可解除 Drop 回滚。
                // 保存实际观察结果即可阻止后备路径二次虚报。
                drop(owner.live.take());
                observed.outcome.attempt_id = operation_id.to_vec();
                self.attempt_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .last_rollback = Some(observed.outcome);
                return;
            }
        }
        owner
            .attempt
            .as_mut()
            .expect("actual attempt binding")
            .prior_results = observed.outcome;
        // 同一个 rollback 接口只消费刚取得的结果；不再次清旧 owner 或 pending groups。
        // 仍持原资源锁，先收口关联痕，再发布 Failed；后备路径不会覆盖本次 last_rollback。
        drop(owner);
    }

    /// 调用方持有 resources 锁。失败时保存只剩清理工作的 live，绝不算作已连接。
    fn cleanup_or_retain(&self, mut live: LiveTunnel, reason: &str) -> Result<(), String> {
        if let Err(error) = live.disconnect_for(reason) {
            if let Some(diag) = &live.diagnostics {
                diag.emit(
                    "tunnel.route.cleanup",
                    reason,
                    "pending",
                    [("detail".into(), error.clone())],
                );
            }
            if live
                .stop_report_for_outcome(&Default::default())
                .blocks_new_connect()
            {
                *self.live.lock().expect("live lock") = Some(live);
            }
            return Err(error);
        }
        Ok(())
    }

    /// 唯一装配回滚入口：owner 组先执行既有原语，再记录其真实结果。
    /// 不持 lifecycle_commit 执行 join、子进程等待、路由重试。
    fn rollback_attempt(
        &self,
        mut live: LiveTunnel,
        reason: &str,
    ) -> crate::connect_ledger::RollbackOutcome {
        let id = live
            .attempt
            .as_ref()
            .map(|b| b.id.clone())
            .unwrap_or_default();
        let (_, mut outcome) = live.disconnect_for_observed(reason);
        outcome.attempt_id = id.clone();
        let trace = live
            .attempt
            .as_ref()
            .and_then(|binding| binding.trace.clone());
        let diagnostics = live.diagnostics.clone();
        let resources_pending = live.campus_dns.is_some()
            || !live.retired_nrpt.is_empty()
            || live.route.is_some()
            || live.pending_route_cleanup.is_some();
        if let Some((sequence, acquired_this_attempt)) = live.attempt.as_ref().and_then(|b| b.nic) {
            let mut nic = self
                .nic
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let result = settle_nic_attempt(acquired_this_attempt, resources_pending, |release| {
                if release {
                    if let Some(owner) = nic.take() {
                        // Close 本轮句柄；Opened 不声称删除设备。
                        drop(owner);
                        Ok(())
                    } else {
                        Err("engine: acquired NIC owner missing during rollback".into())
                    }
                } else {
                    nic.as_ref()
                        .ok_or_else(|| "engine: reused NIC owner missing during rollback".into())
                        .and_then(NicOwner::verify_retained)
                }
            });
            outcome.observe(
                sequence,
                LedgerDomain::Nic,
                result,
                !acquired_this_attempt,
                "nic_cleanup",
            );
        }
        // apply Err 的未竟义务仍交给原 pending_routes；没有成功的 retry 不得报干净。
        if let Some(routes) = live.pending_route_cleanup.take() {
            self.pending_routes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(routes);
        }
        if live.campus_dns.is_some() || !live.retired_nrpt.is_empty() || live.route.is_some() {
            *self
                .live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(live);
        }
        // 原资源回滚及未竟义务移交完成后，仍在原 resources 锁的 owner 作用域内收口。
        // 只使用实际回执 UUID；Unwired/Skipped(None) 没有删除权限。
        if let Some((sequence, connection_id)) = trace {
            let effect = self
                .trace_gate
                .as_ref()
                .ok_or_else(|| "engine: associated rollback trace gate unavailable".to_owned())
                .and_then(|gate| gate.finish_rollback(&connection_id, &outcome));
            let retained = matches!(
                effect,
                Ok(crate::connection_trace::RollbackTraceEffect::Preserved
                    | crate::connection_trace::RollbackTraceEffect::Replaced)
            );
            outcome.observe(
                Some(sequence),
                LedgerDomain::Trace,
                effect.map(|_| ()),
                retained,
                "trace_store",
            );
        }
        if let Some(diag) = diagnostics {
            diag.emit(
                if outcome.resolved() {
                    "tunnel.rollback.completed"
                } else {
                    "tunnel.rollback.unresolved"
                },
                reason,
                if outcome.resolved() {
                    "completed"
                } else {
                    "unresolved"
                },
                [(
                    "unresolved_domains".into(),
                    outcome.unresolved_domains().join(","),
                )],
            );
        }
        let mut slot = self
            .attempt_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in &outcome.entries {
            if let Some(sequence) = entry.sequence {
                slot.record_result(&id, sequence, entry.state.clone());
            }
        }
        slot.last_rollback = Some(outcome.clone());
        outcome
    }

    /// 启动掉线 sweeper 线程（T3/F6；assemble 在落 `LiveTunnel` 前调用——sweeper
    /// 先于 live 存在，writer 若在安装后立即死亡，信号也已有人接收）。
    ///
    /// **不 join**：自终止路径 = 发送端全部 drop（writer 线程退出 + LiveTunnel
    /// drop，P1-1 双份安放）或运行时已 drop（`Weak` 失效，P2-1——sweeper 不延长
    /// 运行时生命周期，`RealTunnelRuntime::Drop` 兜底不被阻止）。
    fn spawn_live_sweeper(self: &Arc<Self>, lost_rx: std::sync::mpsc::Receiver<DataPlaneLost>) {
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("exv-live-sweeper".to_string())
            .spawn(move || live_sweeper_loop(weak, lost_rx));
    }
}

impl RealTunnelRuntime {
    /// 取得 owner 后登记；实际身份缺失/失配是硬错误，owner 仍在 guard 内回滚。
    fn record_acquired(
        &self,
        id: &[u8],
        domain: LedgerDomain,
        persistent: bool,
    ) -> Result<usize, TunnelError> {
        let mut slot = self.attempt_ledger.lock().expect("attempt ledger lock");
        let sequence = slot.register(id, domain, persistent);
        let failure = if sequence.is_none() {
            Some(if slot.active.is_none() {
                "active_missing"
            } else {
                "attempt_id_mismatch"
            })
        } else {
            None
        };
        drop(slot);
        if let Some(category) = failure {
            if let Some(diag) = self.diagnostic_context() {
                diag.emit(
                    "tunnel.ledger.registration_failed",
                    "resource_acquired",
                    "failed",
                    [
                        ("domain".into(), format!("{domain:?}")),
                        ("failure_category".into(), category.into()),
                    ],
                );
            } else {
                eprintln!(
                    "[warn] engine ledger: registration failed domain={domain:?} category={category}"
                );
            }
        }
        sequence.ok_or_else(|| {
            TunnelError::plain_with_reason(
                ConnectPhase::ApplyingPlatformTunnel,
                format!(
                    "engine: ledger registration {domain:?}: {}",
                    failure.expect("observed registration failure")
                ),
                "platform/ledger_registration",
            )
        })
    }

}

impl TunnelRuntime for RealTunnelRuntime {
    fn start_apply_with_receipt(
        self: Arc<Self>,
        ctx: ApplyContext,
        trace: TraceAdmission,
    ) -> Result<ApplyAcceptance, TunnelError> {
        self.start_apply_recorded_with_receipt(ctx, Some(trace), None)
            .map(|(_, receipt)| receipt)
    }
    fn start_reconnect(
        self: Arc<Self>,
        ctx: ApplyContext,
        attempt: u32,
    ) -> Result<u64, TunnelError> {
        self.start_apply_recorded(ctx, None, Some(attempt))
    }

    fn start_apply(self: Arc<Self>, ctx: ApplyContext) -> Result<(), TunnelError> {
        // R1 直接入口没有新建痕事实；只读取实际现存关联，不触发新的门检。
        self.start_apply_recorded(ctx, None, None).map(|_| ())
    }

    fn start_apply_admitted(
        self: Arc<Self>,
        ctx: ApplyContext,
        trace: TraceAdmission,
    ) -> Result<(), TunnelError> {
        self.start_apply_recorded(ctx, Some(trace), None)
            .map(|_| ())
    }

    fn cancel(&self) {
        let _commit = self.lifecycle_commit.lock().expect("runtime commit lock");
        if let Some(diag) = self.diagnostic_context() {
            diag.emit(
                "tunnel.runtime.cancel",
                "caller_cancel_request",
                "requested",
                [],
            );
        }
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn disconnect(&self) -> Result<(), String> {
        self.disconnect_report().result()
    }

    fn disconnect_report(&self) -> StopReport {
        self.disconnect_lifecycle()
    }

    fn teardown(&self) -> Result<(), String> {
        self.teardown_lifecycle()
    }

    fn is_connected(&self) -> bool {
        self.live
            .lock()
            .map(|g| {
                g.as_ref()
                    .and_then(|live| live.data_plane_threads.as_ref())
                    .is_some_and(EngineDataPlaneThreads::is_alive)
            })
            .unwrap_or(false)
    }

    fn is_assembling(&self) -> bool {
        self.assembling.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn is_paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for RealTunnelRuntime {
    fn drop(&mut self) {
        let diagnostics = self.diagnostic_context();
        let started = Instant::now();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.drop",
                "runtime_drop_fallback",
                "started",
                [],
            );
        }
        // 兜底：任何未显式 teardown 的退出路径都先断开（session-end + route 清 +
        // 有界 join），再关 adapter（0 网卡残留）。字段声明序 `live` 先于 `nic` 也
        // 保证 session 先于 adapter close。
        if let Some(mut live) = self.live.get_mut().expect("live lock").take() {
            let _ = live.disconnect();
        }
        for pending in self.pending_routes.get_mut().expect("pending routes lock") {
            if let Err(error) = pending.retry_clear()
                && let Some(diag) = &diagnostics
            {
                diag.emit(
                    "tunnel.route.cleanup",
                    "runtime_drop",
                    "failed",
                    [("detail".into(), error)],
                );
            }
        }
        if let Some(nic) = self.nic.get_mut().expect("nic lock").take() {
            drop(nic);
        }
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.drop",
                "runtime_drop_fallback",
                "finished",
                [(
                    "elapsed_ms".into(),
                    started.elapsed().as_millis().to_string(),
                )],
            );
        }
        // Tokio Runtime 的析构会等待其阻塞线程停机；若最后的 Arc 由 async
        // task（包括 tonic detached serve）释放，直接字段析构会在该上下文 panic。
        // 业务资源已在上面按原顺序拆完，此处只转移内层 Runtime 的所有权。
        if let Some(rt) = self.rt.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                std::thread::Builder::new()
                    .name("exv-inner-runtime-shutdown".to_string())
                    .spawn(move || drop(rt))
                    .expect("spawn inner runtime shutdown thread")
                    .join()
                    .expect("inner runtime shutdown thread");
            } else {
                drop(rt);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PublicAssemblyFailure {
    reason_id: &'static str,
    message: &'static str,
}

/// 仅把封闭的已知原因映射为用户可见文案；任意错误文本（可能含路径或网络信息）不外显。
fn public_assembly_failure(detail: &str) -> Option<PublicAssemblyFailure> {
    let reason = if detail.starts_with("engine: config-load: read ")
        || detail.starts_with("engine: config-load: parse ")
        || detail == "engine: config-load: unsupported explicit protocol"
    {
        PublicAssemblyFailure {
            reason_id: "connection_config_read_or_parse_failed",
            message: "EXV 无法读取或解析连接配置。请检查应用配置权限和配置有效性，然后重启重试。",
        }
    } else if matches!(
        detail,
        "node-tls-verification-failed"
            | "node-peer-certificate-mismatch"
            | "node-tls-name"
            | "node-certificate-invalid"
            | "observed-certificate-scope-mismatch"
            | "node-platform-verifier"
            | "node-tls-config"
    ) {
        PublicAssemblyFailure {
            reason_id: "atrust_node_tls_verification_failed",
            message: "aTrust 节点身份验证失败。请检查系统时间和网络；仍失败请导出日志联系管理员。",
        }
    } else if matches!(
        detail,
        "aTrust authorized resource node group unavailable"
            | "aTrust observed-certificate node absent from authorized catalog"
            | "aTrust node unavailable"
            | "aTrust node does not belong to selected resource group"
    ) {
        PublicAssemblyFailure {
            reason_id: "atrust_authorized_node_unavailable",
            message: "当前账号的 aTrust 授权资源中没有可用节点。请重新登录刷新授权；仍失败请联系管理员。",
        }
    } else {
        return None;
    };
    Some(reason)
}

/// 已知原因只写固定、脱敏的结构化事件；其它错误保留现有诊断详情。
fn emit_assemble_failure_log(log: &LogSink, error: &TunnelError) {
    let phase = format!("{:?}", error.connect_phase);
    if let Some(public) = public_assembly_failure(&error.detail) {
        log.emit(
            LogLevel::Warn,
            "engine",
            "tunnel.assemble.failed",
            public.message,
            &[("reason_id", public.reason_id), ("phase", &phase)],
        );
    } else {
        log.emit(
            LogLevel::Warn,
            "engine",
            "tunnel.assemble.failed",
            "tunnel assembly failed (diagnostic; state on StreamConnectStatus)",
            &[("detail", &error.detail), ("phase", &phase)],
        );
    }
}

/// Failed 的诊断详情追加真实未竟因果；已知错误仍使用原安全文案，不外泄原始错误。
fn emit_assemble_failure_with_rollback(
    log: &LogSink,
    error: &TunnelError,
    rollback: Option<&crate::connect_ledger::RollbackOutcome>,
) {
    let Some(rollback) = rollback.filter(|outcome| !outcome.resolved()) else {
        emit_assemble_failure_log(log, error);
        return;
    };
    let public = public_assembly_failure(&error.detail);
    let primary = public.map_or(error.detail.as_str(), |failure| failure.message);
    let detail = format!(
        "{primary}；连接失败后的实际回滚未竟域：{}；残留事实保留供下次预检处理，可重试并反馈日志",
        rollback.unresolved_domains().join(",")
    );
    log.emit(
        LogLevel::Warn,
        "engine",
        "tunnel.assemble.failed",
        "连接失败，实际清理未完成；本次失败仍已上报",
        &[
            ("detail", &detail),
            ("phase", &format!("{:?}", error.connect_phase)),
            (
                "reason_id",
                public.map_or(error.reason, |failure| failure.reason_id),
            ),
        ],
    );
}

impl RealTunnelRuntime {
    /// 组装线程入口（P3）：包裹 `assemble` + `catch_unwind`，**无论如何都复位
    /// `assembling`**，杜绝 panic 后 assembling 永真导致后续连接全部被拒为 duplicate。
    ///
    /// 成功/失败均推终态（Connected/Failed）；panic 路径推 Failed + EFFECT_UNKNOWN。
    /// 本函数在线程内调用，接收 `&Arc<Self>` 避免 move 限制。
    fn run_assemble_with_reset(
        rt: &Arc<Self>,
        ctx: ApplyContext,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        stats: &Arc<crate::stats::StatsPublisher>,
        status: &Arc<StatusPublisher>,
        op_id: &[u8],
    ) {
        let _completion = lifecycle::AssemblyCompletion(rt);
        // 克隆 LogSink 供失败详情落盘（ctx 随后被 assemble 消费；log.emit 才是可达
        // 聚合器/raw 的通道，tracing 无 subscriber 会丢）。
        let log = ctx.log.clone();
        let diagnostics = RuntimeDiagnostics::new(Arc::clone(&log), op_id);
        let started = Instant::now();
        tracing::info!(
            config_dir = %rt.config_dir.display(),
            "assembly thread entered (engine assembling start)"
        );
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.assemble(ctx, cancel)));
        let record_end = match &result {
            Ok(Ok(_)) => AttemptEnd::Committed,
            Ok(Err(err)) if err.detail == "engine: cancelled" => AttemptEnd::Cancelled,
            Ok(Err(_)) => AttemptEnd::Failed,
            Err(_) => AttemptEnd::Panicked,
        };
        if record_end != AttemptEnd::Committed
            && !rt
                .attempt_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last_rollback
                .as_ref()
                .is_some_and(|outcome| outcome.attempt_id == op_id)
        {
            // 空新 owner 不能证明旧资源已干净；原资源锁内观察 held 事实后统一收口。
            rt.rollback_unstarted_attempt(op_id, diagnostics.clone());
        }
        let rollback = rt
            .attempt_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_rollback
            .as_ref()
            .filter(|outcome| outcome.attempt_id == op_id)
            .cloned();
        match result {
            Ok(Ok(_)) => {
                let diag = rt
                    .diagnostic_context()
                    .unwrap_or_else(|| diagnostics.clone());
                diag.emit(
                    "tunnel.runtime.apply",
                    "assembly_completed",
                    "completed",
                    [(
                        "elapsed_ms".into(),
                        started.elapsed().as_millis().to_string(),
                    )],
                );
                // Connected 已在真实 live 安装点提交并放行工作线程；这里不能覆盖
                // 线程在安装后立即报告的 Failed。
            }
            Ok(Err(err)) => {
                diagnostics.emit(
                    "tunnel.runtime.apply",
                    if err.detail == "engine: cancelled" {
                        "cooperative_cancel_observed"
                    } else {
                        "assembly_failed"
                    },
                    "failed",
                    [
                        (
                            "elapsed_ms".into(),
                            started.elapsed().as_millis().to_string(),
                        ),
                        ("phase".into(), format!("{:?}", err.connect_phase)),
                        ("error_kind".into(), format!("{:?}", err.kind)),
                    ],
                );
                emit_assemble_failure_with_rollback(&log, &err, rollback.as_ref());
                stats.registry().set_phase(generated::StatsPhase::Failed);
                let wire = tunnel_error_to_wire(&err);
                status.publish(StatusEvent::failed(op_id.to_vec(), err.connect_phase, wire));
            }
            Err(_panic) => {
                diagnostics.emit(
                    "tunnel.runtime.apply",
                    "assembly_panicked",
                    "failed",
                    [(
                        "elapsed_ms".into(),
                        started.elapsed().as_millis().to_string(),
                    )],
                );
                // assemble 内部 panic（.expect() 逃逸）：推 Failed + EFFECT_UNKNOWN，
                // 不让 assembling 永真导致后续连接全部被拒。此处不经
                // `tunnel_error_to_wire` 直构 wire——reason 落点必须显式补上
                // （复审 2.1-5a：只挂构造点会漏此直构点）。
                stats.registry().set_phase(generated::StatsPhase::Failed);
                tracing::error!("tunnel assembly panicked (catch_unwind captured)");
                emit_assemble_failure_with_rollback(
                    &log,
                    &TunnelError {
                        kind: TunnelErrorKind::Platform,
                        connect_phase: ConnectPhase::ApplyingPlatformTunnel,
                        code: 0,
                        a0: 0,
                        detail: "engine: assembly panicked".into(),
                        reason: "platform/assembly_internal",
                    },
                    rollback.as_ref(),
                );
                status.publish(StatusEvent::failed(
                    op_id.to_vec(),
                    ConnectPhase::ApplyingPlatformTunnel,
                    generated::VpnError {
                        code: 13, // EFFECT_UNKNOWN
                        stage: 1, // Ingress
                        certainty: 0,
                        retry: 1, // DoNotRetry
                        subject: None,
                        resource: None,
                        native: Some(generated::RedactedNativeError {
                            category: generated::NativeErrorCategory::Transport as i32,
                            namespace: generated::NativeErrorNamespace::Win32 as i32,
                            code: 0,
                        }),
                        // T-13 panic 兜底：直构 wire 点同样携带稳定 reason id。
                        reason: "platform/assembly_internal".to_string(),
                        operation_detail: None,
                    },
                ));
            }
        }
        // 成功已在 install_live 提交；这里仅结束失败/取消/panic 记录，不清资源。
        // 结束槽和释放单飞同属短锁，下一尝试不能继承旧条目。
        let _commit = rt.lifecycle_commit.lock().expect("runtime commit lock");
        if record_end != AttemptEnd::Committed {
            rt.attempt_ledger
                .lock()
                .expect("attempt ledger lock")
                .finish(op_id, record_end);
        }
        // P3：无论如何都复位 assembling——含 panic 路径。
        // _completion 在短提交锁释放后通知所有等待者。
    }

    /// 组装被取消时的受控错误（StopTunnel 中途中止）。
    fn cancelled_error(phase: ConnectPhase) -> TunnelError {
        TunnelError {
            kind: TunnelErrorKind::Platform,
            connect_phase: phase,
            code: 0,
            a0: 0,
            detail: "engine: cancelled".to_string(),
            reason: "platform/assembly_cancelled",
        }
    }

    /// prepare 返回后立即保存真实 aTrust 控制 owner，并登记取得事实。
    fn hold_atrust_control(
        self: &Arc<Self>,
        prepared: crate::atrust_prepare::PreparedAtrust,
        operation_id: &[u8],
        connection_mode: ConnectionMode,
        diagnostics: RuntimeDiagnostics,
    ) -> PreparedConnection {
        // prepare 返回的真实 owner 先进入既有清理槽，下一可失败步骤前即具备 graceful 义务。
        let mut control = LiveTunnel::connecting_control(connection_mode, diagnostics);
        control.protocol = Protocol::Atrust;
        control.atrust = Some(EngineAtrust {
            owner: prepared.owner,
            runtime: self.inner_runtime().handle().clone(),
            _lost_tx: None,
        });
        let mut control = AttemptOwner::new(self, control, operation_id);
        match control.register(LedgerDomain::SessionLease, false) {
            Ok(sequence) => control.attempt.as_mut().unwrap().session = Some(sequence),
            Err(error) => control.attempt.as_mut().unwrap().registration_error = Some(error),
        }
        PreparedConnection {
            offer: prepared.offer,
            campus_dns_servers: prepared.campus_dns_servers,
            bypasses: prepared.bypasses,
            packets: PreparedPackets::Atrust {
                channels: prepared.channels,
            },
            control,
        }
    }

    fn hold_cisco_control(
        self: &Arc<Self>,
        session: CstpSession,
        operation_id: &[u8],
        connection_mode: ConnectionMode,
        diagnostics: RuntimeDiagnostics,
    ) -> PreparedConnection {
        let mut control = LiveTunnel::connecting_control(connection_mode, diagnostics);
        let write_channel = session.write_channel;
        let read_channel = Arc::new(Mutex::new(session.read_channel));
        let control_rx = Arc::new(Mutex::new(session.control_rx));
        control.cstp = Some(EngineCstp {
            write_channel: write_channel.clone(),
            read_channel: read_channel.clone(),
            control_rx: Some(control_rx.clone()),
            lost_tx: None,
            tasks: session.tasks,
            runtime: self.inner_runtime().handle().clone(),
        });
        let mut control = AttemptOwner::new(self, control, operation_id);
        match control.register(LedgerDomain::SessionLease, false) {
            Ok(sequence) => control.attempt.as_mut().unwrap().session = Some(sequence),
            Err(error) => control.attempt.as_mut().unwrap().registration_error = Some(error),
        }
        let offer = session.offer_plan;
        let latency_cfg = LatencyProbeConfig {
            target: network_base(offer.ipv4_address, offer.prefix),
            source: offer.ipv4_address,
            dpd_enabled: DPD_PROBE_ENABLED,
            ping_interval: Duration::from_secs(LATENCY_PING_INTERVAL_SECS),
            refresh_marker: Some(self.refresh_marker_path()),
        };
        PreparedConnection {
            campus_dns_servers: offer.dns_servers.iter().map(ToString::to_string).collect(),
            offer,
            bypasses: Vec::new(),
            packets: PreparedPackets::Cisco {
                write_channel,
                read_channel,
                control_rx,
                latency_cfg,
            },
            control,
        }
    }

    /// 后台组装主体（A1b）：逐段执行，段边界检查取消令牌（协作式中止 + 自清理）；
    /// 终态（Connected/Failed）经 `ctx.status` 推送（受理即回后由本函数负责终态）。
    ///
    /// 取消语义：`cancel_flag` 置位后，在登录后/CSTP 后/apply 后各段边界中断——已建
    /// 局部资源（CSTP 通道 / adapter / route）随返回路径清理，不泄漏连接/网卡/路由。
    ///
    /// S1.5 三 owner 连接序（D11）：session 协商 offer → 协调者 → NIC 确保/复用
    /// adapter（D12）→ route 应用地址/DNS/路由（D17）→ **屏障（all-or-nothing，D16）**
    /// → 数据面启动。任一 owner 失败 → 整连接 Failed 回滚。
    ///
    /// `self: &Arc<Self>`（T3/F6，复审 P2-4）：sweeper 启动需要 `Arc::downgrade`
    /// 出 `Weak`——调用方 `run_assemble_with_reset` 手头即有 `&Arc<Self>`。
    fn assemble(
        self: &Arc<Self>,
        mut ctx: ApplyContext,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<PlatformFacts, TunnelError> {
        use std::sync::atomic::Ordering;
        let diagnostics = RuntimeDiagnostics::new(Arc::clone(&ctx.log), &ctx.operation_id);
        let connection_mode = ctx.connection_mode;
        let is_cancelled = || cancel_flag.load(Ordering::SeqCst);
        if is_cancelled() {
            return Err(Self::cancelled_error(ConnectPhase::ObservingOwnedState));
        }

        self.clear_previous_connection_for_attempt(
            &ctx.operation_id,
            connection_mode,
            diagnostics.clone(),
        )
        .map_err(|error| {
            TunnelError::plain_with_reason(
                ConnectPhase::ObservingOwnedState,
                format!("engine: previous connection cleanup:{error}"),
                "platform/previous_cleanup",
            )
        })?;
        if is_cancelled() {
            return Err(Self::cancelled_error(ConnectPhase::ObservingOwnedState));
        }

        // 首次创建 NIC 前先校验本地依赖，缺失时不向网关发起无意义的认证。
        // 已加载的 NIC 持有 DLL 引用，复用时无需再次依赖磁盘上的旧路径。
        let prepared_wintun = if self.nic.lock().expect("nic lock").is_none() {
            ctx.status.publish(StatusEvent::connecting(
                ctx.operation_id.clone(),
                ConnectPhase::ObservingOwnedState,
            ));
            let installed = std::env::current_exe()
                .map(|exe| exe.with_file_name("wintun.dll"))
                .unwrap_or_else(|_| self.wintun_dll.clone());
            ctx.log.emit(
                LogLevel::Debug,
                "engine",
                "wintun.preflight.started",
                "检查本地 Wintun 网络组件",
                &[
                    ("configured_path", &self.wintun_dll.display().to_string()),
                    ("installed_path", &installed.display().to_string()),
                    ("operation_id", &diagnostics.operation_id),
                ],
            );
            let prepared = crate::wintun_dependency::prepare_from(
                &self.wintun_dll,
                &installed,
                |original, path| {
                    ctx.log.emit(
                        LogLevel::Warn,
                        "engine",
                        "wintun.recovery.started",
                        "配置的 Wintun 无法加载，尝试校验并加载安装目录副本",
                        &[
                            ("original_error", &format!("{original:?}")),
                            ("installed_path", &path.display().to_string()),
                            ("operation_id", &diagnostics.operation_id),
                        ],
                    );
                },
            )
            .map_err(|error| {
                let code = error.actionable_error().code;
                let detail = format!("engine: wintun preflight failed: {error:?}");
                ctx.log.emit(
                    LogLevel::Error,
                    "engine",
                    "wintun.recovery.failed",
                    "Wintun 网络组件不可用，本次连接在登录前停止；请修复安装后重试",
                    &[
                        ("detail", &detail),
                        ("native_code", &code.to_string()),
                        ("operation_id", &diagnostics.operation_id),
                    ],
                );
                TunnelError {
                    kind: TunnelErrorKind::PlatformDependency,
                    connect_phase: ConnectPhase::ObservingOwnedState,
                    code,
                    a0: 0,
                    detail,
                    // T-06/07 Wintun：native 分档文案已达标，不设 id（词表 §2.3）。
                    reason: "",
                }
            })?;
            ctx.log.emit(
                if prepared.recovered_from.is_some() {
                    LogLevel::Info
                } else {
                    LogLevel::Debug
                },
                "engine",
                if prepared.recovered_from.is_some() {
                    "wintun.recovery.succeeded"
                } else {
                    "wintun.preflight.ready"
                },
                "Wintun 已校验并加载，继续连接",
                &[
                    ("loaded_path", &prepared.path.display().to_string()),
                    ("operation_id", &diagnostics.operation_id),
                ],
            );
            Some(prepared)
        } else {
            None
        };
        if is_cancelled() {
            return Err(Self::cancelled_error(ConnectPhase::ObservingOwnedState));
        }

        // 认证材料与本次磁盘配置必须一致，再进入任何网关认证分支。
        let credentials = ctx.credentials.take().ok_or_else(|| TunnelError {
            kind: TunnelErrorKind::MissingCredentials,
            connect_phase: ConnectPhase::ConnectingControl,
            code: 0,
            a0: 0,
            detail: "engine: no credentials provided".to_string(),
            reason: "auth/credentials_missing",
        })?;
        let config = self.load_connection_config(&credentials).map_err(|e| {
            TunnelError::plain(ConnectPhase::ConnectingControl, format!("engine: {e}"))
        })?;
        diagnostics.emit(
            "tunnel.config.loaded",
            "connection_configuration",
            "observed",
            [
                ("directory".into(), self.config_dir.display().to_string()),
                (
                    "file_exists".into(),
                    self.config_dir.join("config.json").is_file().to_string(),
                ),
                ("protocol".into(), config.protocol.to_string()),
            ],
        );
        let hostname = config.server.clone();
        let user_agent = config.user_agent.clone();

        let prepared = match credentials {
            ConnectionCredentials::Atrust(browser) => {
                ctx.status.publish(StatusEvent::connecting(
                    ctx.operation_id.clone(),
                    ConnectPhase::ConnectingControl,
                ));
                let certificate =
                    match std::fs::read(self.config_dir.join("atrust-node-certificate.der")) {
                        Ok(bytes) => Some(bytes),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(_) => {
                            return Err(TunnelError::plain(
                                ConnectPhase::ConnectingControl,
                                "engine: cannot read explicitly configured aTrust node certificate",
                            ));
                        }
                    };
                let prepared = self
                    .inner_runtime()
                    .block_on(crate::atrust_prepare::prepare(
                        browser,
                        &config,
                        certificate.as_deref(),
                        &ctx.log,
                    ))
                    .map_err(|error| TunnelError::plain(ConnectPhase::ConnectingControl, error))?;
                self.hold_atrust_control(
                    prepared,
                    &ctx.operation_id,
                    connection_mode,
                    diagnostics.clone(),
                )
            }
            ConnectionCredentials::Cisco(mut credentials) => {
                // ---- 阶段 1：登录 + CSTP 控制面协商（真实学校网关；plan 来自真实 offer）。
                //      **session owner**：登录/CSTP/TLS 通道在此建立，offer 由此产出。 ----
                ctx.status.publish(StatusEvent::connecting(
                    ctx.operation_id.clone(),
                    ConnectPhase::ConnectingControl,
                ));
                // 网关解析：VGDC 双线（DoH 直连 + 绑物理网卡 UDP/53 兜底），物理出口 ifindex
                // 自动发现；解析出**真实网关地址**（222.66.117.109），供 login/CSTP 直连。
                // `perform_login` 内部用 `gateway_addr` 参数自建 BootstrapConfig（不带
                // resolver）——必须预解析真实地址传入，绝不传占位符。
                //
                // **C4/G-⑤ 物理出口绑定（R1b 落地）**：CSTP 控制面 socket 用 `IP_UNICAST_IF`
                // 钉在物理网卡出口——否则（本机 Mihomo TUN 默认路由下）控制面/data 走 Mihomo
                // 代理路径，隧道数据面无回包（实测：engine 的 222.66.117.109:443 连接源地址
                // 是 198.18.0.1=Mihomo，ring_sent=0）。解析器与 binder 共用同一物理 ifindex。
                let nics = exv_vpn_win32_resource::vgdc_dns::find_physical_nics().map_err(|e| {
                    TunnelError::plain(ConnectPhase::ConnectingControl, format!("engine: nics:{e}"))
                })?;
                let physical_nic = nics.first().ok_or_else(|| {
                    TunnelError::plain(ConnectPhase::ConnectingControl, "engine: no physical nic")
                })?;
                let physical_ifindex = physical_nic.ifindex;
                let dns_diag = diagnostics.clone();
                let dns_observer: Arc<GatewayResolutionObserver> = Arc::new(move |observation| {
                    let mut fields = BTreeMap::from([
                        (
                            "elapsed_ms".into(),
                            observation.elapsed.as_millis().to_string(),
                        ),
                        (
                            "resolved_gateway".into(),
                            observation
                                .address
                                .map_or_else(|| "unavailable".into(), |value| value.to_string()),
                        ),
                    ]);
                    if let Some(failed_layers) = observation.failed_layers {
                        fields.insert("reported_failed_layers".into(), failed_layers.to_string());
                    }
                    if let Some(source) = observation.source {
                        use exv_vpn_win32_resource::vgdc_dns::ResolutionSource;
                        fields.insert("resolution_source".into(), source.as_str().into());
                        fields.insert(
                            "resolution_mode".into(),
                            match source {
                                ResolutionSource::System => "ip_literal",
                                ResolutionSource::Doh(_) => "doh",
                                ResolutionSource::Udp53(_) => "udp53",
                            }
                            .into(),
                        );
                        match source {
                            ResolutionSource::Doh(ip) | ResolutionSource::Udp53(ip) => {
                                fields.insert("resolver_addr".into(), ip.to_string());
                            }
                            _ => {}
                        }
                    }
                    dns_diag.emit(
                        "tunnel.gateway.resolved",
                        "actual_gateway_resolution",
                        if observation.address.is_some() {
                            "completed"
                        } else {
                            "failed"
                        },
                        fields,
                    );
                });
                let gateway_resolver = production_gateway_resolver_observed(
                    physical_ifindex,
                    CSTP_GATEWAY_PORT,
                    Some(dns_observer),
                )
                .map_err(|e| {
                    TunnelError::plain(ConnectPhase::ConnectingControl, format!("engine: {e}"))
                })?;
                let socket_binder = socket_binder_for_mode(connection_mode, physical_ifindex);
                let resolver = gateway_resolver.clone();
                let gateway_addr =
                    self.inner_runtime()
                        .block_on(resolver(&hostname))
                        .map_err(|e| {
                            TunnelError::plain(
                                ConnectPhase::ConnectingControl,
                                format!("engine: {e}"),
                            )
                        })?;
                if is_cancelled() {
                    return Err(Self::cancelled_error(ConnectPhase::ConnectingControl));
                }
                // 网关 bypass /32 行（2026-09-08 计划 T2）：VGDC 解析地址驱动（地址来源唯一
                // 化——拒绝手填，config 无网关 IP 字段），与解析器/binder 同一物理出口发现
                // 结果。行在 apply 内先于全部隧道路由入表；网关所在校园网段随后整段进隧道，
                // 路由表层对网关的选路由本 /32 覆盖到物理出口（socket binder 之外的第二层
                // 独立防御）。fake-ip / IPv6 防御拒绝（fail-closed）。
                let gateway_ip = match gateway_addr.ip() {
                    std::net::IpAddr::V4(ip) => ip,
                    std::net::IpAddr::V6(_) => {
                        return Err(TunnelError::plain(
                            ConnectPhase::ConnectingControl,
                            "engine: gateway resolved to IPv6 (VGDC dual-line is v4-only)",
                        ));
                    }
                };
                // 登录 socket 与 CSTP 控制面使用同一次会话冻结的出口策略。
                // `production_socket_binder` 与 CSTP 的 `BootstrapConfig.socket_binder`
                // 复用同一闭包——否则登录 TLS（aggregate-auth XML 通道 + form 通道的
                // logon GET / credential POST）在 Mihomo TUN 默认路由下走代理路径，登录
                // 流量绕过物理出口（与 R1b 已修的 CSTP 控制面 C4/G-⑤ 同机理）。
                let login = self
                    .inner_runtime()
                    .block_on(WebvpnLogin::perform_login_with_socket_binder(
                        &hostname,
                        gateway_addr,
                        TrustPolicy::Production, // 真实学校链（同 school.rs 策略）
                        None,
                        credentials.username.as_bytes(),
                        credentials.password.as_bytes(),
                        &user_agent,
                        socket_binder.clone(),
                    ))
                    .map_err(|err| {
                        // 凭据一次性契约：登录失败同样一次消费后立即 zeroize。
                        credentials.zeroize();
                        ctx.log.emit(
                            LogLevel::Warn,
                            "engine",
                            "tunnel.login.failed",
                            "login failed (diagnostic; state on StreamConnectStatus)",
                            &[("detail", &format!("{:?}", err))],
                        );
                        TunnelError::login(&err)
                    })?;
                // 凭据一次性契约：登录已消费 → 立即 zeroize（不等待 ApplyContext Drop）。
                credentials.zeroize();
                if is_cancelled() {
                    // 取消：登录已消费，未建任何资源——直接中断。
                    return Err(Self::cancelled_error(ConnectPhase::ConnectingControl));
                }

                let cfg = BootstrapConfig {
                    hostname: hostname.clone(),
                    // 已预解析真实网关地址；CSTP bootstrap 不再重复解析（同 acceptance engine
                    // 的 `real_ip` 直连语义；`gateway_resolver` 置 None——已解析）。
                    gateway_addr,
                    trust: TrustPolicy::Production,
                    dtls_offered: false, // MVP：TLS/CSTP only（offer 出现 dtls 即拒绝）
                    deadline: None,
                    // 标准模式绑定物理出口；兼容模式使用系统选路，两者与登录 socket 策略一致。
                    socket_binder,
                    gateway_resolver: None,
                };
                let actual_endpoints = Mutex::new(None);
                let socket_observer = |socket: &tokio::net::TcpStream| {
                    *actual_endpoints.lock().expect("socket observation lock") = Some(
                        socket
                            .local_addr()
                            .and_then(|local| socket.peer_addr().map(|peer| (local, peer))),
                    );
                    let mut fields = connected_socket_fields(socket);
                    fields.insert("connection_mode".into(), connection_mode.as_str().into());
                    if let Ok(address) = socket.local_addr() {
                        fields.insert("socket_selected_source".into(), address.ip().to_string());
                    }
                    fields.extend([
                        (
                            "selected_physical_ifindex".into(),
                            physical_nic.ifindex.to_string(),
                        ),
                        (
                            "selected_physical_luid".into(),
                            physical_nic.luid.to_string(),
                        ),
                        (
                            "selected_physical_source".into(),
                            physical_nic.local_ip.to_string(),
                        ),
                        (
                            "selected_physical_gateway".into(),
                            physical_nic.gateway.to_string(),
                        ),
                    ]);
                    diagnostics.emit(
                        "tunnel.cstp.connected_socket",
                        "validated_cstp_session",
                        "connected",
                        fields,
                    );
                };
                let session = self
                    .inner_runtime()
                    .block_on(CstpSession::open_with_user_agent_observed(
                        cfg,
                        Some(&login),
                        Some(&user_agent),
                        Some(&socket_observer),
                    ))
                    .map_err(TunnelError::session)?;
                let mut prepared = self.hold_cisco_control(
                    session,
                    &ctx.operation_id,
                    connection_mode,
                    diagnostics.clone(),
                );
                prepared.control.check_registration()?;
                // 在写入校园路由前，以长期 CSTP socket 的实际源地址确定出口。
                // 兼容模式不能在此切回物理网卡，否则将改变已经建立的连接路径。
                let egress_result = (|| {
                    Ok(match connection_mode {
                        ConnectionMode::Standard => (
                            exv_vpn_win32_resource::routes::gateway_bypass_row(
                                physical_nic,
                                gateway_ip,
                            )
                            .map_err(|error| {
                                TunnelError::plain(
                                    ConnectPhase::ConnectingControl,
                                    format!("engine: bypass-row:{error:?}"),
                                )
                            })?,
                            physical_ifindex,
                        ),
                        ConnectionMode::Compatibility => {
                            let endpoints = actual_endpoints
                                .into_inner()
                                .expect("socket observation lock")
                                .ok_or_else(|| {
                                    TunnelError::plain(
                                        ConnectPhase::ConnectingControl,
                                        "engine: established CSTP socket endpoints unavailable",
                                    )
                                })?
                                .map_err(|error| {
                                    TunnelError::plain(
                                        ConnectPhase::ConnectingControl,
                                        format!("engine: CSTP endpoint query:{error}"),
                                    )
                                })?;
                            let own_ifindex = self
                                .nic
                                .lock()
                                .expect("nic lock")
                                .as_ref()
                                .map(NicOwner::ifindex);
                            observed_cstp_bypass(gateway_addr, endpoints, own_ifindex)?
                        }
                    })
                })();
                let (bypass_row, egress_ifindex) = match egress_result {
                    Ok(value) => value,
                    Err(error) => {
                        drop(prepared.packets);
                        return Err(error);
                    }
                };
                diagnostics.emit(
                    "tunnel.gateway.egress",
                    "gateway_bypass_selection",
                    "observed",
                    [
                        ("connection_mode".into(), connection_mode.as_str().into()),
                        ("egress_ifindex".into(), egress_ifindex.to_string()),
                        ("egress_luid".into(), bypass_row.interface_luid.to_string()),
                        ("gateway_ip".into(), gateway_ip.to_string()),
                        ("next_hop".into(), bypass_row.next_hop.to_string()),
                        ("route_metric".into(), bypass_row.metric.to_string()),
                    ],
                );
                let offer = &prepared.offer;
                diagnostics.emit(
                    "tunnel.cstp.effective_parameters",
                    "negotiated_offer",
                    "observed",
                    [
                        ("mtu".into(), offer.mtu.to_string()),
                        (
                            "keepalive_interval_secs".into(),
                            crate::data_plane::CSTP_KEEPALIVE_INTERVAL_SECS.to_string(),
                        ),
                        ("dpd_probe_enabled".into(), DPD_PROBE_ENABLED.to_string()),
                        (
                            "ping_probe_interval_secs".into(),
                            LATENCY_PING_INTERVAL_SECS.to_string(),
                        ),
                    ],
                );
                ctx.log.emit(
                    LogLevel::Info,
                    "engine",
                    "tunnel.offer.received",
                    "CSTP offer received (diagnostic)",
                    &[
                        ("address", &offer.ipv4_address.to_string()),
                        ("prefix", &offer.prefix.to_string()),
                        ("mtu", &offer.mtu.to_string()),
                        (
                            "dns",
                            &offer
                                .dns_servers
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(","),
                        ),
                        ("routes", &offer.routes.join(",")),
                    ],
                );
                prepared.bypasses.push(bypass_row);
                prepared
            }
        };
        self.assemble_prepared(ctx, config, prepared_wintun, prepared, cancel_flag)
    }

    fn assemble_prepared(
        self: &Arc<Self>,
        ctx: ApplyContext,
        config: exv_vpn_win32_config::ExvConfig,
        prepared_wintun: Option<crate::wintun_dependency::PreparedWintun>,
        prepared: PreparedConnection,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<PlatformFacts, TunnelError> {
        use std::sync::atomic::Ordering;
        let diagnostics = RuntimeDiagnostics::new(Arc::clone(&ctx.log), &ctx.operation_id);
        let connection_mode = ctx.connection_mode;
        let is_cancelled = || cancel_flag.load(Ordering::SeqCst);
        let campus_routes = config.routes.clone();
        let PreparedConnection {
            offer,
            campus_dns_servers,
            bypasses,
            packets: prepared_packets,
            control,
        } = prepared;
        let mut live = control;
        // 失败时通道保留到显式协议清理之后；成功时只在 spawn 点取走。
        let mut prepared_packets = Some(prepared_packets);
        // 控制段只有既有 LiveTunnel owner；失败回到其原语，成功只把同一 owner 移交。
        // 登记关联本 owner，错误/取消/panic 都由 guard 同入口回滚；trace 留阶段 5。
        let result = (|| {
            live.check_registration()?;
            if is_cancelled() {
                // 尚无平台资源：退出时 guard 以同一控制 owner 显式清理协议。
                return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
            }

            // ---- 阶段 2：**NIC owner 确保/复用 + route owner 应用**（D11 连接序：session
            //      协商 offer → NIC 确保/复用 adapter → route 应用地址/DNS/路由）。 ----
            #[cfg(test)]
            control_tests::observe_before_resources_lock();
            let resources = self.resources.lock().expect("runtime resources lock");
            drop(resources); // 原装配占用仍有效；失败 owner 的 Drop 可在锁外等待。
            // Err/unwind 先完成原 owner 回滚和义务移交，再由 AssemblyCompletion
            // 释放装配占用；失败 join 不持资源锁，不扩大 lifecycle_commit。
            let mut live = live;
            if is_cancelled() {
                return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
            }
            // sweeper 可能还未处理掉线信号。新连接既不能 Adopt 死数据面，也不能在旧
            // 路由仍在清理时写入新路由；原装配占用覆盖清理和本次应用/安装。
            let old_live = self.live.lock().expect("live lock").take();
            if let Some(old_live) = old_live {
                self.cleanup_or_retain(old_live, "replace_failed_data_plane")
                    .map_err(|error| {
                        TunnelError::plain(ConnectPhase::ApplyingPlatformTunnel, error)
                    })?;
            }
            ctx.status.publish(StatusEvent::connecting(
                ctx.operation_id.clone(),
                ConnectPhase::ApplyingPlatformTunnel,
            ));
            if self.acceptance_fault_once.swap(false, Ordering::SeqCst) {
                diagnostics.emit(
                    "tunnel.acceptance.fault.injected",
                    "acceptance_fault",
                    "injected",
                    [
                        ("fault_id".into(), "after-control-before-nic".into()),
                        ("phase".into(), "ApplyingPlatformTunnel".into()),
                    ],
                );
                return Err(TunnelError::plain(
                    ConnectPhase::ApplyingPlatformTunnel,
                    "engine: acceptance fault after-control-before-nic",
                ));
            }
            // 2a. NIC owner：首次连接建 adapter（D12），断开后复用（`nic` 已存在）。接口级
            //     配置（DAD 禁用/接口启用）随 ensure 一次完成，跨连接存续。
            {
                let mut guard = self.nic.lock().expect("nic lock");
                if guard.is_none() {
                    let prepared = prepared_wintun.ok_or_else(|| {
                        TunnelError::plain(
                            ConnectPhase::ApplyingPlatformTunnel,
                            "engine: preflight NIC changed unexpectedly",
                        )
                    })?;
                    let nic = NicOwner::ensure_with_library(prepared.library, &self.adapter_name)
                        .map_err(|error| {
                        use crate::platform_tunnel::NicInitializationError;
                        match error {
                            NicInitializationError::Native(native) => {
                                ctx.log.emit(
                                    LogLevel::Error,
                                    "engine",
                                    "wintun.adapter.failed",
                                    "Wintun 已加载，但网卡初始化失败；请检查组件和权限后重试",
                                    &[
                                        ("native_code", &native.code.to_string()),
                                        ("detail", &format!("{native:?}")),
                                        ("operation_id", &diagnostics.operation_id),
                                    ],
                                );
                                TunnelError {
                                    kind: TunnelErrorKind::PlatformDependency,
                                    connect_phase: ConnectPhase::ApplyingPlatformTunnel,
                                    code: native.code,
                                    a0: 0,
                                    detail: format!("engine: wintun-open-or-create:{native:?}"),
                                    // T-06/07 Wintun：native 分档文案已达标，不设 id（词表 §2.3）。
                                    reason: "",
                                }
                            }
                            NicInitializationError::Interface(native) => {
                                let mut error = TunnelError::plain(ConnectPhase::ApplyingPlatformTunnel, format!("engine: interface-enable:{native:?}"));
                                error.code = native.code;
                                error
                            }
                            NicInitializationError::Configuration(detail) => TunnelError::plain(
                                ConnectPhase::ApplyingPlatformTunnel,
                                format!("engine: {detail}"),
                            ),
                        }
                    })?;
                    let created = nic.adapter().owned();
                    *guard = Some(nic);
                    live.attempt.as_mut().unwrap().nic = Some((None, true));
                    let sequence = live.register(LedgerDomain::Nic, true)?;
                    live.attempt.as_mut().unwrap().nic = Some((Some(sequence), true));
                    record_nic_creation(&self.nic_created_count, true, created);
                } else {
                    // D12 复用证据：断开后重连 adapter 句柄未重建。
                    live.attempt.as_mut().unwrap().nic = Some((None, false));
                    let sequence = live.register(LedgerDomain::Nic, true)?;
                    live.attempt.as_mut().unwrap().nic = Some((Some(sequence), false));
                    self.nic_reused_count
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    ctx.log.emit(
                        LogLevel::Info,
                        "engine",
                        "tunnel.nic.reused",
                        "NIC adapter reused across connections (D12; handle not rebuilt)",
                        &[("reused_count", &self.nic_reused_count().to_string())],
                    );
                }
            };

            if is_cancelled() {
                return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
            }

            // 2b. route owner：经 NIC 交出的 LUID 应用真实 offer 五族（D17 + 2026-09-08
            //     计划网关 bypass /32 族）。发起用户 SID 一并传入（系统代理豁免写入该
            //     用户 HKU；None = 跳过 system_proxy 族）。系统代理账本一并传入
            //     （apply 记账 / clear 清账；J4 接线）。bypass 行来自阶段 1 的 VGDC
            //     解析结果；aTrust 可含认证网关和数据节点的多条旁路。
            let route_result = {
                let guard = self.nic.lock().expect("nic lock");
                let nic = guard.as_ref().expect("nic ensured");
                if bypasses.iter().any(|row| row.interface_luid == nic.luid()) {
                    Err("engine: gateway bypass points into EXV's own tunnel".into())
                } else {
                    nic.apply_offer_with_bypasses(
                        &offer,
                        &campus_routes,
                        &config.campus_dns_domains,
                        &bypasses,
                        self.core_user_sid.clone(),
                        &self.journal,
                    )
                }
            };
            // apply 失败仍携带 pending_routes 时，本次已取得可重试义务，不能漏记。
            let facts = match route_result {
                Ok((route, facts)) => {
                    live.route = Some(route);
                    let sequence = live.register(LedgerDomain::RouteFamily, false)?;
                    live.attempt.as_mut().unwrap().route = Some(sequence);
                    facts
                }
                Err(e) => {
                    // 内部回滚后仍携义务则由同 owner 重试；未清完不得提前关闭 NIC。
                    let detail = format!("engine: {e}");
                    let route_native_code = e.route_native_code;
                    if !e.pending_routes.is_empty() {
                        ctx.log.emit(
                            LogLevel::Warn,
                            "connection",
                            "tunnel.route.rollback.pending",
                            "平台配置失败，部分本次创建的路由清理失败，已保留后续重试记录",
                            &[
                                ("connection_mode", connection_mode.as_str()),
                                ("operation_id", &diagnostics.operation_id),
                                ("pending_route_count", &e.pending_routes.len().to_string()),
                                ("detail", &detail),
                            ],
                        );
                        live.pending_route_cleanup = Some(e.pending_routes);
                        let sequence = live.register(LedgerDomain::RouteFamily, false)?;
                        live.attempt.as_mut().unwrap().route = Some(sequence);
                    }
                    let native_code = route_native_code.unwrap_or(0);
                    // T-05 裁定：有原始 native 码（1168/5 等）→ 不挂 reason，壳侧
                    // native 分档文案保留；无 native 码 → 挂 route_apply 细分 id。
                    let mut error = if native_code == 0 {
                        TunnelError::plain_with_reason(
                            ConnectPhase::ApplyingPlatformTunnel,
                            detail,
                            "platform/route_apply",
                        )
                    } else {
                        TunnelError::plain(ConnectPhase::ApplyingPlatformTunnel, detail)
                    };
                    error.code = native_code;
                    return Err(error);
                }
            };
            ctx.log.emit(
                if facts.route_ownership.pending > 0 {
                    LogLevel::Warn
                } else {
                    LogLevel::Info
                },
                "engine",
                "tunnel.platform.applied",
                "platform tunnel applied (diagnostic)",
                &[
                    ("connection_mode", connection_mode.as_str()),
                    (
                        "route_created_count",
                        &facts.route_ownership.created.to_string(),
                    ),
                    (
                        "route_borrowed_count",
                        &facts.route_ownership.borrowed.to_string(),
                    ),
                    (
                        "route_pending_count",
                        &facts.route_ownership.pending.to_string(),
                    ),
                    (
                        "address",
                        facts.address_applied.as_deref().unwrap_or("none"),
                    ),
                    (
                        "mtu",
                        &facts
                            .mtu_applied
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                    ),
                    ("routes", &facts.routes_applied.join(",")),
                    ("dns", &facts.dns_applied.join(",")),
                ],
            );
            if is_cancelled() {
                // 取消：同一 guard 清理 route；依赖已清完才释放新建 adapter handle。
                return Err(Self::cancelled_error(ConnectPhase::StartingDataPlane));
            }

            // ---- 阶段 3：engine 数据面（session owner 接续——engine 自己建 session +
            //      reader/writer 线程，全在 engine 进程内零跨进程）。 ----
            ctx.status.publish(StatusEvent::connecting(
                ctx.operation_id.clone(),
                ConnectPhase::StartingDataPlane,
            ));
            let plane = {
                let guard = self.nic.lock().expect("nic lock");
                let nic = guard.as_ref().expect("nic ensured");
                EngineDataPlane::start(nic.library(), nic.adapter(), WINTUN_RING_CAPACITY)
            };
            match plane {
                Ok(plane) => {
                    // Wintun session 已建立；后续线程 spawn / NRPT 失败仍须保留取得事实。
                    live.data_plane = Some(plane);
                    let sequence = live.register(LedgerDomain::DataPlane, false)?;
                    live.attempt.as_mut().unwrap().data_plane = Some(sequence);
                }
                Err(e) => {
                    // 数据面启动失败：同一 guard 按安全序清理，不丢 route 或新建 NIC 的义务。
                    return Err(TunnelError::plain_with_reason(
                        ConnectPhase::StartingDataPlane,
                        format!("engine: data-plane:{e}"),
                        "platform/data_plane_start",
                    ));
                }
            };
            if is_cancelled() {
                return Err(Self::cancelled_error(ConnectPhase::StartingDataPlane));
            }
            // ---- T3（F6）：掉线 sweeper 接线——世代号分配 + 信号通道 + 双份发送端
            //      安放（P1-1：writer 闭包一份 + EngineCstp 一份）。世代号在安装 live
            //      前递增分配；sweeper 先于 live 安装启动——writer 若在安装后立即死亡，
            //      信号也已有人接收。----
            let generation = self
                .generation
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            let mut live_diagnostics = diagnostics.clone();
            live_diagnostics.generation = Some(generation);
            *self.diagnostics.lock().expect("diagnostics lock") = Some(live_diagnostics.clone());
            live_diagnostics.emit(
                "tunnel.runtime.generation",
                "data_plane_install",
                "assigned",
                [],
            );
            let (lost_tx, lost_rx) = std::sync::mpsc::channel::<DataPlaneLost>();
            self.spawn_live_sweeper(lost_rx);
            // writer 闭包内的发送端：`Arc<dyn Fn>` 持 Sender 克隆 + 预制世代号（复审
            // P2-5）——writer 掉线时调用一次，DataPlaneAux 无需感知世代细节。
            let notify_data_plane_lost: Arc<dyn Fn() + Send + Sync> = {
                let lost_tx = lost_tx.clone();
                Arc::new(move || {
                    // send 失败（sweeper 已退出）→ stderr 单行、忽略（F6，不重试——
                    // 该失败仅发生在运行时已 drop 的退出边界）。
                    if lost_tx.send(DataPlaneLost { generation }).is_err() {
                        eprintln!(
                            "[info] engine data-plane: lost signal send failed (sweeper gone)"
                        );
                    }
                })
            };
            let aux = DataPlaneAux {
                stats: Some(Arc::clone(ctx.stats.registry())),
                latency: None,
                control_rx: None,
                status: Some(Arc::clone(&ctx.status)),
                operation_id: ctx.operation_id.clone(),
                notify_data_plane_lost: Some(notify_data_plane_lost),
            };
            let threads = match prepared_packets.take().expect("packet channels held") {
                PreparedPackets::Cisco {
                    write_channel,
                    read_channel,
                    control_rx,
                    latency_cfg,
                } => {
                    let threads = live
                        .data_plane
                        .as_ref()
                        .expect("plane held")
                        .spawn_data_plane(
                            write_channel,
                            &read_channel,
                            Some(Arc::clone(&ctx.log)),
                            DataPlaneAux {
                                latency: Some(latency_cfg),
                                control_rx: Some(control_rx),
                                ..aux
                            },
                        );
                    live.cstp.as_mut().expect("Cisco control held").lost_tx = Some(lost_tx);
                    threads
                }
                PreparedPackets::Atrust { channels } => {
                    let threads = live
                        .data_plane
                        .as_ref()
                        .expect("plane held")
                        .spawn_raw_packet_data_plane(
                            channels.outgoing,
                            channels.incoming,
                            Some(Arc::clone(&ctx.log)),
                            aux,
                        );
                    live.atrust.as_mut().expect("aTrust control held")._lost_tx = Some(lost_tx);
                    threads
                }
            };
            // 同一控制 owner 接续到 live；移交后早期失败收尾没有重复释放权限。
            match threads {
                Ok(threads) => live.data_plane_threads = Some(threads),
                Err(error) => {
                    live.data_plane_threads = Some(error.threads);
                    return Err(TunnelError::plain_with_reason(
                        ConnectPhase::StartingDataPlane,
                        format!("engine: data-plane spawn:{}", error.message),
                        "platform/data_plane_start",
                    ));
                }
            }
            live.diagnostics = Some(live_diagnostics);
            if is_cancelled() {
                return Err(Self::cancelled_error(ConnectPhase::StartingDataPlane));
            }
            // T4：NRPT owner 标记随 lease 建立即捕获（worker 移入 live 后不再可及），
            // 供装配成功回写 trace 的 nrpt_owner_guids。
            let mut nrpt_owner_guids: Vec<String> = Vec::new();
            // ---- 等效接纳（2026-10-08 计划阶段 1）+ 静默精准接管（D8 阶段 2）----
            // 有界只读探测（复用预检探针子进程的**既有**有界生命周期：spawn ≤2.5s、
            // op ≤2s、退出 ≤0.5s——不启动 NRPT worker 做判定，不新增「等待 PS 进程
            // 退出」依赖）+ 纯函数判定（preflight/classify.rs 单一指纹规则）：外来规则
            // 与本次会话 campus_dns_servers 完全等效的形式跳过自有添加
            // （external-provided：台账标记 + teardown 绝不删它、绝不期待它消失）；
            // 下次连接重新判定（规则消失→正常添加；变不等效→接管路径）。
            // 探测失败 → 空接纳（fail closed，worker 冲突检测自行裁定，语义与
            // 既有行为一致，无重试循环）。
            //
            // **静默精准接管（2026-10-09 用户裁定）**：conflict 形式非空且探测成功 →
            // `nrpt_takeover`（journal 前置 → 原动作完成，会话服务器权威判定）→
            // 继续实际 NRPT Add。失败/重建环 → 连接失败终态（reason 走 D9 的
            // `VpnError.reason` 通道：`platform/nrpt_takeover_failed` /
            // `platform/nrpt_foreign_rebuild`）。最小必要由判定结构保证：等效不覆盖、
            // 清单外/自有前缀绝不进接管目标。
            let mut nrpt_acceptance =
                self.inner_runtime()
                    .block_on(crate::preflight::determine_nrpt_acceptance(
                        &config.campus_dns_domains,
                        &campus_dns_servers,
                    ));
            if !nrpt_acceptance.accept_forms().is_empty() {
                ctx.log.emit(
                crate::log_sink::LogLevel::Info,
                "engine",
                "tunnel.nrpt.equivalent_accepted",
                "外来 NRPT 规则与本次校园 DNS 配置完全等效：跳过自有添加（external-provided；teardown 不动该规则，下次连接重验）",
                &[
                    ("forms", &nrpt_acceptance.accept_forms().join(",")),
                    ("fingerprint", &nrpt_acceptance.decision.digest_hex),
                    ("servers", &campus_dns_servers.join(",")),
                    ("probed_ok", &nrpt_acceptance.probed_ok.to_string()),
                ],
            );
            }
            let mut nrpt_external_forms = nrpt_acceptance.accept_forms().to_vec();
            // 接管共享状态：本连接内已覆盖指纹账本（初始分支与 worker conflict 分支
            // 共用——单指纹至多覆盖一次，重建即停）+ 已接管形式台账（trace 取证）。
            let mut covered_fingerprints: Vec<String> = Vec::new();
            let mut nrpt_taken_over_forms: Vec<String> = Vec::new();
            let mut takeover_engaged = false;
            if nrpt_acceptance.probed_ok && !nrpt_acceptance.decision.conflict_forms.is_empty() {
                // 10s 接管预算 vs 5s 断开有界等待的竞态处理（复审遗漏 7）：接管前的
                // 取消段边界检查——断开已取消时尽早退出，不进入 10s 预算的接管段。
                if is_cancelled() {
                    return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
                }
                let outcome =
                    self.inner_runtime()
                        .block_on(crate::nrpt_takeover::takeover_foreign_nrpt(
                            &config.campus_dns_domains,
                            &campus_dns_servers,
                            &mut covered_fingerprints,
                            &ctx.log,
                            &diagnostics.operation_id,
                        ));
                takeover_engaged = outcome.engaged();
                if let Some(failure) = &outcome.failed {
                    return Err(self.fail_after_takeover(
                        live,
                        ConnectPhase::ApplyingPlatformTunnel,
                        format!(
                            "engine: nrpt takeover {}: {}",
                            failure.detail(),
                            failure.message()
                        ),
                        "platform/nrpt_takeover_failed",
                    ));
                }
                if !outcome.rebuilt_forms.is_empty() {
                    return Err(self.fail_after_takeover(
                        live,
                        ConnectPhase::ApplyingPlatformTunnel,
                        format!(
                            "engine: nrpt takeover rebuilt: {}",
                            outcome.rebuilt_forms.join(",")
                        ),
                        "platform/nrpt_foreign_rebuild",
                    ));
                }
                nrpt_taken_over_forms.extend(outcome.taken_over_forms.iter().cloned());
            }
            // 全部校园形式都被等效接纳 → **不启动 worker**（防死锁硬约束：等效接纳
            // 路径不启动 NRPT worker 或 worker 即刻正常收尾——此处直接零进程）。
            if is_cancelled() {
                return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
            }
            let mut campus_dns_start = if nrpt_acceptance
                .decision
                .accepts_all(&config.campus_dns_domains)
            {
                Ok(None)
            } else {
                crate::nrpt_worker::NrptWorkerLease::start(
                    &config.campus_dns_domains,
                    &campus_dns_servers,
                    nrpt_acceptance.accept_forms(),
                )
            };
            // Err 也可能持有 lease；首次 conflict 在重试前就有真实进程义务。
            live.hold_nrpt_result(&mut campus_dns_start)?;
            // 兜底认领清理（计划 v6.1 §8.2 触发模型补洞）：撞 NRPT conflict——上次
            // 会话善后未竟的自有残留（预检三触发源在该路径全不命中），或判定与
            // worker 启动之间重现的外来覆盖（竞态，D8 起接管处置）→ 按 EXV-NRPT:
            // 前缀认领清理（预检清理器同款策略与代码）→ 按原前置目标分流 →
            // 仍有外来冲突则静默接管一次 → 仍冲突 → 重建环收口。自动重试有界
            // （无循环）；D8 阶段 2 起认领清理集为空也重判（外来冲突的处置点在此）。
            if let Err(first) = &mut campus_dns_start
                && first.is_conflict()
            {
                // 首次尝试在 stage=conflict 早退（未 add 任何规则）——有界 shutdown 收掉
                // 其子进程（EOF + 清理确认），再清理重试。
                live.release_first_nrpt();
                ctx.log.emit(
                crate::log_sink::LogLevel::Warn,
                "engine",
                "tunnel.nrpt.conflict.claim_started",
                "校园 DNS 精确规则冲突：按 EXV-NRPT: 前缀认领清理后自动重试一次（预检清理器同款策略；外来冲突走静默精准接管）",
                &[("first_message", first.message.as_str())],
            );
                let claim = self
                    .inner_runtime()
                    .block_on(crate::preflight::claim_clean_own_nrpt(
                        &config.campus_dns_domains,
                        &campus_dns_servers,
                    ));
                ctx.log.emit(
                    if claim.all_ok {
                        crate::log_sink::LogLevel::Info
                    } else {
                        crate::log_sink::LogLevel::Warn
                    },
                    "engine",
                    "tunnel.nrpt.conflict.claim_cleaned",
                    "NRPT 认领清理完成（自有前缀规则已删；外来校园域规则交静默精准接管处置）",
                    &[
                        ("removed", &claim.removed.join(",")),
                        ("removed_count", &claim.removed.len().to_string()),
                        ("foreign_present", &claim.foreign_present.join(",")),
                        ("all_ok", &claim.all_ok.to_string()),
                    ],
                );
                if !claim.errors.is_empty() {
                    return Err(self.fail_after_takeover(
                        live,
                        ConnectPhase::ApplyingPlatformTunnel,
                        format!("engine: nrpt claim failed: {}", claim.errors.join("；")),
                        "platform/nrpt_takeover_failed",
                    ));
                }
                // claim 已有前置快照是重试的等效输入；错误先返回，不追加 reaccept。
                // 同步 trace 与实际 worker 消费者，接管仍自行精确定位非等效目标。
                let can_skip_worker = claim.can_skip_worker(&config.campus_dns_domains);
                nrpt_acceptance.decision = claim.decision;
                nrpt_external_forms = nrpt_acceptance.accept_forms().to_vec();
                if !claim.foreign_present.is_empty() {
                    if is_cancelled() {
                        return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
                    }
                    let outcome =
                        self.inner_runtime()
                            .block_on(crate::nrpt_takeover::takeover_foreign_nrpt(
                                &config.campus_dns_domains,
                                &campus_dns_servers,
                                &mut covered_fingerprints,
                                &ctx.log,
                                &diagnostics.operation_id,
                            ));
                    takeover_engaged |= outcome.engaged();
                    if let Some(failure) = &outcome.failed {
                        return Err(self.fail_after_takeover(
                            live,
                            ConnectPhase::ApplyingPlatformTunnel,
                            format!(
                                "engine: nrpt takeover (retry) {}: {}",
                                failure.detail(),
                                failure.message()
                            ),
                            "platform/nrpt_takeover_failed",
                        ));
                    }
                    if !outcome.rebuilt_forms.is_empty() {
                        return Err(self.fail_after_takeover(
                            live,
                            ConnectPhase::ApplyingPlatformTunnel,
                            format!(
                                "engine: nrpt takeover (retry) rebuilt: {}",
                                outcome.rebuilt_forms.join(",")
                            ),
                            "platform/nrpt_foreign_rebuild",
                        ));
                    }
                    nrpt_taken_over_forms.extend(outcome.taken_over_forms.iter().cloned());
                }
                if is_cancelled() {
                    return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
                }
                // 本次删除的 own 可能仍留在旧 effective 投影中；原 worker 在 Add 前
                // 选择仍在的等效 foreign，已消失形式继续真实 Add。
                campus_dns_start = if can_skip_worker {
                    Ok(None)
                } else {
                    crate::nrpt_worker::NrptWorkerLease::start(
                        &config.campus_dns_domains,
                        &campus_dns_servers,
                        nrpt_acceptance.accept_forms(),
                    )
                };
                // 重试取得另一份 lease 时另行登记；等效接纳的 Ok(None) 不造条目。
                live.hold_nrpt_result(&mut campus_dns_start)?;
            }
            match campus_dns_start {
                Ok(worker) => {
                    if let Some(marker) = live
                        .campus_dns
                        .as_ref()
                        .and_then(|lease| lease.owner_guid())
                    {
                        nrpt_owner_guids.push(marker.to_owned());
                    }
                    debug_assert!(worker.is_none(), "lease already held by attempt owner");
                }
                Err(error) => {
                    // 接管已介入后的残余 conflict = 外来软件持续重建（单指纹单次覆盖
                    // 硬闸拒绝二次覆盖）→ rebuild 收口；未介入的 conflict（如认领
                    // 清理未竟的自有残留）维持既有泛化语义（reason=""，T-22）。
                    let reason = if error.is_conflict() && takeover_engaged {
                        "platform/nrpt_foreign_rebuild"
                    } else {
                        error.reason
                    };
                    let message = error.message.clone();
                    debug_assert!(
                        error.worker.is_none(),
                        "failed lease already held by attempt owner"
                    );
                    let mut failure = TunnelError::plain_with_reason(
                        ConnectPhase::ApplyingPlatformTunnel, message, reason,
                    );
                    failure.code = error.native_code.unwrap_or(0);
                    return Err(failure);
                }
            }
            self.install_live(live.into_live(), &ctx.status, &ctx.stats, &ctx.operation_id)?;
            // T4 装配成功回写：trace connecting → live + 网卡/NRPT owner/engine PID 补丁
            // （端口清单暂空——系统代理 PAC 回环端口深藏于 route owner 的 teardown 所有权，
            // 与预检代理域的接线一并后置）。失败只告警：装配已成功，痕迹写失败不得
            // 拆毁活隧道；connecting 残留下次预检兜底。
            if let Some(gate) = &self.trace_gate {
                gate.mark_live(
                    &crate::connection_trace::TracePatch {
                        adapter_names: Some(vec![self.adapter_name.clone()]),
                        nrpt_owner_guids: Some(nrpt_owner_guids),
                        // 台账标记（防死锁约束的账面落地）：external-provided 形式
                        // 不归 EXV 所有——认领清理/善后只认 owner GUID 前缀，本清单
                        // 仅作取证与下次连接重验的对照，不驱动任何删除。
                        nrpt_external_forms: Some(nrpt_external_forms),
                        // 接管台账（D8 阶段 2）：journal 有完整形状与完成标记，是
                        // `-Restore` 的数据源；teardown 不恢复被接管规则（恢复=制造
                        // EXV 不拥有的规则）。
                        nrpt_taken_over_forms: Some(nrpt_taken_over_forms),
                        rollback_unresolved: Some(Vec::new()),
                        proxy_ports: Some(Vec::new()),
                        engine_pid: Some(std::process::id()),
                    },
                    &ctx.log,
                );
            }
            ctx.log.emit(
                LogLevel::Info,
                "engine",
                "tunnel.dataplane.started",
                "data plane started (diagnostic; state on StreamConnectStatus)",
                &[],
            );
            Ok(facts)
        })();
        result
    }

    /// 接管失败收尾（D8 阶段 2）：拆掉已组装的数据面并返回带稳定 reason 的平台
    /// 失败（与既有 `campus_dns_start_failed` 路径同一清理语义；新建网卡回收）。
    fn fail_after_takeover(
        &self,
        _live: AttemptOwner,
        phase: ConnectPhase,
        detail: String,
        reason: &'static str,
    ) -> TunnelError {
        TunnelError::plain_with_reason(phase, detail, reason)
    }
}

/// **测试注入 fake**（非生产路径）：契约测试用——同步发射与真实运行时一致的相位
/// 推进（ApplyingPlatformTunnel → StartingDataPlane → Connected）但不做任何真实
/// 工作。生产路径（`main.rs`）一律注入 [`RealTunnelRuntime`]；本 fake 只出现在
/// 测试构造（`HelperControlService::new()` 默认）。
///
/// 与旧占位信号的本质区别：fake 是**测试替身**（生产默认 = 真实运行时），旧占位是
/// **生产代码凭空记账**——R1b 已把生产默认切到真实组装。
#[derive(Debug, Default)]
pub struct FakeTunnelRuntime {
    connected: std::sync::atomic::AtomicBool,
    /// 断开/清理调用计数（测试观测；disconnect 与 teardown 都累加——契约测试只断言
    /// 调用发生，不区分语义）。
    pub teardown_count: std::sync::atomic::AtomicUsize,
    /// apply 调用计数（测试观测）。
    pub apply_count: std::sync::atomic::AtomicUsize,
    /// Paused 标记（D13：disconnect 后置位，start_apply 清）。
    paused: std::sync::atomic::AtomicBool,
}

impl FakeTunnelRuntime {
    /// 建一个 fake（计数从 0 开始）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl TunnelRuntime for FakeTunnelRuntime {
    fn start_apply(self: Arc<Self>, ctx: ApplyContext) -> Result<(), TunnelError> {
        self.apply_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.paused
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // 与真实运行时一致的相位推进（test seam：不触碰网络/特权资源）。受理即回：
        // 相位在 start_apply 内同步发射（A1b 下 handler 返回 pending 后契约测试读取）；
        // 终态 Connected + stats Connected 同真实路径。
        ctx.status.publish(StatusEvent::connecting(
            ctx.operation_id.clone(),
            ConnectPhase::ApplyingPlatformTunnel,
        ));
        ctx.status.publish(StatusEvent::connecting(
            ctx.operation_id.clone(),
            ConnectPhase::StartingDataPlane,
        ));
        ctx.stats
            .registry()
            .set_phase(generated::StatsPhase::Connected);
        ctx.status
            .publish(StatusEvent::connected(ctx.operation_id.clone()));
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn cancel(&self) {
        // fake 无真实组装可取消；no-op（语义保留）。
    }

    fn disconnect(&self) -> Result<(), String> {
        self.teardown_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // 减负断开：置 Paused 标记（D13）。
        self.paused.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn teardown(&self) -> Result<(), String> {
        self.teardown_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn is_assembling(&self) -> bool {
        false
    }

    fn is_paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// 单元测试：纯逻辑（错误分类：login a0/SAML 映射、plain 阶段；fake 相位推进）。
// ---------------------------------------------------------------------------

