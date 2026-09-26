//! `MAC-UI-CONFIG-10b` 的普通用户 `KernelControl` 适配器。
//!
//! 本模块把已经认证的 UI 请求映射到 Core 配置和 `Idle` 状态。普通 Config/Idle 路径不启动
//! Engine；明确的 Connect/Stop 才复用同一已认证本地通道调用 Engine 的 `ApplyTunnel`/
//! `StopTunnel`。本阶段不实现网络、CSTP 或 utun。

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

use exv_vpn_host::event_bus::EventBus;
use exv_vpn_wire::generated as wire;
use exv_vpn_wire::generated::{
    Attempt, ConfigGetRequest, ConfigItem, ConfigPayload, ConfigReply, ConfigSetRequest,
    ConnectPhase, ConnectRequest, ConnectingState, GetKernelOperationRequest, IdleState,
    InteractionResponse, KernelOperationReply, LogsClearReply, LogsClearRequest, LogsListReply,
    LogsListRequest, OperationReply, ReconcileRequest, RuntimeEvent, RuntimeEventKind,
    RuntimeSnapshot, ServiceControlReply, ServiceControlRequest, SnapshotRequest, StopRequest,
    StoppingState, WatchEventsRequest, kernel_control_server::KernelControl, runtime_snapshot,
};
use tokio::sync::{oneshot, watch};
use tokio_stream::StreamExt;
use tonic::{Code, Request, Response, Status, codegen::tokio_stream::Stream};
use zeroize::Zeroize;

mod sleep_recovery;
use crate::network_diagnostics::{self, Context as DiagnosticContext, NetworkMonitor};

use crate::{
    DarwinConfigError, DarwinConfigItem, DarwinUiConfig, EngineLifecycleError,
    SECRET_PAYLOAD_VERSION, SavedConnectEnvelopeError, UiCredentialPackage, build_connect_envelope,
    build_saved_connect_envelope, config_hygiene,
    engine_lifecycle_real::{EngineControlSession, launch_fixed_engine_session},
    log_aggregator::LogAggregator,
    log_control::LogControl,
    parse_ui_secret_payload,
    proxy_tun::ProxyTunCache,
    service_lifecycle::{
        ELEVATION_DENIED_CODE, ELEVATION_FAILED_CODE, LifecycleError, ServiceLifecycle,
        ServiceStatusProbe, SERVICE_NOT_INSTALLED_CODE, SERVICE_NOT_READY_CODE,
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
fn snapshot_with_environment(snapshot: RuntimeSnapshot, cache: &ProxyTunCache) -> RuntimeSnapshot {
    let mut snapshot = snapshot_with_proxy_tun(snapshot, cache.current());
    snapshot.system_proxy = cache.system_proxy();
    snapshot
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

/// 可重试数据面掉线判定（镜像 win32 `is_retryable_disconnect`）：engine
/// `session_lost_vpn_error` 升级后的「`stage=DataPlane` + `retry=RetrySameOperation`」
/// 是**唯一**来源（`code=EffectUnknown` 不参与判定——平台类通用码）；据此把「连接
/// 建立后数据面掉线（可重试）」与「连接期失败（DoNotRetry / 其他 stage）」明确区分。
/// 与 Core 自身的 Engine 失联终态（`ProtocolSession` + `UseNewOperation`）天然区分。
#[must_use]
fn is_retryable_disconnect(error: &wire::VpnError) -> bool {
    error.stage == wire::ErrorStage::DataPlane as i32
        && error.retry == wire::RetryAdvice::RetrySameOperation as i32
}

/// 组装 wire `ReconnectStatus`（纯字段映射，不读配置；镜像 win32）。
///
/// `attempts`/`active` 取投影侧 `reconnect_attempts`/`reconnect_active` 原子（状态
/// 投影维护）；`auto_reconnect`/`max_attempts` 由调用方从内存配置读（0 = unlimited）。
#[must_use]
fn reconnect_status_from_state(
    attempts: u32,
    active: bool,
    auto_reconnect: bool,
    max_attempts: u32,
) -> wire::ReconnectStatus {
    wire::ReconnectStatus {
        auto_reconnect,
        max_attempts,
        current_attempt: attempts,
        active,
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
/// 激活触发语义）；具体 grpc code/message 只进 `kernel.reconnect.failed` 诊断日志，
/// 不进 wire（native 细节留空，不伪造分类）。
fn reconnect_dispatch_vpn_error(_status: &Status) -> exv_vpn_wire::generated::VpnError {
    exv_vpn_wire::generated::VpnError {
        code: exv_vpn_wire::generated::ErrorCode::EffectUnknown as i32,
        stage: exv_vpn_wire::generated::ErrorStage::ConnectingControl as i32,
        certainty: exv_vpn_wire::generated::EffectCertainty::Unknown as i32,
        retry: exv_vpn_wire::generated::RetryAdvice::UseNewOperation as i32,
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

/// 有界纯 libc 进程存活观察（MAC-LIFECYCLE-15 S1；判据参照
/// `kill_matrix_real.rs` 的 `wait_for_engine_exit`：`kill(pid, 0)`）。
///
/// 对存活进程返回 0；进程已消失返回 ESRCH；非 root Core 探测 root Engine 得到
/// EPERM——进程存在，视为存活。已知局限：僵尸进程对 `kill(pid, 0)` 仍返回成功
/// （Engine 由服务代理 fork，退出后由其回收）；本检查只是会话复用前的第二道观察，
/// 主判据是事件驱动的状态流终止观察点（见 `spawn_status_projection`）。
fn engine_session_alive(pid: u32) -> bool {
    // SAFETY: kill(2) 只读取参数；signal 0 不向目标进程发送任何信号。
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0
        || (result == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH))
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
            Self::Fixture(_) => "",
        }
    }

    /// 复用前的 liveness 观察：生产会话形态=有界纯 libc 存活检查；服务形态=固定
    /// 端点文件在场（engine 由 launchd 常驻；崩溃残留窗口由状态流 EOF 投影重置
    /// 兜底——本检查只是复用前第二道观察）；fixture=受控存活标志。
    fn is_alive(&self) -> bool {
        match self {
            Self::Live(session) => match session.kind() {
                crate::engine_lifecycle_real::EngineSessionKind::Session => {
                    engine_session_alive(session.engine_pid())
                }
                crate::engine_lifecycle_real::EngineSessionKind::Service => {
                    std::path::Path::new(crate::service_status::SERVICE_ENGINE_SOCKET_PATH)
                        .exists()
                }
            },
            #[cfg(test)]
            Self::Fixture(session) => session.is_alive(),
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
            Self::Fixture(session) => Ok(session.attach_connect_status()),
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
            Self::Fixture(session) => Ok(session.attach_stats()),
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
            Self::Fixture(session) => Ok(session.attach_logs()),
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
    async fn stop_tunnel(
        &mut self,
        request: exv_vpn_wire::generated::StopTunnelRequest,
    ) -> Result<exv_vpn_wire::generated::StopTunnelReply, EngineLifecycleError> {
        match self {
            Self::Live(session) => session.stop_tunnel(request).await,
            #[cfg(test)]
            Self::Fixture(session) => Ok(session.stop_tunnel(request)),
        }
    }

    /// 关闭 Engine client；真实 Engine 会在唯一已认证连接结束后自行退出并清理。
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

/// 一次用户连接受理时冻结的重连策略；自动重连尝试复用同一份值。
#[derive(Clone, Copy)]
struct SessionReconnectPolicy {
    enabled: bool,
    max_attempts: u32,
    backoff: bool,
}

impl SessionReconnectPolicy {
    fn from_config(config: &DarwinUiConfig) -> Self {
        Self {
            enabled: config.auto_reconnect(),
            max_attempts: config.auto_reconnect_max_attempts(),
            backoff: config.auto_reconnect_backoff(),
        }
    }
}

/// 当前会话策略与重连进度共同构成运行事实；尚未连接时不投影保存设置。
fn session_reconnect_status(
    policy: &Mutex<Option<SessionReconnectPolicy>>,
    attempts: &AtomicU32,
    active: &AtomicBool,
) -> Option<wire::ReconnectStatus> {
    (*lock_recover(policy)).map(|policy| {
        reconnect_status_from_state(
            attempts.load(Ordering::Relaxed),
            active.load(Ordering::Acquire),
            policy.enabled,
            policy.max_attempts,
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
    reconnect_generation: Arc<AtomicU64>,
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
    reconnect_tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<u64>>>>,
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
    /// W2.5：服务形态连接路径接缝（facts→修复阶梯→固定端点直连；夹具默认恒
    /// 回退会话路径）。
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
type ServiceEngineConnectFuture = Pin<
    Box<
        dyn Future<
                Output = Result<
                    crate::engine_lifecycle_real::EngineControlSession,
                    crate::engine_lifecycle_real::EngineLifecycleError,
                >,
            > + Send,
    >,
>;
type ServiceEngineConnect = Arc<dyn Fn() -> ServiceEngineConnectFuture + Send + Sync>;

/// W2.5 服务形态连接路径的可注入接缝（生产默认真实探测/帧/直连；夹具默认恒
/// 「engine down 且不可修复」——连接路径走既有会话路径，既有测试零扰动）。
///
/// 编排语义（权威规范 §二.2.2 修复分层 + W2.5 设计 §3）：
/// 1. facts healthy → 直连固定端点；
/// 2. down → daemon socket 可连 → `EnsureEngine` 帧（免提权）；daemon 不在 →
///    经 [`ServiceLifecycle::ensure_running`] 的既有 osascript `start`（该动词已
///    扩展为同时确保 engine job bootstrap）；
/// 3. 有界等端点（15s/250ms）→ 直连；任一步失败回退既有会话路径（过渡兼容，
///    E8 的 `StartEngine` 会话路径删除随 W3 收口）。
pub(crate) struct ServiceEnginePath {
    /// engine 端点事实探测（engine.sock 在+可连=healthy）。
    pub(crate) probe: ServiceStatusProbe,
    /// `EnsureEngine` 帧发送（成功=true）。
    pub(crate) ensure_engine: Arc<
        dyn Fn() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync,
    >,
    /// 固定端点直连（失败=Err）。
    pub(crate) connect: ServiceEngineConnect,
    /// 有界等端点的上界与轮询间隔。
    pub(crate) wait_bound: Duration,
    pub(crate) wait_poll: Duration,
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
                    .is_ok()
                })
            }),
            connect: std::sync::Arc::new(|| {
                Box::pin(crate::engine_lifecycle_real::connect_service_engine_session())
            }),
            wait_bound: crate::service_status::ENGINE_ENDPOINT_WAIT_BOUND,
            wait_poll: crate::service_status::ENGINE_ENDPOINT_WAIT_POLL,
        }
    }

    /// 夹具默认接缝：engine 恒 down 且端点永不出现（连接路径直接回退会话路径；
    /// 不触宿主文件系统/网络）。
    pub(crate) fn hermetic() -> Self {
        Self {
            probe: std::sync::Arc::new(|| {
                Box::pin(std::future::ready(crate::service_status::ServiceAgentFacts {
                    engine_socket_present: false,
                    engine_socket_connectable: false,
                    ..crate::service_status::ServiceAgentFacts::default()
                }))
            }),
            ensure_engine: std::sync::Arc::new(|| Box::pin(std::future::ready(false))),
            connect: std::sync::Arc::new(|| {
                Box::pin(std::future::ready(Err(
                    crate::engine_lifecycle_real::EngineLifecycleError::Bootstrap,
                )))
            }),
            wait_bound: Duration::from_millis(1),
            wait_poll: Duration::from_millis(1),
        }
    }
}

/// 一次 operation 的状态投影参数包：状态投影任务所需的共享状态与身份信息。
struct OperationProjection {
    stats_state: Arc<std::sync::Mutex<LiveStatsState>>,
    runtime_epoch: Vec<u8>,
    operation_id: Vec<u8>,
    intent: Option<exv_vpn_wire::generated::ConnectIntent>,
    session_slot: Arc<tokio::sync::Mutex<Option<LiveEngineSession>>>,
    /// W2 心跳：会话伴生 ticker 槽——失联重置 session 槽时一并 abort 对应 ticker。
    keepalive_ticker: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    engine_pid: u32,
}

impl LiveConnectionProjection {
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
            reconnect_generation: Arc::new(AtomicU64::new(0)),
            events,
            stats_state: Arc::new(std::sync::Mutex::new(LiveStatsState::new())),
            logs,
            launch_session,
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

    async fn connect(
        &self,
        request: &ConnectRequest,
        ui_credentials: Option<UiCredentialPackage>,
        persist_credentials: bool,
    ) -> Result<OperationReply, Status> {
        let generation = self.reconnect_generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        self.reconnect_active.store(false, Ordering::Release);
        self.connect_in_generation(request, ui_credentials, persist_credentials, generation, true).await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "连接受理承载 attach-before-apply 的三条流挂接与入库节点，线性展开保持事实顺序可读"
    )]
    async fn connect_in_generation(
        &self,
        request: &ConnectRequest,
        ui_credentials: Option<UiCredentialPackage>,
        persist_credentials: bool,
        generation: u64,
        new_session: bool,
    ) -> Result<OperationReply, Status> {
        let intent = request
            .intent
            .as_ref()
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let lookup_key = intent
            .lookup_key
            .as_ref()
            .filter(|key| key.operation_id.len() == 16)
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let dispatch_started = std::time::Instant::now();
        network_diagnostics::request_snapshot(
            Arc::clone(self.logs.aggregator()), Arc::clone(&self.proxy_tun),
            DiagnosticContext::new(&lookup_key.operation_id, generation, self.reconnect_attempts.load(Ordering::Relaxed)),
            "connect_admitted",
        );
        // MAC-PROXY-16 S1 连接受理边界探测：此刻 Engine 尚未拉起/未 apply 隧道，
        // 不存在己方 utun，探测事实不含自身（无特权只读；失败 fail open 到 None）。
        self.proxy_tun.refresh();
        let (secret_payload, plan) =
            self.connect_payload_and_plan(ui_credentials, persist_credentials)?;
        let mut session = self.session.lock().await;
        if generation != self.reconnect_generation.load(Ordering::Acquire) {
            return Err(Status::cancelled("连接已被后续用户操作取消"));
        }
        if new_session {
            let config = lock_recover(&self.config);
            *lock_recover(&self.reconnect_session_policy) =
                Some(SessionReconnectPolicy::from_config(&config));
            self.reconnect_attempts.store(0, Ordering::Relaxed);
            self.reconnect_active.store(false, Ordering::Release);
        }
        // 复用前的 liveness 观察（MAC-LIFECYCLE-15 S1）：engine_pid 已死即丢弃旧会话，
        // 本次 Connect 走完整拉起路径。这是状态流终止事件驱动重置之外的第二道观察点
        // （覆盖 Engine 在无活动状态投影期间死亡的场景）。死会话的伴生 keepalive
        // ticker 一并 abort（W2：不留孤儿 ticker）。
        if session
            .as_ref()
            .is_some_and(|existing| !existing.is_alive())
        {
            *session = None;
            self.abort_keepalive_ticker().await;
        }
        if session.is_none() {
            // W3：连接传输判定唯一分叉（前端零逻辑；v3 §一 四态表）。服务在位
            // （端点 healthy 或已装）走常驻端点+修复阶梯；未装/不可判定走 oneshot
            // 一次性提权直拉（弹窗可能在此出现——对齐 PromptStart 的提权时机）。
            let new_session = match self.resolve_transport().await {
                ResolvedTransport::Service => {
                    LiveEngineSession::Live(self.service_engine_session_or_reject().await?)
                }
                ResolvedTransport::Oneshot => {
                    (self.launch_session)()
                        .await
                        .map_err(|error| engine_session_status(&error))?
                }
            };
            // W2.5：会话形态即 mode 事实（发布边界附加；服务形态="service"）。
            *lock_recover(&self.engine_mode) = new_session.mode();
            // W2 心跳：会话建立（session 槽位填充点）→ 拉起伴生 keepalive ticker
            //（首拍立即，经克隆通道发送；观察流任务不刷心跳——ticker 是唯一发送方）。
            let ticker = new_session
                .heartbeat_handle()
                .map(|heartbeat| spawn_keepalive_ticker(heartbeat, HEARTBEAT_PERIOD));
            {
                let mut ticker_slot = self.keepalive_ticker.lock().await;
                if let Some(previous) = ticker_slot.take() {
                    previous.abort();
                }
                *ticker_slot = ticker;
            }
            *session = Some(new_session);
            // MAC-OBS-13 S1：Engine 会话建立的真实节点入库（Core 自身事件）。
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
        let engine_pid = session.as_ref().map_or(0, LiveEngineSession::engine_pid);
        let connect_status_stream = session
            .as_mut()
            .ok_or_else(|| Status::unavailable(CONNECTION_ENGINE_SESSION_CODE))?
            .attach_connect_status()
            .await
            .map_err(|error| engine_session_status(&error))?;
        // W-MVP-13：同一已认证控制通道上的既有 `StreamStats` 接缝；在 Apply 前挂接，
        // 数据面建立后样本不丢。
        let engine_stats_stream = session
            .as_mut()
            .ok_or_else(|| Status::unavailable(CONNECTION_ENGINE_SESSION_CODE))?
            .attach_stats()
            .await
            .map_err(|error| engine_session_status(&error))?;
        // MAC-OBS-13 S1：同一已认证控制通道上的既有 `StreamLogs` 接缝；同样在
        // Apply 前挂接，管线启动期的 sink 事件不丢。每次 Connect 重挂即替换
        // Engine 侧推送通道（last-writer-wins），旧 ingest 任务随旧流 EOF 自行退出。
        let engine_logs_stream = session
            .as_mut()
            .ok_or_else(|| Status::unavailable(CONNECTION_ENGINE_SESSION_CODE))?
            .attach_logs()
            .await
            .map_err(|error| engine_session_status(&error))?;
        if generation != self.reconnect_generation.load(Ordering::Acquire) {
            return Err(Status::cancelled("连接已被后续用户操作取消"));
        }
        let reply = {
            let session = session
                .as_mut()
                .ok_or_else(|| Status::unavailable(CONNECTION_ENGINE_SESSION_CODE))?;
            let mut apply_key = lookup_key.clone();
            apply_key.method = exv_vpn_wire::generated::OperationMethod::ApplyTunnel as i32;
            session
                .apply_tunnel(exv_vpn_wire::generated::ApplyTunnelRequest {
                    lookup_key: Some(apply_key),
                    plan: Some(plan),
                    request_digest: intent.request_digest.clone(),
                    secret_payload,
                    windows_connection_mode:
                        exv_vpn_wire::generated::WindowsConnectionMode::Standard as i32,
                })
                .await
                .map_err(|error| engine_session_status(&error))?
        };
        let pending = match reply.result {
            Some(exv_vpn_wire::generated::apply_tunnel_reply::Result::Pending(pending))
                if pending.operation_id == lookup_key.operation_id =>
            {
                pending
            }
            _ => return Err(Status::internal(CONNECTION_ENGINE_SESSION_CODE)),
        };
        self.publish(pending_connect_snapshot(
            request,
            pending.operation_id.clone(),
        ));
        // 新 operation：重置统计状态机（旧 operation 的任务持有旧 Arc，互不影响），
        // 并登记 operation 身份供 `GetSnapshot` live 回放共用同一事实源（B-09①）。
        {
            let mut state = lock_recover(&self.stats_state);
            *state = LiveStatsState::new();
            state.begin_operation(pending.operation_id.clone());
        }
        // MAC-OBS-13 S1：Engine 日志流消费任务（纯单向写聚合存储；流 EOF/错误仅
        // 结束消费，不触碰状态投影——「关闭实时流不影响连接状态」）。
        self.spawn_logs_ingest(engine_logs_stream);
        self.logs.aggregator().append_core("debug", "kernel", "kernel.connect.dispatch_timing",
            "连接受理至 Engine 派发完成的宿主耗时", &[
                ("operation_id", lookup_key.operation_id.iter().map(|byte| format!("{byte:02x}")).collect()),
                ("connection_generation", generation.to_string()),
                ("dispatch_duration_ms", dispatch_started.elapsed().as_millis().to_string()),
                ("engine_pid", engine_pid.to_string()),
            ]);
        self.spawn_status_projection(
            connect_status_stream,
            engine_stats_stream,
            OperationProjection {
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
    /// `publish_stats` 语义）。样本经 [`EventBus::publish_stats`] 进总线——当前快照为
    /// Connected 时核心铸新 tick 并把携带样本的 `Snapshot` 重发给 `WatchEvents`
    /// 订阅者（事件 kind 由 Transition 对齐为 win32 的 Snapshot；前端两态都渲染快照
    /// 内容）；铸造后的样本写回缓存（B-09①）：`GetSnapshot` live 回放与已发布统计
    /// 一致。operation 终结（失败/Stop）后丢弃流，Engine 采样 task 随接收端 drop
    /// 自行退出，不留跨 operation 的统计发布者。
    fn spawn_stats_projection(
        &self,
        mut engine_stats_stream: EngineUpdateStream<exv_vpn_wire::generated::StatsEvent>,
        stats_state: Arc<std::sync::Mutex<LiveStatsState>>,
    ) {
        let events = Arc::clone(&self.events);
        tokio::spawn(async move {
            let mut traffic = TrafficSample::new();
            loop {
                let Ok(Some(sample)) = engine_stats_stream.message().await else {
                    break;
                };
                // 门控（Connected 且未终结）经 LiveStatsState 判定；`publish_stats`
                // 侧另有总线级 Connected 守卫（当前快照非 Connected 时不重发、不
                // 铸 tick），两层一致时才产生 UI 事件。
                let normalized = {
                    let mut state = lock_recover(&stats_state);
                    state.observe(&sample, &mut traffic, 0)
                };
                if let Some(stats) = normalized {
                    let published = events.publish_stats(stats);
                    // 铸造后的样本（含 sample_tick）同步回缓存（B-09①）；终结后
                    // 不复活样本（cache_minted 只在 live 时写入）。
                    lock_recover(&stats_state).cache_minted(published);
                }
                if !lock_recover(&stats_state).is_live() {
                    break;
                }
            }
        });
    }

    /// 消费 Engine 状态流与统计流，把真实事实投影为 UI 事件。
    ///
    /// 只接受当前 operation 的事件；阶段事件投影为对应 `Connecting` 快照；
    /// coarse Connected 投影为 `Connected` 快照（附带当时的最新统计）；携带
    /// `VpnError` 的事件投影为 Engine 自身的 `FailedClean` 终态并结束本次投影。
    /// 统计流（W-MVP-13）由 [`Self::spawn_stats_projection`] 消费：经
    /// [`crate::stats::LiveStatsState`] 门控，样本沿用既有 `WatchEvents` 通道的
    /// wire `snapshot.stats` 字段到达 UI，不新增第二条 Core→UI 通道。
    ///
    /// MAC-LIFECYCLE-15 S1（Engine 失联终态）：状态流 EOF/transport 错误是 Engine
    /// 失联的事件驱动观察点。此时若本次 operation 仍活着（未收到 Engine 失败终态、
    /// 未显式 Stop），Core 发布 typed `FailedClean` 失联终态（`engine_lost_vpn_error`，
    /// 不伪造 Engine 错误码）、终结统计发布，并按 pid 身份把死 session 槽位重置为
    /// `None`——下一次 Connect 走完整拉起路径（服务代理启动新 Engine）。
    ///
    /// W3-2/P3（数据面自动重连）：Failed 终态在**事件进总线前**判定是否触发重连
    /// （双轨：`is_retryable_disconnect` 主轨 + 本连接生命周期「曾 Connected」守卫轨
    /// [`Self::reconnect_ever_connected`]），经信号通道异步通知单 worker；**Engine
    /// 失联（流 EOF）路径一期明确不自动重连**——EOF 终态的 `engine_lost_vpn_error`
    /// 携带 `ProtocolSession + UseNewOperation`（非可重试标记），且 Engine 进程已死，
    /// 自动重连须经服务代理 fork 全新 root Engine，风暴风险不可控（对齐 win32 §8
    /// 冻结边界；用户手动 Connect 仍走完整拉起路径）。
    #[allow(
        clippy::too_many_lines,
        reason = "状态投影承载阶段/终态/失联三类事实转换与对应入库节点，线性展开保持事实顺序可读"
    )]
    fn spawn_status_projection(
        &self,
        mut connect_status_stream: EngineUpdateStream<exv_vpn_wire::generated::ConnectStatusEvent>,
        engine_stats_stream: EngineUpdateStream<exv_vpn_wire::generated::StatsEvent>,
        projection: OperationProjection,
    ) {
        use exv_vpn_wire::generated::{Attempt, ConnectingState, runtime_snapshot};
        let OperationProjection {
            stats_state,
            runtime_epoch,
            operation_id,
            intent,
            session_slot,
            keepalive_ticker,
            engine_pid,
        } = projection;
        let events = Arc::clone(&self.events);
        let proxy_tun = Arc::clone(&self.proxy_tun);
        // W3-2/P3：重连状态/信号（与投影共享原子 + 信号通道）。
        let reconnect_attempts = Arc::clone(&self.reconnect_attempts);
        let reconnect_active = Arc::clone(&self.reconnect_active);
        let reconnect_ever_connected = Arc::clone(&self.reconnect_ever_connected);
        let reconnect_tx = Arc::clone(&self.reconnect_tx);
        let reconnect_generation = Arc::clone(&self.reconnect_generation);
        // 该投影只可收口自己所属的连接 generation；用户已开始新连接时，迟到失败
        // 事件绝不能 Stop 新管线。
        let projection_generation = reconnect_generation.load(Ordering::Acquire);
        let reconnect_session_policy = Arc::clone(&self.reconnect_session_policy);
        // MAC-OBS-13 S1：连接期 Core 自身事实（Connected/失败/失联）入库用。
        let logs = Arc::clone(self.logs.aggregator());
        self.spawn_stats_projection(engine_stats_stream, Arc::clone(&stats_state));
        let diagnostic_context = DiagnosticContext::new(
            &operation_id,
            reconnect_generation.load(Ordering::Acquire),
            reconnect_attempts.load(Ordering::Relaxed),
        );
        tokio::spawn(async move {
            let mut network_monitor = NetworkMonitor::default();

            // 快照出向边界统一附加最近代理 TUN 检测（MAC-PROXY-16 S1；mirror stats
            // 方案 A；连接期沿用连接受理边界探测的最近结果，不重新探测）与自动重连
            // 状态（只读会话冻结策略，不读取 ConfigSet 更新后的保存设置）。
            // tick 由总线在 publish_gate 内铸造（W3-1/P1 S2）。
            let publish = |snapshot: RuntimeSnapshot| {
                events.publish(
                    RuntimeEventKind::Transition,
                    snapshot_with_reconnect(
                        snapshot_with_environment(snapshot, &proxy_tun),
                        session_reconnect_status(
                            &reconnect_session_policy,
                            &reconnect_attempts,
                            &reconnect_active,
                        ),
                    ),
                );
            };
            // 退出方式事实（W3-2/P3）：EOF/transport 失败 = Engine 失联（保持
            // false）；Engine Failed 终态 break = Engine 进程仍存活（置 true——
            // 数据面掉线重连/手动 Connect 复用会话，不重新拉起）。
            let mut engine_alive_at_exit = false;
            loop {
                let Ok(event) = connect_status_stream.message().await else {
                    break;
                };
                let Some(event) = event else {
                    break;
                };
                if event.operation_id != operation_id {
                    continue;
                }
                let status_received = std::time::Instant::now();
                proxy_tun.set_owned_interface(
                    event.own_tunnel_if_index,
                    event.coarse_phase != wire::StatsPhase::Connecting as i32,
                );
                // Engine 把 coarse 置为 Connected 且阶段为 StartingDataPlane 时，
                // 数据面已真实运行（Connected 充要条件由 Engine 保证）。
                if event.coarse_phase == exv_vpn_wire::generated::StatsPhase::Connected as i32 {
                    network_monitor.start(
                        Arc::clone(&logs),
                        Arc::clone(&proxy_tun),
                        diagnostic_context.clone(),
                    );
                    // W3-2/P3：Connected = 一次真实成功连接（新连接生命周期）——
                    // 清零 per-connection 重连计数 + 释放重连在途标记（预算/退避序列
                    // 回到起点；镜像 win32 :2042-2044），并置位生命周期级「曾连接」
                    // 守卫事实。先清零再发布——本条快照携带的 `ReconnectStatus` 即为
                    // 清零后的事实。
                    reconnect_attempts.store(0, Ordering::Relaxed);
                    reconnect_active.store(false, Ordering::Release);
                    reconnect_ever_connected.store(true, Ordering::Release);
                    // B-09②：连接期缓存的样本带着 Engine 相位（如 Idle）与未铸造的
                    // tick 0——直接附加会被前端 `hasUsableConnectedSample` 守卫拒绝
                    // （统计首帧延迟约 1s）。发布经 `publish_composing`：铸造后的
                    // tick 传入快照组装闭包（总线 publish_gate 内原子发生），
                    // `connected_baseline` 以该 tick 重铸样本相位——`sample_tick` 与
                    // 事件 `monotonic_tick` 同轴同值（与统计发布一致）。
                    events.publish_composing(RuntimeEventKind::Transition, |tick_value| {
                        let latest = {
                            let mut state = lock_recover(&stats_state);
                            let latest = state.connected_baseline(tick_value);
                            state.mark_connected();
                            latest
                        };
                        snapshot_with_reconnect(
                            snapshot_with_environment(
                                connected_snapshot(
                                    &operation_id,
                                    latest,
                                    event.session_established_at_ms,
                                ),
                                &proxy_tun,
                            ),
                            session_reconnect_status(
                                &reconnect_session_policy,
                                &reconnect_attempts,
                                &reconnect_active,
                            ),
                        )
                    });
                    // MAC-OBS-13 S1：连接终态（Connected）真实节点入库。
                    logs.append_core(
                        "info",
                        "core",
                        "CONNECTED",
                        "data plane connected",
                        &[("pid", engine_pid.to_string())],
                    );
                    continue;
                }
                if let Some(error) = event.error {
                    // Engine 自身的失败终态：仅当 operation 仍活着时发布，避免
                    // 显式 Stop 已回可重试态后被迟到的失败事件覆盖。
                    if lock_recover(&stats_state).is_live() {
                        lock_recover(&stats_state).mark_finished();
                        // W3-2/P3：任何 Failed 终态都释放重连在途标记——本次重连
                        // 尝试已结束（镜像 win32 :2058-2066）。
                        reconnect_active.store(false, Ordering::Release);
                        // MAC-OBS-13 S1：Engine 失败终态真实节点入库（typed 码，
                        // 不含凭据/主机名）。
                        logs.append_core(
                            "error",
                            "core",
                            "ENGINE_FAILED",
                            "engine reported failure",
                            &[
                                ("pid", engine_pid.to_string()),
                                ("code", error.code.to_string()),
                            ],
                        );
                        // W3-2/P3 触发双轨（事件进 bus 前判定）：主轨 = 可重试数据面
                        // 掉线标记（engine `session_lost_vpn_error` 升级后的
                        // stage=DataPlane + retry=RetrySameOperation）；守卫轨 = 本
                        // 连接生命周期内曾到达 Connected（`reconnect_ever_connected`
                        // ——链式重连尝试的掉线继续触发，预算/退避语义由此成立；
                        // 连接期失败因引擎侧标记非 RetrySameOperation 被主轨拦下，
                        // 守卫轨兜底拦截伪造标记的连接期掉线）。两轨同时成立才异步
                        // 通知重连 worker（worker 内再判 auto_reconnect 开关/预算/
                        // 在途）；worker 未拉起时信号丢弃（无重连语义）。
                        let retryable_drop = is_retryable_disconnect(&error)
                            && reconnect_ever_connected.load(Ordering::Acquire);
                        network_monitor.record_and_stop(
                            &logs,
                            &diagnostic_context,
                            "engine_failed",
                        );
                        if retryable_drop {
                            // 该 Engine 失败保留空闲 utun/控制专用路由供可能的自动重连；
                            // 在 Core 真正 Stop 并得到 Stopped 前不得发布 FailedClean。
                            publish(pending_stop_snapshot(
                                &StopRequest {
                                    intent: Some(reconnect_stop_intent()),
                                },
                                operation_id.clone(),
                            ));
                            logs.append_core(
                                "info",
                                "kernel",
                                "kernel.reconnect.trigger",
                                "retryable data-plane drop detected; signaling reconnect worker",
                                &[],
                            );
                            if let Some(tx) = lock_recover(&reconnect_tx).as_ref() {
                                let _ = tx.send(reconnect_generation.load(Ordering::Acquire));
                            }
                            logs.append_core(
                                "debug",
                                "kernel",
                                "kernel.reconnect.trigger_timing",
                                "Engine 状态接收至重连信号的宿主耗时",
                                &[
                                    (
                                        "operation_id",
                                        operation_id
                                            .iter()
                                            .map(|byte| format!("{byte:02x}"))
                                            .collect(),
                                    ),
                                    (
                                        "status_to_signal_ms",
                                        status_received.elapsed().as_millis().to_string(),
                                    ),
                                    (
                                        "reconnect_attempt",
                                        reconnect_attempts.load(Ordering::Relaxed).to_string(),
                                    ),
                                ],
                            );
                        } else {
                            // 非可重试失败同样可能发生在 Engine 已写控制 /32、随后平台
                            // 早退的窗口。先以真实 StopTunnel 收口；只有 Engine 确认
                            // Stopped 才能对 UI 发布 FailedClean。
                            let stop_intent = reconnect_stop_intent();
                            let mut stop_key = stop_intent
                                .lookup_key
                                .clone()
                                .expect("generated Stop intent has lookup key");
                            stop_key.method =
                                exv_vpn_wire::generated::OperationMethod::StopTunnel as i32;
                            publish(pending_stop_snapshot(
                                &StopRequest {
                                    intent: Some(stop_intent.clone()),
                                },
                                operation_id.clone(),
                            ));
                            let stopped = {
                                let mut session = session_slot.lock().await;
                                match session.as_mut() {
                                    Some(engine)
                                        if projection_generation
                                            == reconnect_generation.load(Ordering::Acquire)
                                            && engine.is_alive() => engine
                                        .stop_tunnel(exv_vpn_wire::generated::StopTunnelRequest {
                                            lookup_key: Some(stop_key),
                                            request_digest: stop_intent.request_digest,
                                        })
                                        .await
                                        .map_or(false, |reply| matches!(
                                            reply.result,
                                            Some(exv_vpn_wire::generated::stop_tunnel_reply::Result::Stopped(_))
                                        )),
                                    _ => false,
                                }
                            };
                            if stopped
                                && projection_generation == reconnect_generation.load(Ordering::Acquire)
                            {
                                publish(failed_clean_snapshot(&operation_id, error));
                            } else {
                                logs.append_core(
                                    "error",
                                    "core",
                                    "NONRETRY_CLEANUP_FAILED",
                                    "non-retryable failure cleanup did not reach Engine Stopped",
                                    &[("pid", engine_pid.to_string())],
                                );
                            }
                        }
                        // Engine 仍存活（失败后 attempt 槽已清空、会话可复用）：
                        // 不重置 session 槽位（W3-2/P3：数据面掉线重连复用会话）。
                        engine_alive_at_exit = true;
                        break;
                    }
                    // 迟到失败事件对应的 operation 已经结束，不得把它投影回 Connecting。
                    continue;
                }
                publish(RuntimeSnapshot {
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
                            runtime_epoch: runtime_epoch.clone(),
                            attempt_id: operation_id.clone(),
                            intent: intent.clone(),
                            prior_error: None,
                        }),
                        phase: event.connect_phase,
                    })),
                });
            }
            // 状态流终止（EOF/transport 错误）＝Engine 失联的事件驱动观察点：
            // operation 仍活着时发布 Core 观察到的 typed 失联终态并终结统计发布。
            // W3-2/P3：失联终态也是一次 Failed 终态——释放重连在途标记；但**不触发
            // 自动重连**（见函数文档：服务代理 fork 风暴，一期明确排除；恢复路径 =
            // 用户手动 Connect 走完整拉起）。Engine Failed 终态退出（Engine 存活）
            // 不进入失联块——会话保留复用，统计状态在终态处置时已终结。
            if !engine_alive_at_exit {
                if lock_recover(&stats_state).is_live() {
                    lock_recover(&stats_state).mark_finished();
                    reconnect_active.store(false, Ordering::Release);
                    // MAC-OBS-13 S1：Engine 失联真实节点入库。
                    network_monitor.record_and_stop(
                        &logs,
                        &diagnostic_context,
                        "stream_disconnect",
                    );
                    logs.append_core(
                        "error",
                        "core",
                        "ENGINE_LOST",
                        "engine status stream terminated while operation live",
                        &[("pid", engine_pid.to_string())],
                    );
                    publish(failed_clean_snapshot(
                        &operation_id,
                        engine_lost_vpn_error(),
                    ));
                }
                // 死 session 槽位重置：仅当槽位仍是产生本状态流的那个 Engine（pid 身份）
                // 时才清空——防止迟到的旧投影把并发重连已换入的新会话误删。EOF =
                // Engine 失联证据，槽位随投影退出清空；Engine Failed 终态（Engine
                // 存活）不重置——复用会话是 P3 掉线重连的生产路径。
                let mut slot = session_slot.lock().await;
                if slot.as_ref().map(LiveEngineSession::engine_pid) == Some(engine_pid) {
                    *slot = None;
                }
                drop(slot);
                // W2 心跳：失联会话的伴生 keepalive ticker 一并 abort——旧 ticker 持
                // 的是死通道，继续空转只会打死引擎的误判面（新会话建立时会 spawn
                // 新 ticker），不留孤儿。
                if let Some(ticker) = keepalive_ticker.lock().await.take() {
                    ticker.abort();
                }
            }
        });
    }

    async fn stop(&self, request: &StopRequest) -> Result<OperationReply, Status> {
        let generation = self.reconnect_generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        self.reconnect_active.store(false, Ordering::Release);
        self.stop_in_generation(request, generation, false).await
    }

    async fn stop_in_generation(&self, request: &StopRequest, generation: u64, preserve_reconnect_policy: bool) -> Result<OperationReply, Status> {
        let intent = request
            .intent
            .as_ref()
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let lookup_key = intent
            .lookup_key
            .as_ref()
            .filter(|key| key.operation_id.len() == 16)
            .ok_or_else(|| Status::invalid_argument(CONNECTION_ENGINE_SESSION_CODE))?;
        let mut session = self.session.lock().await;
        if generation != self.reconnect_generation.load(Ordering::Acquire) {
            return Err(Status::cancelled("停止已被后续用户操作取消"));
        }
        // 唤醒恢复只收束旧隧道，仍属于原连接会话；用户断开才结束冻结策略。
        if !preserve_reconnect_policy {
            *lock_recover(&self.reconnect_session_policy) = None;
        }
        let operation_id = lookup_key.operation_id.clone();
        // 死 session liveness 观察（MAC-LIFECYCLE-15 S1）：Engine 已死（或会话已因
        // 状态流终止重置）时，网络资源已由 Engine 自退/内核回收，Core 不再欠网络
        // 清理——发布 typed 失联终态并回可重试态，而非笼统 unavailable
        // （W-RECOVERY-01：清理后可重试，不自动重连）。
        if !session.as_ref().is_some_and(LiveEngineSession::is_alive) {
            *session = None;
            // W2 心跳：死会话清槽 → 伴生 ticker 一并 abort（不留孤儿）。
            self.abort_keepalive_ticker().await;
            lock_recover(&self.stats_state).mark_finished();
            // W3-2/P3：终态释放重连在途标记（若重连尝试在途时 Engine 死亡）。
            self.reconnect_active.store(false, Ordering::Release);
            self.publish(failed_clean_snapshot(
                &operation_id,
                engine_lost_vpn_error(),
            ));
            return Ok(OperationReply { terminal: None });
        }
        self.publish(pending_stop_snapshot(request, operation_id.clone()));
        let reply = {
            let session = session
                .as_mut()
                .ok_or_else(|| Status::failed_precondition(CONNECTION_ENGINE_SESSION_CODE))?;
            session
                .attach_connect_status()
                .await
                .map_err(|error| engine_session_status(&error))?;
            let mut stop_key = lookup_key.clone();
            stop_key.method = exv_vpn_wire::generated::OperationMethod::StopTunnel as i32;
            session
                .stop_tunnel(exv_vpn_wire::generated::StopTunnelRequest {
                    lookup_key: Some(stop_key),
                    request_digest: intent.request_digest.clone(),
                })
                .await
                .map_err(|error| engine_session_status(&error))?
        };
        if !matches!(
            reply.result,
            Some(exv_vpn_wire::generated::stop_tunnel_reply::Result::Stopped(
                _
            ))
        ) {
            return Err(Status::internal(CONNECTION_ENGINE_SESSION_CODE));
        }
        // Engine 已答复 Stopped（本次资源回收完成）：Core 发布真实 Idle 终态，
        // UI 由此回到可重试状态。Engine 自身不发布 Idle 事件。终态 Idle 快照的
        // `operation_id` 为空（win32 同款 `event.operation_id = snapshot.operation_id`）。
        // W3-2/P3：显式 Stop 是终态——释放重连在途标记（对齐 win32 Stopped 终态；
        // `reconnect_attempts` 不在此清零——计数只在 Connected 成功时清零，win32
        // 同款语义）。守卫事实随之清零：新连接生命周期从「未连接」起算。
        self.reconnect_active.store(false, Ordering::Release);
        self.reconnect_ever_connected.store(false, Ordering::Release);
        // MAC-OBS-13 S1：断开终态真实节点入库。
        self.logs.aggregator().append_core(
            "info",
            "core",
            "DISCONNECTED",
            "tunnel stopped and resources reclaimed",
            &[],
        );
        self.publish(idle_snapshot());
        // MAC-PROXY-16 S1 断开终态边界探测：Engine 已回收己方 utun，此刻重新探测
        // 能如实反映断开后的共存事实（本条 idle 事件沿用断开前最近结果）。
        self.proxy_tun.set_owned_interface(0, true);
        self.proxy_tun.refresh();
        // W-MVP-13：Stop 终结当前 operation 的统计发布（统计任务随门控退出）。
        lock_recover(&self.stats_state).mark_finished();
        Ok(OperationReply { terminal: None })
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

    /// W2.5 服务形态连接编排：facts healthy → 直连固定端点；down → 修复阶梯
    /// （daemon socket 可连 → `EnsureEngine` 帧免提权拉起；daemon 不在 → 经
    /// `service_lifecycle` 既有 osascript `start`——该动词已扩展为同时确保 engine
    /// job bootstrap）→ 有界等端点 → 直连。
    ///
    /// 任一步失败返回 `None`（W3 起调用方 [`Self::service_engine_session_or_reject`]
    /// 据此发出 typed 拒绝——E8 后不再回退会话引擎路径）。
    async fn try_service_engine_session(&self) -> Option<EngineControlSession> {
        let facts = (self.service_engine.probe)().await;
        if !crate::service_status::engine_endpoint_healthy(&facts) {
            let daemon_socket_connectable =
                (self.service_lifecycle.probe())().await.socket_connectable;
            let repaired = if daemon_socket_connectable {
                (self.service_engine.ensure_engine)().await
            } else {
                self.service_lifecycle.ensure_running().await.is_ok()
            };
            if !repaired {
                return None;
            }
            if !self.wait_engine_endpoint().await {
                return None;
            }
        }
        (self.service_engine.connect)().await.ok()
    }

    /// W2.5：有界轮询 engine 端点 healthy（默认 15s/250ms；接缝注入毫秒级钉
    /// 节奏）。
    async fn wait_engine_endpoint(&self) -> bool {
        let deadline = tokio::time::Instant::now() + self.service_engine.wait_bound;
        loop {
            let facts = (self.service_engine.probe)().await;
            if crate::service_status::engine_endpoint_healthy(&facts) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(self.service_engine.wait_poll).await;
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

    /// W3：Service 传输的会话获取——直连常驻端点；不可用时修复阶梯（daemon 在→
    /// `EnsureEngine` 帧免提权；不在→osascript `start`）后有界重试。阶梯穷尽仍
    /// 不可用 → `service_not_running|` typed 拒绝（壳层恢复 modal 出口；E8 后
    /// 不再回退会话引擎路径），并刷新服务状态缓存供快照。
    async fn service_engine_session_or_reject(
        &self,
    ) -> Result<EngineControlSession, Status> {
        if let Some(session) = self.try_service_engine_session().await {
            return Ok(session);
        }
        let facts = (self.service_lifecycle.probe())().await;
        *lock_recover(&self.service_status_cache) =
            Some(crate::service_status::wire_service_status(&facts));
        Err(Status::failed_precondition(format!(
            "{SERVICE_NOT_RUNNING_PREFIX}服务引擎经修复阶梯后仍不可用（常驻端点未能恢复）"
        )))
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
    }

    /// W1-C（P7）双分支（win32 `execute_connect` 对应）：UI 一次性凭据存在 → 用户名
    /// 仅在 persist 时保存；临时连接使用内存副本的 UI username/password + config
    /// 其余字段（`server`/`routes`/`mtu`/`user_agent`）组装 envelope；UI 凭据不存在 →
    /// 维持 `build_saved_connect_envelope` 磁盘回退。修复连接弹窗死循环：此前 payload
    /// 被直接 zeroize 丢弃、恒走磁盘回退。
    fn connect_payload_and_plan(
        &self,
        ui_credentials: Option<UiCredentialPackage>,
        persist_credentials: bool,
    ) -> Result<(Vec<u8>, exv_vpn_wire::generated::TunnelPlan), Status> {
        let mut config = lock_recover(&self.config);
        let payload = match ui_credentials {
            Some(mut credentials) => {
                if persist_credentials {
                    config.apply_ui_credentials_and_save(&credentials.username, Some(credentials.password.as_str()))
                        .map_err(ui_persist_status)?;
                }
                let temporary = config.with_temporary_username(&credentials.username).map_err(ui_persist_status)?;
                // 密码 ownership 移入 envelope（零化类型）；包内残留副本由 Drop 清零。
                let password = std::mem::take(&mut credentials.password);
                build_connect_envelope(&temporary, password)
                    .map_err(ui_envelope_status)?
                    .encode()
                    .to_vec()
            }
            None => build_saved_connect_envelope(&config)
                .map_err(|error| saved_envelope_status(&error))?
                .encode()
                .to_vec(),
        };
        Ok((
            payload,
            exv_vpn_wire::generated::TunnelPlan {
                mtu: config.mtu(),
                ..Default::default()
            },
        ))
    }

    /// 经共享总线发布过渡事件（tick 由总线铸造；快照出向边界附加最近 proxy TUN
    /// 检测、自动重连状态与 P4 v2 服务状态缓存伴生值——后两者均为内存直读，零
    /// 磁盘零探测）。事件 `operation_id` 镜像快照 `operation_id`（win32 同款）。
    fn publish(&self, mut snapshot: RuntimeSnapshot) {
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

    /// W3-2/P3：向重连 worker 发送一条掉线信号（worker 未拉起时 `None`，信号丢弃
    /// ——无重连语义；状态投影触发点与测试注入共用）。
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "直接注入信号仅由串行防重入测试使用；生产信号全部来自状态投影触发点"
        )
    )]
    fn signal_reconnect(&self) {
        if let Some(tx) = lock_recover(&self.reconnect_tx).as_ref() {
            let _ = tx.send(self.reconnect_generation.load(Ordering::Acquire));
        }
    }

    /// W3-2/P3：消费一条掉线信号，驱动一次自动重连（镜像 win32 `drive_reconnect`）。
    ///
    /// 单 worker 串行消费 + 在途标记（`reconnect_active`）双重防重入：上一次重连
    /// 已派发、终态未到时新信号直接跳过（`kernel.reconnect.skip_in_flight`）。重连
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
    async fn drive_reconnect(&self) {
        let reconnect_started = std::time::Instant::now();
        let generation = self.reconnect_generation.load(Ordering::Acquire);
        // 防重入：上一次重连尝试仍在途（已派发、终态未到）→ 不重复触发。
        if self.reconnect_active.load(Ordering::Acquire) {
            self.logs
                .aggregator()
                .append_core(
                    "info",
                    "kernel",
                    "kernel.reconnect.skip_in_flight",
                    "auto reconnect skipped: a reconnect attempt is still in flight",
                    &[],
                );
            return;
        }
        // 当前连接的实际重连决策只读受理时冻结的策略。
        let Some(SessionReconnectPolicy {
            enabled,
            max_attempts: max,
            backoff,
        }) = *lock_recover(&self.reconnect_session_policy)
        else {
            return;
        };
        if !enabled {
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
        // 重连**必然再次提权**时拦截：上次会话形态确为 oneshot ∧ 会话槽已空/已死
        // （engine 失联或已终结）∧ 传输仍解析为 Oneshot（用户未在此期间装服务）。
        // 会话槽仍有活会话时重连走复用（零提权，放行）；形态未知（""=夹具/未建立）
        // 不断言 oneshot 语义（放行）。稳定码只进日志；UI 的失败态「重试」按钮
        // 即手动重连出口。
        let last_mode_oneshot = *lock_recover(&self.engine_mode) == "oneshot";
        let needs_launch = self
            .session
            .lock()
            .await
            .as_ref()
            .is_none_or(|existing| !existing.is_alive());
        if generation != self.reconnect_generation.load(Ordering::Acquire) { return; }
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
            self.finish_reconnect_and_cleanup(generation, reconnect_exhausted_vpn_error())
                .await;
            return;
        }

        // 重连执行：占用在途标记 + 计数 + 复用 connect（凭据从磁盘重新组装）。
        self.reconnect_active.store(true, Ordering::Release);
        self.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
        let attempt = attempts + 1;
        self.logs
            .aggregator()
            .append_core(
                "info",
                "kernel",
                "kernel.reconnect.attempt",
                &format!(
                    "auto reconnect attempt {attempt} (max={})",
                    if max == 0 {
                        "unlimited".to_string()
                    } else {
                        max.to_string()
                    }
                ),
                &[],
            );
        // 自动重连只允许读取已持久化的凭据；绝不复用先前 UI 的一次性密码（connect
        // 侧 ui_credentials=None）。退避开启时在派发前 sleep（占用在途标记之后——
        // 睡眠期间新掉线信号按防重入跳过；关闭（默认）→ 零 sleep，立即重连）。
        if backoff {
            let delay = reconnect_backoff_delay(
                self.reconnect_backoff_base,
                self.reconnect_backoff_cap,
                attempt,
            );
            self.logs
                .aggregator()
                .append_core(
                    "info",
                    "kernel",
                    "kernel.reconnect.backoff",
                    &format!(
                        "auto reconnect backoff: sleeping {} ms before attempt {attempt}",
                        delay.as_millis()
                    ),
                    &[],
                );
            tokio::time::sleep(delay).await;
        }
        // 退避期间用户可能已断开或重新连接，旧工作不能覆盖新意图。
        if generation != self.reconnect_generation.load(Ordering::Acquire) { return; }
        let intent = reconnect_intent();
        let operation_id = intent
            .lookup_key
            .as_ref()
            .map_or_else(Vec::new, |key| key.operation_id.clone());
        let request = ConnectRequest {
            intent: Some(intent),
            secret_payload: Vec::new(),
        };
        let dispatch_started = std::time::Instant::now();
        let result = self.connect_in_generation(&request, None, false, generation, false).await;
        self.logs.aggregator().append_core("debug", "kernel", "kernel.reconnect.dispatch_timing",
            "自动重连退避及派发耗时", &[
                ("operation_id", operation_id.iter().map(|byte| format!("{byte:02x}")).collect()),
                ("connection_generation", generation.to_string()), ("reconnect_attempt", attempt.to_string()),
                ("total_duration_ms", reconnect_started.elapsed().as_millis().to_string()),
                ("dispatch_duration_ms", dispatch_started.elapsed().as_millis().to_string()),
                ("effective_auto_reconnect", enabled.to_string()), ("effective_backoff", backoff.to_string()),
                ("effective_max_attempts", max.to_string()), ("dispatch_ok", result.is_ok().to_string()),
            ]);
        if generation != self.reconnect_generation.load(Ordering::Acquire) { return; }
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
                        &[],
                    );
            }
            Err(status) => {
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
        if generation != self.reconnect_generation.load(Ordering::Acquire) {
            return;
        }
        let intent = reconnect_stop_intent();
        // StopTunnel 的幂等键属于内部清理；对 UI 的失败仍属于刚结束的连接操作。
        let operation_id = self.events.current_snapshot()
            .map(|snapshot| snapshot.operation_id).unwrap_or_default();
        let request = StopRequest { intent: Some(intent) };
        match self.stop_in_generation(&request, generation, false).await {
            Ok(_) if generation == self.reconnect_generation.load(Ordering::Acquire) => {
                self.publish(failed_clean_snapshot(&operation_id, terminal_error));
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

    /// 用户在自动重连等待期间关闭开关时，取消旧 generation 并同步释放 Engine 暂存
    /// 的资源；连接已正常 Connected 时仅保存下次会话设置，不主动断开。
    async fn cancel_waiting_reconnect_after_setting_disabled(&self) {
        let waiting = self.reconnect_active.load(Ordering::Acquire)
            || (self.reconnect_ever_connected.load(Ordering::Acquire)
                && !lock_recover(&self.stats_state).is_live());
        if !waiting {
            return;
        }
        let generation = self.reconnect_generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        self.reconnect_active.store(false, Ordering::Release);
        if let Some(policy) = lock_recover(&self.reconnect_session_policy).as_mut() {
            policy.enabled = false;
        }
        self.finish_reconnect_and_cleanup(generation, reconnect_disabled_vpn_error())
            .await;
    }

    /// 拉起自动重连 worker（W3-2/P3）：换入绑定本 worker 接收端的发送端，此后状态
    /// 投影的掉线信号经该通道到达本 worker；单 worker 串行消费（天然防并发重连）。
    /// worker 持投影 `Arc` 克隆（轻量共享句柄，内部全部 `Arc`）。返回 `JoinHandle`
    /// 供测试 abort（取消退避 sleep）；darwin Core 随 UI session 结束整体退出，生产
    /// 路径 detach（进程 teardown 取消一切，无需 win32 常驻服务的显式 abort 纪律）。
    fn spawn_reconnect_worker(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        *lock_recover(&self.reconnect_tx) = Some(tx);
        tokio::spawn(async move {
            while let Some(generation) = rx.recv().await {
                if generation == self.reconnect_generation.load(Ordering::Acquire) {
                    self.drive_reconnect().await;
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
            connected_snapshot(&operation_id, latest, 0),
            self.reconnect_status(),
        ))
    }
}

fn saved_envelope_status(error: &SavedConnectEnvelopeError) -> Status {
    match error {
        SavedConnectEnvelopeError::Envelope(
            exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError::InvalidUsername
            | exv_vpn_darwin_ipc::connect_envelope::ConnectEnvelopeError::InvalidPassword,
        ) => Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE),
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
) -> Result<Option<UiCredentialPackage>, Status> {
    let mut payload = std::mem::take(&mut request.secret_payload);
    if payload.is_empty() {
        return Ok(None);
    }
    let parsed = parse_ui_secret_payload(&payload)
        .map_err(|_| Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE));
    payload.zeroize();
    let mut credentials = parsed?;
    if credentials.version != SECRET_PAYLOAD_VERSION
        || credentials.username.trim().is_empty()
        || credentials.password.is_empty()
    {
        credentials.zeroize();
        return Err(Status::invalid_argument(
            CONNECTION_CREDENTIALS_INVALID_CODE,
        ));
    }
    Ok(Some(credentials))
}

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
    match error {
        LifecycleError::Denied(_) => Status::failed_precondition(format!(
            "{ELEVATION_DENIED_CODE}: 用户取消了管理员授权，服务操作未执行"
        )),
        LifecycleError::Failed(_, detail) => Status::failed_precondition(format!(
            "{ELEVATION_FAILED_CODE}: 服务操作提权执行失败：{detail}"
        )),
        LifecycleError::NotReady(_) => Status::failed_precondition(format!(
            "{SERVICE_NOT_READY_CODE}: 服务操作已提交但等待就绪超时，请稍后在服务面板重新查询状态"
        )),
        LifecycleError::NotInstalled(_) => Status::failed_precondition(format!(
            "{SERVICE_NOT_INSTALLED_CODE}: 服务未安装，请先安装服务"
        )),
        LifecycleError::SourceMissing(_) => Status::failed_precondition(format!(
            "{SERVICE_SOURCE_MISSING_CODE}: 找不到服务安装源二进制（应用包内未随附且本机无已安装副本）"
        )),
    }
}

/// [`LifecycleError`] 的 `SERVICE_ACTION` 日志 detail 字段：稳定码；提权失败
/// （`Failed`）再附增强 stderr 细节（service agent 稳定码 + artifact `contract_key` +
/// 不符维度，`elevation_failure_detail` 已有界）。全部固定常量与系统文本，无秘密。
fn lifecycle_failure_detail(error: &LifecycleError) -> String {
    match error {
        LifecycleError::Denied(_) => ELEVATION_DENIED_CODE.to_string(),
        LifecycleError::Failed(_, detail) => format!("{ELEVATION_FAILED_CODE}: {detail}"),
        LifecycleError::NotReady(_) => SERVICE_NOT_READY_CODE.to_string(),
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

#[tonic::async_trait]
impl KernelControl for DarwinKernelControlService {
    type WatchEventsStream = WatchEventsStream;

    async fn connect(
        &self,
        request: Request<ConnectRequest>,
    ) -> Result<Response<OperationReply>, Status> {
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
            live.connect(&request, ui_credentials, persist_credentials)
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
        request.response_payload.zeroize();
        prep_only()
    }

    async fn stop(
        &self,
        request: Request<StopRequest>,
    ) -> Result<Response<OperationReply>, Status> {
        let request = request.into_inner();
        if let Some(live) = self.live_connection.as_deref() {
            live.stop(&request).await.map(Response::new)
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
            snapshot_with_reconnect(snapshot_with_environment(base, proxy_tun), reconnect)
        } else {
            snapshot_with_environment(idle_snapshot(), proxy_tun)
        };
        snapshot.service_status = Some(service_status);
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

    async fn logs_list(
        &self,
        request: Request<LogsListRequest>,
    ) -> Result<Response<LogsListReply>, Status> {
        // MAC-OBS-13 S1：真实历史分页（尾部/增量/limit clamp/filter，语义见
        // log_control；条目经聚合存储入库边界脱敏）。
        Ok(Response::new(self.logs.list(&request.into_inner())))
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
        // W3-3/P2b：读路径不 gate，但每次调用都经卫生层**重读磁盘**（win32
        // `config_get` 同款）：missing/损坏 → 默认 bootstrap 落盘 +
        // `requires_quick_start=true`；合法对象 → hydrate 已知字段保留未知字段 +
        // 幂等原子写回 + false——`requires_quick_start` 由此真实产码（此前恒 false）。
        let config_dir = self.config().config_dir().to_path_buf();
        let startup = config_hygiene::load_for_startup(&config_dir).map_err(config_status)?;
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
        let disables_auto_reconnect = items
            .iter()
            .any(|item| item.key() == "auto_reconnect" && item.value() == "false");
        // MAC-OBS-13 S4：保存成功的敏感值登记进聚合存储的入库脱敏表——此后任何
        // 日志节点把该值拼进 message/fields，入库边界都替换为 `<redacted>`（值
        // 本身仍不作为日志内容；这是纵深防御接线，见 secret scan 端到端契约测试）。
        let saved_password = items
            .iter()
            .find(|item| item.key() == "password")
            .map(|item| item.value().to_owned());
        self.config().apply_and_save(items).map_err(config_status)?;
        if disables_auto_reconnect
            && let Some(live) = self.live_connection.as_deref()
        {
            // 本次会话在掉线等待窗口时，用户关闭开关立即终止旧 generation 并请求
            // StopTunnel；正常 Connected 时该方法不主动断开，只影响下一次掉线策略。
            live.cancel_waiting_reconnect_after_setting_disabled().await;
        }
        if let Some(password) = saved_password {
            self.logs.aggregator().register_secret(&password);
        }
        // MAC-OBS-13 S1：config 变更真实节点入库（只记键名，值永不入库——
        // secret-bearing 键由 config 层拒绝，非敏感键的值也无诊断价值）。
        self.logs.aggregator().append_core(
            "info",
            "core",
            "CONFIG_SET",
            "configuration updated",
            &[],
        );
        Ok(Response::new(ConfigReply { ok: true }))
    }

    /// P4 v2（动作面实装）：`query` 返回 service agent 健康探测的真实五态事实（见
    /// [`crate::service_status`]）；`install`/`uninstall`/`start` 经
    /// [`ServiceLifecycle`] 以管理员提权执行固定 argv 的 service agent CLI 动词
    /// （一次授权一个动作，win32「1 动作 = 1 次 runas」同构），变更动作加
    /// 串行锁并在前置尽力停业务（[`LiveConnectionProjection::stop_for_service_transition`]，
    /// win32 `stop_engine_for_transition` 对应）；回复携带 post-action 状态并刷新
    /// 服务状态缓存（快照伴生值同源）。`rotate_key` 诚实空缺：darwin Engine 会话
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
        let reply_with = |facts: &crate::service_status::ServiceAgentFacts,
                          message: &'static str|
         -> Response<ServiceControlReply> {
            let status = crate::service_status::wire_service_status(facts);
            *lock_recover(&self.service_status_cache) = Some(status.clone());
            Response::new(ServiceControlReply {
                service_status: Some(status),
                ok: true,
                message: message.to_string(),
            })
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
                // Query 成功路径入 aggregator（三字段观测才有出口；与 install/
                // uninstall/start 的 record 同构）。
                record("query", true, &message);
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
                    Ok(facts) => {
                        record("install", true, "service installed and started");
                        Ok(reply_with(&facts, "服务安装完成并已启动。"))
                    }
                    Err(error) => {
                        record_failure("install", "service install failed", &error);
                        Err(lifecycle_status(error))
                    }
                }
            }
            Some(Action::Uninstall(_)) => {
                let _guard = self.service_control_lock.lock().await;
                if let Some(live) = self.live_connection.as_deref() {
                    live.stop_for_service_transition().await;
                }
                match self.service_lifecycle.uninstall().await {
                    Ok(facts) => {
                        record("uninstall", true, "service uninstalled");
                        Ok(reply_with(&facts, "服务已卸载。"))
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
                    Ok(facts) => {
                        record("start", true, "service ensured running");
                        Ok(reply_with(
                            &facts,
                            if facts.socket_connectable {
                                "服务已在运行。"
                            } else {
                                "服务启动已提交。"
                            },
                        ))
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
    }
}

fn pending_connect_snapshot(request: &ConnectRequest, operation_id: Vec<u8>) -> RuntimeSnapshot {
    let runtime_epoch = request
        .intent
        .as_ref()
        .and_then(|intent| intent.lookup_key.as_ref())
        .map_or_else(Vec::new, |key| key.runtime_epoch.clone());
    RuntimeSnapshot {
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
    }
}

fn pending_stop_snapshot(request: &StopRequest, operation_id: Vec<u8>) -> RuntimeSnapshot {
    RuntimeSnapshot {
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
    }
}

/// 构造 10b 唯一允许的真实状态快照。
#[must_use]
pub(crate) fn idle_snapshot() -> RuntimeSnapshot {
    RuntimeSnapshot {
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

