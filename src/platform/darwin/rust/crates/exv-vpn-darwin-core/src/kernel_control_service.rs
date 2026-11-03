//! `MAC-UI-CONFIG-10b` 的普通用户 `KernelControl` 适配器。
//!
//! 本模块把已经认证的 UI 请求映射到 Core 配置和 `Idle` 状态。普通 Config/Idle 路径不启动
//! Engine；明确的 Connect/Stop 才复用同一已认证本地通道调用 Engine 的 `ApplyTunnel`/
//! `StopTunnel`。本阶段不实现网络、CSTP 或 utun。

mod session_orchestration;
mod auth_continuation;

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use exv_vpn_host::event_bus::EventBus;
use exv_vpn_darwin_ipc::auth_deadline::{AuthDeadline, AuthDeadlineError};
use exv_vpn_wire::generated as wire;
use exv_vpn_wire::generated::{
    Attempt, ConfigGetRequest, ConfigItem, ConfigPayload, ConfigReply, ConfigSetRequest,
    ConnectPhase, ConnectRequest, ConnectingState, GetKernelOperationRequest, IdleState,
    InteractionResponse, KernelOperationReply, LogsClearReply, LogsClearRequest, LogsListReply,
    LogsListRequest, OperationReply, ReconcileRequest, RuntimeEvent, RuntimeEventKind,
    RuntimeSnapshot, ServiceControlReply, ServiceControlRequest, SnapshotRequest, StopRequest,
    StoppingState, WatchEventsRequest, WatchStatsRequest, kernel_control_server::KernelControl,
    runtime_snapshot,
};
use tokio::sync::{oneshot, watch};
use tokio_stream::StreamExt;
use tonic::{Code, Request, Response, Status, codegen::tokio_stream::Stream};
use zeroize::Zeroize;

mod sleep_recovery;
mod cached_headless;
use crate::atrust_session_cache;
use crate::credential::normalized_connect_routes;

use crate::{
    DarwinConfigError, DarwinConfigItem, DarwinUiConfig, EngineLifecycleError, Protocol,
    SECRET_PAYLOAD_VERSION, ATRUST_SECRET_PAYLOAD_VERSION, SavedConnectEnvelopeError, UiSecretPayload, UiConnectMaterials, build_connect_envelope,
    config_hygiene,
    engine_lifecycle_real::{EngineControlSession, launch_fixed_engine_session},
    log_aggregator::LogAggregator,
    log_control::LogControl,
    parse_ui_secret_payload,
    proxy_tun::ProxyTunCache,
    service_lifecycle::{
        ELEVATION_DENIED_CODE, ELEVATION_FAILED_CODE, LifecycleError, ServiceLifecycle,
        ServiceStatusProbe, SERVICE_NOT_INSTALLED_CODE,
        SERVICE_SOURCE_MISSING_CODE,
    },
    stats::{LiveStatsState, TrafficSample},
};

const CONFIG_PREP_ONLY_CODE: &str = "DARWIN_UI_CONFIG_PREP_ONLY";
const SNAPSHOT_EPOCH_UNSUPPORTED_CODE: &str = "DARWIN_UI_SNAPSHOT_EPOCH_UNSUPPORTED";
/// 无 fake Engine fixture 可用时的连接/停止路径 typed 拒绝。
///
/// 生产 runner（`new_with_live_engine`）走 `LiveConnectionProjection` 真实链路，不构建
/// fixture 投影；本码只在 fixture 缺席（`connection` 为 `None`，如仅配置构造）而
/// Connect/Stop 误入 fixture-only 路径时给出稳定拒绝，不伪装成未实现或不可用。
const CONNECTION_ENGINE_NOT_ATTACHED_CODE: &str = "DARWIN_CORE_ENGINE_NOT_ATTACHED";
const CONNECTION_ENGINE_SESSION_CODE: &str = "DARWIN_CORE_ENGINE_SESSION_FAILED";
const CONNECTION_ENGINE_ELEVATION_CODE: &str = "DARWIN_CORE_ENGINE_ELEVATION_FAILED";
/// `LogsClear` 落盘 truncate 失败（存储自行降级内存-only）时的 typed 错误码；
/// 不向 UI 泄露底层 IO 文本。
const LOGS_CLEAR_IO_FAILED_CODE: &str = "DARWIN_LOGS_CLEAR_IO_FAILED";
const CONNECTION_CREDENTIALS_MISSING_CODE: &str = "DARWIN_CORE_CONNECT_CREDENTIALS_MISSING";
/// S2（2026-10-26 验收修复计划）：缓存探测**网络层失败**（underlay/tls/timeout
/// 等未知结果）的独立 typed 码——与「网关明确报告未登录」
/// （[`CONNECTION_CREDENTIALS_MISSING_CODE`]）分离；事故链：resolver 孤儿致
/// 全系统 DNS 死路时 6s 探测超时曾被合流为凭据缺失，前端弹凭据模态是错误处方。
const CONNECTION_PROBE_UNAVAILABLE_CODE: &str = "DARWIN_CORE_ATRUST_PROBE_UNAVAILABLE";
/// W1-C（P7）：UI 一次性凭据载荷无法解析、版本非法或字段为空时的 typed 拒绝码
/// （`invalid_argument`）——这是编程/契约错误，不触发前端凭据弹窗重试。
const CONNECTION_CREDENTIALS_INVALID_CODE: &str = "DARWIN_CORE_CONNECT_CREDENTIALS_INVALID";
/// W2 心跳：core 侧 `KeepAlive` 发送周期（10s；对齐 win32 `KEEPALIVE_PERIOD` 与权威
/// 规范 §二 oneshot.2「core 每 10s KeepAlive」，engine 侧看门狗 15s 上界）。
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(10);
/// W2 退出清理包：`close` 前 best-effort `StopTunnel` 的有界等待（300ms，对齐
/// win32 退出 flush；正确性兜底是 W1 的 EOF 收敛，本包只是加速器）。
const EXIT_STOP_BOUND: Duration = Duration::from_millis(300);
// ---------------------------------------------------------------------------
// P4 v2：ServiceControl 动作面的 typed 码与前缀词汇。
// ---------------------------------------------------------------------------
/// 服务路由失败前缀（win32 既有词汇；壳层 `map_status` 依此分流恢复 modal——
/// `ServiceNotRunning` 触发前端 `ServiceConnectFailureModal`）。
const SERVICE_NOT_RUNNING_PREFIX: &str = "service_not_running|";
/// 服务在但连接/会话建立失败的前缀（win32 既有词汇；同上分流）。
const SERVICE_CONNECT_FAILED_PREFIX: &str = "service_connect_failed|";
/// `rotate_key` 的 typed 拒绝码（`invalid_argument`）：一次性 ticket 模型无持久服务
/// 密钥，语义不适用（诚实空缺，非「暂时未做」）。
const SERVICE_ROTATE_KEY_CODE: &str = "DARWIN_CORE_SERVICE_ROTATE_KEY_INAPPLICABLE";
/// action oneof 缺失（proto 契约要求 exactly one）的 typed 拒绝码。
const SERVICE_ACTION_MISSING_CODE: &str = "DARWIN_CORE_SERVICE_ACTION_MISSING";

/// Core 这次唯一已认证 UI session 的一次性 shutdown 触发器。
///
/// `WatchEvents` cancellation、handoff/cleanup 失败或 server 自身提前结束都只能消费同一
/// sender。它不会打开第二个 listener，也不会重用认证 key。
#[derive(Clone)]
pub(crate) struct SessionShutdown {
    sender: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    triggered: watch::Sender<bool>,
}

impl SessionShutdown {
    /// 创建 session 的唯一 shutdown receiver。
    pub(crate) fn new() -> (Self, oneshot::Receiver<()>) {
        let (sender, receiver) = oneshot::channel();
        let (triggered, _) = watch::channel(false);
        (
            Self {
                sender: Arc::new(Mutex::new(Some(sender))),
                triggered,
            },
            receiver,
        )
    }

    /// 幂等触发 Core session shutdown。
    pub(crate) fn trigger(&self) {
        let sender = lock_recover(&self.sender).take();
        if let Some(sender) = sender {
            let _ = self.triggered.send_replace(true);
            let _ = sender.send(());
        }
    }

    /// 等待唯一 shutdown 被触发。
    ///
    /// `watch` 保留最后状态：即使 watcher 在 trigger 后才订阅，也会立即观察到 `true`，
    /// 不会因 oneshot 已被 tonic 消费而丢失 runner 的 drain 起点。
    pub(crate) async fn wait_for_trigger(&self) {
        let mut receiver = self.triggered.subscribe();
        loop {
            if *receiver.borrow_and_update() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

/// `WatchEvents` server-streaming 响应体（win32 同款 boxed stream；事件源是共享
/// [`EventBus`] 的订阅流——多订阅者 fan-out、resume 重放，见 [`EventBus::subscribe`]）。
type WatchEventsStream = Pin<Box<dyn Stream<Item = Result<RuntimeEvent, Status>> + Send>>;

/// `WatchStats` server-streaming 响应体（T14 拆流，win32 同款：统计样本独立流，
/// 事件源是共享 [`EventBus`] 的统计 lane——`subscribe_stats`，见
/// [`EventBus::subscribe_stats`]）。
type WatchStatsStream = Pin<Box<dyn Stream<Item = Result<wire::RuntimeStats, Status>> + Send>>;

/// 10b 的认证 UI-facing service。
#[derive(Clone)]
pub(crate) struct DarwinKernelControlService {
    config: Arc<Mutex<DarwinUiConfig>>,
    /// 共享事件总线（W3-1/P1 S2：common `exv_vpn_host::event_bus::EventBus`）。
    /// 构造期同步发布 initial Idle（tick 1 Snapshot），保「首订阅 resume=0 恰得
    /// tick==1 Snapshot Idle」契约；`WatchEvents` handler 只做一行订阅（无 claim、
    /// 无一次性 mpsc），连接/统计投影经同一总线发布。
    events: Arc<EventBus>,
    connection: Option<Arc<ConnectionProjection>>,
    live_connection: Option<Arc<LiveConnectionProjection>>,
    /// 日志聚合语义层（MAC-OBS-13 S1）：`LogsList` 历史分页与 Core 自身事件入库。
    logs: Arc<LogControl>,
    /// 上游代理 TUN 检测缓存（MAC-PROXY-16 S1）：Core 启动/连接受理/断开终态边界
    /// 刷新，快照出向边界只读附加；仅状态上报，绝不改路由（PRD O1）。
    proxy_tun: Arc<ProxyTunCache>,
    /// P4 v2：服务生命周期执行器（管理员提权编排 install/uninstall/start；
    /// 测试经 [`Self::with_service_lifecycle`] 注入 fake，夹具默认恒拒绝提权）。
    service_lifecycle: Arc<ServiceLifecycle>,
    /// P4 v2：服务状态缓存伴生值（win32 同款模型：动作/快照边界刷新，事件发布
    /// 边界只读附加——与 `LiveConnectionProjection` 共享同一实例）。
    service_status_cache: Arc<Mutex<Option<exv_vpn_wire::generated::ServiceStatus>>>,
    /// P4 v2：变更动作串行锁（win32 `service_control_lock` 对应——防重复点击/
    /// 多窗口交叉改安装态；query 不加锁）。
    service_control_lock: Arc<tokio::sync::Mutex<()>>,
}

impl DarwinKernelControlService {
    /// 为实际 Core session 创建按需启动固定 Engine 的连接投影。
    ///
    /// Engine 不会在 Core 启动时拉起；只有 UI 发送 Connect 后才创建这一会话。
    /// 构造期同步发布 initial Idle（tick 1）——Core 的退出主路径是认证 UDS 连接
    /// EOF（`close_signal`）+ server 退出，WatchEvents 流取消**不再**触发 shutdown。
    pub(crate) fn new_with_live_engine() -> (Self, Arc<LiveConnectionProjection>) {
        let config = Arc::new(Mutex::new(DarwinUiConfig::load().unwrap_or_default()));
        let logs = default_log_control();
        let proxy_tun = live_proxy_tun_cache();
        let events = Arc::new(EventBus::new());
        publish_initial_idle(&events, &proxy_tun);
        // P4 v2：服务生命周期与状态缓存的生产实例（真实管理员提权 + 真实
        // service agent 探测；投影与 service 共享同一缓存）。
        let service_lifecycle = Arc::new(ServiceLifecycle::production());
        let service_status_cache = Arc::new(Mutex::new(None));
        let live = Arc::new(
            LiveConnectionProjection::with_launch(
                Arc::clone(&config),
                Arc::clone(&events),
                production_session_launch(),
                Arc::clone(&logs),
                Arc::clone(&proxy_tun),
            )
            .with_service_seams(Arc::clone(&service_lifecycle), Arc::clone(&service_status_cache))
            .with_service_engine(Arc::new(ServiceEnginePath::production())),
        );
        // W3-2/P3：拉起自动重连 worker（单 worker 串行消费掉线信号）。darwin Core
        // 随 UI session 结束整体退出——worker detach，进程 teardown 取消退避 sleep，
        // 无需 win32 常驻服务的显式 abort 纪律（JoinHandle 丢弃即 detach）。
        let _reconnect_worker = Arc::clone(&live).spawn_reconnect_worker();
        (
            Self {
                config,
                events,
                connection: None,
                live_connection: Some(Arc::clone(&live)),
                logs,
                proxy_tun,
                service_lifecycle,
                service_status_cache,
                service_control_lock: Arc::new(tokio::sync::Mutex::new(())),
            },
            live,
        )
    }

    /// 通用构造体：**仅测试构建可达**（`with_connection_engine*`/`with_config` 均
    /// `#[cfg(test)]`）；生产 runner 走 [`Self::new_with_live_engine`]。总线由调用方
    /// 传入（测试 fixture 与投影共享同一实例，镜像生产 `new_with_live_engine` 的
    /// 接线）；构造期内发布 initial Idle（tick 1）。
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "仅由 #[cfg(test)] fixture 构造调用；生产 runner 走 new_with_live_engine"
        )
    )]
    fn new_inner(
        events: Arc<EventBus>,
        engine: Option<Box<dyn ConnectionEngine>>,
        live_connection: Option<Arc<LiveConnectionProjection>>,
        config: DarwinUiConfig,
        logs: Arc<LogControl>,
        proxy_tun: Arc<ProxyTunCache>,
    ) -> Self {
        publish_initial_idle(&events, &proxy_tun);
        let connection = engine.map(|engine| {
            Arc::new(ConnectionProjection::new(
                engine,
                Arc::clone(&events),
                Arc::clone(&proxy_tun),
            ))
        });
        Self {
            config: Arc::new(Mutex::new(config)),
            events,
            connection,
            live_connection,
            logs,
            proxy_tun,
            // 夹具默认：恒 healthy 探测（连接路径不触发提权）+ 恒拒绝 elevator
            //（service 动作默认 typed 拒绝；需要成功编排的测试注入 fake）。
            service_lifecycle: Arc::new(crate::service_lifecycle::hermetic_lifecycle()),
            service_status_cache: Arc::new(Mutex::new(None)),
            service_control_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn config(&self) -> MutexGuard<'_, DarwinUiConfig> {
        lock_recover(&self.config)
    }

    fn connection(&self) -> Result<&ConnectionProjection, Status> {
        self.connection.as_deref().ok_or_else(|| {
            Status::new(
                Code::FailedPrecondition,
                CONNECTION_ENGINE_NOT_ATTACHED_CODE,
            )
        })
    }

}

/// 构造期发布 initial Idle（W3-1/P1 S2：并入总线 current，tick 1 Snapshot）。
///
/// 旧 `initial_idle_event`（watch 流首条硬编码事件）由此退役：任何 resume=0 的首订阅
/// 都经总线重放语义拿到同一条 tick==1 Snapshot Idle；`proxy_tun` 附加事实取构造时刻
/// 的最近检测（生产构造前 `live_proxy_tun_cache` 已探测一次）。
fn publish_initial_idle(events: &Arc<EventBus>, proxy_tun: &Arc<ProxyTunCache>) {
    events.publish(
        RuntimeEventKind::Snapshot,
        snapshot_with_environment(idle_snapshot(), proxy_tun),
    );
}

/// 受控 fake Engine 向 Core 投递的终态占位。
///
/// 这个切片只关心 operation 关联与 UI 投影；`snapshot` 由 fixture 直接给出，不把
/// Engine 网络、CSTP、utun 或任何系统资源概念搬进普通用户 Core。
#[derive(Clone)]
struct EngineStatus {
    snapshot: RuntimeSnapshot,
}

type EngineStatusSink = Arc<dyn Fn(EngineStatus) + Send + Sync>;

/// MAC-CORE-03 的最小 Engine 合同。
///
/// `attach_status` 必须在任一写请求前发生；受控 fixture 通过返回的 sink 在稍后投递
/// terminal snapshot。这里不是实际 Engine client，也没有 UDS、root、网络或特权操作。
trait ConnectionEngine: Send {
    fn attach_status(&mut self, sink: EngineStatusSink) -> Result<(), Status>;

    fn apply_connect(&mut self, request: &ConnectRequest) -> Result<(), Status>;

    fn stop(&mut self, request: &StopRequest) -> Result<(), Status>;
}

/// T5：重连信号的触发类别（worker 据此决定派发失败的处置——数据面掉线沿用
/// 一期"派发前失败即终态"；引擎失联的派发失败多为引擎尚未被 launchd 拉回，
/// 按预算退避重试）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReconnectSignal {
    /// 数据面掉线（可重试标记的主轨触发）。
    WakeResume,
    /// 引擎失联（service 形态；R2——launchd KeepAlive 自启，等待回归后重发）。
    EngineLost,
    /// 原 worker 的同轮认证续段，不接纳新一轮预算。
    AuthContinuation,
}

/// Core 观察到的 Engine 失联 typed 错误（状态流终止/进程死亡），MAC-LIFECYCLE-15 S1。
///
/// 不伪造 Engine 自身的错误码与平台细节：`code`/`stage` 对齐 Engine 既有 `SessionLost`
/// 语义（`EffectUnknown` + `ProtocolSession`——Engine 已死，隧道效果由内核回收，结果
/// 未知）；`certainty` 如实声明效果未知；`retry` 显式声明恢复路径=用新 operation 重试
/// （用户再点 Connect，Core 走完整拉起路径，Core 自身不自动重连）。
fn engine_lost_vpn_error() -> exv_vpn_wire::generated::VpnError {
    use exv_vpn_wire::generated::{EffectCertainty, ErrorCode, ErrorStage, RetryAdvice};
    exv_vpn_wire::generated::VpnError {
        code: ErrorCode::EffectUnknown as i32,
        stage: ErrorStage::ProtocolSession as i32,
        certainty: EffectCertainty::Unknown as i32,
        retry: RetryAdvice::UseNewOperation as i32,
        reason: "session/engine_control_stream_ended".into(),
        ..Default::default()
    }
}

/// `FailedClean` 终态快照：`last_error` 携带真实失败事实，不携带统计（不为终态伪造样本）。
/// `proxy_tun` 由快照出向边界统一附加（见 `snapshot_with_proxy_tun`），构造函数恒置空。
fn failed_clean_snapshot(
    operation_id: &[u8],
    last_error: exv_vpn_wire::generated::VpnError,
) -> RuntimeSnapshot {
    RuntimeSnapshot {
        atrust_auth_acquisition: None,
        engine_reconnect: None,
        headless: None,
        relaunched_by_engine: false,
        stop_completion: None,
        system_proxy: None,
        reconnect: None,
        self_heal: None,
        stats: None,
        proxy_tun: None,
        operation_id: operation_id.to_vec(),
        service_status: None,
        mode: String::new(),
        state: Some(runtime_snapshot::State::FailedClean(
            exv_vpn_wire::generated::FailedCleanState {
                last_error: Some(last_error),
                proof: None,
            },
        )),
        session_identity: None,
    }
}

/// 把上游代理 TUN 检测状态附加到 wire 快照（None → `proxy_tun` 留空）。
///
/// MAC-PROXY-16 S1（mirror stats 方案 A）：检测是状态事件的伴生数据——`GetSnapshot`、
/// `WatchEvents` initial Idle 与全部发布边界都从 [`ProxyTunCache`] 读最近检测后经本
/// 函数附加；仅状态上报，不改任何路由/连接行为（PRD O1）。
fn snapshot_with_proxy_tun(
    mut snapshot: RuntimeSnapshot,
    detection: Option<wire::ProxyTunDetection>,
) -> RuntimeSnapshot {
    snapshot.proxy_tun = detection;
    snapshot
}

/// 系统代理与外部 TUN 共享同一次快照刷新；各自读取失败保留空值，交给界面明确提示。
fn snapshot_with_session_identity(
    mut snapshot: RuntimeSnapshot,
    identity: Option<wire::SessionIdentity>,
) -> RuntimeSnapshot {
    snapshot.session_identity = identity;
    snapshot
}

/// 从即将编码的同一真实信封捕获非秘密身份，避免再读可变配置。
fn encode_with_session_identity(envelope: crate::credential::DarwinConnectEnvelope) -> (Vec<u8>, wire::SessionIdentity) {
    use crate::credential::DarwinConnectEnvelope;
    use exv_vpn_darwin_ipc::connect_envelope::EngineConnectPayload;
    let identity = match &envelope {
        DarwinConnectEnvelope::Cisco(cisco) => wire::SessionIdentity {
            protocol: "cisco".into(), username: cisco.username().into(), server: cisco.server().into(),
        },
        DarwinConnectEnvelope::Atrust(atrust) => match atrust.payload() {
            EngineConnectPayload::Atrust(atrust) => wire::SessionIdentity {
                protocol: "atrust".into(), username: String::new(), server: atrust.gateway().into(),
            },
            EngineConnectPayload::Cisco(cisco) => wire::SessionIdentity {
                protocol: "cisco".into(), username: cisco.username().into(), server: cisco.server().into(),
            },
        },
    };
    (envelope.encode().to_vec(), identity)
}

fn snapshot_with_environment(snapshot: RuntimeSnapshot, cache: &ProxyTunCache) -> RuntimeSnapshot {
    let mut snapshot = snapshot_with_proxy_tun(snapshot, cache.current());
    snapshot.system_proxy = cache.system_proxy();
    // 政策事实不延续到连接之外；R1/停止窗口不能沿用上次 Connected 的成功值。
    if !matches!(snapshot.state.as_ref(), Some(runtime_snapshot::State::Connected(_)))
        && let Some(proxy) = snapshot.system_proxy.as_mut()
    {
        proxy.bypass_merged = false;
    }
    snapshot
}

fn observed_proxy_merged(reply: Option<wire::ObserveOwnedStateReply>, operation_id: &[u8]) -> bool {
    reply.and_then(|reply| reply.snapshot)
        .filter(|snapshot| snapshot.operation_id == operation_id)
        .and_then(|snapshot| snapshot.system_proxy)
        .is_some_and(|proxy| proxy.mode == "manual" && proxy.bypass_merged)
}

// ---------------------------------------------------------------------------
// W3-2/P3：数据面自动重连（镜像 win32 C3a `drive_reconnect`；darwin 形态差异见
// 各条注释）。触发双轨 = engine `session_lost_vpn_error` 升级的
// 「stage=DataPlane + retry=RetrySameOperation」标记（主轨）+ 本次 operation
// 曾到达 Connected 的守卫（守卫轨，连接期失败不重连）；Engine 失联（状态流 EOF）
// 路径一期明确**不**自动重连——服务代理 fork 风暴（对齐 win32 §8 冻结边界，
// 见 `spawn_status_projection` 的 EOF 分支注释）。
// ---------------------------------------------------------------------------

/// 数据面自动重连退避 base（计划冻结 2s——首次掉线后 sleep 的延时）；测试经
/// [`LiveConnectionProjection::with_reconnect_backoff`] 注入短时长。
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_secs(2);
/// 数据面自动重连退避 cap（计划冻结 30s）；同上 seam。
const RECONNECT_BACKOFF_CAP: Duration = Duration::from_secs(30);

/// 第 `attempt` 次自动重连派发前的退避延时（指数退避，几何翻倍封顶）。
///
/// 返回值 = `min(base * 2^(attempt-1), cap)`；`attempt` 从 1 计（既有
/// `reconnect_attempts` 语义：每次实际派发 +1、Connected 成功后清零——清零即让下一次
/// 掉线回到 base，成功自动重置退避序列）。产品默认 base=2s/cap=30s → 序列
/// 2s→4s→8s→16s→30s→30s…；测试经 seam 注入毫秒级 base/cap（生产路径不改默认）。
/// `attempt` 越界（0 / 超大）由 `saturating_sub`/封顶收敛到确定值，不循环、零开销。
/// （镜像 win32 `reconnect_backoff_delay`，逐字平移。）
#[must_use]
fn reconnect_backoff_delay(base: Duration, cap: Duration, attempt: u32) -> Duration {
    let base_ns = base.as_nanos();
    let cap_ns = cap.as_nanos();
    debug_assert!(base_ns > 0, "reconnect backoff base must be non-zero");
    // k = 从 base 翻倍多少次到达/超过 cap（此后恒 cap）；封顶 63 防 u128 溢出。
    let mut doublings: u32 = 0;
    let mut growth = base_ns;
    while growth < cap_ns && doublings < 63 {
        growth <<= 1;
        doublings += 1;
    }
    let shift = attempt.saturating_sub(1).min(doublings);
    let delay_ns = (base_ns << shift).min(cap_ns);
    Duration::from_nanos(u64::try_from(delay_ns).unwrap_or(u64::MAX))
}

/// 组装 wire `ReconnectStatus`（纯字段映射，不读配置；镜像 win32）。
///
/// `attempts`/`active` 取投影侧 `reconnect_attempts`/`reconnect_active` 原子（状态
/// 投影维护）；`auto_reconnect`/`max_attempts` 由调用方从内存配置读（0 = unlimited）。
/// `reason` 为稳定 reason id：S5b 起承载重连暂停事实（`RECONNECT_PAUSE_*`，前端
/// toast 按 id 路由）；darwin 不产生 win32 的 R2 恢复事件 id，空串 = 无暂停事实。
#[must_use]
fn reconnect_status_from_state(
    attempts: u32,
    active: bool,
    auto_reconnect: bool,
    max_attempts: u32,
    reason: &str,
) -> wire::ReconnectStatus {
    wire::ReconnectStatus {
        auto_reconnect,
        max_attempts,
        current_attempt: attempts,
        active,
        reason: reason.to_owned(),
    }
}

/// 把自动重连状态附加到 wire 快照（None → `reconnect` 留空；镜像 win32，mirror
/// `snapshot_with_proxy_tun` 模式 A——重连状态是状态事件的伴生数据，发布边界附加，
/// 仅状态上报，不改任何重连行为）。
#[must_use]
fn snapshot_with_reconnect(
    mut snapshot: RuntimeSnapshot,
    reconnect: Option<wire::ReconnectStatus>,
) -> RuntimeSnapshot {
    snapshot.reconnect = reconnect;
    snapshot
}

/// 自动重连的 connect 意图（darwin 版 `reconnect_intent`；镜像 win32）。
///
/// 复用既有 connect 流程（[`LiveConnectionProjection::connect`] →
/// `connect_payload_and_plan` 恒走 saved envelope / UI 优先链，凭据从磁盘重新组装）
/// 需要一份合法 `ConnectIntent`：`lookup_key.method=Connect` + 全新随机 16 字节
/// `runtime_epoch`/`operation_id`（**新 operation——绝不复用旧 id**：旧投影的迟到终态
/// 事件按 `operation_id` 过滤丢弃）+ 32 字节 `request_digest`。darwin 无 Windows SID，
/// `principal_digest` 按合同取全零 32 字节（win32 的「SID 不可用回落」同款，仍为
/// 合法 32 字节；darwin engine 侧操作身份以已认证控制通道为准，digest 只做长度
/// 合法性参照）。随机源为 `getrandom`（与密钥生成同源），本意图不携带任何秘密。
fn reconnect_intent() -> wire::ConnectIntent {
    let mut operation_id = [0u8; 16];
    let _ = getrandom::fill(&mut operation_id);
    let mut epoch = [0u8; 16];
    let _ = getrandom::fill(&mut epoch);
    let mut request_digest = [0u8; 32];
    let _ = getrandom::fill(&mut request_digest);
    wire::ConnectIntent {
        lookup_key: Some(wire::OperationLookupKey {
            principal_digest: vec![0u8; 32],
            method: wire::OperationMethod::Connect as i32,
            runtime_epoch: epoch.to_vec(),
            operation_id: operation_id.to_vec(),
        }),
        request_digest: request_digest.to_vec(),
        profile: None,
    }
}

/// 自动重连终止清理使用的独立 Stop 意图。StopTunnel 需要自己的 method=Stop lookup
/// key，不能把 ConnectIntent 塞进 StopRequest。
fn reconnect_stop_intent() -> wire::StopIntent {
    let mut operation_id = [0u8; 16];
    let _ = getrandom::fill(&mut operation_id);
    let mut epoch = [0u8; 16];
    let _ = getrandom::fill(&mut epoch);
    let mut request_digest = [0u8; 32];
    let _ = getrandom::fill(&mut request_digest);
    wire::StopIntent {
        lookup_key: Some(wire::OperationLookupKey {
            principal_digest: vec![0u8; 32],
            method: wire::OperationMethod::Stop as i32,
            runtime_epoch: epoch.to_vec(),
            operation_id: operation_id.to_vec(),
        }),
        request_digest: request_digest.to_vec(),
    }
}

/// 自动重连**派发前**同步失败的 typed 终态错误（`drive_reconnect` 的 Err 分支；
/// win32 `apply_connect_failed_for_route` 的 darwin 对应——把 gRPC `Status` 诚实
/// 归类为「连接控制阶段失败 + 用新 operation 重试」：失败发生在 Engine 派发之前
/// （凭据缺失/会话建立失败），数据面从未建立，不用 `DataPlane` 掉线标记（避免自
/// 激活触发语义）。仅生产点明确标记的缺材料原因保留为 reason，其他 gRPC 细节
/// 留在诊断日志；不把服务端认证拒绝与材料缺失混为一类。
fn auth_deadline_status(error: AuthDeadlineError) -> Status {
    match error {
        AuthDeadlineError::Invalid => Status::invalid_argument("DARWIN_CORE_AUTH_DEADLINE_INVALID"),
        AuthDeadlineError::Expired => Status::deadline_exceeded("DARWIN_CORE_AUTH_DEADLINE_EXPIRED"),
        AuthDeadlineError::Clock => Status::internal("DARWIN_CORE_AUTH_CLOCK_FAILED"),
    }
}
fn check_auth_deadline(deadline: Option<AuthDeadline>) -> Result<(), Status> {
    deadline.map(|deadline| deadline.remaining().map(|_| ()).map_err(auth_deadline_status)).unwrap_or(Ok(()))
}

fn connect_materials_missing(cause: &'static str) -> Status {
    let mut status = Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE);
    status.metadata_mut().insert("x-exv-connect-cause", tonic::metadata::MetadataValue::from_static(cause));
    status
}

fn reconnect_materials_missing(status: &Status) -> bool {
    if status.code() != Code::FailedPrecondition { return false; }
    let mut causes = status.metadata().get_all("x-exv-connect-cause").iter();
    let Some(cause) = causes.next() else { return false; };
    causes.next().is_none() && matches!(cause.to_str().ok(),
        Some("atrust-material-missing" | "cisco-material-missing"))
}

fn reconnect_dispatch_vpn_error(status: &Status) -> exv_vpn_wire::generated::VpnError {
    exv_vpn_wire::generated::VpnError {
        code: exv_vpn_wire::generated::ErrorCode::EffectUnknown as i32,
        stage: exv_vpn_wire::generated::ErrorStage::ConnectingControl as i32,
        certainty: exv_vpn_wire::generated::EffectCertainty::Unknown as i32,
        retry: exv_vpn_wire::generated::RetryAdvice::UseNewOperation as i32,
        reason: if reconnect_materials_missing(status) {
            CONNECTION_CREDENTIALS_MISSING_CODE.into()
        } else {
            String::new()
        },
        ..Default::default()
    }
}

/// W3 连接传输判定结果（唯一判定点在 core；前端零逻辑分叉）。
enum ResolvedTransport {
    /// 服务在位（端点 healthy，或已装未跑/拉不起——修复阶梯在会话获取内）。
    Service,
    /// 未安装或探测不可判定：一次性提权直拉（v3 §一 四态表的 Oneshot 兜底）。
    Oneshot,
}

/// W3：oneshot 形态自动重连被拦截时的终态错误——wire 词汇冻结（proto 不动），
/// 复用 EffectUnknown/UseNewOperation；稳定码
/// `DARWIN_CORE_ONESHOT_MANUAL_RECONNECT_REQUIRED` 只进聚合日志（快照无字符串域）。
fn oneshot_manual_reconnect_vpn_error() -> exv_vpn_wire::generated::VpnError {
    reconnect_dispatch_vpn_error(&Status::internal(String::new()))
}

/// 自动重连被当前会话策略关闭后的终态；只在 Engine 已确认清理完成后投影。
fn reconnect_disabled_vpn_error() -> exv_vpn_wire::generated::VpnError {
    reconnect_dispatch_vpn_error(&Status::cancelled("auto reconnect disabled"))
}

/// 重连预算耗尽后的终态；只在 Engine 已确认清理完成后投影。
fn reconnect_exhausted_vpn_error() -> exv_vpn_wire::generated::VpnError {
    reconnect_dispatch_vpn_error(&Status::resource_exhausted("auto reconnect exhausted"))
}

/// 仅记录本次 Stop RPC 的原错误；未取得回执不证明资源已清或仍有残留。
const STOP_TOTAL_BUDGET: Duration = Duration::from_secs(20);

/// 标准 grpc-timeout 是逐跳剩余量；先消费 metadata，再解包消息。
fn stop_request_deadline<T>(request: &Request<T>) -> Result<tokio::time::Instant, Status> {
    let mut budget = STOP_TOTAL_BUDGET;
    if let Some(value) = request.metadata().get("grpc-timeout") {
        let text = value.to_str().map_err(|_| Status::invalid_argument("stop: invalid grpc-timeout"))?;
        if text.len() < 2 || text.len() > 9 { return Err(Status::invalid_argument("stop: invalid grpc-timeout")); }
        let (digits, unit) = text.split_at(text.len() - 1);
        if !digits.bytes().all(|digit| digit.is_ascii_digit()) { return Err(Status::invalid_argument("stop: invalid grpc-timeout")); }
        let value: u64 = digits.parse().map_err(|_| Status::invalid_argument("stop: invalid grpc-timeout"))?;
        let duration = match unit {
            "H" => Duration::from_secs(value * 3600), "M" => Duration::from_secs(value * 60),
            "S" => Duration::from_secs(value), "m" => Duration::from_millis(value),
            "u" => Duration::from_micros(value), "n" => Duration::from_nanos(value),
            _ => return Err(Status::invalid_argument("stop: invalid grpc-timeout")),
        };
        budget = budget.min(duration);
    }
    Ok(tokio::time::Instant::now() + budget)
}

fn log_stop_unconfirmed(logs: &crate::log_aggregator::LogAggregator, error: &EngineLifecycleError) {
    let fields = match error {
        EngineLifecycleError::Observe(crate::ObserveError::Rpc(status)) => vec![
            ("rpc_code", format!("{:?}", status.code())),
            ("rpc_message", status.message().to_owned()),
        ],
        _ => vec![("error", format!("{error:?}"))],
    };
    logs.append_core("error", "core", "STOP_UNCONFIRMED", "停止RPC未取得完成回执", &fields);
}

/// Engine 会话拉起接缝：生产固定为服务代理的完整拉起路径；测试注入可计数的
/// fixture 拉起（每次调用必然对应一次全新 Engine 会话启动，不自动重连）。
type LaunchFuture =
    Pin<Box<dyn Future<Output = Result<LiveEngineSession, EngineLifecycleError>> + Send>>;
type SessionLaunch = Arc<dyn Fn() -> LaunchFuture + Send + Sync>;

/// 生产拉起接缝：经服务代理启动固定 root Engine 并建立已认证控制会话。
fn production_session_launch() -> SessionLaunch {
    Arc::new(|| {
        Box::pin(async {
            launch_fixed_engine_session()
                .await
                .map(LiveEngineSession::Live)
        })
    })
}

/// Core 对 Engine 控制会话的统一会话面（[`EngineControlSession`] 的机械适配）。
///
/// 生产变体持有真实 root Engine 会话；fixture 变体只在测试构建存在，注入受控的
/// 状态流/统计流与 Apply/Stop 回放，不触碰 UDS、root、服务代理或网络。
enum LiveEngineSession {
    Live(EngineControlSession),
    #[cfg(test)]
    Fixture(test_support::FixtureEngineSession),
}

impl LiveEngineSession {
    fn proxy_observe_handle(&self) -> Option<crate::engine_lifecycle_real::EngineProxyObserveHandle> {
        match self {
            Self::Live(session) => session.proxy_observe_handle(),
            #[cfg(test)]
            Self::Fixture(_) => None,
        }
    }

    fn latency_refresh_handle(&self) -> Option<EngineLatencyRefresh> {
        match self {
            Self::Live(session) => session
                .latency_refresh_handle()
                .map(EngineLatencyRefresh::Live),
            #[cfg(test)]
            Self::Fixture(session) => Some(session.latency_refresh_handle()),
        }
    }

    /// 本会话对应 Engine 进程的 pid（bootstrap `Ready` record 的真实 pid；服务
    /// 形态常驻 engine pid 未知，恒 0）。
    fn engine_pid(&self) -> u32 {
        match self {
            Self::Live(session) => session.engine_pid(),
            #[cfg(test)]
            Self::Fixture(session) => session.engine_pid(),
        }
    }

    /// W2.5：本会话形态的 `snapshot.mode` 事实源（服务形态="service"；会话形态与
    /// fixture=空串——W3 补 "oneshot"）。
    fn mode(&self) -> &'static str {
        match self {
            Self::Live(session) => session.kind().as_mode_str(),
            #[cfg(test)]
            Self::Fixture(session) => session.mode,
        }
    }

    /// 挂接 Engine 状态流（attach-before-apply：必须先挂流再 `ApplyTunnel`）。
    async fn attach_connect_status(
        &mut self,
    ) -> Result<EngineUpdateStream<wire::ConnectStatusEvent>, EngineLifecycleError> {
        match self {
            Self::Live(session) => session
                .attach_connect_status()
                .await
                .map(|stream| EngineUpdateStream::Live(Box::new(stream))),
            #[cfg(test)]
            Self::Fixture(session) => session.attach_connect_status().await,
        }
    }

    /// 挂接 Engine 统计流（W-MVP-13 的既有 `StreamStats` 接缝）。
    async fn attach_stats(
        &mut self,
    ) -> Result<EngineUpdateStream<wire::StatsEvent>, EngineLifecycleError> {
        match self {
            Self::Live(session) => session
                .attach_stats()
                .await
                .map(|stream| EngineUpdateStream::Live(Box::new(stream))),
            #[cfg(test)]
            Self::Fixture(session) => session.attach_stats(),
        }
    }

    /// 挂接 Engine 日志流（MAC-OBS-13 S1 的既有 `StreamLogs` 接缝）。
    async fn attach_logs(
        &mut self,
    ) -> Result<EngineUpdateStream<wire::LogEvent>, EngineLifecycleError> {
        match self {
            Self::Live(session) => session
                .attach_logs()
                .await
                .map(|stream| EngineUpdateStream::Live(Box::new(stream))),
            #[cfg(test)]
            Self::Fixture(session) => session.attach_logs(),
        }
    }

    /// 把连接请求交给同一已认证 Engine 通道。
    async fn apply_tunnel(
        &mut self,
        request: exv_vpn_wire::generated::ApplyTunnelRequest,
    ) -> Result<exv_vpn_wire::generated::ApplyTunnelReply, EngineLifecycleError> {
        match self {
            Self::Live(session) => session.apply_tunnel(request).await,
            #[cfg(test)]
            Self::Fixture(session) => session.apply_tunnel(request),
        }
    }

    /// 把断开请求交给同一已认证 Engine 通道。
    async fn auth_receiver_control(&mut self, request: wire::AuthReceiverControlRequest) -> Result<wire::AuthReceiverControlReply, EngineLifecycleError> {
        match self {
            Self::Live(session) => session.auth_receiver_control(request).await,
            #[cfg(test)]
            Self::Fixture(session) => session.auth_receiver_control(request),
        }
    }

    async fn stop_tunnel(
        &mut self,
        request: exv_vpn_wire::generated::StopTunnelRequest,
    ) -> Result<exv_vpn_wire::generated::StopTunnelReply, EngineLifecycleError> {
        match self {
            Self::Live(session) => session.stop_tunnel(request).await,
            #[cfg(test)]
            Self::Fixture(session) => session.stop_tunnel(request).await,
        }
    }

    /// 释放本会话 client；不读取 Engine 进程终态，也不生成 Stop 完成回执。
    fn close(self) -> Result<(), EngineLifecycleError> {
        match self {
            Self::Live(session) => session.close(),
            #[cfg(test)]
            Self::Fixture(_) => Ok(()),
        }
    }

    /// W2 心跳句柄：克隆通道（生产）或共享记录器（fixture）——keepalive ticker
    /// 独立持有，不借用会话槽的 `&mut`。
    fn heartbeat_handle(&self) -> Option<EngineHeartbeat> {
        match self {
            Self::Live(session) => session.heartbeat_handle().map(EngineHeartbeat::Live),
            #[cfg(test)]
            Self::Fixture(session) => Some(EngineHeartbeat::Fixture(session.heartbeat_shared())),
        }
    }
}

/// W2 心跳句柄的统一面：生产=克隆自已认证通道的 tonic client；fixture=共享记录器
/// （测试观测点：记录每条 `KeepAlive` 的到达时刻与 tick）。
enum EngineLatencyRefresh {
    Live(crate::engine_lifecycle_real::EngineLatencyRefreshHandle),
    #[cfg(test)]
    UnsupportedFixture,
}
impl EngineLatencyRefresh {
    async fn trigger(
        &mut self,
        operation_id: Vec<u8>,
    ) -> Result<wire::HelperLatencyRefreshReply, Status> {
        match self {
            Self::Live(handle) => handle.trigger_latency_refresh(operation_id).await,
            #[cfg(test)]
            Self::UnsupportedFixture => Err(Status::unimplemented("fixture has no latency owner")),
        }
    }
}
enum EngineHeartbeat {
    Live(crate::engine_lifecycle_real::EngineHeartbeatHandle),
    #[cfg(test)]
    Fixture(Arc<Mutex<test_support::FixtureShared>>),
}

impl EngineHeartbeat {
    /// 发送一条 `KeepAlive`（`monotonic_tick` 由 ticker 严格递增）；best-effort——
    /// 错误上抛给调用方忽略（不 panic、不重试风暴）。
    async fn keep_alive(&mut self, tick: u64) -> Result<wire::KeepAliveReply, EngineLifecycleError> {
        match self {
            Self::Live(handle) => handle.keep_alive(tick).await,
            #[cfg(test)]
            Self::Fixture(shared) => {
                shared
                    .lock()
                    .expect("fixture shared state mutex")
                    .keeps
                    .push((tokio::time::Instant::now(), tick));
                Ok(wire::KeepAliveReply { monotonic_tick: tick })
            }
        }
    }
}

/// W2 keepalive ticker：按 `period`（生产 [`HEARTBEAT_PERIOD`] = 10s）周期经克隆
/// 通道发 `KeepAlive`（`monotonic_tick` 严格递增，从 1 起）——Engine 侧 touch
/// 心跳时间戳，15s 未收到即触发 W1 统一收敛（hung-core / EOF 路径故障的硬时间界
/// 兜底）。对齐 win32 `spawn_keepalive_ticker`。
///
/// `tokio::time::interval` **首拍立即**（t=0 即第一条心跳，握手零点即刻被维护）
/// ——默认语义，测试钉死（win32 `grpc_control` 测试同款断言）。发送 best-effort：
/// 失败（engine 掉线）不 panic、不重试风暴——下一个周期再发。观察流
/// （stats/status/logs）不刷心跳：本 ticker 是唯一心跳发送方。
///
/// 生命周期与 session 槽位绑定：spawn 于会话填充点，全部槽位重置点（EOF 投影
/// 重置、stop 清槽、`close`）abort 对应句柄。
fn spawn_keepalive_ticker(
    heartbeat: EngineHeartbeat,
    period: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut heartbeat = heartbeat;
        let mut ticker = tokio::time::interval(period);
        let mut tick: u64 = 0;
        loop {
            ticker.tick().await;
            tick = tick.saturating_add(1);
            let _ = heartbeat.keep_alive(tick).await;
        }
    })
}

/// Engine 状态/统计更新流的统一消费面。
///
/// 生产变体是同一已认证通道上的真实 tonic 流；fixture 变体只在测试构建存在，
/// 由受控 sender 驱动——drop sender 即模拟 Engine 失联的流终止。
enum EngineUpdateStream<T> {
    Live(Box<tonic::Streaming<T>>),
    #[cfg(test)]
    Fixture(tokio::sync::mpsc::UnboundedReceiver<Result<T, Status>>),
}

impl<T> EngineUpdateStream<T> {
    /// 取下一条更新：`Ok(None)`=流正常结束（EOF），`Err(_)`=transport 失败。
    /// 两者都是 Engine 失联的事件驱动观察点。
    async fn message(&mut self) -> Result<Option<T>, Status> {
        match self {
            Self::Live(stream) => stream.message().await,
            #[cfg(test)]
            Self::Fixture(receiver) => Ok(receiver.recv().await.transpose()?),
        }
    }
}

/// S5b：重连暂停的稳定 reason id（wire `ReconnectStatus.reason`；前端 toast 按 id
/// 路由）。owner 失配=已保存密文归属账户与当前受理/配置账户不一致；清除=已保存
/// 密文被显式清除（受理快照联动丢弃）。
const RECONNECT_PAUSE_OWNER_MISMATCH: &str = "reconnect_paused_owner_mismatch";
const RECONNECT_PAUSE_CREDENTIALS_CLEARED: &str = "reconnect_paused_credentials_cleared";

/// S5b（2026-10-26 C-10 调和）：Cisco 连接受理时对"已持久化凭据对"（用户名+解密
/// 密码）做的内存态快照。重连组装优先使用快照值（免疫连接中的凭据/用户名修改，
/// 重连会话与受理会话同账户同密码）；内存态零化类型，随会话策略同生命周期（新会
/// 话受理替换、显式 Stop 清空时 Drop 零化），显式"清除密码"降权点联动丢弃。
/// 一次性连接（persist=false）不建快照——其资格由受理期规则关闭。
#[derive(Clone)]
struct AdmissionCredentialSnapshot {
    username: String,
    password: zeroize::Zeroizing<String>,
}

impl std::fmt::Debug for AdmissionCredentialSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 秘密不进任何 Debug 输出（结构可能随策略被派生 Debug 打印）。
        f.debug_struct("AdmissionCredentialSnapshot")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// 一次用户连接受理时冻结的重连策略；自动重连尝试复用同一份值。
#[derive(Debug, Clone)]
struct SessionReconnectPolicy {
    atrust_auto_login: bool,
    atrust_server: String,
    auth_mode: String,
    config_dir: String,
    user_agent: String,
    enabled: bool,
    max_attempts: u32,
    backoff: bool,
    protocol: Protocol,
    identity: Option<wire::SessionIdentity>,
    /// S5b：Cisco 受理快照（受理时的已持久化账密对；一次性连接恒 `None`）。
    credential_snapshot: Option<AdmissionCredentialSnapshot>,
    /// S5b：重连暂停的稳定 reason id（`RECONNECT_PAUSE_*`；`None` = 无暂停事实）。
    pause_reason: Option<&'static str>,
}

impl SessionReconnectPolicy {
    fn from_config(config: &DarwinUiConfig) -> Self {
        Self {
            atrust_auto_login: true,
            atrust_server: config.atrust_server().to_owned(),
            auth_mode: "headless".to_owned(),
            config_dir: config.config_dir().to_string_lossy().into_owned(),
            user_agent: config.user_agent().to_owned(),
            // S5b（C-10）：受理期冻结值与活评估同公式——凭据门槛不再对网页登录
            // 用户恒 false（aTrust 臂=会话缓存/恢复材料）；ENGINE_LOST 时点由
            // `valid_reconnect_credential_source` 对活状态另行求值。
            enabled: config.auto_reconnect() && valid_reconnect_credential_source(config, None),
            max_attempts: config.auto_reconnect_max_attempts(),
            backoff: config.auto_reconnect_backoff(),
            protocol: config.protocol(),
            identity: None,
            credential_snapshot: None,
            pause_reason: None,
        }
    }
}

/// C-10（S5b）ENGINE_LOST 时点活评估的有效凭据源单一公式（失联信号发射与 worker
/// 消费两点共用）：
///
/// 有效凭据源 =（Cisco：受理快照存在）∨（Cisco：磁盘账密对 owner 匹配且与受理
/// 身份一致）∨（aTrust：会话缓存或恢复材料）。
///
/// * 快照臂：受理时已持久化的账密对在内存——免疫连接中凭据/用户名修改（受理
///   账户与磁盘已保存账户不一致时仍按受理账户重连，静默账号切换由本臂消灭）；
/// * 磁盘臂：`has_saved_credential_pair`（owner 匹配门）且冻结受理身份与当前配置
///   用户名一致（受理身份缺席——policy 生命周期洞——按磁盘对自身求值）；
/// * aTrust 臂（S5b 路径结论验证）：重连凭据由既有 session cache 与 recovery
///   material（认证续段获取轮）承担——受理材料即快照语义，无需另建账密快照。
///   缓存命中为直接源；缓存缺席时恢复轮可再获取（auth continuation），故 aTrust
///   会话的有效性由受理事实本身承载（获取失败仍按既有"一次尝试即收束"终止）。
fn valid_reconnect_credential_source(
    config: &DarwinUiConfig,
    policy: Option<&SessionReconnectPolicy>,
) -> bool {
    let protocol = policy
        .map(|policy| policy.protocol)
        .unwrap_or_else(|| config.protocol());
    match protocol {
        Protocol::Cisco => {
            if policy.is_some_and(|policy| policy.credential_snapshot.is_some()) {
                return true;
            }
            config.has_saved_credential_pair()
                && policy
                    .and_then(|policy| policy.identity.as_ref())
                    .map_or(true, |identity| identity.username == config.username())
        }
        Protocol::Atrust => true,
    }
}

/// 当前会话策略与重连进度共同构成运行事实；尚未连接时不投影保存设置。
/// S5b：暂停 reason lane（`RECONNECT_PAUSE_*`）镜像进 wire 供前端 toast 路由。
fn session_reconnect_status(
    policy: &Mutex<Option<SessionReconnectPolicy>>,
    attempts: &AtomicU32,
    active: &AtomicBool,
) -> Option<wire::ReconnectStatus> {
    lock_recover(policy).as_ref().map(|policy| {
        reconnect_status_from_state(
            attempts.load(Ordering::Relaxed),
            active.load(Ordering::Acquire),
            policy.enabled,
            policy.max_attempts,
            policy.pause_reason.unwrap_or(""),
        )
    })
}

/// 实际 Core→Engine 会话的最小适配器。
///
/// 它只把 Windows 已有的 Connect/Stop 意图机械转换为同一 Common 控制通道上的
/// ApplyTunnel/StopTunnel。Engine 会话按需启动并保留到 Core UI session 结束；本类型不
/// 拥有 CSTP、utun 或任何网络数据通路。
pub(crate) struct LiveConnectionProjection {
    /// 每次连接／停止递增，取消此前排队的唤醒或退避重连。
    reconnect_generation: Arc<session_orchestration::SessionAdmission>,
    auth_runtime_epoch: Vec<u8>,
    #[cfg(test)]
    auth_acquisition_duration: Duration,
    #[cfg(test)]
    auth_cleanup_duration: Duration,
    config: Arc<Mutex<DarwinUiConfig>>,
    /// 用户连接受理时冻结；ConfigSet 只更新 config，自动重连复用本策略。
    reconnect_session_policy: Arc<Mutex<Option<SessionReconnectPolicy>>>,
    /// 唯一 Engine 会话槽位。MAC-LIFECYCLE-15 S1：Engine 失联（状态流终止或 pid 已死）
    /// 的观察点会把槽位重置为 `None`，下一次 Connect 走完整拉起路径（服务代理启动新
    /// Engine）；`Arc` 使状态投影任务也能在流终止时完成同一重置。
    session: Arc<tokio::sync::Mutex<Option<LiveEngineSession>>>,
    /// 共享事件总线（W3-1/P1 S2：与 service/watch 同一实例；tick 由总线铸造）。
    events: Arc<EventBus>,
    /// 当前 operation 的统计状态（W-MVP-13）：connect 时替换为新状态，失败/Stop 时终结。
    /// 状态投影与统计投影两个任务共享同一实例。
    stats_state: Arc<std::sync::Mutex<LiveStatsState>>,
    /// 日志聚合语义层（MAC-OBS-13 S1）：Engine `StreamLogs` ingest 与 Core 自身
    /// 连接期事实（会话建立/失联/终态）入库。
    logs: Arc<LogControl>,
    /// Engine 会话拉起接缝：生产=服务代理完整拉起；测试=可计数的 fixture 拉起。
    launch_session: SessionLaunch,
    #[cfg(test)]
    cached_session_probe: Option<cached_headless::ProbeSeam>,
    /// 上游代理 TUN 检测缓存（MAC-PROXY-16 S1，与 service 共享同一实例）：发布边界
    /// 只读附加，连接受理/断开终态边界刷新。
    proxy_tun: Arc<ProxyTunCache>,
    /// W3-2/P3：本连接生命周期内已派发的自动重连次数（per-connection；Connected
    /// 成功时由状态投影清零——预算回到 0，退避序列随之重置；镜像 win32
    /// `reconnect_attempts`）。
    reconnect_attempts: Arc<AtomicU32>,
    /// W3-2/P3：自动重连在途标记（派发起点到终态事件之间为 `true`；与单 worker
    /// 串行消费共同防重入——镜像 win32 `reconnect_active`）。
    reconnect_active: Arc<AtomicBool>,
    /// W3-2/P3：状态投影→重连 worker 的掉线信号通道（单发送端槽位随 worker 拉起
    /// 换入；`std::sync::Mutex` 短临界区，不跨 await——win32 :1915-1925 同形态）。
    reconnect_tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<(u64, ReconnectSignal)>>>>,
    /// W3-2/P3：退避 base（产品默认 [`RECONNECT_BACKOFF_BASE`] = 2s；测试经
    /// [`Self::with_reconnect_backoff`] 注入毫秒级，生产路径不改默认）。
    reconnect_backoff_base: Duration,
    /// W3-2/P3：退避 cap（产品默认 [`RECONNECT_BACKOFF_CAP`] = 30s；同上 seam）。
    reconnect_backoff_cap: Duration,
    /// W3-2/P3 守卫轨：本连接生命周期内（自上次显式 Stop→Idle 起）任一 operation
    /// 曾到达 Connected——数据面曾真实运行过的生命周期级事实。per-operation 的
    /// `LiveStatsState` 每次 connect 重建、终态清零，无法承载跨重连尝试的「曾连接」
    /// 守卫（重连预算/无限重试语义要求链式掉线持续触发），故由投影在 Connected
    /// 事件处置位；显式 Stop 清零（新连接生命周期从「未连接」起算）。
    reconnect_ever_connected: Arc<AtomicBool>,
    /// P4 v2：服务生命周期执行器（连接挂钩 `ensure_running` 用；夹具默认恒
    /// healthy 探测，生产经 [`Self::with_service_seams`] 注入真实提权编排）。
    service_lifecycle: Arc<ServiceLifecycle>,
    /// 服务形态原连接与一次恢复动作接缝。
    service_engine: Arc<ServiceEnginePath>,
    /// W2.5：当前会话形态的 `snapshot.mode` 值（会话建立点写入；发布边界附加。
    /// 会话/fixture 形态为空串——W3 补 "oneshot"）。
    engine_mode: Mutex<&'static str>,
    /// P4 v2：服务状态缓存伴生值（发布边界只读附加；与 service 共享同一实例）。
    service_status_cache: Arc<Mutex<Option<exv_vpn_wire::generated::ServiceStatus>>>,
    /// W2 心跳：会话伴生的 keepalive ticker 句柄（与 session 槽同生命周期——槽位
    /// 填充时 spawn、全部重置点 abort，不留孤儿 ticker；状态投影也持 Arc 以在
    /// 失联重置时一并 abort）。
    keepalive_ticker: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

/// W2.5 服务形态固定端点直连 future/接缝类型。
///
/// 2026-09-30 修复（M2b）：返回 [`LiveEngineSession`] 而非裸
/// [`crate::engine_lifecycle_real::EngineControlSession`]——生产恒为 `Live` 变体
///（行为不变）；测试可注入 `Fixture` 变体复现「attach 遭占用型拒绝」，
/// 由原挂流返回驱动，不触碰宿主端点。
type ServiceEngineConnectFuture =
    Pin<Box<dyn Future<Output = Result<LiveEngineSession, EngineLifecycleError>> + Send>>;
type ServiceEngineConnect = Arc<dyn Fn(Option<tokio::time::Instant>) -> ServiceEngineConnectFuture + Send + Sync>;

/// 服务端点原连接、EnsureEngine 原帧接缝；probe 仅供独立模式展示选择。
pub(crate) struct ServiceEnginePath {
    /// engine 端点事实探测（engine.sock 在+可连=healthy）。
    pub(crate) probe: ServiceStatusProbe,
    /// `EnsureEngine` 完整原帧结果。
    pub(crate) ensure_engine: Arc<
        dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), crate::service_agent_client::EnsureEngineError>> + Send>> + Send + Sync,
    >,
    /// 固定端点直连（失败=Err）。
    connect: ServiceEngineConnect,
}

impl ServiceEnginePath {
    /// 生产接缝：真实 facts 探测 + `EnsureEngine` 帧 + 固定端点直连。
    pub(crate) fn production() -> Self {
        Self {
            probe: std::sync::Arc::new(|| {
                Box::pin(crate::service_status::probe_service_agent_facts())
            }),
            ensure_engine: std::sync::Arc::new(|| {
                Box::pin(async {
                    crate::service_agent_client::ensure_engine(
                        crate::elevation::current_core_credentials(),
                    )
                    .await
                })
            }),
            connect: std::sync::Arc::new(|deadline| {
                Box::pin(async move {
                    crate::engine_lifecycle_real::connect_service_engine_session_with_deadline(deadline)
                        .await
                        .map(LiveEngineSession::Live)
                })
            }),
        }
    }

    /// 夹具默认原连接失败；不触宿主文件系统或网络。
    pub(crate) fn hermetic() -> Self {
        Self {
            probe: std::sync::Arc::new(|| {
                Box::pin(std::future::ready(crate::service_status::ServiceAgentFacts {
                    engine_socket_present: false,
                    engine_socket_connectable: false,
                    ..crate::service_status::ServiceAgentFacts::default()
                }))
            }),
            ensure_engine: std::sync::Arc::new(|| Box::pin(std::future::ready(Err(crate::service_agent_client::EnsureEngineError::InvalidOwner)))),
            connect: std::sync::Arc::new(|_| {
                Box::pin(std::future::ready(Err(
                    crate::engine_lifecycle_real::EngineLifecycleError::Bootstrap,
                )))
            }),
        }
    }
}

/// 一次 operation 的状态投影参数包：状态投影任务所需的共享状态与身份信息。
struct OperationProjection {
    candidate_receipt: Option<atrust_session_cache::CandidateReceipt>,
    generation: u64,
    stats_state: Arc<std::sync::Mutex<LiveStatsState>>,
    runtime_epoch: Vec<u8>,
    operation_id: Vec<u8>,
    intent: Option<exv_vpn_wire::generated::ConnectIntent>,
    session_slot: Arc<tokio::sync::Mutex<Option<LiveEngineSession>>>,
    /// W2 心跳：会话伴生 ticker 槽——失联重置 session 槽时一并 abort 对应 ticker。
    keepalive_ticker: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    engine_pid: u32,
    /// T5：会话形态快照（构造点=self.engine_mode；投影期内不变）——EOF 失联
    /// 分支据此裁决 R2 资格（service 才自动重连）。
    engine_mode: &'static str,
}

impl LiveConnectionProjection {
    fn validate_latency_operation(&self, operation_id: &[u8]) -> Result<(), Status> {
        let reject = || Status::failed_precondition("DARWIN_LATENCY_OWNER_NOT_ACTIVE");
        let snapshot = self.events.current_snapshot().ok_or_else(reject)?;
        if operation_id.len() != 16
            || snapshot.operation_id != operation_id
            || !matches!(
                snapshot.state,
                Some(wire::runtime_snapshot::State::Connected(_))
            )
        {
            return Err(reject());
        }
        let identity = self
            .reconnect_generation
            .identity_for(operation_id)
            .ok_or_else(reject)?;
        if identity.protocol != "cisco" {
            return Err(Status::unimplemented("当前协议不支持时延刷新"));
        }
        Ok(())
    }

    async fn trigger_latency_refresh(
        &self,
        operation_id: Vec<u8>,
    ) -> Result<wire::KernelLatencyRefreshReply, Status> {
        let mut handle = {
            let session = self.session.lock().await;
            self.validate_latency_operation(&operation_id)?;
            session
                .as_ref()
                .and_then(LiveEngineSession::latency_refresh_handle)
                .ok_or_else(|| Status::failed_precondition("DARWIN_LATENCY_OWNER_NOT_ACTIVE"))?
        };
        // 同一已认证 client 的短句柄；不持会话/组合锁等待 Engine。
        let reply = handle.trigger(operation_id.clone()).await?;
        self.validate_latency_operation(&operation_id)?;
        if !reply.accepted {
            return Err(Status::failed_precondition(
                "DARWIN_LATENCY_REFRESH_NOT_ACCEPTED",
            ));
        }
        Ok(wire::KernelLatencyRefreshReply { accepted: true })
    }

    /// 以固定拉起接缝创建投影；`new` 传入生产接缝，测试注入可计数的 fixture
    /// 拉起。生产行为不因接缝存在而变化：每次 Connect 缺失会话时调用恰好一次。
    /// 事件经共享 [`EventBus`] 发布（tick 由总线铸造；构造期 initial Idle 已占
    /// tick 1，投影发布从 tick 2 起——与旧自带 tick 轴的序列一致）。
    fn with_launch(
        config: Arc<Mutex<DarwinUiConfig>>,
        events: Arc<EventBus>,
        launch_session: SessionLaunch,
        logs: Arc<LogControl>,
        proxy_tun: Arc<ProxyTunCache>,
    ) -> Self {
        Self {
            config,
            reconnect_session_policy: Arc::new(Mutex::new(None)),
            session: Arc::new(tokio::sync::Mutex::new(None)),
            reconnect_generation: Arc::new(session_orchestration::SessionAdmission::default()),
            auth_runtime_epoch: auth_continuation::interaction_id(),
            #[cfg(test)]
            auth_acquisition_duration: Duration::from_secs(300),
            #[cfg(test)]
            auth_cleanup_duration: STOP_TOTAL_BUDGET,
            events,
            stats_state: Arc::new(std::sync::Mutex::new(LiveStatsState::new())),
            logs,
            launch_session,
            #[cfg(test)]
            cached_session_probe: None,
            proxy_tun,
            reconnect_attempts: Arc::new(AtomicU32::new(0)),
            reconnect_active: Arc::new(AtomicBool::new(false)),
            reconnect_tx: Arc::new(Mutex::new(None)),
            reconnect_backoff_base: RECONNECT_BACKOFF_BASE,
            reconnect_backoff_cap: RECONNECT_BACKOFF_CAP,
            reconnect_ever_connected: Arc::new(AtomicBool::new(false)),
            service_lifecycle: Arc::new(crate::service_lifecycle::hermetic_lifecycle()),
            service_status_cache: Arc::new(Mutex::new(None)),
            service_engine: Arc::new(ServiceEnginePath::hermetic()),
            engine_mode: Mutex::new(""),
            keepalive_ticker: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// P4 v2：注入生产服务接缝（真实提权生命周期 + 与 service 共享的
    /// 状态缓存）；`with_launch` 的夹具默认保持 hermetic（不触发提权与真实探测）。
    fn with_service_seams(
        mut self,
        service_lifecycle: Arc<ServiceLifecycle>,
        service_status_cache: Arc<Mutex<Option<exv_vpn_wire::generated::ServiceStatus>>>,
    ) -> Self {
        self.service_lifecycle = service_lifecycle;
        self.service_status_cache = service_status_cache;
        self
    }

    /// W2.5：注入服务形态连接路径接缝（生产真实探测/帧/直连；夹具默认恒回退
    /// 会话路径）。修复阶梯编排契约测试注入 fake。
    fn with_service_engine(mut self, service_engine: Arc<ServiceEnginePath>) -> Self {
        self.service_engine = service_engine;
        self
    }

    /// 注入自动重连退避参数（W3-2/P3）：`drive_reconnect` 在 `auto_reconnect_backoff`
    /// 开启时按 `min(base·2^(n-1), cap)` sleep；测试注入毫秒级 base/cap 钉死节奏，
    /// 生产路径维持产品默认 base=2s/cap=30s（[`RECONNECT_BACKOFF_BASE`]/
    /// [`RECONNECT_BACKOFF_CAP`]）。
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "退避毫秒 seam 仅由重连矩阵测试注入；生产路径恒用产品默认 2s/30s"
        )
    )]
    fn with_reconnect_backoff(mut self, base: Duration, cap: Duration) -> Self {
        self.reconnect_backoff_base = base;
        self.reconnect_backoff_cap = cap;
        self
    }

    async fn prepare_engine_session(
        &self, session: &mut Option<LiveEngineSession>, generation: u64,
        auth_deadline: Option<AuthDeadline>,
    ) -> Result<(), Status> {
        self.check_service_attempt(generation, auth_deadline)?;
        if session.is_none() {
            // W3：连接传输判定唯一分叉（前端零逻辑；v3 §一 四态表）。服务在位
            //（端点 healthy 或已装）走常驻端点+修复阶梯；未装/不可判定走 oneshot
            // 一次性提权直拉（弹窗可能在此出现——对齐 PromptStart 的提权时机）。
            let transport = self.await_auth_preparation(generation, auth_deadline, self.resolve_transport()).await?;
            check_auth_deadline(auth_deadline)?;
            let new_session = match transport {
                ResolvedTransport::Service => {
                    self.service_engine_session_or_reject(generation, auth_deadline).await?
                }
                ResolvedTransport::Oneshot => {
                    self.await_auth_preparation(generation, auth_deadline, (self.launch_session)())
                        .await?
                        .map_err(|error| engine_session_status(&error))?
                }
            };
            // W2.5：会话形态即 mode 事实（发布边界附加；服务形态="service"）。
            *lock_recover(&self.engine_mode) = new_session.mode();
            // W2 心跳：会话建立（session 槽位填充点）→ 拉起伴生 keepalive
            // ticker（首拍立即，经克隆通道发送；观察流任务不刷心跳——ticker 是
            // 唯一发送方）。
            {
                let mut ticker_slot = self.await_auth_preparation(generation, auth_deadline, self.keepalive_ticker.lock()).await?;
                let ticker = new_session.heartbeat_handle()
                    .map(|heartbeat| spawn_keepalive_ticker(heartbeat, HEARTBEAT_PERIOD));
                if let Some(previous) = ticker_slot.take() {
                    previous.abort();
                }
                *ticker_slot = ticker;
            }
            *session = Some(new_session);
            // MAC-OBS-13 S1：Engine 会话建立的真实节点入库（Core 自身事件）。
            // 2026-09-30（M2b）：重建复用本建立块——每次真实建立各记一条。
            self.logs.aggregator().append_core(
                "info",
                "core",
                "ENGINE_SESSION_STARTED",
                "engine session started via service agent",
                &[(
                    "pid",
                    session
                        .as_ref()
                        .map_or(0, LiveEngineSession::engine_pid)
                        .to_string(),
                )],
            );
        }
        Ok(())
    }

    async fn prepare_auth_control(&self, deadline: AuthDeadline) -> Result<(), Status> {
        let generation = self.reconnect_generation.current();
        let mut session = self.await_auth_preparation(generation, Some(deadline), self.session.lock()).await?;
        self.prepare_engine_session(&mut session, generation, Some(deadline)).await
    }

    async fn auth_receiver_control(&self, request: wire::AuthReceiverControlRequest, deadline: AuthDeadline) -> Result<wire::AuthReceiverControlReply, Status> {
        let generation = self.reconnect_generation.current();
        let mut session = self.await_auth_preparation(generation, Some(deadline), self.session.lock()).await?;
        self.prepare_engine_session(&mut session, generation, Some(deadline)).await?;
        let engine = session.as_mut().ok_or_else(|| Status::unavailable(CONNECTION_ENGINE_NOT_ATTACHED_CODE))?;
        let result = self.await_auth_preparation(generation, Some(deadline), engine.auth_receiver_control(request)).await?
            .map_err(|error| match error {
                EngineLifecycleError::Observe(crate::ObserveError::Rpc(status)) => status,
                error => engine_session_status(&error),
            });
        if result.as_ref().is_err_and(|status| status.code() == tonic::Code::Unimplemented) {
            // 本次准备已发现旧引擎缺少接口；释放通道，服务更新后重新建立会话。
            session.take();
            if let Some(ticker) = self.keepalive_ticker.lock().await.take() {
                ticker.abort();
            }
        }
        result
    }

    async fn connect(
        &self,
        request: &ConnectRequest,
        ui_credentials: Option<UiSecretPayload>,
        persist_credentials: bool,
    ) -> Result<OperationReply, Status> {
        self.connect_with_auth_deadline(request, ui_credentials, persist_credentials, None).await
    }

    async fn connect_with_auth_deadline(
        &self,
        request: &ConnectRequest,
        ui_credentials: Option<UiSecretPayload>,
        persist_credentials: bool,
        auth_deadline: Option<AuthDeadline>,
    ) -> Result<OperationReply, Status> {
        check_auth_deadline(auth_deadline)?;
        let clear_auth_acquisition = self.auth_acquisition().is_some();
        let generation = self.reconnect_generation.advance();
        if clear_auth_acquisition { self.publish_auth_acquisition(); }
        let result = self.connect_in_generation_with_auth_deadline(request, ui_credentials, persist_credentials, generation, true, None, auth_deadline).await;
        self.reconnect_generation.finish_request(generation);
        result
    }

    #[allow(
        clippy::too_many_lines,
        reason = "连接受理承载 attach-before-apply 的三条流挂接与入库节点，线性展开保持事实顺序可读"
    )]
    async fn connect_in_generation(
        &self,
        request: &ConnectRequest,
        ui_credentials: Option<UiSecretPayload>,
        persist_credentials: bool,
        generation: u64,
        new_session: bool,
        recovery_policy: Option<SessionReconnectPolicy>,
    ) -> Result<OperationReply, Status> {
        self.connect_in_generation_with_auth_deadline(request, ui_credentials, persist_credentials,
            generation, new_session, recovery_policy, None).await
    }

    async fn connect_in_generation_with_auth_deadline(
        &self,
        request: &ConnectRequest,
        ui_credentials: Option<UiSecretPayload>,
        persist_credentials: bool,
        generation: u64,
        new_session: bool,
        recovery_policy: Option<SessionReconnectPolicy>,
        auth_deadline: Option<AuthDeadline>,
    ) -> Result<OperationReply, Status> {
        self.connect_in_generation_material(request, ui_credentials, persist_credentials, generation,
            new_session, recovery_policy, auth_deadline, false).await
    }

    async fn connect_in_generation_material(
        &self, request: &ConnectRequest, ui_credentials: Option<UiSecretPayload>, persist_credentials: bool,
        generation: u64, new_session: bool, recovery_policy: Option<SessionReconnectPolicy>,
        auth_deadline: Option<AuthDeadline>, admitted_recovery_material: bool,
    ) -> Result<OperationReply, Status> {
        if new_session == recovery_policy.is_some() {
            return Err(Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE));
        }
        if !new_session { self.require_background_reconnect()?; }
        if admitted_recovery_material && !self.admitted_auth_material(generation, auth_deadline) {
            return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"));
        }
        let intent = request
            .intent
            .as_ref()
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let lookup_key = intent
            .lookup_key
            .as_ref()
            .filter(|key| key.operation_id.len() == 16)
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        if self.events.current_snapshot().is_some_and(|snapshot| snapshot.stop_completion.as_ref().is_some_and(|stop| stop.blocks_new_connect)) {
            return Err(Status::failed_precondition("DARWIN_ENGINE_CLEANUP_INCOMPLETE：请先重新断开以重试清理"));
        }
        // MAC-PROXY-16 S1 连接受理边界探测：此刻 Engine 尚未拉起/未 apply 隧道，
        // 不存在己方 utun，探测事实不含自身（无特权只读；失败 fail open 到 None）。
        self.proxy_tun.refresh();
        check_auth_deadline(auth_deadline)?;
        let prepared = self.prepare_connect_payload_inner(ui_credentials, persist_credentials, recovery_policy, new_session, admitted_recovery_material)
            .map(|(payload, plan, policy, receipt, identity, probe)|
                (zeroize::Zeroizing::new(payload), plan, policy, receipt, identity, probe));
        // 配置锁等待也消耗入口期限；过期优先于缺材料回退，已组装秘密按值零化。
        check_auth_deadline(auth_deadline)?;
        let (mut secret_payload, plan, mut admitted_policy, candidate_receipt, mut admitted_identity, cached_probe) = prepared?;
        if new_session {
            admitted_policy.identity = Some(admitted_identity.clone());
        } else if let Some(identity) = admitted_policy.identity.as_ref() {
            admitted_identity = identity.clone();
        }
        let auth_deadline = if (new_session || admitted_recovery_material) && admitted_policy.protocol == Protocol::Atrust { auth_deadline } else { None };
        if let Some(probe) = cached_probe {
            self.prepare_cached_headless(probe, candidate_receipt.as_ref(), generation, auth_deadline).await?;
        }
        let mut session = if admitted_recovery_material {
            self.await_auth_preparation(generation, auth_deadline, self.session.lock()).await?
        } else { match auth_deadline {
            Some(deadline) => tokio::time::timeout_at(deadline.tokio_deadline().map_err(auth_deadline_status)?, self.session.lock()).await
                .map_err(|_| auth_deadline_status(AuthDeadlineError::Expired))?,
            None => self.session.lock().await,
        }};
        check_auth_deadline(auth_deadline)?;
        if generation != self.reconnect_generation.current() {
            return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"));
        }
        // 2026-09-30 修复（M2/M2b）：会话建立与 attach 三连流改为循环编排——attach
        // 任一失败即清会话槽并 abort 伴生 keepalive ticker；若失败为占用型拒绝
        //（`FailedPrecondition` + `DARWIN_ENGINE_ATTEMPT_ACTIVE`，即被 engine
        // `SessionOccupiedService` 接管）则清槽后就地重建一次会话（`continue` 重走
        // 本循环头部的会话建立块——resolve_transport→建立→ticker/mode 全部重建；
        // ticker 槽的 take/abort 天然防双 ticker）。上界恒 1 次重建，不做循环；重建
        // 后仍失败按原错误返回（generation 校验仍由本循环之后的既有检查承担）。
        // engine 侧契约自述「busy 窗口内客户端按真实重试语义重连，直至被接纳」
        //（service_runtime.rs `retry_until_served`）——M1 消除了探针制造者后，本重建
        // 覆盖 service agent nudge（uninstall/服务变更路径的瞬时探针形态连接）残余
        // 场景，把「用户必须手点重试」降为「本次连接自愈」。apply 阶段失败**不清槽**
        //：attach 已成功证明通道健康，apply 的 typed 拒绝（ADOPT_*、attempt 门）与
        // 会话可用性无关。
        let mut occupied_rebuild_budget: u8 = 1;
        let (connect_status_stream, engine_stats_stream, engine_logs_stream) = loop {
            check_auth_deadline(auth_deadline)?;
            self.prepare_engine_session(&mut session, generation, auth_deadline).await?;
            check_auth_deadline(auth_deadline)?;
            let attached = self.await_auth_preparation(generation, auth_deadline, attach_engine_streams(&mut session)).await?;
            check_auth_deadline(auth_deadline)?;
            match attached {
                Ok(streams) => break streams,
                Err(error) => {
                    // attach 原失败清除此会话；占用型 typed 拒绝仍只重建一次。
                    *session = None;
                    self.await_auth_preparation(generation, auth_deadline, self.abort_keepalive_ticker()).await?;
                    if occupied_rebuild_budget > 0 && is_engine_attempt_active(&error) {
                        occupied_rebuild_budget -= 1;
                        self.logs.aggregator().append_core(
                            "info",
                            "core",
                            "ENGINE_SESSION_OCCUPIED_REBUILD",
                            "attach was rejected by the occupied engine endpoint; dropped the poisoned channel and rebuilding the session once",
                            &[],
                        );
                        continue;
                    }
                    return Err(engine_session_status(&error));
                }
            }
        };
        let engine_pid = session.as_ref().map_or(0, LiveEngineSession::engine_pid);
        if generation != self.reconnect_generation.current() {
            return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"));
        }
        let reply = {
            let session = session
                .as_mut()
                .ok_or_else(|| Status::unavailable(CONNECTION_ENGINE_SESSION_CODE))?;
            let mut apply_key = lookup_key.clone();
            apply_key.method = exv_vpn_wire::generated::OperationMethod::ApplyTunnel as i32;
            let build_request = |secret_payload| exv_vpn_wire::generated::ApplyTunnelRequest {
                    lookup_key: Some(apply_key.clone()),
                    plan: Some(plan.clone()),
                    request_digest: intent.request_digest.clone(),
                    secret_payload,
                    windows_connection_mode:
                        exv_vpn_wire::generated::WindowsConnectionMode::Standard as i32,
                    // T4 连接生命周期信封（EXV_UNFREEZE 2026-09-27）：测试/过渡构造用
                    // proto3 缺省值 = 信封缺省语义（engine 读作 legacy teardown / 不重连 / ui 未知）。
                    on_core_exit: exv_vpn_wire::generated::OnCoreExitPolicy::Unspecified as i32,
                    reconnect_policy: Some(wire::ReconnectPolicyEnvelope {
                        enabled: admitted_policy.enabled,
                        on_oneshot_engine_crash: false,
                        notify: false,
                    }),
                    ui_pid: 0,
                };
            let retry_payload = zeroize::Zeroizing::new(secret_payload.to_vec());
            check_auth_deadline(auth_deadline)?;
            if !new_session { self.require_background_reconnect()?; }
            match session.apply_tunnel(build_request(std::mem::take(&mut *secret_payload))).await {
                Err(error) if matches!(&error, EngineLifecycleError::Observe(crate::ObserveError::Rpc(status)) if status.code() == Code::FailedPrecondition && status.message() == "DARWIN_ENGINE_ADOPT_NEW_AUTH") => {
                    self.reconnect_generation.clear_source(generation);
                    let stop = reconnect_stop_intent();
                    self.reconnect_generation.in_generation(generation, || {
                        lock_recover(&self.stats_state).mark_finished();
                        if !admitted_recovery_material { self.reconnect_active.store(false, Ordering::Release); }
                        self.publish(pending_stop_snapshot(&StopRequest { intent: Some(stop.clone()) }, lookup_key.operation_id.clone()));
                    });
                    let mut stop_key = stop.lookup_key.expect("generated stop key");
                    stop_key.method = wire::OperationMethod::StopTunnel as i32;
                    let reply = match session.stop_tunnel(wire::StopTunnelRequest { lookup_key: Some(stop_key), request_digest: stop.request_digest }).await {
                        Ok(reply) => reply,
                        Err(error) => {
                            self.reconnect_generation.in_generation(generation, || self.publish(session_orchestration::snapshot(&lookup_key.operation_id, session_orchestration::unconfirmed(), None)));
                            return Err(engine_session_status(&error));
                        }
                    };
                    let done = session_orchestration::completion(&reply);
                    if !done.stopped || done.blocks_new_connect {
                        self.reconnect_generation.in_generation(generation, || self.publish(session_orchestration::snapshot(&lookup_key.operation_id, done, None)));
                        return Err(Status::failed_precondition("DARWIN_ENGINE_CLEANUP_INCOMPLETE：旧认证连接尚未完成停止，请重试断开"));
                    }
                    if generation != self.reconnect_generation.current() { return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED")); }
                    if let Err(status) = check_auth_deadline(auth_deadline) {
                        // 已清除旧来源，必须公布真实 Stop 回执；否则旧投影无法结束 Stopping。
                        self.reconnect_generation.in_generation(generation, || {
                            self.publish(session_orchestration::snapshot(&lookup_key.operation_id, done, None));
                        });
                        return Err(status);
                    }
                    if !new_session {
                        if let Err(status) = self.require_background_reconnect() {
                            // 许可撤销只阻止后续 Apply，原 Stop 的真实回执仍须收口。
                            self.reconnect_generation.in_generation(generation, || {
                                self.publish(session_orchestration::snapshot(&lookup_key.operation_id, done, None));
                            });
                            return Err(status);
                        }
                    }
                    match session.apply_tunnel(build_request(retry_payload.to_vec())).await {
                        Ok(reply) => reply,
                        Err(error) => {
                            self.reconnect_generation.in_generation(generation, || self.publish(session_orchestration::snapshot(&lookup_key.operation_id, done, Some(wire::VpnError { code: wire::ErrorCode::ObservationFailed as i32, stage: wire::ErrorStage::ConnectingControl as i32, retry: wire::RetryAdvice::UseNewOperation as i32, ..Default::default() }))));
                            return Err(engine_session_status(&error));
                        }
                    }
                }
                result => result.map_err(|error| engine_session_status(&error))?,
            }
        };
        let pending = match reply.result {
            Some(exv_vpn_wire::generated::apply_tunnel_reply::Result::Pending(pending))
                if pending.operation_id == lookup_key.operation_id =>
            {
                pending
            }
            _ => return Err(Status::internal(CONNECTION_ENGINE_SESSION_CODE)),
        };
        self.reconnect_generation.install(generation, &pending.operation_id, Some(admitted_identity), |identity| {
            self.proxy_tun.set_engine_proxy_merged(false);
            if let Some(round) = lock_recover(&self.reconnect_generation.recovery).as_mut()
                && round.generation == generation && !round.ended {
                round.execution = pending.operation_id.clone();
            }
            if new_session {
                *lock_recover(&self.reconnect_session_policy) = Some(admitted_policy);
                self.reconnect_attempts.store(0, Ordering::Relaxed);
                self.reconnect_active.store(false, Ordering::Release);
            }
            self.publish_with_identity(pending_connect_snapshot(request, pending.operation_id.clone()), identity);
            let mut state = lock_recover(&self.stats_state);
            *state = LiveStatsState::new();
            state.begin_operation(pending.operation_id.clone());
        }).ok_or_else(|| Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"))?;
        // MAC-OBS-13 S1：Engine 日志流消费任务（纯单向写聚合存储；流 EOF/错误仅
        // 结束消费，不触碰状态投影——「关闭实时流不影响连接状态」）。
        self.spawn_logs_ingest(engine_logs_stream);
        if let Some(observer) = session.as_ref().and_then(LiveEngineSession::proxy_observe_handle) {
            self.spawn_proxy_policy_observation(observer, generation, pending.operation_id.clone());
        }
        self.spawn_status_projection(
            connect_status_stream,
            engine_stats_stream,
            OperationProjection {
                candidate_receipt,
                generation,
                engine_mode: *lock_recover(&self.engine_mode),
                stats_state: Arc::clone(&self.stats_state),
                runtime_epoch: lookup_key.runtime_epoch.clone(),
                operation_id: pending.operation_id,
                intent: request.intent.clone(),
                session_slot: Arc::clone(&self.session),
                keepalive_ticker: Arc::clone(&self.keepalive_ticker),
                engine_pid,
            },
        );
        Ok(OperationReply { terminal: None })
    }

    /// 仅消费 Observe 快照中的代理事实，绝不将其占位 Idle 状态投影为业务状态。
    fn spawn_proxy_policy_observation(
        &self,
        mut observer: crate::engine_lifecycle_real::EngineProxyObserveHandle,
        generation: u64,
        operation_id: Vec<u8>,
    ) {
        let admission = Arc::clone(&self.reconnect_generation);
        let stats_state = Arc::clone(&self.stats_state);
        let cache = Arc::clone(&self.proxy_tun);
        let events = Arc::clone(&self.events);
        tokio::spawn(async move {
            loop {
                let live = admission.accept(generation, &operation_id, |_| {
                    lock_recover(&stats_state).is_live()
                });
                if live != Some(true) {
                    let _ = admission.accept(generation, &operation_id, |_| cache.set_engine_proxy_merged(false));
                    return;
                }
                let reply = tokio::time::timeout(Duration::from_secs(2), observer.observe())
                    .await.ok().and_then(Result::ok);
                let merged = observed_proxy_merged(reply, &operation_id);
                let accepted = admission.accept(generation, &operation_id, |_| {
                    let live = lock_recover(&stats_state).is_live();
                    cache.set_engine_proxy_merged(live && merged);
                    if let Some(snapshot) = events.current_snapshot()
                        && snapshot.operation_id == operation_id
                    {
                        let updated = snapshot_with_environment(snapshot.clone(), &cache);
                        if updated.system_proxy != snapshot.system_proxy {
                            events.publish(RuntimeEventKind::Transition, updated);
                        }
                    }
                    live
                });
                if accepted != Some(true) { return; }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    /// 消费 Engine `StreamLogs` 推送：逐条经脱敏边界写入聚合存储（S1 会话期聚合）。
    ///
    /// 纯单向（对齐 win32 ingest 的 D3 铁律）：本任务只写聚合存储，绝不驱动任何
    /// 状态/事件。流 EOF/transport 错误仅结束任务；不重连（下次 Connect 重挂）、
    /// 不影响状态投影与统计投影。
    fn spawn_logs_ingest(&self, mut engine_logs_stream: EngineUpdateStream<wire::LogEvent>) {
        let aggregator = Arc::clone(self.logs.aggregator());
        tokio::spawn(async move {
            // EOF（Ok(None)）/transport 错误（Err）都终止循环：任务自然结束。
            while let Ok(Some(event)) = engine_logs_stream.message().await {
                aggregator.append_engine(&event);
            }
        });
    }

    /// 消费 Engine 统计流：归一化样本并按门控发布（W-MVP-13；W3-1/P1 S2 对齐 win32
    /// `publish_stats` 语义）。样本经 [`EventBus::publish_stats`] 进总线（T14 拆流：
    /// 铸造同轴 tick、写缓存、广播统计 lane——**不再**向 `WatchEvents` 重发快照；
    /// 统计的 wire 交付走独立 `WatchStats` 流，挂死的统计消费者只降级该流）；铸造后
    /// 的样本写回缓存（B-09①）：`GetSnapshot` live 回放与已发布统计一致。operation
    /// 终结（失败/Stop）后丢弃流，Engine 采样 task 随接收端 drop 自行退出，不留跨
    /// operation 的统计发布者。
    fn spawn_stats_projection(
        &self,
        mut engine_stats_stream: EngineUpdateStream<exv_vpn_wire::generated::StatsEvent>,
        stats_state: Arc<std::sync::Mutex<LiveStatsState>>,
        generation: u64,
        operation_id: Vec<u8>,
    ) {
        let events = Arc::clone(&self.events);
        let admission = Arc::clone(&self.reconnect_generation);
        tokio::spawn(async move {
            let mut traffic = TrafficSample::new();
            loop {
                let Ok(Some(sample)) = engine_stats_stream.message().await else {
                    break;
                };
                let accepted = admission.accept(generation, &operation_id, |_| {
                    let normalized = lock_recover(&stats_state).observe(&sample, &mut traffic, 0);
                    if let Some(stats) = normalized {
                        let published = events.publish_stats(stats);
                        lock_recover(&stats_state).cache_minted(published);
                    }
                    lock_recover(&stats_state).is_live()
                });
                if accepted != Some(true) { break; }

            }
        });
    }

    /// Core按当前已接纳派发来源消费Engine事实；已结束Failed经真实Stop报告收口。
    /// Engine独占R1，service控制通道失联按现有冻结政策通知Core R2；旧EOF无权清新槽。
    #[allow(
        clippy::too_many_lines,
        reason = "状态投影承载阶段/终态/失联三类事实转换与对应入库节点，线性展开保持事实顺序可读"
    )]
    fn spawn_status_projection(
        &self,
        mut connect_status_stream: EngineUpdateStream<wire::ConnectStatusEvent>,
        engine_stats_stream: EngineUpdateStream<wire::StatsEvent>,
        projection: OperationProjection,
    ) {
        let OperationProjection { candidate_receipt, generation, stats_state, runtime_epoch, operation_id, intent,
            session_slot, keepalive_ticker, engine_pid, engine_mode } = projection;
        let events = Arc::clone(&self.events);
        let proxy_tun = Arc::clone(&self.proxy_tun);
        let admission = Arc::clone(&self.reconnect_generation);
        let attempts = Arc::clone(&self.reconnect_attempts);
        let active = Arc::clone(&self.reconnect_active);
        let ever_connected = Arc::clone(&self.reconnect_ever_connected);
        let policy = Arc::clone(&self.reconnect_session_policy);
        let config_for_cache = Arc::clone(&self.config);
        let reconnect_tx = Arc::clone(&self.reconnect_tx);
        let logs = Arc::clone(self.logs.aggregator());
        let service_status = Arc::clone(&self.service_status_cache);
        self.spawn_stats_projection(engine_stats_stream, Arc::clone(&stats_state), generation, operation_id.clone());
        tokio::spawn(async move {
            // 调用仅发生于本地accept临界区；这里不触发RPC。
            let publish = |mut snapshot: RuntimeSnapshot, identity: Option<wire::SessionIdentity>| {
                snapshot.atrust_auth_acquisition = lock_recover(&admission.recovery).as_ref().filter(|round| round.generation == admission.current() && !round.ended).and_then(|round| round.pending_request());
                snapshot.mode = engine_mode.into();
                snapshot.service_status = lock_recover(&service_status).clone();
                events.publish(RuntimeEventKind::Transition, snapshot_with_reconnect(
                    snapshot_with_environment(snapshot_with_session_identity(snapshot, identity), &proxy_tun), session_reconnect_status(&policy, &attempts, &active)));
            };
            while let Ok(Some(event)) = connect_status_stream.message().await {
                if event.operation_id != operation_id { continue; }
                let failure = admission.accept(generation, &operation_id, |identity| {
                    if !lock_recover(&stats_state).is_live() { return None; }
                    proxy_tun.set_owned_interface(event.own_tunnel_if_index, event.coarse_phase != wire::StatsPhase::Connecting as i32);
                    if event.coarse_phase == wire::StatsPhase::Connected as i32 {
                        lock_recover(&admission.recovery).take();
                        attempts.store(0, Ordering::Relaxed);
                        active.store(false, Ordering::Release);
                        ever_connected.store(true, Ordering::Release);
                        events.publish_composing(RuntimeEventKind::Transition, |tick| {
                            let latest = {
                                let mut state = lock_recover(&stats_state);
                                let latest = state.connected_baseline(tick);
                                state.mark_connected();
                                latest
                            };
                            let mut snapshot = connected_snapshot(&operation_id, latest, event.session_established_at_ms);
                            snapshot.mode = engine_mode.into();
                            snapshot.service_status = lock_recover(&service_status).clone();
                            snapshot_with_reconnect(snapshot_with_environment(snapshot_with_session_identity(snapshot, identity.clone()), &proxy_tun), session_reconnect_status(&policy, &attempts, &active))
                        });
                        return None;
                    }
                    if let Some(error) = event.error {
                        if exv_vpn_darwin_ipc::atrust_auth_error::is_session_invalidated(&error)
                            && let Some(receipt) = &candidate_receipt {
                            let _config = lock_recover(&config_for_cache);
                            let outcome = match receipt.forget_if_current() { Ok(true) => "forgotten", Ok(false) => "already_absent_or_replaced", Err(_) => "delete_failed" };
                            logs.append_core("info", "core", "ATRUST_SESSION_CACHE_INVALIDATED", "源会话失效，等待用户重新网页登录", &[("outcome", outcome.to_owned())]);
                        }
                        // Failed是整个Engine执行的终态；retry advice不证明R1仍在运行。
                        lock_recover(&stats_state).mark_finished();
                        proxy_tun.set_engine_proxy_merged(false);
                        let continuation = Self::queue_auth_continuation(&admission, generation, &operation_id, &error);
                        active.store(continuation, Ordering::Release);
                        logs.append_core("error", "core", "ENGINE_FAILED", "engine execution terminated", &[("code", error.code.to_string())]);
                        identity.take();
                        publish(session_orchestration::snapshot(&operation_id, session_orchestration::unconfirmed(), Some(error.clone())), None);
                        return Some((error, continuation));
                    }
                    lock_recover(&stats_state).pause_connected();
                    let engine_reconnect = event.reconnect_state.or_else(|| events.current_snapshot().and_then(|snapshot| snapshot.engine_reconnect));
                    if engine_reconnect.is_some() { active.store(true, Ordering::Release); }
                    publish(RuntimeSnapshot {
                        operation_id: operation_id.clone(), engine_reconnect,
                        state: Some(runtime_snapshot::State::Connecting(wire::ConnectingState {
                            attempt: Some(wire::Attempt { runtime_epoch: runtime_epoch.clone(), attempt_id: operation_id.clone(), intent: intent.clone(), prior_error: None }),
                            phase: event.connect_phase,
                        })), ..Default::default()
                    }, identity.clone());
                    None
                });
                let Some(failure) = failure else { return; };
                if let Some((error, continuation)) = failure {
                    if continuation {
                        if let Some(tx) = lock_recover(&reconnect_tx).as_ref() {
                            let _ = tx.send((generation, ReconnectSignal::AuthContinuation));
                        }
                        return;
                    }
                    let deadline = tokio::time::Instant::now() + STOP_TOTAL_BUDGET;
                    let completion = {
                        let Ok(mut session) = tokio::time::timeout_at(deadline, session_slot.lock()).await else { return; };
                        if tokio::time::Instant::now() >= deadline { return; }
                        // 原投影任务持许可到真实RPC回执；失败事实已经在锁外发布。
                        if admission.accept(generation, &operation_id, |_| ()).is_none() { return; }
                        let stop = reconnect_stop_intent();
                        let mut key = stop.lookup_key.expect("generated stop key");
                        key.method = wire::OperationMethod::StopTunnel as i32;
                        match session.as_mut() {
                            Some(engine) => match engine.stop_tunnel(wire::StopTunnelRequest { lookup_key: Some(key), request_digest: stop.request_digest }).await {
                                Ok(reply) => session_orchestration::completion(&reply),
                                Err(error) => {
                                    log_stop_unconfirmed(&logs, &error);
                                    session_orchestration::rpc_failure(&error)
                                },
                            },
                            _ => session_orchestration::unconfirmed(),
                        }
                    };
                    admission.accept(generation, &operation_id, |identity| { identity.take(); publish(session_orchestration::snapshot(&operation_id, completion, Some(error)), None) });
                    return;
                }
            }
            let reset = admission.accept(generation, &operation_id, |identity| {
                identity.take();
                if lock_recover(&stats_state).is_live() {
                    lock_recover(&stats_state).mark_finished();
                    proxy_tun.set_engine_proxy_merged(false);
                    active.store(false, Ordering::Release);
                    // S5b（C-10）：资格判定改在 ENGINE_LOST 时点对活状态求值
                    //（auto_reconnect && 有效凭据源——受理快照 / owner 匹配磁盘对 /
                    // aTrust 会话缓存与恢复材料，见 `valid_reconnect_credential_source`），
                    // 不再读受理时冻结的 policy.enabled——同时消灭凭据门槛对网页
                    // 登录用户的恒假与 policy 生命周期洞（仅首会话安装+Stop 清空）。
                    let r2_eligible = engine_mode == "service"
                        && ever_connected.load(Ordering::Acquire)
                        && {
                            let config = lock_recover(&config_for_cache);
                            config.auto_reconnect()
                                && valid_reconnect_credential_source(&config, lock_recover(&policy).as_ref())
                        };
                    logs.append_core("error", "core", "ENGINE_LOST", "engine status stream ended", &[
                        ("pid", engine_pid.to_string()), ("r2_eligible", r2_eligible.to_string()),
                    ]);
                    let mut completion = session_orchestration::unconfirmed();
                    completion.errors = vec![engine_lost_vpn_error()];
                    publish(session_orchestration::snapshot(&operation_id, completion, None), None);
                    if r2_eligible {
                        if let (Some(tx), Some(recovery_generation)) = (lock_recover(&reconnect_tx).as_ref(), admission.recovery_generation()) { let _ = tx.send((recovery_generation, ReconnectSignal::EngineLost)); }
                    }
                }
            });
            // 失败已发布；仅真实旧槽清理等待许可，不能挡住本地终态。
            if reset.is_some() {
                let deadline = tokio::time::Instant::now() + STOP_TOTAL_BUDGET;
                let Ok(mut session) = tokio::time::timeout_at(deadline, session_slot.lock()).await else { return; };
                if admission.accept(generation, &operation_id, |_| { session.take(); }).is_some() {
                    if let Some(ticker) = keepalive_ticker.lock().await.take() { ticker.abort(); }
                }
            }
        });
    }

    async fn stop(&self, request: &StopRequest) -> Result<OperationReply, Status> {
        self.stop_with_deadline(request, tokio::time::Instant::now() + STOP_TOTAL_BUDGET).await
    }

    async fn stop_with_deadline(&self, request: &StopRequest, deadline: tokio::time::Instant) -> Result<OperationReply, Status> {
        request.intent.as_ref().and_then(|intent| intent.lookup_key.as_ref()).filter(|key| key.operation_id.len() == 16)
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let generation = self.reconnect_generation.retire();
        self.proxy_tun.set_engine_proxy_merged(false);
        self.reconnect_active.store(false, Ordering::Release);
        self.stop_in_generation_with_deadline(request, generation, false, deadline).await
    }

    async fn stop_in_generation(&self, request: &StopRequest, generation: u64, preserve_reconnect_policy: bool) -> Result<OperationReply, Status> {
        self.stop_in_generation_with_deadline(request, generation, preserve_reconnect_policy, tokio::time::Instant::now() + STOP_TOTAL_BUDGET).await
    }

    async fn stop_in_generation_with_deadline(&self, request: &StopRequest, generation: u64, preserve_reconnect_policy: bool, deadline: tokio::time::Instant) -> Result<OperationReply, Status> {
        let intent = request.intent.as_ref().ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let lookup_key = intent.lookup_key.as_ref().filter(|key| key.operation_id.len() == 16)
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let operation_id = lookup_key.operation_id.clone();
        let mut stop_key = lookup_key.clone();
        stop_key.method = wire::OperationMethod::StopTunnel as i32;
        let stop = wire::StopTunnelRequest { lookup_key: Some(stop_key), request_digest: intent.request_digest.clone() };
        let pending = pending_stop_snapshot(request, operation_id.clone());
        let admission = Arc::clone(&self.reconnect_generation);
        let session_slot = Arc::clone(&self.session);
        let ticker_slot = Arc::clone(&self.keepalive_ticker);
        let policy = Arc::clone(&self.reconnect_session_policy);
        let stats = Arc::clone(&self.stats_state);
        let active = Arc::clone(&self.reconnect_active);
        let ever_connected = Arc::clone(&self.reconnect_ever_connected);
        let proxy = Arc::clone(&self.proxy_tun);
        let events = Arc::clone(&self.events);
        let attempts = Arc::clone(&self.reconnect_attempts);
        let service_status = Arc::clone(&self.service_status_cache);
        let mode = *lock_recover(&self.engine_mode);
        let logs = Arc::clone(self.logs.aggregator());
        let previous = events.current_snapshot();
        let previous_completion = previous.as_ref().and_then(|snapshot| snapshot.stop_completion.clone());
        let origin = previous.as_ref().and_then(|snapshot| match snapshot.state.as_ref() {
            Some(runtime_snapshot::State::FailedDirty(failed)) => failed.last_error.clone(),
            Some(runtime_snapshot::State::FailedClean(failed)) => failed.last_error.clone(),
            _ => None,
        });
        // 全部等待在本次局部任务内：传输 drop handler 不撤销受理或真实清理责任。
        tokio::spawn(async move {
            let publish: Arc<dyn Fn(wire::StopCompletion, bool) -> wire::StopCompletion + Send + Sync> = {
                let admission = Arc::clone(&admission); let events = Arc::clone(&events);
                let active = Arc::clone(&active); let stats = Arc::clone(&stats); let proxy = Arc::clone(&proxy);
                let operation_id = operation_id.clone(); let policy = Arc::clone(&policy); let attempts = Arc::clone(&attempts);
                let service_status = Arc::clone(&service_status);
                Arc::new(move |mut completion, unconfirmed| {
                    let deadline_only = unconfirmed && completion.errors.iter()
                        .all(|error| error.code == wire::ErrorCode::DeadlineExceeded as i32);
                    if !completion.stopped {
                        if unconfirmed && let Some(previous) = previous_completion.as_ref() {
                            completion.blocks_new_connect |= previous.blocks_new_connect;
                            let mut errors = previous.errors.clone();
                            for error in completion.errors {
                                if !errors.contains(&error) { errors.push(error); }
                            }
                            completion.errors = errors;
                        }
                        if let Some(error) = origin.clone() && !completion.errors.contains(&error) { completion.errors.insert(0, error); }
                    }
                    admission.in_generation(generation, || {
                        // 原RPC真实完成已发布时，join与期限的竞争不能用合成结果倒写。
                        if deadline_only && let Some(current) = events.current_snapshot()
                            && current.operation_id == operation_id && let Some(reported) = current.stop_completion {
                            completion = reported;
                            return;
                        }
                        lock_recover(&stats).mark_finished();
                        active.store(false, Ordering::Release);
                        let mut snapshot = session_orchestration::snapshot(&operation_id, completion.clone(), None);
                        snapshot.mode = mode.into(); snapshot.service_status = lock_recover(&service_status).clone();
                        events.publish(RuntimeEventKind::Transition, snapshot_with_reconnect(snapshot_with_environment(snapshot, &proxy), session_reconnect_status(&policy, &attempts, &active)));
                    });
                    completion
                })
            };
            if admission.in_generation(generation, || {
                if !preserve_reconnect_policy { *lock_recover(&policy) = None; }
                let mut pending = pending; pending.mode = mode.into(); pending.service_status = lock_recover(&service_status).clone();
                events.publish(RuntimeEventKind::Transition, snapshot_with_reconnect(
                    snapshot_with_environment(pending, &proxy), session_reconnect_status(&policy, &attempts, &active)));
            }).is_none() { return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED")); }
            let publish_late = Arc::clone(&publish);
            let admission_late = Arc::clone(&admission);
            let mut dispatch = tokio::spawn(async move {
                let mut session = match tokio::time::timeout_at(deadline, session_slot.lock_owned()).await {
                    Ok(session) => session,
                    Err(_) => return session_orchestration::reply(publish_late(session_orchestration::deadline("session_lock"), true)),
                };
                if tokio::time::Instant::now() >= deadline || generation != admission_late.current() {
                    return session_orchestration::reply(publish_late(session_orchestration::deadline("before_dispatch"), true));
                }
                let no_session = session.is_none();
                let mut actual_completion = false;
                let completion = match session.as_mut() {
                    None => { let mut done = session_orchestration::unconfirmed(); done.errors = vec![engine_lost_vpn_error()]; done },
                    Some(engine) => match engine.stop_tunnel(stop).await {
                        Ok(reply) => {
                            if matches!(reply.result, Some(wire::stop_tunnel_reply::Result::Pending(_))) { return OperationReply { terminal: None }; }
                            actual_completion = true;
                            session_orchestration::completion(&reply)
                        },
                        Err(error) => { log_stop_unconfirmed(&logs, &error); session_orchestration::rpc_failure(&error) },
                    },
                };
                // session 许可一直持有到原RPC完成，晚到结果仍经原代际接纳。
                if completion.stopped && !completion.blocks_new_connect && completion.errors.is_empty() {
                    if generation == admission_late.current() {
                        ever_connected.store(false, Ordering::Release);
                        proxy.set_owned_interface(0, true);
                    }
                }
                let completion = publish_late(completion, !actual_completion);
                drop(session);
                if no_session {
                    let admission = Arc::clone(&admission_late);
                    tokio::spawn(async move {
                        let mut ticker = ticker_slot.lock().await;
                        admission.in_generation(generation, || { if let Some(task) = ticker.take() { task.abort(); } });
                    });
                }
                if completion.stopped && !completion.blocks_new_connect && completion.errors.is_empty()
                    && generation == admission_late.current() {
                    // 独立显示刷新不延长已经取得的真实停止回执。
                    tokio::task::spawn_blocking(move || proxy.refresh());
                }
                session_orchestration::reply(completion)
            });
            match tokio::time::timeout_at(deadline, &mut dispatch).await {
                Ok(Ok(reply)) => Ok(reply),
                _ => Ok(session_orchestration::reply(publish(session_orchestration::deadline("completion"), true))),
            }
        }).await.map_err(|_| Status::internal("stop: owned task ended unexpectedly"))?
    }

    /// 在 Core UI session 收束时关闭唯一 Engine client 并读取 bootstrap 终态。
    ///
    /// W2 退出清理包（加速器，权威规范 §二 oneshot.2）：drop client 前对在跑隧道
    /// best-effort `StopTunnel`——合成全新 16B `operation_id`（`StopTunnel` method，
    /// 参照 [`Self::stop_for_service_transition`] 的 `StopRequest` 构造）+ 有界等待
    /// 300ms（对齐 win32 退出 flush），失败忽略。正确性兜底是 W1 的 EOF 收敛
    /// （drop client 后 Engine 侧唯一已认证连接 EOF → 统一收敛 teardown），本包
    /// 只加速无主隧道的回收；隧道未在跑时跳过（不发多余 Stop）。
    pub(crate) async fn close(&self) -> Result<(), Status> {
        self.reconnect_generation.retire();
        self.proxy_tun.set_engine_proxy_merged(false);
        // W2 心跳：会话收束 → 伴生 ticker 一并 abort（与 session 槽同生命周期）。
        self.abort_keepalive_ticker().await;
        let mut session = self.session.lock().await;
        if let Some(engine) = session.as_mut() {
            let mut operation_id = [0u8; 16];
            let _ = getrandom::fill(&mut operation_id);
            let mut epoch = [0u8; 16];
            let _ = getrandom::fill(&mut epoch);
            let request = wire::StopTunnelRequest {
                lookup_key: Some(wire::OperationLookupKey {
                    principal_digest: vec![0u8; 32],
                    method: wire::OperationMethod::StopTunnel as i32,
                    runtime_epoch: epoch.to_vec(),
                    operation_id: operation_id.to_vec(),
                }),
                request_digest: vec![0u8; 32],
            };
            // 无论当前是否有活跃数据泵都发送 Stop：自动重连窗口可能只剩空闲 utun 与
            // 控制专用路由，不能因 stats 已终态而漏掉它们。EOF 仍是失败兜底。
            let _ = tokio::time::timeout(EXIT_STOP_BOUND, engine.stop_tunnel(request)).await;
        }
        let session = session.take();
        if let Some(session) = session {
            session
                .close()
                .map_err(|error| engine_session_status(&error))?;
        }
        Ok(())
    }

    /// W2 心跳：abort 会话伴生的 keepalive ticker（全部 session 槽位重置点共用；
    /// 无 ticker 时幂等）。
    async fn abort_keepalive_ticker(&self) {
        if let Some(ticker) = self.keepalive_ticker.lock().await.take() {
            ticker.abort();
        }
    }

    fn check_service_attempt(&self, generation: u64, deadline: Option<AuthDeadline>) -> Result<(), Status> {
        check_auth_deadline(deadline)?;
        if generation != self.reconnect_generation.current() {
            return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"));
        }
        Ok(())
    }

    fn service_attempt_error(&self, step: &str, error: impl std::fmt::Display) -> Status {
        let message = format!("{step}: {error}");
        self.logs.aggregator().append_core("error", "core", "SERVICE_ACTION", &message,
            &[("action", "connect".to_string())]);
        Status::failed_precondition(format!("{SERVICE_NOT_RUNNING_PREFIX}{message}"))
    }

    fn service_startup_deadline(&self, causes: &str) -> Status {
        let message = format!("{causes}; service startup connection deadline expired");
        self.logs.aggregator().append_core("error", "core", "SERVICE_ACTION", &message,
            &[("action", "connect".to_string())]);
        Status::deadline_exceeded(format!("{SERVICE_NOT_RUNNING_PREFIX}{message}"))
    }

    async fn await_service_connection<T>(
        &self, generation: u64, deadline: Option<AuthDeadline>,
        connect: impl std::future::Future<Output = T>,
    ) -> Result<T, Status> {
        let auth_limit = deadline.map(|deadline| deadline.tokio_deadline())
            .transpose().map_err(auth_deadline_status)?;
        tokio::pin!(connect);
        loop {
            tokio::select! {
                biased;
                result = &mut connect => {
                    self.check_service_attempt(generation, None)?;
                    return Ok(result);
                }
                _ = async { match auth_limit {
                    Some(limit) => tokio::time::sleep_until(limit).await,
                    None => std::future::pending::<()>().await,
                }} => return Err(auth_deadline_status(AuthDeadlineError::Expired)),
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                    self.check_service_attempt(generation, None)?;
                }
            }
        }
    }

    /// 服务启动拨号复用原认证/取消等待，仅本次服务尝试附加原启动截止。
    async fn await_service_connection_with_startup_deadline<T>(
        &self, generation: u64, deadline: Option<AuthDeadline>, attempt_limit: tokio::time::Instant,
        connect: impl std::future::Future<Output = T>,
    ) -> Result<T, Status> {
        tokio::select! {
            biased;
            result = self.await_service_connection(generation, deadline, connect) => result,
            _ = tokio::time::sleep_until(attempt_limit) => {
                Err(self.service_startup_deadline("service connection action still pending"))
            }
        }
    }

    /// 同一次既有截止覆盖启动等待、恢复和最终连接；只重试尚未接通的端点。
    async fn try_service_engine_session(&self, generation: u64, deadline: Option<AuthDeadline>) -> Result<LiveEngineSession, Status> {
        self.check_service_attempt(generation, deadline)?;
        let attempt_limit = match deadline {
            Some(deadline) => deadline.tokio_deadline().map_err(auth_deadline_status)?,
            None => tokio::time::Instant::now() + crate::engine_lifecycle_real::BOOTSTRAP_DEADLINE,
        };
        let error = match self.await_service_connection_with_startup_deadline(generation, deadline, attempt_limit,
            (self.service_engine.connect)(None)).await? {
            Ok(session) => {
                self.check_service_attempt(generation, deadline)?;
                if tokio::time::Instant::now() >= attempt_limit {
                    return Err(self.service_startup_deadline("engine connection completed after the request deadline"));
                }
                self.service_lifecycle.clear_launch_requested();
                return Ok(session);
            }
            Err(error) => error,
        };
        self.check_service_attempt(generation, deadline)?;
        let absent = matches!(&error, EngineLifecycleError::ServiceConnect(source)
            if matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused));
        if !absent { return Err(self.service_attempt_error("initial connect", format!("{error}; {error:?}"))); }
        let mut causes = format!("initial connect: {error}; {error:?}");
        self.logs.aggregator().append_core("warn", "core", "SERVICE_ACTION", &error.to_string(),
            &[("action", "initial connect".to_string())]);
        if let Some(launch_limit) = self.service_lifecycle.launch_window_deadline() {
            let connect = (self.service_engine.connect)(Some(launch_limit.min(attempt_limit)));
            match self.await_service_connection_with_startup_deadline(generation, deadline, attempt_limit, connect).await? {
                Ok(session) => {
                    self.check_service_attempt(generation, deadline)?;
                    if tokio::time::Instant::now() >= attempt_limit {
                        return Err(self.service_startup_deadline(&causes));
                    }
                    self.service_lifecycle.clear_launch_requested();
                    return Ok(session);
                }
                Err(error) => {
                    causes.push_str(&format!("; launch window connect: {error}; {error:?}"));
                    if !matches!(error, EngineLifecycleError::BootstrapConnectDeadline { .. }) {
                        return Err(self.service_attempt_error("launch window connect", causes));
                    }
                    self.logs.aggregator().append_core("warn", "core", "SERVICE_ACTION", &causes,
                        &[("action", "launch window connect".to_string())]);
                }
            }
        }
        self.check_service_attempt(generation, deadline)?;
        if tokio::time::Instant::now() >= attempt_limit {
            return Err(self.service_startup_deadline(&causes));
        }
        self.repair_ladder_and_connect(generation, deadline, attempt_limit, causes).await
    }

    async fn repair_ladder_and_connect(&self, generation: u64, deadline: Option<AuthDeadline>, attempt_limit: tokio::time::Instant, mut causes: String) -> Result<LiveEngineSession, Status> {
        self.check_service_attempt(generation, deadline)?;
        if let Err(error) = self.await_service_connection_with_startup_deadline(generation, deadline, attempt_limit, (self.service_engine.ensure_engine)()).await? {
            self.check_service_attempt(generation, deadline)?;
            if !error.daemon_absent() { return Err(self.service_attempt_error("ensure engine", format!("{causes}; {error}"))); }
            causes.push_str(&format!("; ensure engine: {error}"));
            self.logs.aggregator().append_core("warn", "core", "SERVICE_ACTION", &error.to_string(),
                &[("action", "EnsureEngine connect; attempting start".to_string())]);
            self.check_service_attempt(generation, deadline)?;
            if tokio::time::Instant::now() >= attempt_limit {
                return Err(self.service_startup_deadline(&causes));
            }
            let diagnostics = self.await_service_connection_with_startup_deadline(generation, deadline, attempt_limit,
                self.service_lifecycle.start_after_daemon_absent()).await?
                .map_err(|error| self.service_attempt_error("service start", format!("{causes}; {error}")))?;
            if !diagnostics.is_empty() {
                self.logs.aggregator().append_core("warn", "core", "SERVICE_ACTION", &diagnostics,
                    &[("action", "start".to_string())]);
            }
        }
        self.check_service_attempt(generation, deadline)?;
        if tokio::time::Instant::now() >= attempt_limit {
            return Err(self.service_startup_deadline(&causes));
        }
        let connect = (self.service_engine.connect)(Some(attempt_limit));
        let result = self.await_service_connection_with_startup_deadline(generation, deadline, attempt_limit, connect).await?;
        self.check_service_attempt(generation, None)?;
        match result {
            Ok(session) => {
                self.check_service_attempt(generation, deadline)?;
                if tokio::time::Instant::now() >= attempt_limit {
                    return Err(self.service_startup_deadline("engine connection completed after the request deadline"));
                }
                self.service_lifecycle.clear_launch_requested();
                Ok(session)
            }
            Err(error) => {
                let detail = format!("{causes}; recovery action completed; final connect: {error}; {error:?}");
                if check_auth_deadline(deadline).is_err() { return Err(Status::deadline_exceeded(detail)); }
                Err(self.service_attempt_error("recovery action completed; final connect", format!("{causes}; {error}; {error:?}")))
            }
        }
    }

    /// W3：连接传输判定（唯一判定点在 core，前端零逻辑分叉；v3 §一 四态表）。
    /// engine 常驻端点 healthy → Service；服务已装（service agent binary 在场）但
    /// 端点不可用 → Service（修复阶梯在 [`Self::service_engine_session_or_reject`]
    /// 内）；未装或 facts 不可区分 → Oneshot 兜底（对齐 win32「SCM 查询失败→
    /// Oneshot」语义）。
    async fn resolve_transport(&self) -> ResolvedTransport {
        let engine_facts = (self.service_engine.probe)().await;
        if crate::service_status::engine_endpoint_healthy(&engine_facts) {
            return ResolvedTransport::Service;
        }
        let service_agent_facts = (self.service_lifecycle.probe())().await;
        if service_agent_facts.binary_present {
            return ResolvedTransport::Service;
        }
        ResolvedTransport::Oneshot
    }

    /// 保留现有 UI 路由前缀，失败不追加状态探测或宣称服务已停止。
    async fn service_engine_session_or_reject(&self, generation: u64, deadline: Option<AuthDeadline>) -> Result<LiveEngineSession, Status> {
        self.try_service_engine_session(generation, deadline).await
    }

    /// P4 v2：服务变更（install/uninstall）前的业务停机——win32
    /// `stop_engine_for_transition` 的 darwin 尽力对应：有活跃 Engine 会话时以
    /// 全新 operation id 复用显式 Stop 路径停到 Idle（Stop 是独立 operation，
    /// Engine 不要求与 Connect 的 id 连续——UI 显式断开同样每次新 id）。失败只
    /// 入库不阻断服务变更（已在跑的 root Engine 无法被无特权 Core 强杀是已知
    /// 诚实限制，见 `service_lifecycle` 模块头）。
    async fn stop_for_service_transition(&self) {
        if self.session.lock().await.is_none() {
            return;
        }
        let mut operation_id = [0u8; 16];
        let _ = getrandom::fill(&mut operation_id);
        let mut epoch = [0u8; 16];
        let _ = getrandom::fill(&mut epoch);
        let request = StopRequest {
            intent: Some(wire::StopIntent {
                lookup_key: Some(wire::OperationLookupKey {
                    principal_digest: vec![0u8; 32],
                    method: wire::OperationMethod::StopTunnel as i32,
                    runtime_epoch: epoch.to_vec(),
                    operation_id: operation_id.to_vec(),
                }),
                request_digest: vec![0u8; 32],
            }),
        };
        if let Err(status) = self.stop(&request).await {
            self.logs.aggregator().append_core(
                "error",
                "core",
                "SERVICE_TRANSITION_STOP_FAILED",
                "pre-service-transition stop failed; proceeding with the service action",
                &[("status", status.message().to_string())],
            );
        }
        // 服务替换前释放旧控制通道；停止失败也不保留不可复用的会话和 ticker。
        if let Err(status) = self.close().await {
            self.logs.aggregator().append_core(
                "error", "core", "SERVICE_TRANSITION_CLOSE_FAILED",
                "服务变更前释放旧 Engine 会话失败",
                &[("status", status.message().to_owned())],
            );
        }
    }

    fn prepare_connect_payload(
        &self,
        ui_credentials: Option<UiSecretPayload>,
        persist_credentials: bool,
        recovery_policy: Option<SessionReconnectPolicy>,
        new_session: bool,
    ) -> Result<(Vec<u8>, exv_vpn_wire::generated::TunnelPlan, SessionReconnectPolicy, Option<atrust_session_cache::CandidateReceipt>, wire::SessionIdentity, Option<cached_headless::CachedProbeInput>), Status> {
        self.prepare_connect_payload_inner(ui_credentials, persist_credentials, recovery_policy, new_session, false)
    }

    fn prepare_connect_payload_inner(
        &self, ui_credentials: Option<UiSecretPayload>, persist_credentials: bool,
        recovery_policy: Option<SessionReconnectPolicy>, new_session: bool, admitted_recovery_material: bool,
    ) -> Result<(Vec<u8>, exv_vpn_wire::generated::TunnelPlan, SessionReconnectPolicy, Option<atrust_session_cache::CandidateReceipt>, wire::SessionIdentity, Option<cached_headless::CachedProbeInput>), Status> {
        if recovery_policy.is_some() && (persist_credentials || (ui_credentials.is_some() && !admitted_recovery_material)) {
            return Err(Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE));
        }
        let mut saved_config = lock_recover(&self.config);
        let mut temporary_config = recovery_policy.as_ref().map(|policy| {
            saved_config.with_temporary_protocol(policy.protocol)
                .with_temporary_atrust_server(&policy.atrust_server)
        });
        let config = temporary_config.as_mut().unwrap_or(&mut *saved_config);
        if config.protocol() == Protocol::Atrust && config.atrust_server().is_empty() {
            return Err(Status::failed_precondition("DARWIN_CORE_ATRUST_GATEWAY_INVALID: aTrust服务器未配置或无效，请在设置中填写网关"));
        }
        let mut admitted_policy = recovery_policy.clone().unwrap_or_else(|| SessionReconnectPolicy::from_config(config));
        // Cisco 计划：mtu 随配置（既有语义；路由经信封 V1 携带）。aTrust 计划
        // （M3 定稿）：mtu 同源 + 用户校园路由经 `ipv4_routes` 携带（aTrust V2
        // 信封无路由字段——计划是 Cisco 信封 routes 的 aTrust 等价通道）。
        let cisco_plan = |config: &DarwinUiConfig| exv_vpn_wire::generated::TunnelPlan {
            mtu: config.mtu(),
            ..Default::default()
        };
        let atrust_plan = |config: &DarwinUiConfig| exv_vpn_wire::generated::TunnelPlan {
            mtu: config.mtu(),
            ipv4_routes: normalized_connect_routes(config.routes())
                .iter()
                .filter_map(|text| {
                    let (address, prefix) = text.split_once('/')?;
                    let octets: [u8; 4] = address.parse::<std::net::Ipv4Addr>().ok()?.octets();
                    let prefix_len: u32 = prefix.parse().ok()?;
                    Some(exv_vpn_wire::generated::Ipv4Route {
                        network: octets.to_vec(),
                        prefix_len,
                    })
                })
                .collect(),
            ..Default::default()
        };
        match (config.protocol(), ui_credentials) {
            // Cisco：UI 优先 / 磁盘回退（W1-C 既有路径，字节不变）。
            (Protocol::Cisco, Some(UiSecretPayload::Cisco(mut credentials))) => {
                if persist_credentials {
                    config.apply_ui_credentials_and_save(&credentials.username, Some(credentials.password.as_str()))
                        .map_err(ui_persist_status)?;
                    // S5b：persist 受理=本 UI 账密对成为已持久化凭据对——冻结受理
                    // 快照（重连组装优先使用，免疫连接中凭据/用户名修改）。
                    admitted_policy.credential_snapshot = Some(AdmissionCredentialSnapshot {
                        username: credentials.username.clone(),
                        password: zeroize::Zeroizing::new(credentials.password.clone()),
                    });
                } else if !(config.has_saved_credential_pair()
                    && config.username() == credentials.username)
                {
                    // S5b（一次性资格关闭）：persist=false 的一次性连接，磁盘上没有
                    // 与受理账户匹配的 owner 持久化凭据对——受理期即关闭本会话重连
                    // 资格（堵静默账号切换：否则 ENGINE_LOST 重连会改用磁盘上另一
                    // 账户的凭据而显示身份冻结为一次性用户名）。已保存密文存在但归
                    // 属账户不一致时同时置暂停 reason（前端 toast 通道）。
                    admitted_policy.enabled = false;
                    if config.has_stored_password() {
                        admitted_policy.pause_reason = Some(RECONNECT_PAUSE_OWNER_MISMATCH);
                    }
                }
                let temporary = config.with_temporary_username(&credentials.username).map_err(ui_persist_status)?;
                // 密码 ownership 移入 envelope（零化类型）；包内残留副本由 Drop 清零。
                let password = std::mem::take(&mut credentials.password);
                let envelope = build_connect_envelope(&temporary, UiConnectMaterials::Cisco { password })
                    .map_err(ui_envelope_status)?;
                let (payload, identity) = encode_with_session_identity(envelope);
                Ok((payload, cisco_plan(&temporary), admitted_policy, None, identity, None))
            }
            (Protocol::Cisco, None) => {
                // S5b：重连组装优先受理快照（免疫连接中凭据/用户名修改——快照账户
                // 与密码即受理时已持久化的值）。
                if let Some(snapshot) = recovery_policy
                    .as_ref()
                    .and_then(|policy| policy.credential_snapshot.as_ref())
                {
                    let temporary = config.with_temporary_username(&snapshot.username).map_err(ui_persist_status)?;
                    let envelope = build_connect_envelope(
                        &temporary,
                        UiConnectMaterials::Cisco {
                            password: snapshot.password.as_str().to_owned(),
                        },
                    )
                    .map_err(ui_envelope_status)?;
                    let (payload, identity) = encode_with_session_identity(envelope);
                    return Ok((payload, cisco_plan(&temporary), admitted_policy, None, identity, None));
                }
                // 磁盘回退：单次读取已保存密码（与快照捕获同源）；envelope 组装与既有
                // `build_saved_connect_envelope` 字节等价（load → build_connect_envelope）。
                let password = zeroize::Zeroizing::new(config.load_saved_password()
                    .map_err(|error| saved_envelope_status(&SavedConnectEnvelopeError::Config(error)))?);
                let envelope = match build_connect_envelope(
                    config,
                    UiConnectMaterials::Cisco { password: password.as_str().to_owned() },
                ) {
                    Ok(crate::credential::DarwinConnectEnvelope::Cisco(envelope)) => envelope,
                    Ok(crate::credential::DarwinConnectEnvelope::Atrust(_)) => {
                        return Err(saved_envelope_status(&SavedConnectEnvelopeError::Envelope(
                            exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError::InvalidProtocol,
                        )));
                    }
                    Err(error) => {
                        return Err(saved_envelope_status(&SavedConnectEnvelopeError::Envelope(error)));
                    }
                };
                // S5b：新会话受理时冻结磁盘账密对快照（一次性/恢复派发不建快照——
                // 前者无持久化对，后者的 admitted_policy 已携带原快照）。
                if new_session {
                    admitted_policy.credential_snapshot = Some(AdmissionCredentialSnapshot {
                        username: config.username().to_owned(),
                        password,
                    });
                }
                let (payload, identity) = encode_with_session_identity(crate::credential::DarwinConnectEnvelope::Cisco(envelope));
                Ok((payload, cisco_plan(&config), admitted_policy, None, identity, None))
            }
            // aTrust（M1 受理面；M5 缓存接线）：UI v2 载荷 = 新认证（fresh 位由采集
            // 路径置位）；**先写会话缓存再组装/发连接**（M1 冻结时序——失败不回滚，
            // 候选复核兜底；写入失败只记非秘密日志，不阻断连接受理）。计划=mtu+用户
            // 校园路由（Cisco 经信封 routes 携带同一配置；aTrust 信封无路由字段——
            // wire 计划 ipv4_routes 是等价通道，engine 侧据此安装校园路由，win32
            // "校园范围由 EXV 用户配置提供" 同源）。
            (Protocol::Atrust, Some(UiSecretPayload::Atrust(atrust))) => {
                let gateway = exv_vpn_darwin_ipc::browser_session::GatewayOrigin::parse(&format!("https://{}", config.atrust_server())).map_err(|_| Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE))?;
                if exv_vpn_darwin_ipc::browser_session::GatewayOrigin::parse(atrust.browser_session.gateway()).map_err(|_| Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE))?.endpoint() != gateway.endpoint() {
                    return Err(Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE));
                }
                if let Err(error) =
                    atrust_session_cache::save_with_username(config.config_dir(), &atrust.browser_session, &atrust.authenticated_username)
                {
                    self.logs.aggregator().append_core(
                        "warn",
                        "core",
                        "ATRUST_SESSION_CACHE_SAVE_FAILED",
                        "aTrust session cache write failed; connecting without a fresh cache entry",
                        &[("error", error.message().to_string())],
                    );
                }
                let receipt = atrust_session_cache::receipt(config.config_dir(), &atrust.browser_session);
                let envelope = build_connect_envelope(
                    &config,
                    UiConnectMaterials::Atrust {
                        fresh_authentication: atrust.fresh_authentication,
                        browser_session: atrust.browser_session,
                    },
                )
                .map_err(ui_envelope_status)?;
                let (payload, mut identity) = encode_with_session_identity(envelope);
                identity.username = atrust.authenticated_username;
                Ok((payload, atrust_plan(&config), admitted_policy, receipt, identity, None))
            }
            // aTrust 无 UI 载荷：缓存继续 fresh=false，由 Engine 最终复核/采纳。
            // 仅用户新建 headless 连接在 Apply 前先探测缓存事实；其他模式及后台
            // 恢复沿既有流程。无候选同步请求凭据，由同次用户请求继续认证。
            (Protocol::Atrust, None) => {
                // 用户手动认证由本次浏览器交付候选；后台恢复保留已冻结的会话策略。
                if new_session && config.atrust_auth_mode() != crate::AtrustAuthMode::Headless {
                    return Err(connect_materials_missing("atrust-material-missing"));
                }
                let endpoint = format!("https://{}", config.atrust_server());
                match atrust_session_cache::load_candidate(config.config_dir(), &endpoint) {
                    Some(candidate) => {
                        let authenticated_username = atrust_session_cache::username_for(config.config_dir(), &candidate).unwrap_or_default();
                        let receipt = atrust_session_cache::receipt(config.config_dir(), &candidate);
                        // 与候选、回执同一配置锁捕获；恢复派发不执行用户认证准备。
                        let cached_probe = (new_session && config.atrust_auto_login()
                            && config.atrust_auth_mode() == crate::AtrustAuthMode::Headless)
                            .then(|| cached_headless::CachedProbeInput::capture(config, &candidate));
                        let envelope = build_connect_envelope(
                            &config,
                            UiConnectMaterials::Atrust {
                                fresh_authentication: false,
                                browser_session: candidate,
                            },
                        )
                        .map_err(ui_envelope_status)?;
                        let (payload, mut identity) = encode_with_session_identity(envelope);
                        identity.username = authenticated_username;
                        Ok((payload, atrust_plan(&config), admitted_policy, receipt, identity, cached_probe))
                    }
                    None => Err(connect_materials_missing("atrust-material-missing")),
                }
            }
            // 协议×材料不匹配：跨协议误投是契约错误，绝不静默改走另一协议。
            (Protocol::Cisco, Some(UiSecretPayload::Atrust(_)))
            | (Protocol::Atrust, Some(UiSecretPayload::Cisco(_))) => {
                Err(Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE))
            }
        }
    }

    /// 经共享总线发布过渡事件（tick 由总线铸造；快照出向边界附加最近 proxy TUN
    /// 检测、自动重连状态与 P4 v2 服务状态缓存伴生值——后两者均为内存直读，零
    /// 磁盘零探测）。事件 `operation_id` 镜像快照 `operation_id`（win32 同款）。
    fn publish(&self, snapshot: RuntimeSnapshot) {
        self.publish_with_identity(snapshot, None);
    }

    fn publish_with_identity(&self, mut snapshot: RuntimeSnapshot, identity: Option<wire::SessionIdentity>) {
        snapshot = snapshot_with_session_identity(snapshot, identity);
        snapshot.atrust_auth_acquisition = self.auth_acquisition();
        if let Some(status) = lock_recover(&self.service_status_cache).clone() {
            snapshot.service_status = Some(status);
        }
        // W2.5：附加当前会话形态（服务形态="service"；会话/fixture=空串——W3 补
        // "oneshot"）。会话建立点写入，连接生命周期内的全部发布快照一致携带。
        snapshot.mode = String::from(*lock_recover(&self.engine_mode));
        self.events.publish(
            RuntimeEventKind::Transition,
            snapshot_with_reconnect(
                snapshot_with_environment(snapshot, &self.proxy_tun),
                self.reconnect_status(),
            ),
        );
    }

    /// 发布边界与 GetSnapshot 共用当前会话事实；保存配置不参与运行时投影。
    fn reconnect_status(&self) -> Option<wire::ReconnectStatus> {
        session_reconnect_status(
            &self.reconnect_session_policy,
            &self.reconnect_attempts,
            &self.reconnect_active,
        )
    }

    /// S5b（C-10）：给定（冻结或合成的）会话策略，按当前活状态求值重连资格：
    /// `auto_reconnect && valid_reconnect_credential_source`。ENGINE_LOST 发射点与
    /// worker 消费点共用本判定。
    fn reconnect_eligible_now(&self, policy: &SessionReconnectPolicy) -> bool {
        let config = lock_recover(&self.config);
        config.auto_reconnect() && valid_reconnect_credential_source(&config, Some(policy))
    }

    fn background_reconnect_allowed(&self) -> bool {
        let config = lock_recover(&self.config);
        // S5b（C-10）：后台重连资格=auto_reconnect && 有效凭据源（活求值）。有效源
        // 含受理快照臂——keep 分支仅改用户名后磁盘对因 owner 失配失效时，受理快照
        // 继续承载受理账户的重连资格（免疫连接中凭据/用户名修改）；无快照会话
        // （一次性/已清除）仍按 owner 匹配门暂停。
        if !config.auto_reconnect() {
            return false;
        }
        let policy = lock_recover(&self.reconnect_session_policy);
        valid_reconnect_credential_source(&config, policy.as_ref())
    }

    fn require_background_reconnect(&self) -> Result<(), Status> {
        if self.background_reconnect_allowed() { Ok(()) }
        else { Err(Status::cancelled("DARWIN_CORE_AUTO_RECONNECT_DISABLED")) }
    }

    fn refresh_background_permission(&self) {
        let allowed = self.background_reconnect_allowed();
        // S5b：暂停原因分类（toast reason lane）+ 清除密码联动丢弃受理快照。注意
        // darwin 配置层在凭据对失效时会把 auto_reconnect 一并强制关闭（含 keep 分支
        // 仅改用户名的场景）——"磁盘对完整时关闭"才视为用户主动关闭（无 toast）。
        let (pause_reason, drop_snapshot) = {
            let config = lock_recover(&self.config);
            if allowed || config.has_saved_credential_pair() {
                (None, false)
            } else if config.has_stored_password() {
                (Some(RECONNECT_PAUSE_OWNER_MISMATCH), false)
            } else {
                (Some(RECONNECT_PAUSE_CREDENTIALS_CLEARED), true)
            }
        };
        let paused = if let Some(policy) = lock_recover(&self.reconnect_session_policy).as_mut() {
            let was_enabled = policy.enabled;
            policy.enabled = allowed;
            policy.pause_reason = pause_reason;
            if drop_snapshot {
                policy.credential_snapshot = None;
            }
            !allowed && was_enabled
        } else {
            false
        };
        if !allowed {
            if let Some(round) = lock_recover(&self.reconnect_generation.recovery).as_mut() {
                round.take_pending();
            }
            self.publish_auth_acquisition();
        }
        if paused {
            // 暂停是可观察状态：重发布当前快照让 reason lane 立即到达前端（不等
            // 下一次轮询/事件），toast 按 reason id 路由。
            if let Some(snapshot) = self.events.current_snapshot() {
                let identity = snapshot.session_identity.clone();
                self.publish_with_identity(snapshot, identity);
            }
        }
    }

    /// 唤醒恢复复用Core的已保存凭据接线；生产R1仍由Engine独占执行。
    fn signal_reconnect(&self) {
        if let Some(tx) = lock_recover(&self.reconnect_tx).as_ref() {
            let _ = tx.send((
                self.reconnect_generation.current(),
                ReconnectSignal::WakeResume,
            ));
        }
    }

    /// W3-2/P3：消费一条掉线信号，驱动一次自动重连（镜像 win32 `drive_reconnect`）。
    ///
    /// 单 worker 串行消费 + 在途标记（`reconnect_active`）双重防重入：上一次重连
    /// 已派发、终态未到时新信号直接跳过（不重复派发）。重连
    /// 前判定：`auto_reconnect` 未开启 → 跳过（`disabled`）；`max_attempts != 0` 且
    /// `attempts >= max` → 耗尽停止（`exhausted`——**停在 `FailedClean` 不回 `Idle`**，
    /// 对齐 win32：耗尽是失败事实，恢复由用户手动驱动）。每次实际派发递增
    /// `reconnect_attempts`（per-connection，Connected 成功后由状态投影清零）。
    ///
    /// 凭据重组**零新路径**：复用 [`Self::connect`]（其 `connect_payload_and_plan`
    /// 恒走 saved envelope / UI 优先链，从磁盘重新组装，不持有任何先前 UI 一次性
    /// 密码）；intent 用 [`reconnect_intent`]（全新 `operation_id`——旧投影的迟到终态
    /// 靠 `operation_id` 过滤丢弃）。
    ///
    /// 退避（`auto_reconnect_backoff` 开启时）：占用在途标记**之后**、派发**之前**
    /// sleep [`reconnect_backoff_delay`]（base·2^(n-1) 封顶 cap）——睡眠期间到达的
    /// 新掉线信号按防重入跳过；worker abort（测试）或 Core 进程退出（darwin Core
    /// 随 UI session 结束退出，进程 teardown 取消一切睡眠——与 win32 常驻服务需
    /// 显式 abort 的差异）会取消进行中的 sleep。退避关闭（默认）→ 零 sleep，立即
    /// 重连（现状行为）。
    ///
    /// 派发成功后保持 in-flight（终态事件由状态投影清 `reconnect_active`）；派发前
    /// 同步失败（凭据缺失/Engine 会话失败）→ 释放在途标记、发布 `FailedClean` 终态
    /// （UI 不悬挂在等待终态），`attempts` 不回退（预算已消耗在真实失败上）。
    #[allow(
        clippy::too_many_lines,
        reason = "判定链（防重入/开关/预算）+ 退避 + 派发/失败分支线性展开保持事实顺序可读，镜像 win32 drive_reconnect"
    )]
    async fn drive_reconnect(&self, kind: ReconnectSignal) {
        if kind == ReconnectSignal::AuthContinuation {
            self.continue_auth_recovery().await;
            return;
        }
        // 防重入首检：上一次重连尝试仍在途（已派发、终态未到）时新信号直接跳过。
        // R2 的预算内再入走 [`Self::drive_reconnect_reentry`]（在途标记保持占用，
        // 不得被自己的首检拦截——worker 单任务串行消费，无并发面）。
        if self.reconnect_active.load(Ordering::Acquire) {
            return;
        }
        self.drive_reconnect_reentry(kind).await;
    }

    /// [`Self::drive_reconnect`] 的再入体：无防重入首检（调用方已持有在途语义）。
    async fn drive_reconnect_reentry(&self, kind: ReconnectSignal) {
        let generation = self.reconnect_generation.current();
        // S5b（C-10）：资格在信号消费时点对活状态求值（auto_reconnect && 有效凭据
        // 源），与 ENGINE_LOST 发射点同一公式。policy 缺席（生命周期洞：仅首会话
        // 安装+Stop 清空）时——ENGINE_LOST 按当前活配置合成策略继续（预算/退避取
        // 当前配置值）；数据面信号维持既有"无冻结策略即结束"语义。
        let frozen_policy = match self
            .reconnect_generation
            .in_generation(generation, || lock_recover(&self.reconnect_session_policy).clone())
            .flatten()
        {
            Some(policy) => policy,
            None if kind == ReconnectSignal::EngineLost => {
                let synthesized = {
                    let config = lock_recover(&self.config);
                    SessionReconnectPolicy::from_config(&config)
                };
                if !self.reconnect_eligible_now(&synthesized) {
                    return;
                }
                synthesized
            }
            None => return,
        };
        let SessionReconnectPolicy { max_attempts: max, backoff, .. } = frozen_policy;
        if !self.reconnect_eligible_now(&frozen_policy) {
            self.logs
                .aggregator()
                .append_core(
                    "info",
                    "kernel",
                    "kernel.reconnect.disabled",
                    "retryable data-plane drop observed but auto_reconnect is disabled",
                    &[],
                );
            self.finish_reconnect_and_cleanup(generation, reconnect_disabled_vpn_error())
                .await;
            return;
        }
        // W3：oneshot 拦截（规范 §二.3「engine 失联→手动重连」；总纲 3）——只在
        // 会话槽已空且传输仍为 Oneshot 时，保留用户手动提权出口。
        // 槽内通道先尝试原 attach/RPC；occupied typed 拒绝的一次原重建例外
        // 保持不变，不能由槽位在场承诺后续绝无 launch。
        let last_mode_oneshot = *lock_recover(&self.engine_mode) == "oneshot";
        let needs_launch = self
            .session
            .lock()
            .await
            .is_none();
        if generation != self.reconnect_generation.current() { return; }
        if last_mode_oneshot
            && needs_launch
            && matches!(self.resolve_transport().await, ResolvedTransport::Oneshot)
        {
            self.logs
                .aggregator()
                .append_core(
                    "warn",
                    "kernel",
                    "kernel.reconnect.oneshot_manual_required",
                    "auto reconnect suppressed in oneshot mode: DARWIN_CORE_ONESHOT_MANUAL_RECONNECT_REQUIRED (re-elevation must be user-initiated)",
                    &[],
                );
            self.finish_reconnect_and_cleanup(generation, oneshot_manual_reconnect_vpn_error())
                .await;
            return;
        }
        let attempts = self.reconnect_attempts.load(Ordering::Relaxed);
        // T5：R2（EngineLost）的派发失败按预算退避重试——循环头的预算门拦截耗尽；
        // 数据面掉线（DataPlaneDrop）沿用一期"派发前失败即终态"。
        let r2_retry = kind == ReconnectSignal::EngineLost;
        if max != 0 && attempts >= max {
            self.logs
                .aggregator()
                .append_core(
                    "warn",
                    "kernel",
                    "kernel.reconnect.exhausted",
                    &format!(
                        "auto reconnect stopped: attempts={attempts} exhausted (max={max})"
                    ),
                    &[],
                );
            // T5 自查修复：预算耗尽必须释放在途标记——R2 再入循环占用标记跨轮，
            // 耗尽出口若不清位，后续掉线信号会被防重入首检永久跳过（公开入口
            // 到达此处的常规路径 active 本就为 false，清位幂等无害）。
            self.reconnect_active.store(false, Ordering::Release);
            self.finish_reconnect_and_cleanup(generation, reconnect_exhausted_vpn_error())
                .await;
            return;
        }

        // 重连执行：占用在途标记 + 计数 + 复用 connect（凭据从磁盘重新组装）。
        // 用户 Stop 与退避占位共用既有 generation 门，旧 worker 不能重新占位。
        if self.reconnect_generation.in_generation(generation, || {
            self.reconnect_active.store(true, Ordering::Release);
            self.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
            if r2_retry && backoff {
                // 保留失联/停止未确认事实，仅及时发布真实在途状态供产品取消。
                if let Some(snapshot) = self.events.current_snapshot() {
                    let identity = snapshot.session_identity.clone();
                    self.publish_with_identity(snapshot, identity);
                }
            }
        }).is_none() { return; }
        let attempt = attempts + 1;
        // 自动重连只允许读取已持久化的凭据；绝不复用先前 UI 的一次性密码（connect
        // 侧 ui_credentials=None）。退避开启时在派发前 sleep（占用在途标记之后——
        // 睡眠期间新掉线信号按防重入跳过；关闭（默认）→ 零 sleep，立即重连）。
        let dispatch_delay = if backoff {
            reconnect_backoff_delay(self.reconnect_backoff_base, self.reconnect_backoff_cap, attempt)
        } else {
            Duration::ZERO
        };
        if backoff {
            tokio::time::sleep(dispatch_delay).await;
        }
        // 退避期间用户可能已断开或重新连接，旧工作不能覆盖新意图。
        if generation != self.reconnect_generation.current() { return; }
        let intent = reconnect_intent();
        let operation_id = intent
            .lookup_key
            .as_ref()
            .map_or_else(Vec::new, |key| key.operation_id.clone());
        let request = ConnectRequest {
            auth_binding: None,
            intent: Some(intent),
            secret_payload: Vec::new(),
        };
        self.begin_auth_round(generation, &request, frozen_policy.clone(), kind);
        let mut result = self.connect_in_generation(&request, None, false, generation, false, Some(frozen_policy)).await;
        if result.as_ref().err().is_some_and(reconnect_materials_missing)
            && self.has_auth_round(generation) {
            result = self.acquire_and_apply_recovery(generation).await;
        }
        if generation != self.reconnect_generation.current() { return; }
        match result {
            Ok(_reply) => {
                // 已派发：保持 in-flight（终态事件由状态投影清 reconnect_active）。
                self.logs
                    .aggregator()
                    .append_core(
                        "info",
                        "kernel",
                        "kernel.reconnect.dispatched",
                        &format!(
                            "auto reconnect attempt {attempt} dispatched (credentials re-assembled from saved envelope)"
                        ),
                        &[("attempt", attempt.to_string()), ("max", max.to_string()),
                            ("trigger", if r2_retry { "engine_lost" } else { "data_plane_drop" }.to_owned()),
                            ("delay_ms", dispatch_delay.as_millis().to_string())],
                    );
            }
            Err(status) => {
                if self.auth_round_ended(generation) { return; }
                // 已精确观察到冻结协议缺材料时等待用户新请求，不能按引擎暂未恢复
                // 无限重试。原清理收口保留真实 Stopped/Dirty 与清理错误优先级。
                if reconnect_materials_missing(&status) {
                    if self.reconnect_generation.in_generation(generation, || {
                        self.reconnect_active.store(false, Ordering::Release);
                    }).is_none() { return; }
                    self.logs.aggregator().append_core("warn", "kernel", "kernel.reconnect.materials_missing",
                        "冻结协议的认证材料缺失，结束自动恢复并等待用户显式重试", &[
                            ("code", CONNECTION_CREDENTIALS_MISSING_CODE.to_owned()),
                            ("attempt", attempt.to_string()), ("generation", generation.to_string()),
                        ]);
                    self.finish_reconnect_and_cleanup(generation, reconnect_dispatch_vpn_error(&status)).await;
                    return;
                }
                // T5 R2：引擎失联触发的派发失败多为引擎尚未被 launchd 拉回（会话
                // 建立失败）——预算未耗尽时退避后重试（循环头的预算门会拦截耗尽），
                // 不发终态。数据面掉线触发沿用一期语义：派发前失败即终态。
                if r2_retry
                    && generation == self.reconnect_generation.current()
                {
                    // 无退避配置时 R2 也须给 launchd 拉回引擎留时间。
                    let delay = if backoff {
                        reconnect_backoff_delay(
                            self.reconnect_backoff_base,
                            self.reconnect_backoff_cap,
                            attempt,
                        )
                    } else {
                        std::time::Duration::from_secs(2)
                    };
                    self.logs
                        .aggregator()
                        .append_core(
                            "info",
                            "kernel",
                            "kernel.reconnect.r2_retry",
                            &format!(
                                "engine-lost reconnect attempt {attempt} failed before dispatch; budget permitting, sleeping before the next attempt: code={:?} msg={}",
                                status.code(),
                                status.message()
                            ),
                            &[("retry_delay_ms", delay.as_millis().to_string())],
                        );
                    tokio::time::sleep(delay).await;
                    if generation != self.reconnect_generation.current() {
                        return;
                    }
                    // 再入驱动下一轮（预算门在轮首拦截耗尽；在途标记保持占用，
                    // 经 reentry 入口绕过防重入首检——自己不能拦自己）。async 再入
                    // 需 boxing——每轮一个 box，退避 30s 封顶下增长率有界。
                    return Box::pin(self.drive_reconnect_reentry(kind)).await;
                }
                // 同步失败（凭据缺失/会话建立失败等）：释放在途标记并发布 FailedClean
                // 终态——错误按「派发前失败」诚实归类为连接控制阶段 + 用新 operation
                // 重试（对齐 engine 取消语义；不携带平台细节）。
                self.reconnect_active.store(false, Ordering::Release);
                self.publish(failed_clean_snapshot(
                    &operation_id,
                    reconnect_dispatch_vpn_error(&status),
                ));
                self.logs
                    .aggregator()
                    .append_core(
                        "warn",
                        "kernel",
                        "kernel.reconnect.failed",
                        &format!(
                            "auto reconnect attempt {attempt} failed before dispatch: code={:?} msg={}",
                            status.code(),
                            status.message()
                        ),
                        &[],
                    );
            }
        }
    }

    /// 自动重连不再安排下一次尝试时，把 Engine 暂存的空闲 utun/控制路由走现有
    /// StopTunnel 完整回收。只有收到 Stopped 后才发布 FailedClean。
    async fn finish_reconnect_and_cleanup(&self, generation: u64, terminal_error: wire::VpnError) {
        if generation != self.reconnect_generation.current() {
            return;
        }
        let intent = reconnect_stop_intent();
        // StopTunnel 的幂等键属于内部清理；对 UI 的失败仍属于刚结束的连接操作。
        let operation_id = self.events.current_snapshot()
            .map(|snapshot| snapshot.operation_id).unwrap_or_default();
        let request = StopRequest { intent: Some(intent) };
        match self.stop_in_generation(&request, generation, false).await {
            Ok(reply) if generation == self.reconnect_generation.current() => {
                if let Some(completion) = reply.terminal.and_then(|terminal| terminal.stop_completion) {
                    self.publish(session_orchestration::snapshot(&operation_id, completion, Some(terminal_error)));
                } else {
                    self.publish(failed_clean_snapshot(&operation_id, terminal_error));
                }
            }
            Ok(_) => {}
            Err(status) => {
                self.logs.aggregator().append_core(
                    "error",
                    "kernel",
                    "RECONNECT_CLEANUP_FAILED",
                    &format!("auto reconnect terminal cleanup failed: {}", status.message()),
                    &[],
                );
            }
        }
    }

    /// 拉起自动重连 worker（W3-2/P3）：换入绑定本 worker 接收端的发送端，此后状态
    /// 投影的掉线信号经该通道到达本 worker；单 worker 串行消费（天然防并发重连）。
    /// worker 持投影 `Arc` 克隆（轻量共享句柄，内部全部 `Arc`）。返回 `JoinHandle`
    /// 供测试 abort（取消退避 sleep）；darwin Core 随 UI session 结束整体退出，生产
    /// 路径 detach（进程 teardown 取消一切，无需 win32 常驻服务的显式 abort 纪律）。
    fn spawn_reconnect_worker(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, ReconnectSignal)>();
        *lock_recover(&self.reconnect_tx) = Some(tx);
        tokio::spawn(async move {
            while let Some((generation, kind)) = rx.recv().await {
                if generation == self.reconnect_generation.current() {
                    self.drive_reconnect(kind).await;
                }
            }
        })
    }

    /// `GetSnapshot` 的 live 分支（B-09①）：当前 operation 已 Connected 且未终结时，
    /// 返回与 `WatchEvents` 推送同构的 Connected 快照（含最新门控统计）；未连接 /
    /// connecting / 已终结返回 `None`，由调用方回退真实 Idle。与统计投影共用
    /// [`crate::stats::LiveStatsState`] 单一事实源，不新建第二份状态。
    /// W3-2/P3：同构含伴生数据——发布边界附加的 `ReconnectStatus` 在此同源附加
    /// （service 层 `GetSnapshot` 的再包一层幂等同值覆盖）。
    fn live_snapshot(&self) -> Option<RuntimeSnapshot> {
        let (operation_id, latest) = lock_recover(&self.stats_state).connected_projection()?;
        Some(snapshot_with_reconnect(
            // 轮询回放路径不携带会话起点（0）——前端 adoptSnapshot 对缺失值保留
            // 上一已知值，事件路径的 Connected 快照才是该字段的事实源。
            snapshot_with_session_identity(connected_snapshot(&operation_id, latest, 0), self.reconnect_generation.identity_for(&operation_id)),
            self.reconnect_status(),
        ))
    }
}

fn saved_envelope_status(error: &SavedConnectEnvelopeError) -> Status {
    match error {
        SavedConnectEnvelopeError::Envelope(
            exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError::InvalidUsername
            | exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError::InvalidPassword,
        ) => connect_materials_missing("cisco-material-missing"),
        _ => Status::failed_precondition(CONNECTION_ENGINE_SESSION_CODE),
    }
}

// ---------------------------------------------------------------------------
// W1-C（P7）：UI 一次性凭据——解析取走、persist 元数据、落盘与 envelope 错误映射
// （win32 kernel_control_service.rs `take_ui_credentials`/`persist_credentials_requested`
// /`persist_ui_credentials`/`credential_error_to_status` 的 darwin 对应）。
// ---------------------------------------------------------------------------

/// 解析本次 RPC 的 UI 一次性凭据（win32 `take_ui_credentials` 逐字对应）。先取走并
/// 归零 wire bytes，再对零化类型解析副本做严格版本/空值校验；任一失败均 typed 拒绝、
/// 不进入 Engine，请求 carrier 不再遗留原始字节。
fn take_ui_credentials(
    request: &mut ConnectRequest,
) -> Result<Option<UiSecretPayload>, Status> {
    let mut payload = std::mem::take(&mut request.secret_payload);
    if payload.is_empty() {
        return Ok(None);
    }
    let parsed = parse_ui_secret_payload(&payload)
        .map_err(|_| Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE));
    payload.zeroize();
    let credentials = parsed?;
    match credentials {
        UiSecretPayload::Cisco(mut credentials) => {
            if credentials.version != SECRET_PAYLOAD_VERSION
                || credentials.username.trim().is_empty()
                || credentials.password.is_empty()
            {
                credentials.zeroize();
                return Err(Status::invalid_argument(
                    CONNECTION_CREDENTIALS_INVALID_CODE,
                ));
            }
            Ok(Some(UiSecretPayload::Cisco(credentials)))
        }
        UiSecretPayload::Atrust(credentials) => {
            // 解析层已校验（防御性复核）；会话秘密随局部值的 Drop 清零。
            if credentials.version != ATRUST_SECRET_PAYLOAD_VERSION
                || credentials.protocol != Protocol::Atrust.as_str()
            {
                return Err(Status::invalid_argument(
                    CONNECTION_CREDENTIALS_INVALID_CODE,
                ));
            }
            Ok(Some(UiSecretPayload::Atrust(credentials)))
        }
    }
}

/// T02：连接入口的协议门禁（win32 `ensure_selected_protocol_is_serviceable` 同语义，
/// 差异仅在 darwin 没有 `windows_connection_mode` 这类会话级覆盖参数）。
///
/// 顺序有意如此：用户选 aTrust 时最可行动的原因是「后端未接入本版本」，而不是
/// 「你保存的密码对不上」。判定只读内存配置的 `protocol`，既不装载磁盘凭据、也不
/// 组装 envelope、不写入任何密码——拒绝发生在**任何凭据被使用之前**。
///
/// # Errors
///
/// 所选协议为 aTrust（后端尚未接线）→ `failed_precondition` + 点名 aTrust 的中文
/// 消息；Cisco → `Ok(())`。

/// `persist=true` 是 Tauri→Core 请求元数据（win32 `:3433-3439` 逐字对应），刻意不进入
/// engine envelope。只有精确 `"true"` 才允许持久化；缺失、格式不符或旧客户端全部保守
/// 视为本次使用。
fn persist_credentials_requested(request: &Request<ConnectRequest>) -> bool {
    request
        .metadata()
        .get("x-exv-persist-credentials")
        .and_then(|value| value.to_str().ok())
        == Some("true")
}

/// UI 凭据落盘失败的错误映射：用户名不合约束（用户输入可经弹窗纠正）→ 凭据缺失；
/// 其余（存储/加密）→ 引擎会话稳定码（win32 把两者均压 internal，darwin 沿用既有
/// `saved_envelope_status` 的 `failed_precondition` 词汇）。
fn ui_persist_status(error: DarwinConfigError) -> Status {
    match error {
        DarwinConfigError::InvalidUsername => {
            Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE)
        }
        _ => Status::failed_precondition(CONNECTION_ENGINE_SESSION_CODE),
    }
}

/// UI 凭据分支 envelope 组装失败的错误映射：用户名/密码不合 envelope 约束（弹窗可
/// 纠正）→ 凭据缺失；其余（`server`/`routes`/`mtu`/`user_agent` 均为 config 派生
/// 字段）→ 引擎会话稳定码。
fn ui_envelope_status(error: exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError) -> Status {
    match error {
        exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError::InvalidUsername
        | exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError::InvalidPassword => {
            Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE)
        }
        _ => Status::failed_precondition(CONNECTION_ENGINE_SESSION_CODE),
    }
}

/// P4 v2：[`LifecycleError`] → typed `Status`（稳定码 + 中文 message；探测事实不
/// 进错误 message——失败路径 UI 只展示 message，事实经面板 query 重新获取）。
fn lifecycle_status(error: LifecycleError) -> Status {
    const PROXY_POLICY_CLEANUP_FAILED_CODE: &str =
        "DARWIN_SERVICE_AGENT_PROXY_POLICY_CLEANUP_FAILED";
    match error {
        LifecycleError::Denied(_) => Status::failed_precondition(format!(
            "{ELEVATION_DENIED_CODE}: 用户取消了管理员授权，服务操作未执行"
        )),
        LifecycleError::Failed(_, detail)
            if detail.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|token| token == PROXY_POLICY_CLEANUP_FAILED_CODE) =>
        {
            Status::failed_precondition(format!(
                "{ELEVATION_FAILED_CODE}: 清理旧系统代理失败，服务操作已中止。请重试；持续失败时反馈日志（{PROXY_POLICY_CLEANUP_FAILED_CODE}）。"
            ))
        }
        LifecycleError::Failed(_, detail) => Status::failed_precondition(format!(
            "{ELEVATION_FAILED_CODE}: 服务操作提权执行失败：{detail}"
        )),
        LifecycleError::NotInstalled(_) => Status::failed_precondition(format!(
            "{SERVICE_NOT_INSTALLED_CODE}: 服务未安装，请先安装服务"
        )),
        LifecycleError::SourceMissing(_) => Status::failed_precondition(format!(
            "{SERVICE_SOURCE_MISSING_CODE}: 找不到服务安装源二进制（应用包内未随附且本机无已安装副本）"
        )),
    }
}

/// 只追加实际可选失败，不将所有成功命令输出变成日志。
fn service_action_detail(summary: &str, diagnostics: &str) -> String {
    if diagnostics.is_empty() { summary.to_owned() } else { format!("{summary}; {diagnostics}") }
}

/// [`LifecycleError`] 的 `SERVICE_ACTION` 日志 detail 字段：稳定码；提权失败
/// （`Failed`）再附增强 stderr 细节（service agent 稳定码 + artifact `contract_key` +
/// 不符维度，`elevation_failure_detail` 已有界）。全部固定常量与系统文本，无秘密。
fn lifecycle_failure_detail(error: &LifecycleError) -> String {
    match error {
        LifecycleError::Denied(_) => ELEVATION_DENIED_CODE.to_string(),
        LifecycleError::Failed(_, detail) => format!("{ELEVATION_FAILED_CODE}: {detail}"),
        LifecycleError::NotInstalled(_) => SERVICE_NOT_INSTALLED_CODE.to_string(),
        LifecycleError::SourceMissing(_) => SERVICE_SOURCE_MISSING_CODE.to_string(),
    }
}

/// 连接 RPC 同步拒绝的结构化留痕：此前该路径零日志，事后无法取证被拒原因
/// （真机案例：internal 兜底 toast 无码可提交，聚合日志也无任何条目）。
/// 只记 gRPC 状态码与 message——typed 拒绝码都包含在 message 文本内。
fn log_connect_rejected(logs: &Arc<LogControl>, status: &Status) {
    logs.aggregator().append_core(
        "warn",
        "core",
        "CONNECT_REJECTED",
        format!("connect rejected: {}: {}", status.code(), status.message()).as_str(),
        &[("grpc_code", status.code().to_string())],
    );
}

fn engine_session_status(error: &crate::EngineLifecycleError) -> Status {
    match error {
        crate::EngineLifecycleError::ServiceAgent => {
            // P4 v2：会话建立失败（service agent 启动 Engine 被拒/中途不可达）带上
            // win32 既有前缀词汇——壳层 map_status 依此分流 `ServiceConnectFailed`
            // 恢复 modal；稳定码保留在 message 内供诊断（旧断言按码匹配仍成立）。
            Status::failed_precondition(format!(
                "{SERVICE_CONNECT_FAILED_PREFIX}{CONNECTION_ENGINE_ELEVATION_CODE}: service agent 会话建立失败（服务在但启动 Engine 被拒或服务中途不可达）"
            ))
        }
        crate::EngineLifecycleError::Observe(crate::ObserveError::Connect(error)) => {
            Status::unavailable(format!(
                "{CONNECTION_ENGINE_SESSION_CODE}: connect kind={:?} errno={:?}: {error}",
                error.kind(),
                error.raw_os_error(),
            ))
        }
        crate::EngineLifecycleError::BootstrapConnectDeadline { .. } => {
            Status::unavailable(format!("{CONNECTION_ENGINE_SESSION_CODE}: {error}"))
        }
        // MAC-LIFECYCLE-15 S1：Engine 已进入 Common handler 的 typed 门（如重复连接的
        // `failed_precondition(DARWIN_ENGINE_ATTEMPT_ACTIVE)`）时原样透传 code 与
        // message，不再压成笼统 unavailable；其余 RPC/transport 失败仍映射为 Core
        // 会话失败的稳定错误（最小透传方案：只放行 failed_precondition 门）。
        crate::EngineLifecycleError::Observe(crate::ObserveError::Rpc(status))
            if status.code() == Code::FailedPrecondition =>
        {
            Status::failed_precondition(status.message().to_owned())
        }
        _ => Status::unavailable(CONNECTION_ENGINE_SESSION_CODE),
    }
}

/// 2026-09-30 修复（M2/M2b）：attach 三连流（状态/统计/日志）一次性挂接，任一
/// 失败返回该错误（调用方负责清槽/重建裁决）。挂接顺序与既有语义一致：attach-
/// before-apply，三条流全部在 `ApplyTunnel` 之前挂上。
async fn attach_engine_streams(
    session: &mut tokio::sync::MutexGuard<'_, Option<LiveEngineSession>>,
) -> Result<
    (
        EngineUpdateStream<wire::ConnectStatusEvent>,
        EngineUpdateStream<wire::StatsEvent>,
        EngineUpdateStream<wire::LogEvent>,
    ),
    EngineLifecycleError,
> {
    // 会话槽由调用方的建立块保证 Some；防御分支经 engine_session_status 映射回
    // 既有 `unavailable(CONNECTION_ENGINE_SESSION_CODE)` 拒绝语义。
    let connect_status_stream = session
        .as_mut()
        .ok_or(EngineLifecycleError::Bootstrap)?
        .attach_connect_status()
        .await?;
    // W-MVP-13：同一已认证控制通道上的既有 `StreamStats` 接缝；在 Apply 前挂接，
    // 数据面建立后样本不丢。
    let engine_stats_stream = session
        .as_mut()
        .ok_or(EngineLifecycleError::Bootstrap)?
        .attach_stats()
        .await?;
    // MAC-OBS-13 S1：同一已认证控制通道上的既有 `StreamLogs` 接缝；同样在 Apply
    // 前挂接，管线启动期的 sink 事件不丢。每次 Connect 重挂即替换 Engine 侧推送
    // 通道（last-writer-wins），旧 ingest 任务随旧流 EOF 自行退出。
    let engine_logs_stream = session
        .as_mut()
        .ok_or(EngineLifecycleError::Bootstrap)?
        .attach_logs()
        .await?;
    Ok((connect_status_stream, engine_stats_stream, engine_logs_stream))
}

/// 2026-09-30 修复（M2b）：engine 会话占用型拒绝稳定码（`SessionOccupiedService`
/// 对 busy 窗口内全部 RPC 的恒拒词汇；core 透传条件见 `engine_session_status` 的
/// MAC-LIFECYCLE-15 分支——code==FailedPrecondition ∧ message 含稳定码）。
const ENGINE_ATTEMPT_ACTIVE_MESSAGE: &str = "DARWIN_ENGINE_ATTEMPT_ACTIVE";

/// 判定 attach 失败是否为占用型拒绝（清槽后就地重建一次的唯一触发；与壳层
/// `map_status` 分支条件对齐，避免 message 碰巧含该词的其他错误误入）。
fn is_engine_attempt_active(error: &EngineLifecycleError) -> bool {
    matches!(
        error,
        EngineLifecycleError::Observe(crate::engine_client::ObserveError::Rpc(status))
            if status.code() == Code::FailedPrecondition
                && status.message().contains(ENGINE_ATTEMPT_ACTIVE_MESSAGE)
    )
}

/// 生产日志聚合语义层：默认路径（`config_dir()/logs/aggregated.jsonl`）打开；
/// 落盘不可用时聚合存储自行降级内存-only，不阻断 Core 启动。
fn default_log_control() -> Arc<LogControl> {
    Arc::new(LogControl::new(Arc::new(LogAggregator::open_default())))
}

/// 生产代理 TUN 检测缓存：真实探针 + 启动后首次快照前探测一次（无特权只读，
/// 有界、fail open 到 None；不阻塞、不改任何网络状态）。
fn live_proxy_tun_cache() -> Arc<ProxyTunCache> {
    let cache = Arc::new(ProxyTunCache::production());
    cache.refresh();
    cache
}

struct ConnectionProjectionState {
    status_attached: bool,
    engine: Box<dyn ConnectionEngine>,
}

/// Core 内存连接投影：只将 fake Engine 的 pending/terminal 事实转换成 UI events。
struct ConnectionProjection {
    state: Mutex<ConnectionProjectionState>,
    /// 共享事件总线（与 service/watch 同一实例；tick 由总线铸造）。
    events: Arc<EventBus>,
    /// 上游代理 TUN 检测缓存（MAC-PROXY-16 S1，fixture 路径与生产发布边界同语义）。
    proxy_tun: Arc<ProxyTunCache>,
    /// P4 v2：与生产投影同构的服务状态缓存槽（夹具从不写入，恒 None——sink 的
    /// 附加逻辑因此是无害 no-op，仅保持两侧 sink 代码同形）。
    service_status_cache: Arc<Mutex<Option<exv_vpn_wire::generated::ServiceStatus>>>,
}

impl ConnectionProjection {
    /// **仅测试构建可达**：只由测试 fixture 构造链
    /// （`DarwinKernelControlService::new_inner`）创建；生产连接走
    /// `LiveConnectionProjection`。
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "仅由 #[cfg(test)] fixture 构造链调用；生产连接走 LiveConnectionProjection"
        )
    )]
    fn new(
        engine: Box<dyn ConnectionEngine>,
        events: Arc<EventBus>,
        proxy_tun: Arc<ProxyTunCache>,
    ) -> Self {
        Self {
            state: Mutex::new(ConnectionProjectionState {
                status_attached: false,
                engine,
            }),
            events,
            proxy_tun,
            service_status_cache: Arc::new(Mutex::new(None)),
        }
    }

    fn connect(&self, request: &ConnectRequest) -> Result<OperationReply, Status> {
        self.ensure_status_attached()?;
        self.proxy_tun.refresh();
        lock_recover(&self.state).engine.apply_connect(request)?;
        let operation_id = operation_id_from_connect(request);
        self.publish(pending_connect_snapshot(request, operation_id));
        Ok(OperationReply { terminal: None })
    }

    fn stop(&self, request: &StopRequest) -> Result<OperationReply, Status> {
        self.ensure_status_attached()?;
        lock_recover(&self.state).engine.stop(request)?;
        let operation_id = operation_id_from_stop(request);
        self.publish(pending_stop_snapshot(request, operation_id));
        self.proxy_tun.refresh();
        Ok(OperationReply { terminal: None })
    }

    fn ensure_status_attached(&self) -> Result<(), Status> {
        let mut state = lock_recover(&self.state);
        if state.status_attached {
            return Ok(());
        }
        let events = Arc::clone(&self.events);
        let proxy_tun = Arc::clone(&self.proxy_tun);
        // P4 v2 闪烁修复（2026-09-08 真机反馈）：引擎状态事件此前直发总线、绕过
        // `publish` 的服务状态缓存附加，导致连接期统计重放（service_status=None）
        // 与 GetSnapshot 轮询（Some）在 UI 交替呈现「未知↔运行中」。sink 与
        // `publish` 同源附加缓存伴生值。
        let service_status_cache = Arc::clone(&self.service_status_cache);
        let sink: EngineStatusSink = Arc::new(move |mut status| {
            if let Some(status_field) = lock_recover(&service_status_cache).clone() {
                status.snapshot.service_status = Some(status_field);
            }
            events.publish(
                RuntimeEventKind::Transition,
                snapshot_with_environment(status.snapshot, &proxy_tun),
            );
        });
        state.engine.attach_status(sink)?;
        state.status_attached = true;
        Ok(())
    }

    /// 经共享总线发布过渡事件（tick 由总线铸造；事件 `operation_id` 镜像快照）。
    fn publish(&self, snapshot: RuntimeSnapshot) {
        self.events.publish(
            RuntimeEventKind::Transition,
            snapshot_with_environment(snapshot, &self.proxy_tun),
        );
    }
}

fn receiver_control_deadline(operation: &[u8], value: u64) -> Result<AuthDeadline, Status> {
    if operation.len() != 16 { return Err(Status::invalid_argument("auth_control/operation_id must be 16 bytes")); }
    AuthDeadline::from_monotonic_ns(value).map_err(auth_deadline_status)
}

#[tonic::async_trait]
impl KernelControl for DarwinKernelControlService {
    type WatchEventsStream = WatchEventsStream;
    type WatchStatsStream = WatchStatsStream;

    async fn trigger_latency_refresh(
        &self,
        request: Request<wire::KernelLatencyRefreshRequest>,
    ) -> Result<Response<wire::KernelLatencyRefreshReply>, Status> {
        let live = self.live_connection.as_deref()
            .ok_or_else(|| Status::failed_precondition("DARWIN_LATENCY_OWNER_NOT_ACTIVE"))?;
        live.trigger_latency_refresh(request.into_inner().operation_id).await.map(Response::new)
    }

    async fn prepare_auth_control(&self, request: Request<wire::PrepareAuthControlRequest>) -> Result<Response<OperationReply>, Status> {
        let request = request.into_inner();
        let deadline = receiver_control_deadline(&request.operation_id, request.deadline_monotonic_ns)?;
        let live = self.live_connection.as_deref().ok_or_else(|| Status::failed_precondition(CONNECTION_ENGINE_NOT_ATTACHED_CODE))?;
        live.prepare_auth_control(deadline).await?;
        Ok(Response::new(OperationReply { terminal: None }))
    }

    async fn auth_receiver_control(&self, request: Request<wire::AuthReceiverControlRequest>) -> Result<Response<wire::AuthReceiverControlReply>, Status> {
        let request = request.into_inner();
        let deadline = receiver_control_deadline(&request.operation_id, request.deadline_monotonic_ns)?;
        if !matches!(wire::AuthReceiverAction::try_from(request.action), Ok(wire::AuthReceiverAction::Inspect | wire::AuthReceiverAction::DisableAndStop | wire::AuthReceiverAction::Restore | wire::AuthReceiverAction::ReadRecovery)) {
            return Err(Status::invalid_argument("auth_receiver_control/action is required"));
        }
        let live = self.live_connection.as_deref().ok_or_else(|| Status::failed_precondition(CONNECTION_ENGINE_NOT_ATTACHED_CODE))?;
        live.auth_receiver_control(request, deadline).await.map(Response::new)
    }

    async fn connect(
        &self,
        request: Request<ConnectRequest>,
    ) -> Result<Response<OperationReply>, Status> {
        let auth_deadline = match AuthDeadline::from_metadata(request.metadata()) {
            Ok(deadline) => deadline,
            Err(error) => {
                let mut request = request.into_inner();
                request.secret_payload.zeroize();
                return Err(auth_deadline_status(error));
            }
        };
        // W1-C（P7）：persist 元数据必须在 `into_inner` 之前读取（win32 :3433-3439）。
        let persist_credentials = persist_credentials_requested(&request);
        let mut request = request.into_inner();
        // W1-C（P7）：UI 一次性凭据先于任何派发取走并归零 wire bytes（win32 :3410-3429）；
        // 解析/校验失败 typed 拒绝，请求 carrier 不再遗留原始字节。此后
        // `request.secret_payload` 恒空，返回前的兜底 zeroize 只是保持既有不变式。
        let ui_credentials = match take_ui_credentials(&mut request) {
            Ok(package) => package,
            Err(status) => {
                log_connect_rejected(&self.logs, &status);
                return Err(status);
            }
        };
        let result = if let Some(live) = self.live_connection.as_deref() {
            live.connect_with_auth_deadline(&request, ui_credentials, persist_credentials, auth_deadline)
                .await
        } else {
            self.connection()
                .and_then(|connection| connection.connect(&request))
        };
        request.secret_payload.zeroize();
        match result {
            Ok(reply) => Ok(Response::new(reply)),
            Err(status) => {
                log_connect_rejected(&self.logs, &status);
                Err(status)
            }
        }
    }

    async fn respond_interaction(
        &self,
        request: Request<InteractionResponse>,
    ) -> Result<Response<OperationReply>, Status> {
        let mut request = request.into_inner();
        let result = if let Some(live) = self.live_connection.as_deref() {
            live.respond_auth_acquisition(&mut request)
        } else { Ok(false) };
        request.response_payload.zeroize();
        match result {
            Ok(true) => Ok(Response::new(OperationReply { terminal: None })),
            Ok(false) => prep_only(),
            Err(status) => Err(status),
        }
    }

    async fn stop(
        &self,
        request: Request<StopRequest>,
    ) -> Result<Response<OperationReply>, Status> {
        let deadline = stop_request_deadline(&request)?;
        let request = request.into_inner();
        if let Some(live) = self.live_connection.as_deref() {
            live.stop_with_deadline(&request, deadline).await.map(Response::new)
        } else {
            self.connection()
                .and_then(|connection| connection.stop(&request))
                .map(Response::new)
        }
    }

    async fn reconcile(
        &self,
        _request: Request<ReconcileRequest>,
    ) -> Result<Response<OperationReply>, Status> {
        prep_only()
    }

    async fn get_operation(
        &self,
        _request: Request<GetKernelOperationRequest>,
    ) -> Result<Response<KernelOperationReply>, Status> {
        prep_only()
    }

    /// 真实快照（B-09①）：存在活跃连接投影且当前 operation 已 Connected 时回放
    /// live 快照（含最新门控统计，语义与 `WatchEvents` 推送一致）；无活动统计时回放最近发布的连接/清理/失败状态，避免轮询把断线提示覆盖成 Idle。
    async fn get_snapshot(
        &self,
        request: Request<SnapshotRequest>,
    ) -> Result<Response<RuntimeSnapshot>, Status> {
        if !request.into_inner().runtime_epoch.is_empty() {
            return Err(Status::new(
                Code::InvalidArgument,
                SNAPSHOT_EPOCH_UNSUPPORTED_CODE,
            ));
        }
        // GetSnapshot 与事件边界共用限频的只读环境探测，实时排除 Engine 上报的自身接口。
        // Idle 与 live 快照同点附加。W3-2/P3：与发布边界
        // 同源附加 `ReconnectStatus`（读内存配置三键 + 原子计数/在途——受理快照
        // 零磁盘约束自动满足）；live 投影缺席（仅测试 fixture 构造）时 reconnect
        // 留空（无连接语义）。
        // P4 v2：GetSnapshot 边界刷新服务状态（win32 缓存伴生值模型：此处探测并
        // 写缓存，事件发布边界只读缓存），快照携带 service_status。
        let proxy_tun = &self.proxy_tun;
        let facts = (self.service_lifecycle.probe())().await;
        let service_status = crate::service_status::wire_service_status(&facts);
        *lock_recover(&self.service_status_cache) = Some(service_status.clone());
        let mut snapshot = if let Some(live) = self.live_connection.as_deref() {
            let reconnect = live.reconnect_status();
            let base = live.live_snapshot().unwrap_or_else(||
                live.events.current_snapshot().unwrap_or_else(idle_snapshot));
            let identity = live.reconnect_generation.identity_for(&base.operation_id);
            snapshot_with_reconnect(snapshot_with_environment(snapshot_with_session_identity(base, identity), proxy_tun), reconnect)
        } else {
            snapshot_with_environment(idle_snapshot(), proxy_tun)
        };
        snapshot.service_status = Some(service_status);
        if let Some(live) = self.live_connection.as_deref() { snapshot.atrust_auth_acquisition = live.auth_acquisition(); }
        Ok(Response::new(snapshot))
    }

    /// `WatchEvents` 真实订阅（W3-1/P1 S2 对齐 win32 形状）：一行总线订阅，无 claim、
    /// 无一次性 mpsc、流 Drop 不触发 shutdown（Core 退出主路径 = 认证连接 EOF + server
    /// 退出，见 `ui_runner::wait_for_terminal`）。resume 语义见 [`EventBus::subscribe`]：
    /// 0 = 重放当前快照（构造期 initial Idle，tick 1）；落后 = 重放当前；随后转发现场
    /// 事件——多订阅者 fan-out，UI 内重连按 resume 游标补齐。
    async fn watch_events(
        &self,
        request: Request<WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        let resume_tick = request.into_inner().resume_tick;
        let stream = self
            .events
            .subscribe(resume_tick)
            .map(Ok::<RuntimeEvent, Status>);
        Ok(Response::new(Box::pin(stream)))
    }

    /// `WatchStats` 真实订阅（T14 拆流，对齐 win32 形状）：一行统计 lane 订阅。
    /// resume 语义见 [`EventBus::subscribe_stats`]——0 = 重放当前缓存样本；随后转发
    /// `sample_tick` 递增的现场样本（`publish_stats` 铸造，与事件 tick 同轴）。拆流
    /// 后统计样本不再随 `WatchEvents` 重发——挂死的统计消费者只降级本流（h2 按流
    /// 流控），事件流与一元 RPC 照常。
    async fn watch_stats(
        &self,
        request: Request<WatchStatsRequest>,
    ) -> Result<Response<Self::WatchStatsStream>, Status> {
        let resume_tick = request.into_inner().resume_tick;
        let stream = self
            .events
            .subscribe_stats(resume_tick)
            .map(Ok::<wire::RuntimeStats, Status>);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn logs_list(
        &self,
        request: Request<LogsListRequest>,
    ) -> Result<Response<LogsListReply>, Status> {
        // MAC-OBS-13 S1：真实历史分页（尾部/增量/limit clamp/filter，语义见
        // log_control；条目经聚合存储入库边界脱敏）。
        Ok(Response::new(self.logs.list(&request.into_inner())))
    }

    async fn record_auth_diagnostic(&self, request: Request<wire::LogEvent>) -> Result<Response<wire::RecordAuthDiagnosticReply>, Status> {
        let event = exv_vpn_wire::auth_diagnostic::normalize(request.into_inner()).map_err(Status::invalid_argument)?;
        let logs = Arc::clone(self.logs.aggregator());
        tokio::task::spawn_blocking(move || {
            let fields: Vec<_> = event.fields.iter().map(|(key, value)| (key.as_str(), value.clone())).collect();
            logs.append_core(&event.level, &event.component, &event.code, &event.message, &fields);
        }).await.map_err(|_| Status::internal("auth_diagnostic/write_task"))?;
        Ok(Response::new(wire::RecordAuthDiagnosticReply { recorded: true }))
    }

    async fn logs_clear(
        &self,
        _request: Request<LogsClearRequest>,
    ) -> Result<Response<LogsClearReply>, Status> {
        // MAC-OBS-13 S4：真实现——truncate 聚合存储（磁盘真相源）+ 游标归零，返回
        // `(cleared, removed_entries)`；此后新条目 seq 从 1 重新开始（存储层保证，
        // 与追加/读取同一锁串行一致）。truncate 失败以固定 typed code 拒绝，不泄露
        // IO 文本（存储进入内存-only 降级态，`LogsList` 仍可用）。
        let (cleared, removed_entries) = self
            .logs
            .clear()
            .map_err(|_| Status::internal(LOGS_CLEAR_IO_FAILED_CODE))?;
        Ok(Response::new(LogsClearReply {
            cleared,
            removed_entries,
        }))
    }

    async fn config_get(
        &self,
        _request: Request<ConfigGetRequest>,
    ) -> Result<Response<ConfigPayload>, Status> {
        // 每次重读磁盘；损坏先备份再默认修复，不可读则正常回复结构化失败，
        // 不把旧内存配置冒充磁盘配置，也不以默认配置覆盖不可读文件。
        let config_dir = self.config().config_dir().to_path_buf();
        let startup = match config_hygiene::load_for_startup(&config_dir) {
            Ok(startup) => startup,
            Err(DarwinConfigError::ReadFailed { path, .. }) => {
                return Ok(Response::new(ConfigPayload {
                    items: Vec::new(),
                    requires_quick_start: false,
                    config_recovery: None,
                    read_failure: Some(wire::ConfigReadFailure {
                        // Darwin errno 不是 Win32 错误码；沿既有 wire 以 0 表示不可用。
                        win32_code: 0,
                        path: path.to_string_lossy().into_owned(),
                    }),
                }));
            }
            Err(error) => return Err(config_status(error)),
        };
        let requires_quick_start = startup.requires_quick_start;
        // 关键：刷新进程内共享 `Arc<Mutex<DarwinUiConfig>>`——连接链路
        // `connect_payload_and_plan` 共享同一实例，不刷新则 Connect 组装 envelope
        // 读到旧值（顺带消灭 config_get 永不重读磁盘、内存态永久陈旧的暗病）。
        let items = {
            let mut config = self.config();
            *config = startup.config;
            config.items()
        };
        Ok(Response::new(ConfigPayload {
            items: items
                .into_iter()
                .map(|item| ConfigItem {
                    key: item.key().to_owned(),
                    value: item.value().to_owned(),
                })
                .collect(),
            requires_quick_start,
            config_recovery: startup.config_recovery.map(|recovery| wire::ConfigRecovery {
                corrupted: recovery.corrupted,
                backup_file: recovery.backup_file,
            }),
            read_failure: None,
        }))
    }

    async fn config_set(
        &self,
        request: Request<ConfigSetRequest>,
    ) -> Result<Response<ConfigReply>, Status> {
        let items = request
            .into_inner()
            .items
            .into_iter()
            .map(|item| DarwinConfigItem::new(item.key, item.value))
            .collect::<Vec<_>>();
        // MAC-OBS-13 S4：保存成功的敏感值登记进聚合存储的入库脱敏表——此后任何
        // 日志节点把该值拼进 message/fields，入库边界都替换为 `<redacted>`（值
        // 本身仍不作为日志内容；这是纵深防御接线，见 secret scan 端到端契约测试）。
        let saved_password = items
            .iter()
            .find(|item| item.key() == "password")
            .map(|item| item.value().to_owned());
        self.config().apply_and_save(items).map_err(config_status)?;
        if let Some(live) = self.live_connection.as_ref() {
            live.refresh_background_permission();
        }
        if let Some(password) = saved_password {
            self.logs.aggregator().register_secret(&password);
        }
        Ok(Response::new(ConfigReply { ok: true }))
    }

    /// P4 v2（动作面实装）：`query` 返回 service agent 健康探测的真实五态事实（见
    /// [`crate::service_status`]）；`install`/`uninstall`/`start` 经
    /// [`ServiceLifecycle`] 以管理员提权执行固定 argv 的 service agent CLI 动词
    /// （一次授权一个动作，win32「1 动作 = 1 次 runas」同构），变更动作加
    /// 串行锁并在前置尽力停业务（[`LiveConnectionProjection::stop_for_service_transition`]，
    /// win32 `stop_engine_for_transition` 对应）；动作回复只表示原命令结果，
    /// 状态通过独立 query 获取。`rotate_key` 诚实空缺：darwin Engine 会话
    /// 使用一次性 ticket（无持久服务密钥），无密钥可轮换，typed 拒绝声明语义
    /// 不适用。
    #[allow(
        clippy::too_many_lines,
        reason = "五个 action 分支线性展开保持 win32 对照可读；公共前置/回填收敛在首尾"
    )]
    async fn service_control(
        &self,
        request: Request<ServiceControlRequest>,
    ) -> Result<Response<ServiceControlReply>, Status> {
        use exv_vpn_wire::generated::service_control_request::Action;
        let record = |action: &str, ok: bool, detail: &str| {
            self.logs.aggregator().append_core(
                if ok { "info" } else { "error" },
                "core",
                "SERVICE_ACTION",
                detail,
                &[("action", action.to_string()), ("ok", ok.to_string())],
            );
        };
        // 2026-09-20 问题四第一层：失败细节进日志——失败记录的 fields 携带稳定码
        //（提权失败再附增强 stderr 细节：service agent 稳定码 + contract_key + 不符
        // 维度）。此前失败日志只有一句 "service install failed"，远程无法定位具体
        // 环节（用户实测 8 次 install 失败仅此一句，细节只在 UI typed message）。
        let record_failure = |action: &str, summary: &str, error: &LifecycleError| {
            self.logs.aggregator().append_core(
                "error",
                "core",
                "SERVICE_ACTION",
                summary,
                &[
                    ("action", action.to_string()),
                    ("ok", "false".to_string()),
                    ("detail", lifecycle_failure_detail(error)),
                ],
            );
        };
        let reply_with = |message: &'static str| -> Response<ServiceControlReply> {
            *lock_recover(&self.service_status_cache) = None;
            Response::new(ServiceControlReply { service_status: None, ok: true, message: message.to_string() })
        };
        match request.into_inner().action {
            Some(Action::Query(_)) => {
                let facts = (self.service_lifecycle.probe())().await;
                let status = crate::service_status::wire_service_status(&facts);
                *lock_recover(&self.service_status_cache) = Some(status.clone());
                // 服务修订三字段（expected/随附/已装——已装=读固定安装路径 engine
                // 二进制的 Mach-O 嵌入式修订号，只读文件头，不运行进程）。随附路径
                // 解析与读段整体经 spawn_blocking 下沉（Core 是 current_thread
                // runtime，保持 gRPC/事件循环不被文件 I/O 冻结；先例
                // service_lifecycle::run_elevated）。
                let revision = tokio::task::spawn_blocking(|| {
                    let bundled = crate::service_revision::bundled_engine_path();
                    crate::service_revision::collect_revision_facts(bundled.as_deref())
                })
                .await
                .unwrap_or_else(|_join_error| {
                    // worker 崩溃/取消（理论不可达）：保守回落全「未知」，不伪造。
                    crate::service_revision::ServiceRevisionFacts::unavailable()
                });
                let message = format!(
                    "服务组件探测完成；{}",
                    revision.summarize()
                );
                Ok(Response::new(ServiceControlReply {
                    service_status: Some(status),
                    ok: true,
                    message,
                }))
            }
            Some(Action::Install(_)) => {
                let _guard = self.service_control_lock.lock().await;
                if let Some(live) = self.live_connection.as_deref() {
                    live.stop_for_service_transition().await;
                }
                match self.service_lifecycle.install().await {
                    Ok(diagnostics) => {
                        record("install", true, &service_action_detail("service install command completed", &diagnostics));
                        Ok(reply_with("服务安装命令已完成。"))
                    }
                    Err(error) => {
                        record_failure("install", "service install failed", &error);
                        Err(lifecycle_status(error))
                    }
                }
            }
            Some(Action::Uninstall(request)) => {
                let _guard = self.service_control_lock.lock().await;
                if let Some(live) = self.live_connection.as_deref() {
                    live.stop_for_service_transition().await;
                }
                let result = if request.full_cleanup {
                    self.service_lifecycle.uninstall_with_cleanup(true).await
                } else {
                    self.service_lifecycle.uninstall().await
                };
                match result {
                    Ok(diagnostics) => {
                        record("uninstall", true, &service_action_detail("service uninstall command completed", &diagnostics));
                        let mut reply = reply_with("服务卸载命令已完成。");
                        for line in diagnostics.lines().filter_map(|line| line.strip_prefix("service-uninstall retained directory=")) {
                            if let Some(directory) = line.strip_suffix(" reason=nonempty") {
                                reply.get_mut().message.push_str(&format!("\n已保留包含其他文件的目录：{}", directory.chars().take(200).collect::<String>()));
                            }
                        }
                        Ok(reply)
                    }
                    Err(error) => {
                        record_failure("uninstall", "service uninstall failed", &error);
                        Err(lifecycle_status(error))
                    }
                }
            }
            Some(Action::Start(_)) => {
                let _guard = self.service_control_lock.lock().await;
                match self.service_lifecycle.ensure_running().await {
                    Ok(diagnostics) => {
                        record("start", true, &service_action_detail("service start command completed or not required", &diagnostics));
                        Ok(reply_with("服务启动命令已完成或无需启动。"))
                    }
                    Err(error) => {
                        record_failure("start", "service start failed", &error);
                        Err(lifecycle_status(error))
                    }
                }
            }
            Some(Action::RotateKey(_)) => Err(Status::invalid_argument(format!(
                "{SERVICE_ROTATE_KEY_CODE}: darwin Engine 会话使用一次性 ticket（无持久服务密钥），没有可轮换的密钥"
            ))),
            None => Err(Status::invalid_argument(SERVICE_ACTION_MISSING_CODE)),
        }
    }
}

fn operation_id_from_connect(request: &ConnectRequest) -> Vec<u8> {
    request
        .intent
        .as_ref()
        .and_then(|intent| intent.lookup_key.as_ref())
        .map_or_else(Vec::new, |key| key.operation_id.clone())
}

fn operation_id_from_stop(request: &StopRequest) -> Vec<u8> {
    request
        .intent
        .as_ref()
        .and_then(|intent| intent.lookup_key.as_ref())
        .map_or_else(Vec::new, |key| key.operation_id.clone())
}

/// Connected 快照：数据面运行中，`stats` 携带当时的最新归一化统计（无样本则 None）。
/// `session_established_at_ms` 来自 Engine Connected 事件（数据面建立时刻）——前端
/// 在线时长的唯一事实源，恒 0 会让 UI 永远显示「—」。
fn connected_snapshot(
    operation_id: &[u8],
    stats: Option<exv_vpn_wire::generated::RuntimeStats>,
    session_established_at_ms: i64,
) -> RuntimeSnapshot {
    RuntimeSnapshot {
        atrust_auth_acquisition: None,
        engine_reconnect: None,
        headless: None,
        relaunched_by_engine: false,
        stop_completion: None,
        system_proxy: None,
        reconnect: None,
        self_heal: None,
        stats,
        proxy_tun: None,
        operation_id: operation_id.to_vec(),
        service_status: None,
        mode: String::new(),
        state: Some(runtime_snapshot::State::Connected(
            exv_vpn_wire::generated::ConnectedState {
                session_established_at_ms,
                session: Some(exv_vpn_wire::generated::ConnectedSession::default()),
            },
        )),
        session_identity: None,
    }
}

fn pending_connect_snapshot(request: &ConnectRequest, operation_id: Vec<u8>) -> RuntimeSnapshot {
    let runtime_epoch = request
        .intent
        .as_ref()
        .and_then(|intent| intent.lookup_key.as_ref())
        .map_or_else(Vec::new, |key| key.runtime_epoch.clone());
    RuntimeSnapshot {
        atrust_auth_acquisition: None,
        engine_reconnect: None,
        headless: None,
        relaunched_by_engine: false,
        stop_completion: None,
        system_proxy: None,
        reconnect: None,
        self_heal: None,
        stats: None,
        proxy_tun: None,
        operation_id: operation_id.clone(),
        service_status: None,
        mode: String::new(),
        state: Some(runtime_snapshot::State::Connecting(ConnectingState {
            attempt: Some(Attempt {
                runtime_epoch,
                attempt_id: operation_id,
                intent: request.intent.clone(),
                prior_error: None,
            }),
            phase: ConnectPhase::ObservingOwnedState as i32,
        })),
        session_identity: None,
    }
}

fn pending_stop_snapshot(request: &StopRequest, operation_id: Vec<u8>) -> RuntimeSnapshot {
    RuntimeSnapshot {
        atrust_auth_acquisition: None,
        engine_reconnect: None,
        headless: None,
        relaunched_by_engine: false,
        stop_completion: None,
        system_proxy: None,
        reconnect: None,
        self_heal: None,
        stats: None,
        proxy_tun: None,
        operation_id,
        service_status: None,
        mode: String::new(),
        state: Some(runtime_snapshot::State::Stopping(StoppingState {
            attempt: None,
            stop: request.intent.clone(),
            queued_connect: None,
        })),
        session_identity: None,
    }
}

/// 构造 10b 唯一允许的真实状态快照。
#[must_use]
pub(crate) fn idle_snapshot() -> RuntimeSnapshot {
    RuntimeSnapshot {
        atrust_auth_acquisition: None,
        engine_reconnect: None,
        headless: None,
        relaunched_by_engine: false,
        stop_completion: None,
        system_proxy: None,
        reconnect: None,
        self_heal: None,
        stats: None,
        proxy_tun: None,
        operation_id: Vec::new(),
        service_status: None,
        mode: String::new(),
        state: Some(runtime_snapshot::State::Idle(IdleState {
            last_cleanup: None,
        })),
        session_identity: None,
    }
}

fn config_status(error: DarwinConfigError) -> Status {
    Status::new(error.grpc_code(), error.message())
}

fn prep_only<T>() -> Result<Response<T>, Status> {
    Err(Status::new(Code::Unimplemented, CONFIG_PREP_ONLY_CODE))
}

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

