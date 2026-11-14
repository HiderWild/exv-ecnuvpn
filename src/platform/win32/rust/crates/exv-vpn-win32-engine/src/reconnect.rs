
//! T6a：R1 引擎侧重连（计划 §2.7 R1——隧道断线、engine 存活 → engine 自愈）。
//!
//! R1 与 R2 的边界（计划 §四 边 19/D10）：**engine 崩溃后任何"engine 内重连"都不
//! 成立**（D9 内存材料随进程消失）——本驱动的全部状态（材料、退避、代次）都在
//! 内存里，engine 重启即清零；R2（engine 崩溃恢复）归 core 侧状态机（T6b），两侧
//! 经 trace 的 `connection_id` 代次互不抢跑（本模块在自愈成功时经
//! [`ConnectGate::update_established_at`] 换新代次，§2.4：engine 是唯一写者）。
//!
//! # 状态机
//!
//! `Armed（Live，材料驻留）→ [隧道异常终止信号] → Reconnecting/Backoff →
//! （Healed → Armed | Backoff' | Terminal）`：
//!
//! - **触发**：数据面掉线 sweeper 清除 live 后回调
//!   [`ReconnectDriver::on_tunnel_lost`]（`tunnel_runtime::live_sweeper_step` 的
//!   `ClearedLive` 分支——信号只在已安装 Live 隧道的数据面死亡时出现，且带世代
//!   守卫，天然满足"session was Live"门控）。触发门控：材料在（受理时
//!   `reconnect.enabled` 才捕获）、无在途自愈循环、隧道确死（`is_connected()`
//!   兜底复核）。**无材料的 worker 死亡**（重连未开启/受理未驻留）走 T5 第五
//!   情形善后（计划 v6.1 §8.2：`abandon_session(DATA_PLANE_WORKER_FAILED)`——
//!   lease 释放复验 + trace 终态化 + 验证收口全链走完，不再静默返回）。
//! - **退避**：[`backoff_delay`]——2s→30s 指数封顶（2,4,8,16,30,30…；不做抖动，
//!   保持可测）。尝试上限 [`MAX_ATTEMPTS`] = 6（五次重试延迟 2/4/8/16/30 完整
//!   实现计划退避表到封顶值；计划未定上限，本值为 D11 邻接的实现裁量），
//!   耗尽即终态。
//! - **自愈（按隧道种类，全部复用既有原语，不改认证设计）**：
//!   - **Cisco**：驻留凭据（version-1 `secret_payload` 字节）→ 重新解析 → 经
//!     [`TunnelRuntime::start_apply`] 全量重跑（登录 → CSTP → 特权 apply → 数据面）。
//!   - **aTrust**：驻留浏览器候选会话（version-2 `BrowserSession` 字节：网关 +
//!     SID cookie + UA）。先做**源会话复核**（既有 `AuthClient::auth_config(false)`
//!     原语，与 `browser_handoff` 交接序列的第一步同款）：`is_login=false` →
//!     认证终态失效（R3）；复核通过才重跑全量交接——交接序列内含接收会话复核
//!     与资源目录重取。材料保留的是**候选会话**而非接收会话：接收会话
//!     （`AuthenticatedAtrustSession`）在现行组装（`atrust_prepare::prepare`）末尾
//!     即释放，且无法脱离浏览器源会话单独换新票据，故保留候选会话是既有原语下
//!     唯一可全量重跑的形态。
//! - **成功**：组装路径自己发布 Connected；本驱动补发布重连遥测
//!   （[`StatusEvent::reconnected`]，`EXV_UNFREEZE` tag 8）+ 计数
//!   （`reconnect_attempts_total` / `reconnect_successes_total`，`StatsEvent`
//!   tag 9/10）+ trace 换新（§2.4 `update_established_at`；采纳路径不触碰）。
//!
//!   材料保留、尝试计数随成功归零（可再次自愈）。
//! - **终态（认证拒绝 / 上限耗尽）→ 善后分叉（计划 §2.7 R1 括注）**：
//!   - **无头**（`HeadlessState` 在册 = core 确死且策略 keep）：engine 完成
//!     善后——`AftermathDriver::abandon_session(R1_TERMINAL)`（T5 钩子：cancel →
//!     teardown → 痕 Aftermath → 预检复验 → 干净删痕 → Idle 事件）。
//!   - **core 在场**：engine 只清残留（幂等减负断开；NIC 保留 D12）+ 痕 →
//!     Aftermath + 发布 R1 终态事件（core 驱动后续善后，T6b/UI；**不删痕**——
//!     删除需验证收口，由 core 反应路径或下次连接门预检完成）。
//!   - 两种分叉都**即刻零化材料**（终态不循环）。
//!
//! # 材料驻留（D9）
//!
//! [`RetainedMaterial`] 持有受理工 `secret_payload` 的**原始 wire 字节副本**
//! （`Zeroizing`：drop/终态/换装即零化；绝不落盘、绝不入日志——日志字段只有
//! 种类与 reason id）。副本仅在 `reconnect.enabled` 时于受理点制造；engine 退出/
//! 重启随进程消失（这正是 R1 与 R2 的分界）。
//!
//! # 红线
//!
//! 本模块不含任何测试注入接缝：自愈经真实 [`TunnelRuntime`] 接缝执行；单测用
//! `#[cfg(test)]` 内的脚本化运行时（与 T5 `SlowAssembleRuntime` 同款纪律）。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use exv_vpn_atrust::Secret;
use exv_vpn_atrust::auth::{AuthClient, AuthClientOptions};
use exv_vpn_win32_config::ConnectionMode;
use exv_vpn_wire::generated::StatsPhase;
use exv_vpn_wire::generated::VpnError;
use zeroize::Zeroizing;

use crate::aftermath::AftermathDriver;
use crate::connect_gate::ConnectGate;
use crate::connection_trace::TraceKind;
use crate::log_sink::{LogLevel, LogSink};
use crate::secret_payload::{ConnectionCredentials, parse_connection_credentials};
use crate::status::{StatusEvent, StatusPublisher};
use crate::stats::StatsPublisher;
use crate::tunnel_runtime::{ApplyContext, TunnelRuntime};

/// 重连 reason id（稳定契约：事件遥测与日志共用；UI 文案按 id 路由）。
pub mod reason {
    /// 触发：隧道异常终止（数据面掉线）。
    pub const TUNNEL_LOST: &str = "tunnel_lost";
    /// 成功：隧道已自愈恢复。
    pub const HEALED: &str = "healed";
    /// 瞬态：自愈组装失败（网络/平台类，待退避重试）。
    pub const HEAL_ASSEMBLY_FAILED: &str = "heal_assembly_failed";
    /// 瞬态：自愈组装终态未在期限内出现（按瞬态退避处理）。
    pub const HEAL_TIMEOUT: &str = "heal_timeout";
    /// 终态：驻留材料复核/消费被拒（Cisco 登录拒绝 `a0≠0` / SAML 要求 / aTrust
    /// 源会话未登录）——R3 认证终态失效，不循环。
    pub const AUTH_REJECTED: &str = "auth_rejected";
    /// 终态：退避尝试上限耗尽。
    pub const ATTEMPTS_EXHAUSTED: &str = "attempts_exhausted";
}

/// 退避基值（计划 §2.7：2s 起）。
const BACKOFF_BASE: Duration = Duration::from_secs(2);
/// 退避封顶（计划 §2.7：30s）。
const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// 尝试上限：六次尝试用满五次重试延迟（2/4/8/16/30）实现完整退避表（D11 邻接
/// 裁量：计划未定上限；取"完整实现退避表所需的最少尝试数"）。
const MAX_ATTEMPTS: u32 = 6;
/// 单次自愈组装的墙钟上界（登录/交接的逐请求期限为 30s 级；超时按瞬态退避）。
const HEAL_BOUND: Duration = Duration::from_mins(2);
/// 自愈终态轮询周期（`status.latest()` 观察）。
const HEAL_POLL: Duration = Duration::from_millis(250);

/// wire `ErrorCode::UNAUTHORIZED` 判别值（`tunnel_error_to_wire` 的登录拒绝映射）。
const WIRE_UNAUTHORIZED: i32 = 15;
/// wire `ErrorCode::SAML_REQUIRED` 判别值（EXV_UNFREEZE 2026-10-09 修撞码：旧值 1
/// 与 `INVALID_INPUT` 同值）。本驱动自建 ctx 不缺输入，收到此码即交互式 SAML
/// 要求——无头不可交互，按认证终态收口。`tunnel_error_to_wire` 的同名常量同改
/// 20（SAML 三落点之二，复审 2.2-2：漏改则 R1 自愈遇 SAML 从立即终态退化为六次
/// 退避耗尽）。
const WIRE_SAML_REQUIRED: i32 = 20;
/// wire `ErrorCode::DEADLINE_EXCEEDED` 判别值（尝试耗尽的"超时类"终态映射）。
const WIRE_DEADLINE_EXCEEDED: i32 = 16;
/// wire `ErrorStage::Ingress` 判别值（engine 内发起、无外部操作主体）。
const WIRE_STAGE_INGRESS: i32 = 1;
/// wire `RetryAdvice::DoNotRetry` 判别值（终态不循环）。
const WIRE_RETRY_NONE: i32 = 1;

/// 退避延迟（纯函数，可单测）：第 `attempt` 次失败后的下一次尝试延迟，
/// 2s 指数增长、30s 封顶（2,4,8,16,30,30…）。
#[must_use]
pub fn backoff_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(31);
    BACKOFF_BASE.saturating_mul(1u32 << shift).min(BACKOFF_CAP)
}

/// D9 材料内存驻留：受理工 `secret_payload` 的原始 wire 字节副本（零化容器）。
///
/// 保留形态按种类（文档化契约）：
/// - **Cisco**：version-1 JSON（username + password）——heal 时重新解析后随
///   `ApplyContext` 走，登录消费即零化（一次性契约不变，副本只多驻留这一份）；
/// - **aTrust**：version-2 `BrowserSession`（网关 + `sid` Cookie 头 + UA）——
///   heal 时 `decode` 出新实例做源会话复核 + 全量交接。
pub struct RetainedMaterial {
    secret_payload: Zeroizing<Vec<u8>>,
}

impl RetainedMaterial {
    /// 从受理工的 wire 字节制造驻留副本（调用点负责仅在重连开启时复制）。
    #[must_use]
    pub fn new(secret_payload: Vec<u8>) -> Self {
        Self {
            secret_payload: Zeroizing::new(secret_payload),
        }
    }

    /// 每次自愈尝试的材料副本（副本随 `parse_connection_credentials` 的
    /// 一次性契约就地零化）。
    fn copy_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.secret_payload.as_slice().to_vec())
    }
}

impl std::fmt::Debug for RetainedMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedMaterial")
            .field("bytes", &"<redacted>")
            .finish()
    }
}

/// 一次受连接的会话级驻留（材料 + 组装上下文快照）。
pub struct RetainedSession {
    /// 首连受理的逻辑身份；R1 内部重试不重新铸造公开身份。
    operation_id: Vec<u8>,
    /// 隧道种类（决定 heal 路径与 aTrust 前置复核）。
    kind: TraceKind,
    /// 冻结的连接模式（标准/兼容；自动重连沿用，不读实时设置）。
    connection_mode: ConnectionMode,
    atrust_dns_enabled: bool,
    /// 冻结的 domain 隧道计划（`opaque_intent` 关联用；真实 apply 以协商 offer 为准）。
    plan: exv_vpn_domain::ports::TunnelPlan,
    /// 认证材料（D9）。
    material: RetainedMaterial,
}

impl RetainedSession {
    pub fn with_atrust_dns_enabled(mut self, enabled: bool) -> Self {
        self.atrust_dns_enabled = enabled;
        self
    }

    /// 组装会话驻留（`grpc_server::apply_tunnel` 在重连开启时捕获）。
    #[must_use]
    pub fn new(
        operation_id: Vec<u8>,
        kind: TraceKind,
        connection_mode: ConnectionMode,
        plan: exv_vpn_domain::ports::TunnelPlan,
        material: RetainedMaterial,
    ) -> Self {
        Self {
            operation_id,
            kind,
            connection_mode,
            atrust_dns_enabled: false,
            plan,
            material,
        }
    }
}

/// 一次自愈尝试的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealOutcome {
    /// 隧道已恢复 Live（组装路径已发布 Connected）。
    Healed,
    /// 瞬态失败（退避后重试），携带 reason id。
    Transient(&'static str),
    /// 终态（认证拒绝 / 材料失配——不循环）。
    Terminal(&'static str),
    /// 会话已被换装/解除（新受理或用户断开）——静默退出。
    Aborted,
}

/// R1 引擎侧重连驱动。强引用由 `HelperControlService` 持有；`RealTunnelRuntime`
/// 经 `attach_reconnect` 持 **Weak**（sweeper 回调用；Weak 防止 运行时↔驱动 强
/// 引用环阻止 `RealTunnelRuntime::Drop` 的兜底清理）。
pub struct ReconnectDriver {
    /// 连接门（trace 单写者：成功换新代次 / 终态 Aftermath）。
    gate: Option<Arc<ConnectGate>>,
    /// 数据面运行时（自愈经 `start_apply` 全量重跑既有组装）。
    runtime: Arc<dyn TunnelRuntime>,
    /// 结构化日志（摘要事件；材料绝不入日志）。
    log: Arc<LogSink>,
    /// 连接状态通道（重连遥测 `EXV_UNFREEZE` tag 8）。
    status: Arc<StatusPublisher>,
    /// 统计相位（R1 计数 tag 9/10 + 终态相位）。
    stats: Arc<StatsPublisher>,
    /// T5 兜底善后（无头终态钩子）。
    aftermath: Arc<AftermathDriver>,
    /// 无头态（终态分叉判定：在册 = core 确死，engine 驱动善后）。
    headless: Arc<crate::aftermath::HeadlessState>,
    /// 内部状态（材料 + 自愈循环占用 + 会话代次）。
    inner: Mutex<DriverInner>,
}

#[derive(Default)]
struct DriverInner {
    /// 当前驻留会话（None = 未武装/已终态/已解除）。
    session: Option<RetainedSession>,
    /// 自愈循环是否在途（触发去重）。
    in_flight: bool,
    /// 会话代次（arm/disarm 递增；循环据此察觉会话已被换装而退出）。
    epoch: u64,
    /// arm 时捕获的 tokio 句柄（sweeper 线程无异步上下文，用它 spawn 自愈循环）。
    handle: Option<tokio::runtime::Handle>,
    /// 最近一次解除原因（arm 清空）。`user_stop` / R1 终态已有各自的善后链
    /// （StopTunnel 的 verify_disconnect_aftermath / terminal 分叉），第五情形
    /// 不重复驱动；None + session 空 = 从未武装（重连未开启）——worker 死亡的
    /// 善后无人接管，第五情形负责。
    disarm_reason: Option<&'static str>,
}

impl ReconnectDriver {
    /// 全接缝构造（生产由 `grpc_server::with_real_tunnel` 组装）。
    #[must_use]
    #[allow(clippy::similar_names)] // status/stats 是两个既有发布器（状态通道/统计）
    pub fn new(
        gate: Option<Arc<ConnectGate>>,
        runtime: Arc<dyn TunnelRuntime>,
        log: Arc<LogSink>,
        status: Arc<StatusPublisher>,
        stats: Arc<StatsPublisher>,
        aftermath: Arc<AftermathDriver>,
    ) -> Self {
        let headless = aftermath.headless();
        Self {
            gate,
            runtime,
            log,
            status,
            stats,
            aftermath,
            headless,
            inner: Mutex::new(DriverInner {
                // 构造点句柄（生产 `with_real_tunnel` 在 async 上下文内；sweeper 线程
                // 无上下文——未武装（第五情形善后）也需可 spawn）。arm 时若在异步
                // 上下文会刷新为当值句柄。
                handle: tokio::runtime::Handle::try_current().ok(),
                ..DriverInner::default()
            }),
        }
    }

    /// 武装：受理连接时捕获会话材料（替换任何先前驻留——新会话取代旧会话）。
    ///
    /// 在异步上下文调用（捕获 tokio 句柄供 sweeper 线程 spawn）。
    pub fn arm(&self, session: RetainedSession) {
        let handle = tokio::runtime::Handle::try_current().ok();
        let kind = session.kind;
        let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.session = Some(session);
        inner.epoch += 1;
        inner.in_flight = false;
        inner.disarm_reason = None;
        if handle.is_some() {
            inner.handle = handle;
        }
        drop(inner);
        self.log.emit(
            LogLevel::Info,
            "engine",
            "reconnect.armed",
            "连接材料已驻留（D9）：隧道异常终止时 engine 自愈（R1）",
            &[("kind", &format!("{kind:?}"))],
        );
    }

    /// 新请求排除旧循环发布；门拒绝不意味着原会话材料或善后责任已结束。
    pub(crate) fn invalidate_for_apply(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.epoch += 1;
        inner.in_flight = false;
    }

    /// 同一 admission 内的同步 runtime 受理。暂移原材料，拒绝即还原；没有第二份材料库。
    /// Adopt 沿原 Live 的 operation 关联重绑定，不改材料、trace identity 或冻结政策。
    pub(crate) fn start_apply_with_session(
        &self,
        session: Option<RetainedSession>,
        adopt: bool,
        operation_id: &[u8],
        start: impl FnOnce() -> Result<
            crate::tunnel_runtime::ApplyAcceptance,
            crate::tunnel_runtime::TunnelError,
        >,
    ) -> Result<(), crate::tunnel_runtime::TunnelError> {
        if adopt {
            let result = start();
            if let Ok(adopted) = &result {
                let mut inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *adopted == crate::tunnel_runtime::ApplyAcceptance::Started {
                    inner.session = None;
                    inner.disarm_reason = None;
                } else if let Some(session) = inner.session.as_mut() {
                    session.operation_id = operation_id.to_vec();
                }
            }
            return result.map(|_| ());
        }
        let previous = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // 新的无 R1 会话仍由 worker-failed 善后；只有真实 Stop/terminal 才负责去重。
            (inner.session.take(), inner.disarm_reason.take())
        };
        if let Some(session) = session {
            self.arm(session); // 保留原句柄捕获、材料驻留和 armed 日志语义。
        }
        let result = start();
        if result.is_err() || matches!(result, Ok(crate::tunnel_runtime::ApplyAcceptance::Adopted))
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.session = previous.0;
            inner.disarm_reason = previous.1;
            inner.epoch += 1; // 本次可能排队的回调亦失效，不复活先前旧循环。
            inner.in_flight = false;
            if result.is_ok()
                && let Some(session) = inner.session.as_mut()
            {
                session.operation_id = operation_id.to_vec();
            }
        }
        result.map(|_| ())
    }

    /// 解除：用户显式断开/会话换装时清驻留（材料即刻零化；在途循环经代次自行退出）。
    /// 记录解除原因：`user_stop` / R1 终态的善后链由各自路径驱动，第五情形不重复。
    pub fn disarm(&self, why: &'static str) {
        let had_material = {
            let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let had = inner.session.is_some();
            inner.session = None; // Drop → Zeroizing 零化
            inner.epoch += 1;
            inner.in_flight = false;
            inner.disarm_reason = Some(why);
            had
        };
        if had_material {
            self.log.emit(
                LogLevel::Info,
                "engine",
                "reconnect.disarmed",
                "连接材料驻留已解除并零化（R1 不再自愈）",
                &[("why", why)],
            );
        }
    }

    /// 当前是否武装（材料在驻；T8 采纳快照的 R1 状态分量）。
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session
            .is_some()
    }

    /// 自愈循环是否在途（T8 采纳快照的 R1 状态分量）。
    #[must_use]
    pub fn is_healing(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .in_flight
    }

    /// 触发入口（sweeper 清除死亡隧道的 live 之后调用；同步、非阻塞）。
    ///
    /// 门控：材料在 / 无在途循环 / 隧道确死（迟到的旧世代信号由 sweeper 世代守卫
    /// 拦截，此处 `is_connected` 兜底）。通过后 spawn 自愈循环（sweeper 线程无
    /// 异步上下文，用 arm 时捕获的句柄）。
    ///
    /// **T5 第五情形（计划 v6.1 §8.2）**：材料不在（重连未开启/受理未驻留）时
    /// 不再静默返回——worker 死亡已清 live，善后链（lease 释放复验 + trace 终态
    /// 化 + 验证收口）转既有 [`AftermathDriver::abandon_session`] 主体驱动
    /// （reason = [`crate::aftermath::reason::DATA_PLANE_WORKER_FAILED`]）。
    /// 2026-09-28 worker-failed 案：该路径静默导致痕停留 Live、NRPT 残留无人
    /// 复验，四次重连全撞 conflict。
    pub fn on_tunnel_lost(self: &Arc<Self>) {
        let (epoch, handle) = {
            let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.in_flight {
                return;
            }
            if self.runtime.is_connected() {
                // 竞态兜底：新隧道已立（信号属于已拆除的旧世代）。
                return;
            }
            let handle = inner.handle.clone();
            if inner.session.is_none() {
                // T5 第五情形：无自愈材料。`user_stop` / R1 终态解除过的会话已有
                // 各自善后链（StopTunnel 验证收口 / terminal 分叉），不重复驱动；
                // 从未武装（重连未开启——2026-09-28 worker-failed 案形态）才由本
                // 情形接管。持锁仅取事实，锁外分派。
                let user_or_terminal_disarmed = inner.disarm_reason.is_some();
                drop(inner);
                if !user_or_terminal_disarmed {
                    self.worker_failed_aftermath(handle);
                }
                return;
            }
            let Some(handle) = handle else {
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "reconnect.trigger.dropped",
                    "掉线信号到达但无可用的异步句柄（未在异步上下文武装）",
                    &[],
                );
                return;
            };
            inner.in_flight = true;
            (inner.epoch, handle)
        };
        self.log.emit(
            LogLevel::Warn,
            "engine",
            "reconnect.triggered",
            "隧道异常终止，engine 开始 R1 自愈（退避 2s→30s）",
            &[("reason", reason::TUNNEL_LOST)],
        );
        let driver = Arc::clone(self);
        handle.spawn(async move {
            driver.heal_loop(epoch).await;
            // 循环退出统一复位占用标记（只在会话代次未变时——防误清新循环的标记）。
            let mut inner = driver.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.epoch == epoch {
                inner.in_flight = false;
            }
        });
    }

    /// T5 第五情形善后分派：数据面工作线程死亡、无 R1 材料——接既有善后主体
    /// （[`AftermathDriver::abandon_session`]：cancel+teardown 复验 lease 释放 →
    /// 痕 Aftermath → 预检验证 → 干净删痕 → Idle 终态事件）。teardown 幂等：sweeper
    /// 的 `disconnect_for` 已做过一轮清理（失败残留的 owner 会被本轮重试）。
    fn worker_failed_aftermath(self: &Arc<Self>, handle: Option<tokio::runtime::Handle>) {
        self.log.emit(
            LogLevel::Warn,
            "engine",
            "reconnect.worker_failed_aftermath",
            "数据面工作线程死亡且无 R1 自愈材料：转 T5 第五情形善后（lease 复验+trace 终态化+验证收口）",
            &[("reason", reason::TUNNEL_LOST)],
        );
        let Some(handle) = handle else {
            // 无异步句柄（构造与武装均在异步上下文外——异常装配）：善后无法驱动，
            // 掉线状态已由 C2 事件上报，残留由下次连接预检兜底（如实记录，不伪造）。
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "reconnect.worker_failed_aftermath.dropped",
                "无可用异步句柄，第五情形善后无法驱动（残留由下次连接预检兜底）",
                &[],
            );
            return;
        };
        let driver = Arc::clone(self);
        let epoch = self.current_epoch();
        handle.spawn(async move {
            let _admission = driver.status.admission.access.lock().await;
            if *_admission {
                return;
            }
            if driver.current_epoch() != epoch || driver.runtime.is_connected() {
                return;
            }
            driver
                .aftermath
                .abandon_session_admitted(crate::aftermath::reason::DATA_PLANE_WORKER_FAILED)
                .await;
        });
    }

    /// 会话代次（循环退出/中止判定）。
    fn current_epoch(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .epoch
    }

    /// 与 arm/disarm 共用局部锁，使已失效代次不能随后发布自愈结果。
    async fn publish_current(&self, epoch: u64, event: StatusEvent) -> bool {
        let admission = self.status.admission.access.lock().await;
        if *admission {
            return false;
        }
        self.publish_current_admitted(epoch, event)
    }

    fn publish_current_admitted(&self, epoch: u64, event: StatusEvent) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.epoch != epoch {
            return false;
        }
        self.status.publish(event);
        true
    }

    /// 自愈循环主体：Armed → Reconnecting →（Healed | Backoff' | Terminal）。
    async fn heal_loop(self: &Arc<Self>, epoch: u64) {
        let started = std::time::Instant::now();
        let mut attempt: u32 = 0;
        let mut failure_reason = reason::TUNNEL_LOST;
        loop {
            let admission = self.status.admission.access.lock().await;
            if *admission || self.current_epoch() != epoch {
                return; // 会话已换装/解除
            }
            attempt += 1;
            let Some(facts) = self.session_facts() else {
                return;
            };
            let op_id = facts.operation_id.clone();
            // 开始本轮决策；真实受理点另发 next_retry_at_ms=0，退避预告不作受理凭据。
            self.stats.registry().record_reconnect_attempt();
            if attempt == 1 {
                if !self.publish_current_admitted(
                    epoch,
                    StatusEvent::reconnecting(op_id.clone(), attempt, failure_reason, 0),
                ) {
                    return;
                }
            }
            drop(admission);
            let outcome = self.heal_once(&facts, &op_id, epoch, attempt).await;
            if self.current_epoch() != epoch { return; }
            match outcome {
                HealOutcome::Healed => {
                    let admission = self.status.admission.access.lock().await;
                    if *admission
                        || !self.publish_current_admitted(
                            epoch,
                            StatusEvent::reconnected(op_id, attempt),
                        )
                    {
                        return;
                    }
                    self.stats.registry().record_reconnect_success();
                    let connection_id = self
                        .gate
                        .as_ref()
                        .and_then(|gate| gate.update_established_at(&self.log));
                    self.log.emit(
                        LogLevel::Info,
                        "engine",
                        "reconnect.healed",
                        "R1 自愈成功：隧道恢复 Live，痕迹已换新代次（§2.4）",
                        &[
                            ("attempt", &attempt.to_string()),
                            ("connection_id", connection_id.as_deref().unwrap_or("")),
                            (
                                "elapsed_ms",
                                &started.elapsed().as_millis().to_string(),
                            ),
                        ],
                    );
                    return; // 材料保留、尝试计数随成功归零（下次触发从 1 起）
                }
                HealOutcome::Terminal(terminal_reason) => {
                    self.terminal(op_id, attempt, terminal_reason, epoch).await;
                    return;
                }
                HealOutcome::Aborted => {
                    return; // 会话换装/解除：新状态由新循环/受理路径负责
                }
                HealOutcome::Transient(next_reason) => {
                    failure_reason = next_reason;
                    if attempt >= MAX_ATTEMPTS {
                        self.terminal(op_id, attempt, reason::ATTEMPTS_EXHAUSTED, epoch)
                            .await;
                        return;
                    }
                    let delay = backoff_delay(attempt);
                    let next_retry_at_ms = wall_now_ms()
                        .saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
                    if !self
                        .publish_current(
                            epoch,
                            StatusEvent::reconnecting(
                                op_id.clone(),
                                attempt + 1,
                                failure_reason,
                                next_retry_at_ms,
                            ),
                        )
                        .await
                    {
                        return;
                    }
                    self.log.emit(
                        LogLevel::Warn,
                        "engine",
                        "reconnect.backoff",
                        "自愈尝试失败，退避后重试（2s→30s 封顶）",
                        &[
                            ("reason", failure_reason),
                            ("attempt", &attempt.to_string()),
                            ("next_attempt", &(attempt + 1).to_string()),
                            ("delay_ms", &delay.as_millis().to_string()),
                        ],
                    );
                    tokio::time::sleep(delay).await;
                    if self.current_epoch() != epoch {
                        return;
                    }
                }
            }
        }
    }

    /// 驻留会话的可发送快照（每次尝试取一份材料副本；plan/mode 廉价克隆）。
    fn session_facts(&self) -> Option<SessionFacts> {
        let inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.session.as_ref().map(|session| SessionFacts {
            operation_id: session.operation_id.clone(),
            kind: session.kind,
            connection_mode: session.connection_mode,
            atrust_dns_enabled: session.atrust_dns_enabled,
            plan: session.plan.clone(),
            material: session.material.copy_bytes(),
        })
    }

    /// 单次自愈尝试：aTrust 前置材料复核 → 全量重跑组装 → 观察终态。
    async fn heal_once(&self, facts: &SessionFacts, op_id: &[u8], epoch: u64, attempt: u32) -> HealOutcome {
        // aTrust：源会话复核（既有 auth_config(false) 原语）。死亡 → 终态；
        // 网络不可核 → 瞬态退避（不值得全量组装）。
        if facts.kind == TraceKind::Atrust
            && let Some(outcome) = self.probe_atrust_source(facts).await
        {
            return outcome;
        }
        // 全量重跑（既有组装原语：登录/CSTP 或 aTrust 交接 + NIC/路由/数据面）。
        let mut bytes = facts.material.clone();
        // 驻留材料失配（版本漂移/损坏）——永久性，终态不循环。
        let Ok(credentials) = parse_connection_credentials(&mut bytes) else {
            return HealOutcome::Terminal(reason::AUTH_REJECTED);
        };
        let ctx = ApplyContext {
            atrust_dns_enabled: facts.atrust_dns_enabled,
            connection_mode: facts.connection_mode,
            plan: facts.plan.clone(),
            credentials,
            operation_id: op_id.to_vec(),
            status: Arc::clone(&self.status),
            stats: Arc::clone(&self.stats),
            log: Arc::clone(&self.log),
        };
        let admission = self.status.admission.access.lock().await;
        if *admission {
            return HealOutcome::Aborted;
        }
        if self.current_epoch() != epoch {
            return HealOutcome::Aborted;
        }
        let cursor = match Arc::clone(&self.runtime).start_reconnect(ctx, attempt) {
            Ok(cursor) => cursor,
            Err(error) => {
                // 受理拒绝（重复连接等）按瞬态退避，不观察旧完成。
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "reconnect.heal.refused",
                    "自愈组装受理被拒（重复连接/状态），退避重试",
                    &[("detail", &error.detail)],
                );
                return if self.current_epoch() != epoch {
                    HealOutcome::Aborted
                } else {
                    HealOutcome::Transient(reason::HEAL_ASSEMBLY_FAILED)
                };
            }
        };
        drop(admission);
        // 只观察真实受理游标之后、同一逻辑 id 的新完成；序列与事件同快照。
        let deadline = tokio::time::Instant::now() + HEAL_BOUND;
        loop {
            let (sequence, latest) = self.status.snapshot();
            if self.current_epoch() != epoch { return HealOutcome::Aborted; }
            if let Some(latest) = latest
                && sequence > cursor
                && latest.operation_id == op_id
            {
                match latest.coarse_phase {
                    StatsPhase::Connected => return HealOutcome::Healed,
                    StatsPhase::Failed => return classify_assembly_failure(&latest),
                    _ => {}
                }
            }
            if self.current_epoch() != epoch {
                return HealOutcome::Aborted;
            }
            if tokio::time::Instant::now() >= deadline {
                return HealOutcome::Transient(reason::HEAL_TIMEOUT);
            }
            tokio::time::sleep(HEAL_POLL).await;
        }
    }

    /// aTrust 源会话复核：`auth_config(false)`（与 `browser_handoff` 交接第一步
    /// 同款原语）。`is_login=false` → 认证终态失效（R3，终态）；网络/TLS 失败 →
    /// 不可核（瞬态）。物理出口选择与生产路径同源（`find_physical_nics` 首选）。
    /// 返回 `None` = 复核通过（或材料非 aTrust，主路径自会分类）。
    async fn probe_atrust_source(&self, facts: &SessionFacts) -> Option<HealOutcome> {
        let mut bytes = facts.material.clone();
        // 材料失配（非 aTrust）在 heal_once 主路径分类。
        let Ok(Some(ConnectionCredentials::Atrust(browser))) =
            parse_connection_credentials(&mut bytes)
        else {
            return None;
        };
        let nics = exv_vpn_win32_resource::vgdc_dns::find_physical_nics().ok()?;
        let nic = nics.first()?;
        let underlay = crate::atrust_underlay::PhysicalUnderlay::new(nic.ifindex)
            .map(Arc::new)
            .ok()?;
        let options = AuthClientOptions {
            user_agent: browser.user_agent().to_owned(),
            ..AuthClientOptions::new(crate::atrust_auth::client_identity())
        };
        let mut client = AuthClient::new(browser.gateway(), underlay, options).ok()?;
        if client
            .import_gateway_cookies(Secret::new(browser.cookie_header().to_owned()))
            .is_err()
        {
            return Some(HealOutcome::Terminal(reason::AUTH_REJECTED)); // 材料形状失配
        }
        match client.auth_config(false).await {
            Ok(status) if status.is_login => None,
            Ok(_) => Some(HealOutcome::Terminal(reason::AUTH_REJECTED)),
            Err(_) => Some(HealOutcome::Transient(reason::HEAL_ASSEMBLY_FAILED)),
        }
    }

    /// R1 终态善后（认证拒绝 / 尝试耗尽）：分叉见模块级文档（无头=engine 完成；
    /// core 在场=事件交 core 驱动，engine 清残留不删痕）。
    async fn terminal(
        &self,
        op_id: Vec<u8>,
        attempt: u32,
        terminal_reason: &'static str,
        epoch: u64,
    ) {
        let _admission = self.status.admission.access.lock().await;
        if *_admission {
            return;
        }
        // 终态决策与发布同 arm/disarm 串行，不能清掉或覆盖已取代的会话。
        {
            let mut inner = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.epoch != epoch { return; }
            inner.session = None;
            inner.epoch += 1;
            inner.in_flight = false;
            inner.disarm_reason = Some(terminal_reason);
            self.status.publish(StatusEvent::reconnect_terminal(
                op_id, attempt, terminal_reason, terminal_wire_error(terminal_reason),
            ));
        }
        let headless = self.headless.current().is_some();
        self.log.emit(
            LogLevel::Warn,
            "engine",
            "reconnect.terminal",
            if headless {
                "R1 终态失败（无头：engine 完成善后并按验证结论删痕）"
            } else {
                "R1 终态失败（core 在场：终态事件交 core 驱动，engine 清残留不删痕）"
            },
            &[
                ("reason", terminal_reason),
                ("attempt", &attempt.to_string()),
                ("headless", &headless.to_string()),
            ],
        );
        if headless {
            // 无头：engine 驱动完整善后（T5 钩子：cancel+teardown+验证+删痕+Idle）。
            let outcome = self
                .aftermath
                .abandon_session_admitted(crate::aftermath::reason::R1_TERMINAL)
                .await;
            self.log.emit(
                if outcome.verified {
                    LogLevel::Info
                } else {
                    LogLevel::Warn
                },
                "engine",
                "reconnect.terminal.aftermath",
                "无头 R1 终态善后已收口（结论见 aftermath_verified）",
                &[
                    ("verified", &outcome.verified.to_string()),
                    ("trace_deleted", &outcome.trace_deleted.to_string()),
                    ("residual_ids", &outcome.residual_reasons.join(",")),
                ],
            );
        } else {
            // core 在场：清死亡隧道残留（幂等减负断开；NIC 保留 D12），痕 → Aftermath
            // （不删——core 反应路径或下次连接门预检收口），相位回 Failed。
            let runtime = Arc::clone(&self.runtime);
            if let Err(error) = tokio::task::spawn_blocking(move || runtime.disconnect())
                .await
                .map_err(|e| format!("disconnect join: {e}"))
                .and_then(|result| result)
            {
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "reconnect.terminal.residue_cleanup_failed",
                    "终态残留清理失败（adapter 由退出清理兜底；残留由下次预检上报）",
                    &[("detail", &error)],
                );
            }
            self.stats.registry().set_phase(StatsPhase::Failed);
            if let Some(gate) = self.gate.as_ref() {
                gate.mark_aftermath(&self.log);
            }
        }
    }
}

/// 单次尝试携带的会话事实（材料副本 + 组装上下文）。
struct SessionFacts {
    atrust_dns_enabled: bool,
    operation_id: Vec<u8>,
    kind: TraceKind,
    connection_mode: ConnectionMode,
    plan: exv_vpn_domain::ports::TunnelPlan,
    material: Zeroizing<Vec<u8>>,
}

/// 组装失败分类（wire 错误码 → 自愈结局）。
///
/// 码表来源 `tunnel_error_to_wire`：15=UNAUTHORIZED（Cisco 登录被拒 `a0≠0`，
/// 唯一真实认证拒绝）；20=SAML_REQUIRED（EXV_UNFREEZE 2026-10-09 起——本驱动
/// 自建 ctx 不缺输入，收到即交互式 SAML 要求，无头不可交互，终态）；1=
/// INVALID_INPUT（仅凭据缺失；本驱动自建 ctx 已带凭据，罕见形态按瞬态退避，
/// 不再误当 SAML 终态）；其余（13 平台/16 传输）瞬态。
fn classify_assembly_failure(latest: &StatusEvent) -> HealOutcome {
    let Some(error) = latest.error.as_ref() else {
        return HealOutcome::Transient(reason::HEAL_ASSEMBLY_FAILED);
    };
    match error.code {
        WIRE_UNAUTHORIZED | WIRE_SAML_REQUIRED => HealOutcome::Terminal(reason::AUTH_REJECTED),
        _ => HealOutcome::Transient(reason::HEAL_ASSEMBLY_FAILED),
    }
}

/// R1 终态事件的 wire 错误（无外部操作主体：engine 内发起的会话终局）。
fn terminal_wire_error(terminal_reason: &str) -> VpnError {
    let code = if terminal_reason == reason::AUTH_REJECTED {
        WIRE_UNAUTHORIZED
    } else {
        WIRE_DEADLINE_EXCEEDED
    };
    VpnError {
        code,
        stage: WIRE_STAGE_INGRESS,
        certainty: 0,
        retry: WIRE_RETRY_NONE,
        subject: None,
        resource: None,
        native: None,
        // R1 终态的断开语义由 reason lane id（T16b）承载，VpnError.reason 不再
        // 重复细分（T-16/17 不设 VpnError reason id，词表 §2.3）。
        reason: String::new(),
        operation_detail: None,
    }
}

/// Unix 毫秒（时钟回拨等异常退化为 0；与 `connection_trace` 同款约定）。
fn wall_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// ---------------------------------------------------------------------------
// 单元测试：脚本化运行时（cfg(test)，与 T5 SlowAssembleRuntime 同款纪律）+
// 真 store 的 gate + 真 AftermathDriver（假 liveness/干净 verifier）。aTrust 源
// 会话复核触真实网络，不在单测覆盖（实机归 T13 验收门禁）；Cisco 路径覆盖
// 状态机、退避、终态分叉、事件/计数与零化。
// ---------------------------------------------------------------------------
