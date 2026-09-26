
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

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_cstp::connector::{BootstrapConfig, TrustPolicy};
use exv_vpn_cstp::session::{CstpControlEvent, CstpSession, SessionError};
use exv_vpn_cstp::webvpn::{LoginError, WebvpnLogin};
use exv_vpn_win32_config::ConnectionMode;
use exv_vpn_wire::generated;
use generated::ConnectPhase;
use tokio::sync::mpsc;

use crate::data_plane::{
    DataPlaneAux, EngineDataPlane, EngineDataPlaneThreads, LatencyProbeConfig, WINTUN_RING_CAPACITY,
};
use crate::log_sink::{LogLevel, LogSink};
use crate::platform_tunnel::{NicOwner, PlatformFacts, RouteCleanupObligations, RouteOwner};
use crate::secret_payload::EngineCredentials;
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
    /// 一次性 CSTP 凭据（`EngineCredentials`，零化类型；登录消费后立即 zeroize）。
    /// `None` = 未提供凭据（真实运行时 fail closed；fake 容忍）。
    pub credentials: Option<EngineCredentials>,
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
}

/// SAML 要求的特定 domain 错误码：MVP 不支持交互式 SAML，engine 诚实标记。
const ERROR_CODE_SAML_REQUIRED: u32 = 1;
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
/// 记录，绝不落 wire（spec §10：wire 只带结构化字段）。
///
/// 精确映射表（按 `TunnelErrorKind` 判别）：
/// - `DuplicateConnect` → `CONNECT_IN_PROGRESS(3)`
/// - `MissingCredentials` → `INVALID_INPUT(1)`
/// - `LoginRejected` → `UNAUTHORIZED(15)`（唯一真实的认证拒绝）
/// - `LoginSaml` → `SAML_REQUIRED(1)`
/// - `Transport` → `DEADLINE_EXCEEDED(16)`
/// - `Platform` → `EFFECT_UNKNOWN(13)`
pub(crate) fn tunnel_error_to_wire(err: &TunnelError) -> generated::VpnError {
    // wire code 判别值（common.proto 定义）。
    const WIRE_CONNECT_IN_PROGRESS: i32 = 3;
    const WIRE_INVALID_INPUT: i32 = 1;
    const WIRE_UNAUTHORIZED: i32 = 15;
    const WIRE_SAML_REQUIRED: i32 = 1;
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
        "tunnel assembly failed"
    );
    generated::VpnError {
        code,
        stage: if matches!(err.kind, TunnelErrorKind::PlatformDependency | TunnelErrorKind::Platform) {
            if err.connect_phase == ConnectPhase::ObservingOwnedState {
                generated::ErrorStage::ObservingOwnedState as i32
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
            code: i64::from(if matches!(err.kind, TunnelErrorKind::PlatformDependency | TunnelErrorKind::Platform) {
                err.code
            } else {
                err.a0
            }),
        }),
    }
}

impl TunnelError {
    /// 认证阶段失败：从 `LoginError` 读网关 `a0` 结果码与 SAML 标记。
    fn login(err: &LoginError) -> Self {
        let a0 = err
            .a0_result()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let (kind, code) = if matches!(err, LoginError::SamlRequired) {
            (TunnelErrorKind::LoginSaml, ERROR_CODE_SAML_REQUIRED)
        } else {
            (TunnelErrorKind::LoginRejected, 0)
        };
        Self {
            kind,
            connect_phase: ConnectPhase::ConnectingControl,
            code,
            a0,
            detail: format!("engine: login:{err:?}"),
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
        }
    }
}

/// `start_apply` 启动决策（P2：已连接时采纳复用，不拒绝）。
///
/// 纯函数——仅由 `assembling`/`is_connected` 两个快照输入决定，无副作用、可单测。
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

/// 决策逻辑（纯函数，可单测）。
#[inline]
fn decide_start_apply(is_assembling: bool, is_connected: bool) -> StartDecision {
    if is_assembling {
        StartDecision::RefuseDuplicate
    } else if is_connected {
        StartDecision::Adopt
    } else {
        StartDecision::Proceed
    }
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
    lost_tx: std::sync::mpsc::Sender<DataPlaneLost>,
}

/// **session owner + route owner 的 per-connection 存活状态**（apply 成功后的存活状态；
/// 减负断开 / 退出清理消费）。NIC owner 独立于本结构常驻（`RealTunnelRuntime.nic`）。
///
/// **字段声明顺序即 Drop 顺序（W17 SAFETY-ORDER + 资源逆序）**：`data_plane_threads`
/// （先 join）→ `data_plane`（session `WintunEndSession`）→ `cstp`（CSTP 通道 drop →
/// TLS 数据任务退出）→ `route`（restore 地址/路由/DNS，adapter 保留）——任何退出路径
/// 都保证 session 先于 adapter 结束、CSTP 任务先于 adapter close。
struct LiveTunnel {
    connection_mode: ConnectionMode,
    data_plane_threads: Option<EngineDataPlaneThreads>,
    data_plane: Option<EngineDataPlane>,
    cstp: Option<EngineCstp>,
    route: Option<RouteOwner>,
    diagnostics: Option<RuntimeDiagnostics>,
}

impl LiveTunnel {
    fn cleanup_only(
        route: RouteOwner,
        connection_mode: ConnectionMode,
        diagnostics: RuntimeDiagnostics,
    ) -> Self {
        Self {
            connection_mode,
            data_plane_threads: None,
            data_plane: None,
            cstp: None,
            route: Some(route),
            diagnostics: Some(diagnostics),
        }
    }
    /// **drop_live——F6 共用清理主体**（`TunnelRuntime::disconnect`（S7）与掉线
    /// sweeper（S4）共用；区别只在调用方的 paused 置位时机——两个路径最终都置
    /// `paused=true`）：停数据面线程（先 join）→ 结束 session → 关闭 CSTP 通道 →
    /// 逆序清 route（地址/路由/DNS，**adapter 保留**）。任一阶段错误即返回（W17
    /// SAFETY-ORDER 仍由字段 Drop 兜底）。
    ///
    /// 调用纪律（F6）：sweeper 侧**锁内仅 take、锁外调用**本函数——绝不持 live 锁
    /// 执行（内含 join，持锁 join 有死锁风险）。
    fn disconnect(&mut self) -> Result<(), String> {
        self.disconnect_for("runtime_disconnect")
    }

    fn disconnect_for(&mut self, reason: &str) -> Result<(), String> {
        let started = Instant::now();
        if let Some(diag) = &self.diagnostics {
            diag.emit("tunnel.runtime.live_cleanup", reason, "started", []);
        }
        let result = (|| {
            // 1. 停数据面线程（先 join，再放 session Arc 克隆；W17 SAFETY-ORDER）。
            if let Some(mut threads) = self.data_plane_threads.take() {
                let _ = threads.stop_and_join();
            }
            // 2. 结束 session（drop data plane → `WintunEndSession`）——必须在 adapter
            //    creator close 之前（W17：session 先于 adapter）。
            self.data_plane = None;
            // 3. 关闭 CSTP 通道（drop write/read channel → TLS 任务退出）。
            self.cstp = None;
            // 4. 逆序清 route（地址/路由/MTU/DNS）——adapter 保留（D12 网卡惰性存续）。
            if let Some(route) = self.route.as_mut() {
                route.clear()?;
            }
            self.route = None;
            Ok(())
        })();
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
        result
    }
}

impl Drop for LiveTunnel {
    fn drop(&mut self) {
        let had_resources = self.data_plane_threads.is_some()
            || self.data_plane.is_some()
            || self.cstp.is_some()
            || self.route.is_some();
        let started = Instant::now();
        if had_resources {
            if let Some(diag) = &self.diagnostics {
                diag.emit(
                    "tunnel.runtime.live_cleanup",
                    "live_drop_fallback",
                    "started",
                    [],
                );
            }
        }
        // 兜底：任何未显式 disconnect 的退出路径都先 join 数据面、再逆序清理。
        if let Some(mut threads) = self.data_plane_threads.take() {
            let _ = threads.stop_and_join();
        }
        self.data_plane = None;
        self.cstp = None;
        // route 兜底清理（best-effort；adapter 由 NicOwner 在退出序 close）。
        let result = if let Some(mut route) = self.route.take() {
            route.clear()
        } else {
            Ok(())
        };
        if had_resources {
            if let Some(diag) = &self.diagnostics {
                diag.emit(
                    "tunnel.runtime.live_cleanup",
                    "live_drop_fallback",
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
            // 与新连接的资源应用串行；take(None) 不能代表旧路由已经清理完成。
            // 数据面线程从不取得此锁，因此清理时可以安全 join。
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
                        *rt.live.lock().expect("live lock") = Some(tunnel);
                    }
                    // S4：掉线清 live 与减负断开同语义——置 Paused（下次连接
                    // start_apply 清）。
                    rt.paused.store(true, Ordering::SeqCst);
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
    /// 专用异步运行时（登录/CSTP；`block_on`，不阻塞调用方 async worker）。
    rt: tokio::runtime::Runtime,
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
    /// 旧会话清理与新会话 NIC/路由应用的串行边界；工作线程不参与此锁。
    resources: Mutex<()>,
    /// 取消与最终 Connected 提交的短临界区；不覆盖登录或 join。
    lifecycle_commit: Mutex<()>,
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
            rt: tokio::runtime::Runtime::new()
                .expect("engine: tokio runtime（登录/CSTP 需要异步运行时）"),
            wintun_dll,
            adapter_name,
            config_dir,
            core_user_sid,
            journal,
            nic: Mutex::new(None),
            live: Mutex::new(None),
            pending_routes: Mutex::new(Vec::new()),
            resources: Mutex::new(()),
            lifecycle_commit: Mutex::new(()),
            assembling: std::sync::atomic::AtomicBool::new(false),
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            paused: std::sync::atomic::AtomicBool::new(false),
            nic_created_count: std::sync::atomic::AtomicUsize::new(0),
            nic_reused_count: std::sync::atomic::AtomicUsize::new(0),
            generation: std::sync::atomic::AtomicU64::new(0),
            diagnostics: Mutex::new(None),
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
        exv_vpn_win32_config::ExvConfig::load_from_dir(&self.config_dir)
            .map_err(|e| format!("config-load:{e}"))
    }

    /// 立即延迟刷新标记与服务安装时固定的用户配置目录保持一致。
    fn refresh_marker_path(&self) -> PathBuf {
        self.config_dir.join(LATENCY_REFRESH_FILE)
    }

    /// 调用方持有 resources 锁；硬失败的清理义务保留到下一次操作。
    fn retry_pending_routes(&self) -> Result<(), String> {
        let mut pending = self
            .pending_routes
            .lock()
            .map_err(|_| "pending routes lock".to_string())?;
        let groups_before = pending.len();
        let mut errors = Vec::new();
        for routes in pending.iter_mut() {
            if let Err(error) = routes.retry_clear() {
                errors.push(error);
            }
        }
        pending.retain(|routes| !routes.is_empty());
        if groups_before > 0 {
            if let Some(diag) = self.diagnostic_context() {
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
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// 登录前清理旧会话，避免旧网关 /32 影响新模式的系统选路。
    fn clear_previous_connection(&self) -> Result<(), String> {
        let _resources = self
            .resources
            .lock()
            .map_err(|_| "runtime resources lock".to_string())?;
        let previous = self
            .live
            .lock()
            .map_err(|_| "live lock".to_string())?
            .take();
        if let Some(mut previous) = previous {
            if let Err(error) = previous.disconnect_for("before_new_connection") {
                *self.live.lock().map_err(|_| "live lock".to_string())? = Some(previous);
                return Err(error);
            }
        }
        self.retry_pending_routes()
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
            *self.live.lock().expect("live lock") = Some(live);
            return Err(error);
        }
        Ok(())
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

impl TunnelRuntime for RealTunnelRuntime {
    fn start_apply(self: Arc<Self>, ctx: ApplyContext) -> Result<(), TunnelError> {
        use std::sync::atomic::Ordering;
        let _commit = self.lifecycle_commit.lock().expect("runtime commit lock");
        let decision =
            decide_start_apply(self.assembling.load(Ordering::SeqCst), self.is_connected());
        match decision {
            StartDecision::RefuseDuplicate => {
                return Err(TunnelError {
                    kind: TunnelErrorKind::DuplicateConnect,
                    connect_phase: ConnectPhase::ApplyingPlatformTunnel,
                    code: 0,
                    a0: 0,
                    detail: "engine: duplicate connect (assembling)".to_string(),
                });
            }
            StartDecision::Adopt => {
                // P2：服务引擎已有活跃隧道——采纳，不重建。直接为本次 operation
                // 发布 Connected 状态，复用现有隧道。
                // 掉线报告与身份转交由线程组的同一短锁串行；死亡后的旧会话不能
                // 被包装成新 operation，存活会话转交后的失败必须携带新的身份。
                let rebound = self
                    .live
                    .lock()
                    .expect("live lock")
                    .as_ref()
                    .filter(|live| live.connection_mode == ctx.connection_mode)
                    .and_then(|live| live.data_plane_threads.as_ref())
                    .is_some_and(|threads| {
                        threads.rebind_operation_with(&ctx.operation_id, || {
                            ctx.stats
                                .registry()
                                .set_phase(generated::StatsPhase::Connected);
                            ctx.status
                                .publish(StatusEvent::connected(ctx.operation_id.clone()));
                        })
                    });
                if rebound {
                    return Ok(());
                }
            }
            StartDecision::Proceed => { /* 正常进入组装 */ }
        }
        let diagnostics = RuntimeDiagnostics::new(Arc::clone(&ctx.log), &ctx.operation_id);
        let connection_mode = ctx.connection_mode;
        ctx.log.emit(
            LogLevel::Info,
            "connection",
            "tunnel.connection_mode.selected",
            "本次连接使用已冻结的连接模式",
            &[
                ("connection_mode", connection_mode.as_str()),
                ("operation_id", &diagnostics.operation_id),
            ],
        );
        diagnostics.emit("tunnel.runtime.apply", "apply_accepted", "started", []);
        *self.diagnostics.lock().expect("diagnostics lock") = Some(diagnostics);
        // 重置取消令牌 → 清 Paused → 标记在途 → 后台 thread 跑组装（受理即回）。
        self.cancel_flag.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        self.assembling.store(true, Ordering::SeqCst);
        let rt = Arc::clone(&self);
        let cancel = Arc::clone(&self.cancel_flag);
        // 终态由后台 thread 统一发布（受理即回后 handler 无法知道结果）：成功 →
        // Connected + stats Connected；失败/取消 → Failed + stats Failed。
        let op_id = ctx.operation_id.clone();
        let status = Arc::clone(&ctx.status);
        let stats = Arc::clone(&ctx.stats);
        std::thread::spawn(move || {
            let result = RealTunnelRuntime::run_assemble_with_reset(
                &rt, ctx, cancel, &stats, &status, &op_id,
            );
            // run_assemble_with_reset 内部处理 assemble 结果的 status 推送；
            // assembling 复位在该函数内完成（含 panic 保护）。
            let _ = result;
        });
        Ok(())
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
        // 先撤销最终提交许可，再等旧资源清理；即使调用方没有单独 cancel，
        // 排队等待资源锁期间也不能新提交 Connected。
        {
            let _commit = self
                .lifecycle_commit
                .lock()
                .map_err(|_| "runtime commit lock".to_string())?;
            self.cancel_flag
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let diagnostics = self.diagnostic_context();
        let started = Instant::now();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.disconnect",
                "caller_disconnect_request",
                "started",
                [],
            );
        }
        let result = (|| {
            use std::sync::atomic::Ordering;
            let _resources = self
                .resources
                .lock()
                .map_err(|_| "runtime resources lock".to_string())?;
            let mut live = {
                let _commit = self
                    .lifecycle_commit
                    .lock()
                    .map_err(|_| "runtime commit lock".to_string())?;
                self.cancel_flag.store(true, Ordering::SeqCst);
                self.live
                    .lock()
                    .map_err(|_| "live lock".to_string())?
                    .take()
            };
            if let Some(tunnel) = live.as_mut() {
                if let Err(error) = tunnel.disconnect() {
                    *self.live.lock().map_err(|_| "live lock".to_string())? = live;
                    return Err(error);
                }
            }
            self.retry_pending_routes()?;
            // 减负断开完成：置 Paused 标记（D13；下次连接 start_apply 清）。
            self.paused.store(true, Ordering::SeqCst);
            Ok(())
        })();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.disconnect",
                "caller_disconnect_request",
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
        result
    }

    fn teardown(&self) -> Result<(), String> {
        let diagnostics = self.diagnostic_context();
        let started = Instant::now();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.teardown",
                "caller_teardown_request",
                "started",
                [],
            );
        }
        let result = (|| {
            // 退出清理（D12/D15）：先减负断开（session-end + route 清 + 有界 join），再
            // NIC owner 关 adapter（0 网卡残留兜底）。
            self.disconnect()?;
            let mut nic = self.nic.lock().map_err(|_| "nic lock".to_string())?;
            // drop NicOwner = adapter creator close 移除 adapter + 释放 lib。
            let _ = nic.take();
            Ok(())
        })();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.teardown",
                "caller_teardown_request",
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
        result
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
            if let Err(error) = pending.retry_clear() {
                if let Some(diag) = &diagnostics {
                    diag.emit(
                        "tunnel.route.cleanup",
                        "runtime_drop",
                        "failed",
                        [("detail".into(), error)],
                    );
                }
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
    }
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
        use std::sync::atomic::Ordering;
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
                let detail = err.detail.clone();
                let phase = err.connect_phase;
                log.emit(
                    LogLevel::Warn,
                    "engine",
                    "tunnel.assemble.failed",
                    "tunnel assembly failed (diagnostic; state on StreamConnectStatus)",
                    &[("detail", &detail), ("phase", &format!("{phase:?}"))],
                );
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
                // 不让 assembling 永真导致后续连接全部被拒。
                stats.registry().set_phase(generated::StatsPhase::Failed);
                tracing::error!("tunnel assembly panicked (catch_unwind captured)");
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
                    },
                ));
            }
        }
        // P3：无论如何都复位 assembling——含 panic 路径。
        rt.assembling.store(false, Ordering::SeqCst);
    }

    /// 组装被取消时的受控错误（StopTunnel 中途中止）。
    fn cancelled_error(phase: ConnectPhase) -> TunnelError {
        TunnelError {
            kind: TunnelErrorKind::Platform,
            connect_phase: phase,
            code: 0,
            a0: 0,
            detail: "engine: cancelled".to_string(),
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
        ctx: ApplyContext,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<PlatformFacts, TunnelError> {
        use std::sync::atomic::Ordering;
        let diagnostics = RuntimeDiagnostics::new(Arc::clone(&ctx.log), &ctx.operation_id);
        let connection_mode = ctx.connection_mode;
        let is_cancelled = || cancel_flag.load(Ordering::SeqCst);
        if is_cancelled() {
            return Err(Self::cancelled_error(ConnectPhase::ObservingOwnedState));
        }

        self.clear_previous_connection().map_err(|error| {
            TunnelError::plain(
                ConnectPhase::ObservingOwnedState,
                format!("engine: previous connection cleanup:{error}"),
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

        let config = self.load_config().map_err(|e| {
            TunnelError::plain(ConnectPhase::ConnectingControl, format!("engine: {e}"))
        })?;
        let hostname = config.server.clone();
        let user_agent = config.user_agent.clone();
        let campus_routes = config.routes.clone();

        // 凭据：来自 `ApplyTunnelRequest.secret_payload`（一次性，零化类型）。未提供
        // → fail closed（真实登录必须有凭据；不静默回退）。
        let mut credentials = ctx.credentials.ok_or_else(|| TunnelError {
            kind: TunnelErrorKind::MissingCredentials,
            connect_phase: ConnectPhase::ConnectingControl,
            code: 0,
            a0: 0,
            detail: "engine: no credentials provided".to_string(),
        })?;

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
        .map_err(|e| TunnelError::plain(ConnectPhase::ConnectingControl, format!("engine: {e}")))?;
        let socket_binder = socket_binder_for_mode(connection_mode, physical_ifindex);
        let resolver = gateway_resolver.clone();
        let gateway_addr = self.rt.block_on(resolver(&hostname)).map_err(|e| {
            TunnelError::plain(ConnectPhase::ConnectingControl, format!("engine: {e}"))
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
            .rt
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
            .rt
            .block_on(CstpSession::open_with_user_agent_observed(
                cfg,
                Some(&login),
                Some(&user_agent),
                Some(&socket_observer),
            ))
            .map_err(|err| TunnelError::session(err))?;
        // 在写入校园路由前，以长期 CSTP socket 的实际源地址确定出口。
        // 兼容模式不能在此切回物理网卡，否则将改变已经建立的连接路径。
        let (bypass_row, egress_ifindex) = match connection_mode {
            ConnectionMode::Standard => (
                exv_vpn_win32_resource::routes::gateway_bypass_row(physical_nic, gateway_ip)
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
        let offer = session.offer_plan.clone();
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
        // 拆流：write_channel（reader 线程发 TLS）克隆 + read_channel（writer 线程收
        // TLS 解码帧）包 Arc<Mutex>。**先作局部持有**——只有全部可失败步骤（apply /
        // data-plane start）成功后才落进 `LiveTunnel`；任何中途错误路径局部 drop 即关闭
        // CSTP 通道（TLS 任务退出、学校连接断开），不泄漏连接。
        let write_channel = session.write_channel.clone();
        let read_channel = Arc::new(Mutex::new(session.read_channel));
        // T1 latency probe：持有 CSTP 控制面接收端（DPD response），并据真实 offer
        // 构建探测配置（目标 = 客户端隧道子网网络基地址；真实学校网关 S5 验证）。
        let control_rx = Arc::new(Mutex::new(session.control_rx));
        let latency_cfg = LatencyProbeConfig {
            target: network_base(offer.ipv4_address, offer.prefix),
            source: offer.ipv4_address,
            dpd_enabled: DPD_PROBE_ENABLED,
            ping_interval: Duration::from_secs(LATENCY_PING_INTERVAL_SECS),
            refresh_marker: Some(self.refresh_marker_path()),
        };
        if is_cancelled() {
            // 取消：drop 局部 CSTP 通道（TLS 任务退出），无其它资源——直接中断。
            return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
        }

        // ---- 阶段 2：**NIC owner 确保/复用 + route owner 应用**（D11 连接序：session
        //      协商 offer → NIC 确保/复用 adapter → route 应用地址/DNS/路由）。 ----
        let _resources = self.resources.lock().expect("runtime resources lock");
        if is_cancelled() {
            return Err(Self::cancelled_error(ConnectPhase::ApplyingPlatformTunnel));
        }
        // sweeper 可能还未处理掉线信号。新连接既不能 Adopt 死数据面，也不能在旧
        // 路由仍在清理时写入新路由；资源锁覆盖 take、真实清理和本次应用/安装。
        let old_live = self.live.lock().expect("live lock").take();
        if let Some(mut old_live) = old_live {
            old_live
                .disconnect_for("replace_failed_data_plane")
                .map_err(|error| TunnelError::plain(ConnectPhase::ApplyingPlatformTunnel, error))?;
        }
        ctx.status.publish(StatusEvent::connecting(
            ctx.operation_id.clone(),
            ConnectPhase::ApplyingPlatformTunnel,
        ));
        // 2a. NIC owner：首次连接建 adapter（D12），断开后复用（`nic` 已存在）。接口级
        //     配置（DAD 禁用/接口启用）随 ensure 一次完成，跨连接存续。
        let nic_newly_created = {
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
                                    detail: format!("engine: wintun-create:{native:?}"),
                                }
                            }
                            NicInitializationError::Configuration(detail) => TunnelError::plain(
                                ConnectPhase::ApplyingPlatformTunnel,
                                format!("engine: {detail}"),
                            ),
                        }
                    })?;
                *guard = Some(nic);
                self.nic_created_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                true
            } else {
                // D12 复用证据：断开后重连 adapter 句柄未重建。
                self.nic_reused_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ctx.log.emit(
                    LogLevel::Info,
                    "engine",
                    "tunnel.nic.reused",
                    "NIC adapter reused across connections (D12; handle not rebuilt)",
                    &[("reused_count", &self.nic_reused_count().to_string())],
                );
                false
            }
        };

        // 2b. route owner：经 NIC 交出的 LUID 应用真实 offer 五族（D17 + 2026-09-08
        //     计划网关 bypass /32 族）。发起用户 SID 一并传入（系统代理豁免写入该
        //     用户 HKU；None = 跳过 system_proxy 族）。系统代理账本一并传入
        //     （apply 记账 / clear 清账；J4 接线）。bypass 行来自阶段 1 的 VGDC
        //     解析结果（Some 恒成立——构造失败已在阶段 1 fail-closed）。
        let route_result = {
            let guard = self.nic.lock().expect("nic lock");
            let nic = guard.as_ref().expect("nic ensured");
            if bypass_row.interface_luid == nic.luid() {
                Err("engine: gateway bypass points into EXV's own tunnel".into())
            } else {
                nic.apply_offer(
                    &offer,
                    &campus_routes,
                    Some(&bypass_row),
                    self.core_user_sid.clone(),
                    &self.journal,
                )
            }
        };
        let (route, facts) = match route_result {
            Ok(v) => v,
            Err(e) => {
                // all-or-nothing（D16）：新 adapter + 失败 → 移除（0 残留兜底）；复用
                // adapter + 失败 → 保留（RouteOwner::apply 已内部逆序回滚各族含
                // bypass 行，无残留）。
                if nic_newly_created {
                    let _ = self.nic.lock().expect("nic lock").take();
                }
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
                    self.pending_routes
                        .lock()
                        .expect("pending routes lock")
                        .push(e.pending_routes);
                }
                let mut error = TunnelError::plain(
                    ConnectPhase::ApplyingPlatformTunnel,
                    detail,
                );
                error.code = route_native_code.unwrap_or(0);
                return Err(error);
            }
        };
        // 诊断：apply 后立即回读接口地址行（含 DadState）——R0 归因的 Wintun IPv4 DAD
        // 恒 Tentative 问题；真实地址行/状态决定业务流量是否可达。
        let addr_readback =
            exv_vpn_win32_resource::ip_address::IpAddressController::new(facts.luid)
                .capture()
                .map(|rows| {
                    rows.iter()
                        .map(|r| {
                            format!(
                                "{}/{} dad={}",
                                r.address, r.on_link_prefix_length, r.dad_state
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_else(|e| format!("capture-err:{e:?}"));
        let route_readback = exv_vpn_win32_resource::routes::capture_rows(facts.luid)
            .map(|rows| {
                rows.iter()
                    .map(|r| format!("{}/{}", r.network, r.prefix_len))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|e| format!("capture-err:{e:?}"));
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
                ("addr_readback", &addr_readback),
                ("route_readback", &route_readback),
            ],
        );
        if is_cancelled() {
            // 取消：显式逆序清理已建 route（地址/路由/DNS）；若本次新建 adapter 则移除。
            let cleaned = self.cleanup_or_retain(
                LiveTunnel::cleanup_only(route, connection_mode, diagnostics.clone()),
                "cancelled_after_platform_apply",
            );
            if nic_newly_created && cleaned.is_ok() {
                let _ = self.nic.lock().expect("nic lock").take();
            }
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
        let plane = match plane {
            Ok(plane) => plane,
            Err(e) => {
                // 数据面启动失败：显式逆序清理已建 route；若本次新建 adapter 则移除。
                let cleaned = self.cleanup_or_retain(
                    LiveTunnel::cleanup_only(route, connection_mode, diagnostics.clone()),
                    "data_plane_start_failed",
                );
                if nic_newly_created && cleaned.is_ok() {
                    let _ = self.nic.lock().expect("nic lock").take();
                }
                return Err(TunnelError::plain(
                    ConnectPhase::StartingDataPlane,
                    format!("engine: data-plane:{e}"),
                ));
            }
        };
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
                    eprintln!("[info] engine data-plane: lost signal send failed (sweeper gone)");
                }
            })
        };
        let threads = plane.spawn_data_plane(
            write_channel.clone(),
            &read_channel,
            Some(Arc::clone(&ctx.log)),
            DataPlaneAux {
                // T1 Part A：数据面计数（reader→tx/writer→rx，方向映射见
                // data_plane::count_reader_upload/count_writer_download）。
                stats: Some(Arc::clone(ctx.stats.registry())),
                // T1 Part B：延迟探测（DPD→ping fallback + 手动刷新标记）。
                latency: Some(latency_cfg),
                control_rx: Some(Arc::clone(&control_rx)),
                // C2：掉线状态上报（read_channel 关闭 → Failed(DataPlane,
                // RetrySameOperation)；自动重连前置，见 data_plane::on_read_channel_closed）。
                status: Some(Arc::clone(&ctx.status)),
                operation_id: ctx.operation_id.clone(),
                // T3b（F6）：掉线信号发射端——writer 检测 read_channel 关闭时先发
                // 信号（sweeper 清 live）再上报。
                notify_data_plane_lost: Some(Arc::clone(&notify_data_plane_lost)),
            },
        );
        // 全部可失败步骤已成功：落进 LiveTunnel（session owner + route owner 存活；
        // NIC owner 常驻 `self.nic`）。
        self.install_live(
            LiveTunnel {
                connection_mode,
                data_plane_threads: Some(threads),
                data_plane: Some(plane),
                cstp: Some(EngineCstp {
                    write_channel,
                    read_channel,
                    control_rx: Some(control_rx),
                    // P1-1：发送端第二份安放——LiveTunnel drop（disconnect/teardown/
                    // Drop 兜底）即关停 sweeper，覆盖 writer 线程卡死、其闭包副本永不
                    // 释放的泄漏边界。
                    lost_tx,
                }),
                route: Some(route),
                diagnostics: Some(live_diagnostics),
            },
            &ctx.status,
            &ctx.stats,
            &ctx.operation_id,
        )?;
        ctx.log.emit(
            LogLevel::Info,
            "engine",
            "tunnel.dataplane.started",
            "data plane started (diagnostic; state on StreamConnectStatus)",
            &[],
        );
        Ok(facts)
    }

    /// 安装、确认连接、放行工作线程属于同一提交。调用者已持资源锁；取消只等待
    /// 这个短提交，不等待网络登录。发布状态时不持 live 锁，观察者可读取运行状态。
    fn install_live(
        &self,
        live: LiveTunnel,
        status: &StatusPublisher,
        stats: &crate::stats::StatsPublisher,
        operation_id: &[u8],
    ) -> Result<(), TunnelError> {
        use std::sync::atomic::Ordering;
        let _commit = self.lifecycle_commit.lock().expect("runtime commit lock");
        if self.cancel_flag.load(Ordering::SeqCst) {
            drop(_commit);
            let _ = self.cleanup_or_retain(live, "cancelled_before_connected");
            return Err(Self::cancelled_error(ConnectPhase::StartingDataPlane));
        }
        if !live
            .data_plane_threads
            .as_ref()
            .is_some_and(EngineDataPlaneThreads::is_alive)
        {
            drop(_commit);
            let _ = self.cleanup_or_retain(live, "workers_exited_before_connected");
            return Err(TunnelError::plain(
                ConnectPhase::StartingDataPlane,
                "engine: data-plane workers exited before activation",
            ));
        }
        *self.live.lock().expect("live lock") = Some(live);
        self.paused.store(false, Ordering::SeqCst);
        stats.registry().set_phase(generated::StatsPhase::Connected);
        status.publish(StatusEvent::connected(operation_id.to_vec()));
        if let Some(threads) = self
            .live
            .lock()
            .expect("live lock")
            .as_ref()
            .and_then(|live| live.data_plane_threads.as_ref())
        {
            threads.activate();
        }
        Ok(())
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
