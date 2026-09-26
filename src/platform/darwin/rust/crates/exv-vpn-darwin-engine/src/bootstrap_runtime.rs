//! 固定 root Engine 的 E4 bootstrap 与唯一 Common 控制服务。
//!
//! `MAC-CSTP-05` 起：`apply_tunnel` 消费一次性封包后执行真实的
//! WebVPN 登录 + CSTP 协商（见 [`crate::protocol`]），经状态流发布真实阶段
//! 与 typed 失败；仍不创建 utun、不应用路由/DNS、不启动数据面。

// 中文文档中的技术术语不逐个加反引号；apply 处理器承载完整受理语义。
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]

use std::{
    env, fmt,
    io::{self, Write},
    net::Ipv4Addr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use exv_vpn_darwin_ipc::{
    bootstrap::{EngineBootstrapRecord, EngineTicketV1, TicketError, consume_engine_ticket},
    connect_envelope::DarwinEngineConnectV1,
    listener::{PreauthListener, RootListenerConfig},
    path::{RuntimeDir, RuntimeDirError},
    peer::ExpectedPeer,
    tonic_bridge::AuthenticatedIncoming,
};

use exv_vpn_cstp::codec::{CSTP_PACKET_TYPE_KEEPALIVE, Codec};
use exv_vpn_cstp::session::{
    CSTP_PACKET_TYPE_DPD_REQUEST, CSTP_PACKET_TYPE_DPD_RESPONSE, CstpControlEvent,
};

use crate::log_sink::LogSink;
use crate::packet::probe::{LatencyProbeConfig, LatencyTap, ProbeState};
use crate::packet::pump;
use crate::platform::route::{self as platform_route, AppliedRoute, ControlRouteUpdate};
use crate::platform::utun::Utun;
use crate::stats::StatsPublisher;
use exv_vpn_wire::generated::{
    self as wire,
    helper_control_server::{HelperControl, HelperControlServer},
};
use tokio_stream::{Empty, StreamExt, wrappers::BroadcastStream};
use tonic::{Request, Response, Status, transport::Server};
use zeroize::Zeroize;

use crate::protocol::EstablishedSession;

/// 数据面存活标记：Some = 管线处于保持态，None = 已 teardown/无连接。
struct HeldTunnel;

/// Engine 唯一持有的跨会话平台资源账本。
///
/// `retained_device` 只在可重连的数据面掉线窗口中存在；业务路由和地址绝不放入此处。
/// 删除失败的路由保留精确原值，后续 Stop/退出会重试，不用“已执行清理”冒充已移除。
#[derive(Default)]
struct ResourceLedger {
    control_route: Option<AppliedRoute>,
    retained_device: Option<Utun>,
    pending_routes: Vec<AppliedRoute>,
}

/// SIGTERM/SIGINT 请求的优雅关停标志（信号处理器唯一写入，async-signal-safe）。
pub(crate) static TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

/// 信号请求后的管线 teardown 完成标志：退出路径必须等它为真，防止进程
/// 退出与逆序 teardown 竞速造成路由残留。
///
/// W1 统一退出收敛：SIGTERM/SIGINT、Core EOF 与 W2 心跳超时三类
/// 触发共用同一收敛等待（见 [`converge_and_await_teardown`]）。语义是「最近一次
/// 声明的 attempt 的 teardown 已完成」：`apply_tunnel` 声明新 attempt 时会先复位
/// 本标志，防止上一代管线的完成位让收敛等待误读提前通过。
static TERMINATION_TEARDOWN_DONE: AtomicBool = AtomicBool::new(false);

/// 统一退出收敛等待管线 teardown 完成的上界（沿用既有 SIGTERM 路径的 10s）。
const TERMINATION_TEARDOWN_BOUND: Duration = Duration::from_secs(10);

/// 收敛等待的轮询间隔（沿用既有 SIGTERM 路径的 50ms）。
const TERMINATION_TEARDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// SIGTERM/SIGINT 标志轮询间隔（信号处理器只做原子写入，主循环负责轮询）。
const TERMINATION_SIGNAL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// W2 心跳：engine 侧看门狗的超时上界（15s，硬时间界）。core 侧 ticker 每 10s 发
/// 一条 KeepAlive（core 侧常量）；认证握手完成起 15s 未收到即判定失联，触发
/// [`EngineRuntime::request_heartbeat_timeout`]（W1 顶层 select 第三臂接管统一
/// 收敛退出）。对齐 win32 `HEARTBEAT_TIMEOUT_MS` 与 proto KeepAlive 注释。
pub(crate) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15);

/// W2 心跳：看门狗的检查周期（500ms；对齐 win32 `heartbeat_timeout_watcher` 的
/// 默认检查节拍）。
pub(crate) const HEARTBEAT_WATCHDOG_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// 信号处理器：只做原子写入，不触碰锁与分配器。
extern "C" fn termination_request_handler(_signal: libc::c_int) {
    TERMINATION_REQUESTED.store(true, Ordering::SeqCst);
}

/// 安装 SIGTERM/SIGINT 的优雅关停处理器。
///
/// Windows 对齐：服务停止请求（macOS 上 launchd/用户的 SIGTERM）必须走与 Core
/// 失联相同的逆序 teardown 路径，而不是默认终止造成路由/地址残留。主循环的
/// 1 秒 tick 读取 [`TERMINATION_REQUESTED`] 收敛到 teardown。
pub fn install_signal_shutdown() {
    // SAFETY: 处理器只做原子 store；注册调用本身无复杂不变量。
    unsafe {
        libc::signal(
            libc::SIGTERM,
            termination_request_handler as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            termination_request_handler as *const () as libc::sighandler_t,
        );
    }
}

/// Engine 的本地连接协调状态：单活动尝试、权威 offer 持有者与状态广播。
pub(crate) struct EngineRuntime {
    attempt: Mutex<Option<ActiveAttempt>>,
    live: Mutex<Option<HeldTunnel>>,
    resources: Mutex<ResourceLedger>,
    status_tx: tokio::sync::broadcast::Sender<wire::ConnectStatusEvent>,
    /// 数据面统计注册表与 `StreamStats` 推送端点（W-MVP-13）。
    stats: Arc<StatsPublisher>,
    /// 结构化日志 sink 与 `StreamLogs` 推送端点（MAC-OBS-13 S1）。
    logs: Arc<LogSink>,
    /// 统一退出收敛的 dead flag：三类终止触发（SIGTERM/SIGINT、Core EOF、W2 心跳
    /// 超时）收敛时置位；保持循环的既有消费点据此 break 进逆序 teardown。
    shutdown: Arc<AtomicBool>,
    /// W2 心跳超时的触发槽：唯一触发方是 [`spawn_heartbeat_watchdog`] 的常驻
    /// 看门狗（心跳监视超 15s）；触发后与信号/EOF 同入 W1 统一收敛。
    heartbeat_timeout: TriggerSlot,
    /// W2.5 service 形态的显式停止触发槽（`Shutdown` RPC 唯一生产触发方；
    /// service 会话循环消费：拆隧道 → exit(0)，`KeepAlive={SuccessfulExit:false}`
    /// 下不复活）。会话形态为 None——Shutdown 对其恒 NOT_APPLICABLE。
    explicit_stop: Option<TriggerSlot>,
    /// W2 心跳监视：最近一次 KeepAlive 到达时刻；零点=认证握手完成时刻
    /// （[`HeartbeatWatch::arm`] 武装），未武装不参与看门狗判定。
    heartbeat: Mutex<HeartbeatWatch>,
}

impl EngineRuntime {
    pub(crate) fn new() -> Self {
        Self::with_explicit_stop(None)
    }

    /// W2.5 service 形态构造：显式停止槽在位（`Shutdown` RPC 实装）。
    pub(crate) fn new_service() -> Self {
        Self::with_explicit_stop(Some(TriggerSlot::default()))
    }

    fn with_explicit_stop(explicit_stop: Option<TriggerSlot>) -> Self {
        let (status_tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            attempt: Mutex::new(None),
            live: Mutex::new(None),
            resources: Mutex::new(ResourceLedger::default()),
            status_tx,
            stats: Arc::new(StatsPublisher::new()),
            logs: Arc::new(LogSink::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            heartbeat_timeout: TriggerSlot::default(),
            explicit_stop,
            heartbeat: Mutex::new(HeartbeatWatch::default()),
        }
    }

    /// W2 心跳超时的触发入口（唯一生产调用方是常驻看门狗）。
    ///
    /// 触发后与 SIGTERM/SIGINT、Core EOF 同入统一退出收敛，不新增第二条
    /// teardown 路径。
    fn request_heartbeat_timeout(&self) {
        self.heartbeat_timeout.request();
    }

    /// 等待 W2 心跳超时触发槽（先触发后等待也能完成：notify_one 留有 permit）。
    pub(crate) async fn wait_heartbeat_timeout(&self) {
        self.heartbeat_timeout.wait().await;
    }

    /// W2.5：武装本会话心跳（uid 门准入完成=心跳零点；service 会话循环入口调用）。
    pub(crate) fn arm_session_heartbeat(&self) {
        self.heartbeat.lock().expect("heartbeat watch lock").arm();
    }

    /// W2.5：结构化日志 sink 只读句柄（service 会话循环的收敛诊断发布点）。
    pub(crate) fn logs(&self) -> &Arc<LogSink> {
        &self.logs
    }

    /// W2.5 显式停止请求（`Shutdown` RPC，service 形态实装；会话形态无槽不动作）。
    pub(crate) fn request_explicit_stop(&self) {
        if let Some(slot) = &self.explicit_stop {
            slot.request();
        }
    }

    /// 显式停止是否已被请求（诊断/测试读取）。
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "电平读取当前仅测试断言使用；生产触发后即收敛退出")
    )]
    pub(crate) fn explicit_stop_requested(&self) -> bool {
        self.explicit_stop
            .as_ref()
            .is_some_and(TriggerSlot::is_requested)
    }

    /// 等待显式停止触发槽；会话形态（无槽）恒不完成。
    pub(crate) async fn wait_explicit_stop(&self) {
        match &self.explicit_stop {
            Some(slot) => slot.wait().await,
            None => std::future::pending::<()>().await,
        }
    }

    /// 登录前读取本次物理出口事实后，确保 VPN 服务端的精确 `/32` 专用路由走当前网关。
    ///
    /// 已由本 Engine 创建/迁移的路由才记入本地所有权账本；外部已有且完全一致的条目
    /// 可以复用，但 Stop 不会删除它。
    fn ensure_control_route(
        &self,
        egress: crate::egress::physical_route::PhysicalEgress,
        server: Ipv4Addr,
    ) -> Result<ControlRouteUpdate, crate::platform::PlatformError> {
        let desired = AppliedRoute {
            destination: server,
            prefix: 32,
            gateway: egress.gateway,
            ifindex: egress.ifindex,
        };
        let mut resources = self.resources.lock().expect("resource ledger lock");
        if let Some(previous) = resources.control_route
            && previous.destination != desired.destination
        {
            platform_route::delete(&previous)?;
            resources.control_route = None;
        }
        let observed = platform_route::read_exact(desired.destination, desired.prefix)?;
        let old_gateway = observed.map(|route| route.gateway);
        let update = match observed {
            Some(actual) if actual == desired => ControlRouteUpdate::Reused,
            Some(_) => {
                platform_route::change(&desired)?;
                // RTM_CHANGE 已被内核接受后先登记。随后回读失败时 Stop/退出仍有精确
                // 候选可删除，不能因“未确认”把已写条目从账本丢掉。
                resources.control_route = Some(desired);
                ControlRouteUpdate::Changed
            }
            None => match platform_route::add(&desired) {
                Ok(()) => {
                    resources.control_route = Some(desired);
                    ControlRouteUpdate::Added
                }
                Err(crate::platform::PlatformError::Route("ACK", error))
                    if error.raw_os_error() == Some(libc::EEXIST) =>
                {
                    match platform_route::read_exact(desired.destination, desired.prefix)? {
                        Some(actual) if actual == desired => ControlRouteUpdate::Reused,
                        Some(_) => {
                            platform_route::change(&desired)?;
                            resources.control_route = Some(desired);
                            ControlRouteUpdate::Changed
                        }
                        None => return Err(crate::platform::PlatformError::Route("RACE", error)),
                    }
                }
                Err(error) => return Err(error),
            },
        };
        if platform_route::read_exact(desired.destination, desired.prefix)? != Some(desired) {
            return Err(crate::platform::PlatformError::Readback("CONTROL_ROUTE"));
        }
        let mut fields = vec![
            ("physical_gateway", egress.gateway.to_string()),
            ("physical_ifindex", egress.ifindex.to_string()),
            ("server_ipv4", server.to_string()),
            ("route_update", format!("{update:?}")),
        ];
        if let Some(gateway) = old_gateway {
            fields.push(("old_gateway", gateway.to_string()));
        }
        self.logs.publish(
            "info",
            "platform",
            "CONTROL_ROUTE_RECONCILED",
            "VPN 服务端专用路由已按当前物理出口回读确认",
            &fields,
        );
        Ok(update)
    }

    fn take_or_acquire_device(&self) -> Result<Utun, crate::platform::PlatformError> {
        if let Some(device) = self
            .resources
            .lock()
            .expect("resource ledger lock")
            .retained_device
            .take()
        {
            return Ok(device);
        }
        Utun::acquire()
    }

    fn retain_device(&self, device: Utun) {
        let mut resources = self.resources.lock().expect("resource ledger lock");
        debug_assert!(
            resources.retained_device.is_none(),
            "only one retained utun"
        );
        resources.retained_device = Some(device);
    }

    fn remember_route_cleanup_failure(&self, route: AppliedRoute) {
        let mut resources = self.resources.lock().expect("resource ledger lock");
        if !resources.pending_routes.contains(&route) {
            resources.pending_routes.push(route);
        }
    }

    /// 完整清理所有仍由本 Engine 持有的跨会话资源。
    ///
    /// 每一项独立尝试；失败条目保持在账本中，utun FD 始终释放，使下一次 Stop/退出可
    /// 重试路由而不会为了一个失败无限保留设备。
    fn complete_resource_cleanup(&self) -> Result<(), crate::platform::PlatformError> {
        let mut resources = self.resources.lock().expect("resource ledger lock");
        let mut first_error = None;
        let mut retry = Vec::new();
        for route in resources.pending_routes.drain(..) {
            if let Err(error) = platform_route::delete(&route) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                retry.push(route);
            }
        }
        resources.pending_routes = retry;
        if let Some(route) = resources.control_route {
            match platform_route::delete(&route) {
                Ok(()) => resources.control_route = None,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        // Drop even if route removal failed: device ownership is independent and must not leak.
        drop(resources.retained_device.take());
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn pipeline_activity_cleared(&self) -> bool {
        let attempt_active = self.attempt.lock().map_or(true, |slot| slot.is_some());
        let live = self.live.lock().map_or(true, |slot| slot.is_some());
        !attempt_active && !live
    }

    fn request_pipeline_cancel(&self) {
        if let Ok(slot) = self.attempt.lock()
            && let Some(attempt) = slot.as_ref()
        {
            attempt.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// StopTunnel 的完成边界：先等待本地 pipeline 停止并释放它临时拥有的资源，再清
    /// ledger 中的空闲 utun/控制路由/失败重试条目。
    async fn stop_and_wait_for_cleanup(&self) -> Result<(), crate::platform::PlatformError> {
        self.request_pipeline_cancel();
        let deadline = tokio::time::Instant::now() + TERMINATION_TEARDOWN_BOUND;
        while !self.pipeline_activity_cleared() {
            if tokio::time::Instant::now() >= deadline {
                return Err(crate::platform::PlatformError::Route(
                    "CLEANUP_WAIT",
                    io::Error::from_raw_os_error(libc::ETIMEDOUT),
                ));
            }
            tokio::time::sleep(TERMINATION_TEARDOWN_POLL_INTERVAL).await;
        }
        self.complete_resource_cleanup()
    }
}

/// W2 心跳监视：`last` = 最近一次心跳到达时刻（或武装零点）；`None` = 未武装。
///
/// 与 win32 `HeartbeatWatch` 的关键差异（权威规范 §二 oneshot.2）：**零点=认证握手
/// 完成时刻**，不是进程启动——握手完成（对端已验证）时 [`Self::arm`] 武装并置
/// 零点；未武装（无已认证活跃会话）看门狗不判定。时间源用 tokio 时钟
/// （`start_paused` 单测注入虚拟时钟）。
#[derive(Debug, Default)]
struct HeartbeatWatch {
    /// 最近一次 KeepAlive 到达时刻；未收到过心跳时为武装零点。
    last: Option<tokio::time::Instant>,
}

impl HeartbeatWatch {
    /// 武装（认证握手完成）：零点=此刻。单连接 accept 模型下握手先于任何 RPC，
    /// arm 与 handler 首条 KeepAlive 之间的微小竞态无害（touch 同样落在握手后）。
    fn arm(&mut self) {
        self.last = Some(tokio::time::Instant::now());
    }

    /// 刷新最近心跳时刻（`KeepAlive` handler 语义：touch）。
    fn touch(&mut self) {
        self.last = Some(tokio::time::Instant::now());
    }

    /// 距最近一次心跳（未收到过心跳时=武装零点）的时长；未武装返回 `None`
    /// （看门狗对无会话状态不判定）。
    fn elapsed(&self) -> Option<Duration> {
        Some(tokio::time::Instant::now() - self.last?)
    }
}

/// W2 心跳超时的触发槽（可 await 信号 + 电平标志）。
///
/// 唯一生产触发方是 [`spawn_heartbeat_watchdog`] 的常驻看门狗；触发后令顶层
/// select 与信号/EOF 走同一收敛序列。W2.5 起 service 形态的显式停止
/// （`Shutdown` RPC）复用同一机制（[`EngineRuntime::explicit_stop`]）。
#[derive(Debug, Default)]
struct TriggerSlot {
    requested: AtomicBool,
    notify: tokio::sync::Notify,
}

impl TriggerSlot {
    /// 触发（幂等；电平置位 + 唤醒一个等待者）。
    fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// 是否已触发（电平语义，供看门狗/诊断读取）。
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "电平读取当前仅测试断言使用；生产触发后即收敛")
    )]
    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// 等待触发；notify_one 在等待前后注册都能完成，不丢触发。
    async fn wait(&self) {
        self.notify.notified().await;
    }
}

/// 统一退出收敛的触发源（仅诊断标注；三类触发共用同一收敛序列）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminationTrigger {
    /// SIGTERM/SIGINT（launchd/用户停止；由信号处理器置标志，主循环轮询）。
    PosixSignals,
    /// Core 关闭已认证连接（EOF/失联，经 handoff_with_close_signal）。
    CoreClosed,
    /// W2 心跳超时（常驻看门狗触发：认证握手完成起 15s 未收到 KeepAlive）。
    HeartbeatTimeout,
}

const OBSERVE_ONLY_MESSAGE: &str = "DARWIN_ENGINE_OBSERVE_ONLY";
const APPLY_INVALID_MESSAGE: &str = "DARWIN_ENGINE_APPLY_INVALID";
/// 当前 Unix epoch 毫秒（诊断与会话起点字段；取时失败返回 0）。
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(0)
        })
}

/// 距 `started_ms` 的流逝毫秒（会话时长的诊断字段）。
fn elapsed_since_ms(started_ms: i64) -> i64 {
    now_epoch_ms().saturating_sub(started_ms)
}

pub(crate) const ATTEMPT_ACTIVE_MESSAGE: &str = "DARWIN_ENGINE_ATTEMPT_ACTIVE";
const STATUS_LAGGED_MESSAGE: &str = "DARWIN_ENGINE_STATUS_LAGGED";

#[derive(Debug)]
pub enum EngineBootstrapError {
    Arguments,
    Credentials,
    RuntimeDir(RuntimeDirError),
    Ticket(TicketError),
    Listener,
    Pipe,
    Server,
}

impl fmt::Display for EngineBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Arguments => "Darwin Engine bootstrap arguments are invalid",
            Self::Credentials => "Darwin Engine bootstrap credentials are invalid",
            Self::RuntimeDir(_) => "Darwin Engine runtime directory is invalid",
            Self::Ticket(_) => "Darwin Engine ticket is invalid",
            Self::Listener => "Darwin Engine listener bootstrap failed",
            Self::Pipe => "Darwin Engine bootstrap pipe failed",
            Self::Server => "Darwin Engine Observe server failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for EngineBootstrapError {}

#[derive(Debug)]
struct EngineArgs {
    runtime_dir: PathBuf,
    owner_uid: u32,
    core_pid: u32,
}

impl EngineArgs {
    fn from_process() -> Result<Self, EngineBootstrapError> {
        let values: Vec<_> = env::args_os().skip(1).collect();
        if values.len() != 6
            || values[0] != "--runtime-dir"
            || values[2] != "--owner-uid"
            || values[4] != "--core-pid"
        {
            return Err(EngineBootstrapError::Arguments);
        }
        let owner_uid = values[3]
            .to_str()
            .and_then(|value| value.parse().ok())
            .filter(|value: &u32| *value != 0)
            .ok_or(EngineBootstrapError::Arguments)?;
        let core_pid = values[5]
            .to_str()
            .and_then(|value| value.parse().ok())
            .filter(|value: &u32| *value > 0)
            .ok_or(EngineBootstrapError::Arguments)?;
        Ok(Self {
            runtime_dir: PathBuf::from(&values[1]),
            owner_uid,
            core_pid,
        })
    }
}

/// 从固定 argv 与 Authorization communications pipe 运行 root bootstrap。
///
/// W2.5 起 argv 形态分派：首参 `--service` 进入常驻 service 形态（见
/// [`crate::service_runtime`]）；否则为既有 per-core-session 会话形态。
///
/// # Errors
///
/// argv、实际 uid/euid、ticket、root listener、pipe record 或控制服务任一
/// 阶段失败时返回稳定的 [`EngineBootstrapError`]，且不创建 utun 或平台网络资源。
pub fn run_from_process() -> Result<(), EngineBootstrapError> {
    // 诊断：Engine stderr 由服务代理置空，panic 消息落盘供开发期读取。
    std::panic::set_hook(Box::new(|info| {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/private/tmp/exv-engine-panic.log")
        {
            use std::io::Write as _;
            let _ = writeln!(file, "{info}");
        }
    }));
    let values: Vec<_> = env::args_os().skip(1).collect();
    if values
        .first()
        .is_some_and(|value| value == std::ffi::OsStr::new("--service"))
    {
        return crate::service_runtime::run_service_from_process(&values);
    }
    let args = EngineArgs::from_process()?;
    // SAFETY: these calls only read this process credentials.
    let (uid, euid) = unsafe { (libc::getuid(), libc::geteuid()) };
    if uid != args.owner_uid || euid != 0 {
        return Err(EngineBootstrapError::Credentials);
    }
    let runtime_dir = RuntimeDir::open_existing_for_uid(&args.runtime_dir, args.owner_uid)
        .map_err(EngineBootstrapError::RuntimeDir)?;
    let (runtime_dir, ticket) =
        open_and_validate_engine_ticket(runtime_dir, args.owner_uid, args.core_pid)?;

    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| EngineBootstrapError::Server)?;
    tokio_runtime.block_on(run_observe_bootstrap(
        runtime_dir,
        args,
        ticket.into_auth_key(),
    ))
}

/// 打开 runtime 目录后的 ticket 消费与 owner/core_pid 校验。
///
/// 本函数是 L2 修复的落点：消费成功但校验失败（或消费本身失败）时，**失败返回前**
/// 必须做 guarded ticket 清理 + `cleanup_empty()`——提前失败不等于可泄漏。清理失败
/// 只写 stderr 并保留原始 typed 错误，不得用 `remove_dir_all` 扩大删除面。
///
/// # Errors
///
/// 消费或校验失败时返回 [`EngineBootstrapError::Ticket`]。
fn open_and_validate_engine_ticket(
    runtime_dir: RuntimeDir,
    owner_uid: u32,
    core_pid: u32,
) -> Result<(RuntimeDir, EngineTicketV1), EngineBootstrapError> {
    let ticket = match consume_engine_ticket(&runtime_dir) {
        Ok(ticket) => ticket,
        Err(error) => {
            return Err(cleanup_failed_bootstrap_runtime(
                runtime_dir,
                EngineBootstrapError::Ticket(error),
            ));
        }
    };
    if ticket.owner_uid() != owner_uid || ticket.core_pid() != core_pid {
        return Err(cleanup_failed_bootstrap_runtime(
            runtime_dir,
            EngineBootstrapError::Ticket(TicketError::Malformed),
        ));
    }
    Ok((runtime_dir, ticket))
}

async fn run_observe_bootstrap(
    mut runtime_dir: RuntimeDir,
    args: EngineArgs,
    auth_key: exv_vpn_darwin_ipc::auth::AuthKey,
) -> Result<(), EngineBootstrapError> {
    runtime_dir
        .seal_for_root_publisher()
        .map_err(EngineBootstrapError::RuntimeDir)?;
    let listener_config = RootListenerConfig::new(
        runtime_dir,
        ExpectedPeer::new(args.owner_uid, args.core_pid),
        auth_key,
    )
    .map_err(|_| EngineBootstrapError::Listener)?;
    // SAFETY: this Engine bootstrap has not started worker threads before this root-only bind;
    // the listener implementation restores the process umask before returning.
    let listener = unsafe { PreauthListener::bind_root_publisher_single_threaded(listener_config) }
        .map_err(|_| EngineBootstrapError::Listener)?;
    let pid = current_pid()?;
    let (sender, incoming) = AuthenticatedIncoming::channel(1);
    // 顶层保留 EngineRuntime 句柄：统一退出收敛要置 shutdown/cancel 并观察
    // attempt/live 收尾，不再让 service 独占 runtime。
    let engine_runtime = Arc::new(EngineRuntime::new());
    let service = EngineControlService::from_runtime(Arc::clone(&engine_runtime));
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(HelperControlServer::new(service))
            .serve_with_incoming(incoming)
            .await
    });

    write_record(EngineBootstrapRecord::Ready { pid })?;
    let operation = async {
        let connection = listener
            .accept_until_authenticated()
            .await
            .map_err(|_| EngineBootstrapError::Listener)?;
        let close_signal = sender
            .handoff_with_close_signal(connection)
            .await
            .map_err(|_| EngineBootstrapError::Listener)?;
        // W2 心跳：认证握手完成=心跳零点（权威规范：零点是握手完成时刻，不是进程
        // 启动），同时拉起 engine 层常驻看门狗（不挂在隧道保持循环上——保持循环
        // 只在持隧道期存在；未武装前看门狗恒不判定）。
        engine_runtime
            .heartbeat
            .lock()
            .expect("heartbeat watch lock")
            .arm();
        spawn_heartbeat_watchdog(Arc::clone(&engine_runtime));
        // W1 统一退出收敛：SIGTERM/SIGINT、Core EOF（close_signal）与 W2 心跳超时
        // 三类触发进入同一收敛序列——置 shutdown（接通
        // dead flag）→ 管线经既有消费点逆序 teardown → 有界等待
        // TERMINATION_TEARDOWN_DONE（10s 上界）。收敛完成前不 drop(sender)/关
        // server：否则会取消在途 handler，进程退出与 teardown 竞速会把已应用的
        // utun 路由与挂物理出口的 bypass /32 静态路由残留在内核。
        let trigger = tokio::select! {
            signal = close_signal => {
                // bridge 内部错误的 Err 与 EOF 同样进入收敛（close 结果本就不再
                // 影响退出 record 语义）。
                let _ = signal;
                TerminationTrigger::CoreClosed
            }
            () = poll_termination_signals() => TerminationTrigger::PosixSignals,
            () = engine_runtime.wait_heartbeat_timeout() => TerminationTrigger::HeartbeatTimeout,
        };
        let converged = converge_and_await_teardown(&engine_runtime).await;
        if !converged {
            // 有界超时：仍走既有退出路径（cleanup + 退出 record），但落诊断。
            engine_runtime.logs.publish(
                "error",
                "lifecycle",
                "TEARDOWN_TIMEOUT",
                format!(
                    "termination convergence hit the teardown bound, exiting with teardown \
                     unconfirmed (trigger={trigger:?})"
                ),
                &[("trigger", format!("{trigger:?}"))],
            );
        }
        drop(sender);
        server
            .await
            .map_err(|_| EngineBootstrapError::Server)?
            .map_err(|_| EngineBootstrapError::Server)?;
        Ok(())
    }
    .await;
    let cleanup = listener
        .cleanup()
        .map_err(|_| EngineBootstrapError::Listener);
    let outcome = operation.and(cleanup);
    match outcome {
        Ok(()) => {
            write_record(EngineBootstrapRecord::ExitOk { pid })?;
            Ok(())
        }
        Err(error) => {
            let _ = write_record(EngineBootstrapRecord::ExitError { pid, code: 1 });
            Err(error)
        }
    }
}

fn current_pid() -> Result<u32, EngineBootstrapError> {
    // SAFETY: this call only reads the current process pid.
    u32::try_from(unsafe { libc::getpid() }).map_err(|_| EngineBootstrapError::Credentials)
}

/// 提前失败路径（尚未发布 listener）的 runtime 目录回收。
///
/// 语义与正常退出收敛一致：先 guarded 清理固定 `engine.ticket`（held dirfd +
/// `unlinkat`，路径字符串不参与删除），再 `cleanup_empty()`（只 `rmdir` 空目录，非空即
/// 保留现场）。任何清理失败都只写一行 stderr 并返回原 typed 错误——清理失败不得
/// 覆盖真正的 bootstrap 失败原因，也不得用 `remove_dir_all` 扩大删除面。
fn cleanup_failed_bootstrap_runtime(
    runtime_dir: RuntimeDir,
    primary: EngineBootstrapError,
) -> EngineBootstrapError {
    if let Err(error) =
        exv_vpn_darwin_ipc::bootstrap::remove_engine_ticket_for_failed_bootstrap(&runtime_dir)
    {
        eprintln!("exv.engine.bootstrap: failed-bootstrap ticket cleanup skipped: {error}");
    }
    if let Err(error) = runtime_dir.cleanup_empty() {
        eprintln!("exv.engine.bootstrap: failed-bootstrap runtime cleanup skipped: {error}");
    }
    primary
}

fn write_record(record: EngineBootstrapRecord) -> Result<(), EngineBootstrapError> {
    let stdout = io::stdout();
    let mut pipe = stdout.lock();
    record
        .write_to(&mut pipe)
        .map_err(|_| EngineBootstrapError::Pipe)?;
    pipe.flush().map_err(|_| EngineBootstrapError::Pipe)
}

/// 轮询 SIGTERM/SIGINT 请求标志。
///
/// 信号处理器只做原子写入（async-signal-safe，不可 await），主循环以固定间隔
/// 轮询收敛；置位后返回，由统一收敛序列接管。
pub(crate) async fn poll_termination_signals() {
    poll_signal_flag(&TERMINATION_REQUESTED).await;
}

/// [`poll_termination_signals`] 的可注入变体：W2.5 service 会话循环经它传入独立
/// 标志（单测隔离进程级静态，避免并行测试间竞态）。
pub(crate) async fn poll_signal_flag(flag: &AtomicBool) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(TERMINATION_SIGNAL_POLL_INTERVAL).await;
    }
}

/// W2 常驻心跳看门狗：按 [`HEARTBEAT_WATCHDOG_CHECK_INTERVAL`] 周期检查
/// [`HeartbeatWatch`]，已武装（存在已认证活跃会话）且超 [`HEARTBEAT_TIMEOUT`]
/// 未收到 KeepAlive → `request_heartbeat_timeout()`（W1 顶层 select 第三臂接管
/// 统一收敛退出）。
///
/// engine 层常驻任务（顶层 spawn，不挂在隧道保持循环上——保持循环只在持隧道期
/// 存在；本波只有会话形态，超时动作=统一收敛退出）。未武装（握手未完成）恒不
/// 判定；触发一次后结束（进程随收敛退出）。锁中毒按超时处理（fail closed：
/// 宁可误收敛也不留无主隧道）。
pub(crate) fn spawn_heartbeat_watchdog(runtime: Arc<EngineRuntime>) -> tokio::task::JoinHandle<()> {
    spawn_heartbeat_watchdog_with(
        runtime,
        HEARTBEAT_WATCHDOG_CHECK_INTERVAL,
        HEARTBEAT_TIMEOUT,
    )
}

/// [`spawn_heartbeat_watchdog`] 的可注入节奏变体（单测用小间隔加速；executor
/// 与生产同一 [`heartbeat_watchdog_loop`]，无第二条看门狗路径）。
pub(crate) fn spawn_heartbeat_watchdog_with(
    runtime: Arc<EngineRuntime>,
    check_interval: Duration,
    timeout: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(heartbeat_watchdog_loop(runtime, check_interval, timeout))
}

/// 看门狗主循环：检查节拍与超时上界由参数注入（测试用小值加速），生产经
/// [`spawn_heartbeat_watchdog`] 传入默认常量。
async fn heartbeat_watchdog_loop(
    runtime: Arc<EngineRuntime>,
    check_interval: Duration,
    timeout: Duration,
) {
    loop {
        tokio::time::sleep(check_interval).await;
        let timed_out = runtime.heartbeat.lock().map_or(true, |watch| {
            watch.elapsed().is_some_and(|elapsed| elapsed > timeout)
        });
        if timed_out {
            runtime.logs.publish(
                "error",
                "lifecycle",
                "HEARTBEAT_TIMEOUT",
                "no KeepAlive within the timeout bound; converging to teardown",
                &[("timeout_secs", timeout.as_secs().to_string())],
            );
            runtime.request_heartbeat_timeout();
            return;
        }
    }
}

/// 统一退出收敛执行体：三类终止触发（SIGTERM/SIGINT、Core EOF、W2 心跳超时）
/// 在触发后调用同一本函数，不区分触发源。
///
/// 顺序：
/// 1. 置 `runtime.shutdown`（接通既有 dead flag，保持循环的消费点不变）；
/// 2. 对进行中的 attempt 置既有 cancel（`establish` 的段边界消费点，隧道组装中
///    也能尽早自清理；持有态由 1 的消费点 break，两条都是既有机制，无第二套）；
/// 3. 有界等待管线 teardown 完成（[`TERMINATION_TEARDOWN_DONE`] 或
///    attempt/live 已清空），上界 [`TERMINATION_TEARDOWN_BOUND`]。
///
/// 返回 true = teardown 确认完成（或本就无管线任务）；false = 有界超时（调用方
/// 仍继续既有退出路径，但应记录诊断）。
pub(crate) async fn converge_and_await_teardown(runtime: &EngineRuntime) -> bool {
    converge_and_await_teardown_with(runtime, &TERMINATION_TEARDOWN_DONE).await
}

/// [`converge_and_await_teardown`] 的可注入变体：teardown 完成标志由调用方提供，
/// 单测用它隔离进程级静态（勿在单测间共享真静态）。
async fn converge_and_await_teardown_with(
    runtime: &EngineRuntime,
    teardown_done: &AtomicBool,
) -> bool {
    runtime.shutdown.store(true, Ordering::SeqCst);
    if let Ok(slot) = runtime.attempt.lock()
        && let Some(attempt) = slot.as_ref()
    {
        attempt.cancel.store(true, Ordering::SeqCst);
    }
    // 边界一（idle）：无管线任务（attempt 与 live 均空）时立即完成，不等一个
    // 永远不会置位的 teardown 完成位。
    if tunnel_activity_cleared(runtime) {
        return runtime.complete_resource_cleanup().is_ok();
    }
    // 边界二（组装中/持有中）：等管线经既有消费点自清理。完成条件是
    // teardown 完成位置位，或 attempt/live 被管线收尾清空（组装失败路径不置
    // 完成位、只清槽位）。
    let deadline = tokio::time::Instant::now() + TERMINATION_TEARDOWN_BOUND;
    loop {
        if teardown_done.load(Ordering::SeqCst) || tunnel_activity_cleared(runtime) {
            return runtime.complete_resource_cleanup().is_ok();
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(TERMINATION_TEARDOWN_POLL_INTERVAL).await;
    }
}

/// attempt 与 live 均为空 = 无进行中的组装/持有（管线的每条收尾路径都会清两者）。
///
/// 锁中毒按「仍有活动」处理：收敛宁可多等（受上界约束）也不误判 idle 提前退出。
fn tunnel_activity_cleared(runtime: &EngineRuntime) -> bool {
    let attempt_active = match runtime.attempt.lock() {
        Ok(slot) => slot.is_some(),
        Err(_) => true,
    };
    let held = match runtime.live.lock() {
        Ok(slot) => slot.is_some(),
        Err(_) => true,
    };
    !attempt_active && !held
}

/// 一个活动连接尝试的关联信息。
struct ActiveAttempt {
    cancel: Arc<AtomicBool>,
}

/// 唯一 Common 控制服务实现。
pub(crate) struct EngineControlService {
    runtime: Arc<EngineRuntime>,
}

impl EngineControlService {
    // 生产路径改由 run_observe_bootstrap 持 runtime 句柄构造（from_runtime）；
    // new 仅供单测独立建服务。
    /// 从外部构造的 runtime 建服务：顶层 run_observe_bootstrap / W2.5 service
    /// 会话循环均需保留同一句柄执行统一收敛。
    pub(crate) fn from_runtime(runtime: Arc<EngineRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl HelperControl for EngineControlService {
    type MaintainOwnerLeaseStream = Empty<Result<wire::HelperLeaseMessage, Status>>;
    type StreamLogsStream =
        std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<wire::LogEvent, Status>> + Send>>;
    type StreamStatsStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<wire::StatsEvent, Status>> + Send>,
    >;
    type StreamConnectStatusStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<wire::ConnectStatusEvent, Status>> + Send>,
    >;

    async fn maintain_owner_lease(
        &self,
        _request: Request<tonic::Streaming<wire::HostLeaseMessage>>,
    ) -> Result<Response<Self::MaintainOwnerLeaseStream>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn observe_owned_state(
        &self,
        request: Request<wire::ObserveOwnedStateRequest>,
    ) -> Result<Response<wire::ObserveOwnedStateReply>, Status> {
        if request.get_ref().lookup_key.is_some() {
            return Err(Status::invalid_argument(OBSERVE_ONLY_MESSAGE));
        }
        Ok(Response::new(wire::ObserveOwnedStateReply {
            snapshot: Some(wire::RuntimeSnapshot {
                system_proxy: None,
                reconnect: None,
                self_heal: None,
                state: Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
                    last_cleanup: None,
                })),
                ..Default::default()
            }),
            authority_fence: Some(wire::AuthorityFence::default()),
        }))
    }

    async fn acquire_lease(
        &self,
        _request: Request<wire::AcquireLeaseRequest>,
    ) -> Result<Response<wire::AcquireLeaseReply>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn apply_tunnel(
        &self,
        request: Request<wire::ApplyTunnelRequest>,
    ) -> Result<Response<wire::ApplyTunnelReply>, Status> {
        // MAC-CSTP-05：消费一次性封包后执行真实 WebVPN 登录 + CSTP 协商；
        // 仍不创建 utun、不应用路由/DNS、不启动数据面。
        let request = request.into_inner();
        let operation_id = request
            .lookup_key
            .as_ref()
            .map(|key| key.operation_id.clone())
            .filter(|value| value.len() == 16)
            .ok_or_else(|| Status::invalid_argument(APPLY_INVALID_MESSAGE))?;
        let plan = request
            .plan
            .ok_or_else(|| Status::invalid_argument(APPLY_INVALID_MESSAGE))?;
        let envelope = DarwinEngineConnectV1::decode(request.secret_payload)
            .map_err(|_| Status::invalid_argument(APPLY_INVALID_MESSAGE))?;

        {
            let mut attempt_slot = self
                .runtime
                .attempt
                .lock()
                .map_err(|_| Status::internal(ATTEMPT_ACTIVE_MESSAGE))?;
            if attempt_slot.is_some() {
                return Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE));
            }
            // 新一代 attempt 声明即复位上一代的 teardown 完成位：完成位语义是
            // 「最近一次声明的 attempt 的 teardown 已完成」，不复位会让重连后的
            // 第二代管线在收敛等待中被上一代的完成位误判为已清理。
            TERMINATION_TEARDOWN_DONE.store(false, Ordering::SeqCst);
            *attempt_slot = Some(ActiveAttempt {
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }

        let runtime = Arc::clone(&self.runtime);
        let task_operation_id = operation_id.clone();
        let server = envelope.server().to_owned();
        let user_agent = envelope.user_agent().to_owned();
        let campus_routes = envelope.routes().to_vec();
        let (username, password) = envelope.take_credentials();
        let plan_mtu = plan.mtu;
        tokio::spawn(async move {
            let cancel = runtime
                .attempt
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(|attempt| Arc::clone(&attempt.cancel)))
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
            let is_cancelled = || cancel.load(std::sync::atomic::Ordering::SeqCst);
            let mut username = username;
            let mut password = password;
            let result = crate::protocol::establish_observed(
                &server,
                username.as_bytes(),
                password.as_bytes(),
                &user_agent,
                is_cancelled,
                &|phase| {
                    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                        own_tunnel_if_index: 0,
                        session_established_at_ms: 0,
                        operation_id: task_operation_id.clone(),
                        connect_phase: phase as i32,
                        coarse_phase: wire::StatsPhase::Connecting as i32,
                        error: None,
                    });
                },
                Some((&runtime.logs, &task_operation_id)),
                |egress, server| runtime.ensure_control_route(egress, server).map(|_| ()),
            )
            .await;
            username.zeroize();
            password.zeroize();
            match result {
                Ok(established) if is_cancelled() => {
                    drop(established);
                    if let Err(error) = runtime.complete_resource_cleanup() {
                        runtime.logs.publish(
                            "error",
                            "engine",
                            "CANCELLED_CONNECT_CLEANUP_INCOMPLETE",
                            format!("cancelled connection cleanup incomplete: {error}"),
                            &[],
                        );
                    }
                    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                        own_tunnel_if_index: 0,
                        session_established_at_ms: 0,
                        operation_id: task_operation_id,
                        connect_phase: wire::ConnectPhase::NegotiatingTunnel as i32,
                        coarse_phase: wire::StatsPhase::Failed as i32,
                        error: Some(ConnectCancelled::vpn_error()),
                    });
                    if let Ok(mut slot) = runtime.attempt.lock() {
                        *slot = None;
                    }
                }
                Ok(established) => {
                    // MAC-UTUN-06 / MAC-NET-07 / MAC-DATA-08：协商成功后建立完整平台
                    // 数据面；失败沿逆序 teardown，成功后保持并按需 teardown。
                    let runtime_for_pipeline = Arc::clone(&runtime);
                    let operation_for_pipeline = task_operation_id.clone();
                    let outcome = tokio::spawn(async move {
                        run_tunnel_pipeline(
                            &runtime_for_pipeline,
                            established,
                            operation_for_pipeline,
                            plan_mtu,
                            campus_routes,
                            &cancel,
                        )
                        .await
                    })
                    .await
                    .unwrap_or_else(|panic| {
                        println!("[pipeline] panicked: {panic}");
                        runtime.logs.publish(
                            "error",
                            "pipeline",
                            "PIPELINE_PANIC",
                            format!("pipeline panicked: {panic}"),
                            &[],
                        );
                        TunnelOutcome::Failed("PANIC", 0, 0)
                    });
                    match outcome {
                        TunnelOutcome::Cancelled => {}
                        TunnelOutcome::SessionLost => {
                            runtime.logs.publish(
                                "error",
                                "pipeline",
                                "PUMP_LOST",
                                "data plane pump finished unexpectedly",
                                &[],
                            );
                            let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                                own_tunnel_if_index: 0,
                                session_established_at_ms: 0,
                                operation_id: task_operation_id,
                                connect_phase: wire::ConnectPhase::NegotiatingTunnel as i32,
                                coarse_phase: wire::StatsPhase::Failed as i32,
                                error: Some(ConnectCancelled::session_lost_vpn_error()),
                            });
                            if let Ok(mut slot) = runtime.attempt.lock() {
                                *slot = None;
                            }
                        }
                        TunnelOutcome::Failed(step, errno, marker) => {
                            println!("[pipeline] platform failed at {step} errno={errno}");
                            runtime.logs.publish(
                                "error",
                                "pipeline",
                                "PLATFORM_STEP_FAILED",
                                format!("platform failed at {step} errno={errno}"),
                                &[
                                    ("step", step.to_owned()),
                                    ("errno", errno.to_string()),
                                    ("marker", marker.to_string()),
                                ],
                            );
                            let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                                own_tunnel_if_index: 0,
                                session_established_at_ms: 0,
                                operation_id: task_operation_id,
                                connect_phase: wire::ConnectPhase::ApplyingPlatformTunnel as i32,
                                coarse_phase: wire::StatsPhase::Failed as i32,
                                error: Some(ConnectCancelled::platform_vpn_error(
                                    step, errno, marker,
                                )),
                            });
                            if let Ok(mut slot) = runtime.attempt.lock() {
                                *slot = None;
                            }
                        }
                    }
                }
                Err(error) => {
                    // 登录前控制路由可能已经成功写入、但随后登录/回读失败。无后续重试
                    // 事件时此处必须释放它；若此前存在自动重连窗口的空闲 utun，同样收口。
                    if let Err(cleanup_error) = runtime.complete_resource_cleanup() {
                        runtime.logs.publish(
                            "error",
                            "engine",
                            "CONNECT_FAILURE_CLEANUP_INCOMPLETE",
                            format!("failed connection cleanup incomplete: {cleanup_error}"),
                            &[],
                        );
                    }
                    let phase = error.failure_phase();
                    let vpn_error = error.to_vpn_error();
                    // MAC-OBS-13 S1：协议协商 typed 失败路径同步进 sink。`ConnectError`
                    // 的 Display 只含 typed 诊断码（凭据与主机名永不入错误文本）。
                    runtime.logs.publish(
                        "error",
                        "protocol",
                        "ESTABLISH_FAILED",
                        format!("CSTP establish failed: {error}"),
                        &[
                            ("code", vpn_error.code.to_string()),
                            ("stage", vpn_error.stage.to_string()),
                        ],
                    );
                    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                        own_tunnel_if_index: 0,
                        session_established_at_ms: 0,
                        operation_id: task_operation_id,
                        connect_phase: phase as i32,
                        coarse_phase: wire::StatsPhase::Failed as i32,
                        error: Some(vpn_error),
                    });
                    if let Ok(mut slot) = runtime.attempt.lock() {
                        *slot = None;
                    }
                }
            }
        });

        Ok(Response::new(wire::ApplyTunnelReply {
            result: Some(wire::apply_tunnel_reply::Result::Pending(
                wire::ApplyAccepted {
                    operation_id,
                    authority_fence: Some(wire::AuthorityFence::default()),
                },
            )),
        }))
    }

    async fn stop_tunnel(
        &self,
        request: Request<wire::StopTunnelRequest>,
    ) -> Result<Response<wire::StopTunnelReply>, Status> {
        let request = request.into_inner();
        if request
            .lookup_key
            .as_ref()
            .is_none_or(|key| key.operation_id.len() != 16)
        {
            return Err(Status::invalid_argument(APPLY_INVALID_MESSAGE));
        }
        // Stopped 只能在 pipeline 实际退出且 ledger 清空后回复。此前这里把 attempt/live
        // 槽直接置空，导致 Core 先显示资源已回收而路由、utun 和泵仍由后台任务持有。
        self.runtime
            .stop_and_wait_for_cleanup()
            .await
            .map_err(|error| {
                Status::internal(format!("DARWIN_ENGINE_CLEANUP_INCOMPLETE: {error}"))
            })?;
        Ok(Response::new(wire::StopTunnelReply {
            result: Some(wire::stop_tunnel_reply::Result::Stopped(
                wire::MutationReceipt::default(),
            )),
        }))
    }

    async fn get_operation(
        &self,
        _request: Request<wire::GetOperationRequest>,
    ) -> Result<Response<wire::GetOperationReply>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn release_lease(
        &self,
        _request: Request<wire::ReleaseLeaseRequest>,
    ) -> Result<Response<wire::ReleaseLeaseReply>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn stream_logs(
        &self,
        _request: Request<wire::StreamLogsRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        // MAC-OBS-13 S1：同一已认证控制通道上的既有 `StreamLogs` 接缝（不新建通道）。
        // `resume_tick` 契约为 0 = 从当前流位置开始；Engine 无离线缓冲补拉，断线缺口
        // 由既有落盘诊断（pump::diag / pipeline println）离线兜底。
        let receiver = self.runtime.logs.open_stream();
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
        )))
    }

    async fn stream_stats(
        &self,
        request: Request<wire::StreamStatsRequest>,
    ) -> Result<Response<Self::StreamStatsStream>, Status> {
        // W-MVP-13：同一已认证控制通道上的既有统计推送接缝（不新建通道）。
        let interval = request.into_inner().sample_interval_ms;
        let receiver = self.runtime.stats.open_stream(interval);
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
        )))
    }

    async fn stream_connect_status(
        &self,
        _request: Request<wire::StreamConnectStatusRequest>,
    ) -> Result<Response<Self::StreamConnectStatusStream>, Status> {
        // attach-before-apply：Core 先挂本流，再 ApplyTunnel；事件经广播分发。
        let stream = BroadcastStream::new(self.runtime.status_tx.subscribe())
            .map(|item| item.map_err(|_| Status::internal(STATUS_LAGGED_MESSAGE)));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn keep_alive(
        &self,
        request: Request<wire::KeepAliveRequest>,
    ) -> Result<Response<wire::KeepAliveReply>, Status> {
        // W2 心跳（E7 两层切分的活性层，本波为会话形态的退出权威）：单连接 accept
        // 模型下只有唯一已认证连接上的 RPC 才能到达本 handler（对端已验证，对齐
        // win32「transport 已 gate、handler 无需二次校验」的语义）。语义 = touch
        // 心跳时间戳 + 回显 `monotonic_tick`（照 win32：tick 仅诊断序号，不校验
        // 递增）。高频心跳不打日志（win32 同款取舍）。
        let tick = request.get_ref().monotonic_tick;
        if let Ok(mut watch) = self.runtime.heartbeat.lock() {
            watch.touch();
        }
        Ok(Response::new(wire::KeepAliveReply {
            monotonic_tick: tick,
        }))
    }

    async fn service_manage(
        &self,
        _request: Request<wire::ServiceManageRequest>,
    ) -> Result<Response<wire::ServiceManageReply>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn shutdown(
        &self,
        _request: Request<wire::ShutdownRequest>,
    ) -> Result<Response<wire::ShutdownReply>, Status> {
        // 2026-09-05 方案 B（win32 Shutdown RPC）的 darwin 侧实装随 W2.5 分叉：
        // - service 形态（显式停止槽在位）：请求显式停止 → 会话循环消费本请求，
        //   执行「拆隧道 → exit(0)」——launchd `KeepAlive={SuccessfulExit:false}`
        //   下干净退出不复活（权威规范 §二.2.4：显式停止是 service 形态的退出权威）。
        //   回 ACCEPTED 与 win32 语义对齐（armed 同一退出序列；本 handler 不直接
        //   exit——先让 RPC 应答落盘，退出由会话循环在收敛后执行）。
        // - 会话/oneshot 形态（无槽）：生命周期随 core 会话，显式停机「不适用」
        //   ——维持 NOT_APPLICABLE（解冻记录 T7 责任项的 darwin 部分至此为真：
        //   service 形态已存在，本 handler 不再是无条件 stub）。
        if self.runtime.explicit_stop.is_some() {
            self.runtime.request_explicit_stop();
            return Ok(Response::new(wire::ShutdownReply {
                outcome: wire::ShutdownOutcome::Accepted as i32,
            }));
        }
        Ok(Response::new(wire::ShutdownReply {
            outcome: wire::ShutdownOutcome::NotApplicable as i32,
        }))
    }
}

/// 平台管线的结果分类。
enum TunnelOutcome {
    /// 用户取消/Stop：已按序 teardown。
    Cancelled,
    /// CSTP 会话在持有期死亡。
    SessionLost,
    /// 平台步骤失败：步骤名、OS errno、DNS 诊断标记。
    Failed(&'static str, i32, i32),
}

/// 一次连接的完整平台管线：utun → 地址/MTU → 路由 → DNS → 双泵 → Connected →
/// 保持（DPD 应答 + keepalive + 泵监视）→ 逆序 teardown。
///
/// 失败时只逆序恢复已真实获取的资源；取消/会话死亡/平台失败都走同一条 teardown。
#[allow(clippy::too_many_lines)]
async fn run_tunnel_pipeline(
    runtime: &EngineRuntime,
    established: EstablishedSession,
    operation_id: Vec<u8>,
    plan_mtu: u32,
    campus_routes: Vec<String>,
    cancel: &Arc<AtomicBool>,
) -> TunnelOutcome {
    // Connected 时刻的 epoch ms：status 事件与会话时长诊断共用的单一事实源；
    // 0 = 尚未到达 Connected。Atomic 是因为闭包随管线 future 跨线程（Cell 非 Send）。
    let session_started_ms = std::sync::atomic::AtomicI64::new(0);
    let own_tunnel_if_index = std::sync::atomic::AtomicU32::new(0);
    let publish = |phase: wire::ConnectPhase, coarse: wire::StatsPhase| {
        // MAC-OBS-13 S1：连接状态推进同步进 sink（每阶段一条 info；字段只含
        // 白名单阶段名，无凭据/证书材料）。
        runtime.logs.publish(
            "info",
            "pipeline",
            "PHASE",
            format!("connect phase -> {phase:?} coarse {coarse:?}"),
            &[
                ("phase", format!("{phase:?}")),
                ("coarse", format!("{coarse:?}")),
            ],
        );
        let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
            own_tunnel_if_index: own_tunnel_if_index.load(Ordering::Relaxed),
            session_established_at_ms: session_started_ms.load(Ordering::Relaxed),
            operation_id: operation_id.clone(),
            connect_phase: phase as i32,
            coarse_phase: coarse as i32,
            error: None,
        });
    };
    let is_cancelled = || cancel.load(std::sync::atomic::Ordering::SeqCst);
    let errno_of = |error: &crate::platform::PlatformError| match error {
        crate::platform::PlatformError::Utun(_, e)
        | crate::platform::PlatformError::Interface(_, e)
        | crate::platform::PlatformError::Route(_, e) => e.raw_os_error().unwrap_or(0),
        _ => 0,
    };

    let EstablishedSession {
        offer,
        egress: _,
        write_channel,
        read_channel,
        mut control_rx,
    } = established;

    // C2 延迟探测：据真实 offer 构建探测配置（目标 = 客户端隧道子网网络基地址
    // `network_base`；真实学校网关应答沿 win32 S5 口径待真机校准）。节拍由本保持
    // 循环的 1s tick 驱动，回包匹配挂下行泵（LatencyTap），RTT 写共享统计注册表；
    // 手动刷新 marker 不做（见 `packet/probe.rs` 模块注释的取舍）。
    let latency_probe = Arc::new(Mutex::new(ProbeState::new(LatencyProbeConfig {
        target: crate::packet::probe::network_base(offer.ipv4_address, offer.prefix),
        source: offer.ipv4_address,
        dpd_enabled: crate::packet::probe::DPD_PROBE_ENABLED,
        ping_interval: std::time::Duration::from_secs(
            crate::packet::probe::LATENCY_PING_INTERVAL_SECS,
        ),
    })));

    // ---- 1. utun（AttachingPacketBoundary 真实入口）。----
    publish(
        wire::ConnectPhase::ApplyingPlatformTunnel,
        wire::StatsPhase::Connecting,
    );
    let device = match runtime.take_or_acquire_device() {
        Ok(device) => device,
        Err(error) => {
            // 登录前已可能写入控制 `/32`；即使 utun 创建失败也必须在非重试终态前
            // 尝试收口，不能让失败分支绕开 ledger。
            let _ = runtime.complete_resource_cleanup();
            return TunnelOutcome::Failed("UTUN", errno_of(&error), 0);
        }
    };
    own_tunnel_if_index.store(device.ifindex(), Ordering::Relaxed);
    publish(
        wire::ConnectPhase::AttachingPacketBoundary,
        wire::StatsPhase::Connecting,
    );

    // ---- 2. 地址/MTU/路由/DNS（ApplyingPlatformTunnel；失败逆序回滚已应用项）。----
    let mut applied_routes: Vec<AppliedRoute> = Vec::new();
    let effective_mtu = if plan_mtu > 0 && plan_mtu < u32::from(offer.mtu) {
        u16::try_from(plan_mtu).unwrap_or(offer.mtu)
    } else {
        offer.mtu
    };
    let mut negotiated = crate::session_diagnostics::identity(&operation_id);
    negotiated.extend(crate::session_diagnostics::build_fields());
    negotiated.extend([
        ("offer_mtu", offer.mtu.to_string()),
        ("requested_mtu", plan_mtu.to_string()),
        ("effective_mtu", effective_mtu.to_string()),
        ("tunnel_ifindex", device.ifindex().to_string()),
    ]);
    runtime.logs.publish(
        "info",
        "cstp",
        "TUNNEL_EFFECTIVE_PARAMETERS",
        "隧道待应用参数",
        &negotiated,
    );

    if let Err(error) = crate::platform::interface::apply_ipv4(
        &device,
        offer.ipv4_address,
        offer.prefix,
        effective_mtu,
    ) {
        let errno = errno_of(&error);
        let substep = match &error {
            crate::platform::PlatformError::Interface(name, _)
            | crate::platform::PlatformError::Readback(name) => name,
            _ => "ADDRESS",
        };
        println!("[pipeline] address failed at {substep}: {error}");
        runtime.logs.publish(
            "error",
            "platform",
            "ADDRESS_FAILED",
            format!("address failed at {substep}: {error}"),
            &[
                ("substep", substep.to_owned()),
                ("errno", errno.to_string()),
            ],
        );
        drop(device);
        // 地址阶段失败时业务路由尚未开始，但控制 `/32` 已在 establish 前创建；统一走
        // ledger，保证后续非重试 Failed 之前没有被遗漏的专用路由或旧 idle utun。
        let _ = runtime.complete_resource_cleanup();
        return TunnelOutcome::Failed(substep, errno, 0);
    }

    let mut wanted: Vec<(Ipv4Addr, u8)> = Vec::new();
    for text in offer.routes.iter().chain(campus_routes.iter()) {
        if let Some(cidr) = parse_cidr(text)
            && !wanted.contains(&cidr)
        {
            wanted.push(cidr);
        }
    }
    for server in &offer.dns_servers {
        let cidr = (*server, 32);
        if !wanted.contains(&cidr) {
            wanted.push(cidr);
        }
    }
    let mut route_failure: Option<(&'static str, i32)> = None;
    for (destination, prefix) in wanted {
        // 网关 = 本隧道地址（INET 网关形态，Clash 同款）：AF_LINK 接口路由形态
        // 在 p2p utun 上建路由时 rt_ifa 解析不到接口地址，未绑定源的 sendto 报
        // EADDRNOTAVAIL；经本隧道地址转发时内核按网关命中 dstaddr=自身，源选择
        // 取本接口地址。
        let applied = AppliedRoute {
            destination,
            prefix,
            gateway: offer.ipv4_address,
            ifindex: device.ifindex(),
        };
        if let Err(error) = platform_route::add(&applied) {
            let errno = match &error {
                crate::platform::PlatformError::Route(_, e) => e.raw_os_error().unwrap_or(0),
                _ => 0,
            };
            route_failure = Some(("ROUTE", errno));
            break;
        }
        applied_routes.push(applied);
    }

    // DNS：不改系统 DNS（Windows 对齐，见 platform/mod.rs 说明）；
    // offer DNS 服务器的 /32 路由已加入，隧道侧可直接查询。
    let dns_failure: Option<(&'static str, i32)> = None;

    if let Some((step, errno)) = route_failure.or(dns_failure) {
        // 逆序恢复：路由 → utun（系统 DNS 未被修改，无需恢复）。
        for route in applied_routes.drain(..).rev() {
            if platform_route::delete(&route).is_err() {
                runtime.remember_route_cleanup_failure(route);
            }
        }
        drop(device);
        let _ = runtime.complete_resource_cleanup();
        return TunnelOutcome::Failed(step, errno, 0);
    }

    // ---- 3. 双泵（StartingDataPlane 真实入口）+ Connected。----
    publish(
        wire::ConnectPhase::StartingDataPlane,
        wire::StatsPhase::Connecting,
    );
    let pumps = pump::start(
        device.fd(),
        write_channel.clone(),
        read_channel,
        &runtime.logs,
        Some(LatencyTap::new(
            Arc::clone(&latency_probe),
            Arc::clone(runtime.stats.registry()),
        )),
    );
    // W-MVP-13：挂接本次双泵计数器并把粗阶段置为 Connected（统计样本的事实源）。
    runtime
        .stats
        .registry()
        .attach_pump(Arc::clone(&pumps.rx_bytes), Arc::clone(&pumps.tx_bytes));
    runtime
        .stats
        .registry()
        .set_phase(wire::StatsPhase::Connected);
    // 数据面真实运行即会话起点——Connected 事件带上真实时间戳（前端在线时长的
    // 事实源；此前恒 0 导致 UI 永远显示「—」）。
    session_started_ms.store(now_epoch_ms(), Ordering::Relaxed);
    publish(
        wire::ConnectPhase::StartingDataPlane,
        wire::StatsPhase::Connected,
    );

    // ---- 4. 保持：DPD 应答 + 周期 keepalive + 泵监视。----
    let keepalive = Codec::encode_raw(CSTP_PACKET_TYPE_KEEPALIVE, &[]).expect("keepalive frame");
    let dpd_response =
        Codec::encode_raw(CSTP_PACKET_TYPE_DPD_RESPONSE, &[]).expect("dpd response frame");
    if let Ok(mut live) = runtime.live.lock() {
        *live = Some(HeldTunnel);
    }
    let mut diagnostics = crate::session_diagnostics::SessionDiagnostics::new();
    let mut session_lost = false;
    let mut ticks: u32 = 0;
    loop {
        if is_cancelled()
            || runtime.shutdown.load(Ordering::SeqCst)
            || TERMINATION_REQUESTED.load(Ordering::SeqCst)
        {
            break;
        }
        if pumps.either_finished() {
            // 任一泵死亡 = 数据面失败，不维持 Connected。
            println!("[pipeline] pump finished unexpectedly");
            runtime.logs.publish(
                "error",
                "packet",
                "PUMP_EXITED",
                "a pump thread exited while tunnel held",
                &[],
            );
            session_lost = true;
            break;
        }
        tokio::select! {
            biased;
            control = control_rx.recv() => {
                if let Some(event) = &control { diagnostics.observe(event, &runtime.logs, &operation_id); }
                match control {
                Some(CstpControlEvent::Control { kind: CSTP_PACKET_TYPE_DPD_REQUEST, .. }) => {
                    pump::diag("control: DPD request -> reply");
                    if write_channel.send(dpd_response.clone()).is_err() {
                        diagnostics.queue_failures += 1; session_lost = true;
                    } else { diagnostics.dpd_reply_queued += 1; }
                }
                Some(CstpControlEvent::DpdResponse) => {
                    // C2 主动 DPD 探测（默认关，`DPD_PROBE_ENABLED`）的应答计时；
                    // 与上一分支网关 DPD request 的被动应答（会话维持）是不同路径。
                    pump::diag("control: DPD response -> rtt");
                    if let Some(rtt) = latency_probe
                        .lock()
                        .expect("latency probe lock")
                        .on_dpd_response(std::time::Instant::now())
                    {
                        runtime.stats.registry().record_latency(rtt);
                    }
                }
                Some(CstpControlEvent::Control { kind: CSTP_PACKET_TYPE_KEEPALIVE, .. }) => {}
                Some(CstpControlEvent::Control { kind, .. }) => {
                    pump::diag(&format!("control: kind={kind:#04x}"));
                    // 非平凡控制帧（如 0x05 = 网关 DISCONNECT）此前只落 /tmp 泵日志，
                    // 聚合日志不可见——服务端主动断线只能靠事后拼时间线。
                    runtime.logs.publish(
                        "warn",
                        "cstp",
                        "CSTP_CONTROL_FRAME",
                        format!("server sent control frame kind={kind:#04x}"),
                        &[
                            ("kind", format!("{kind:#04x}")),
                            (
                                "connected_for_ms",
                                elapsed_since_ms(session_started_ms.load(Ordering::Relaxed)).to_string(),
                            ),
                        ],
                    );
                }
                Some(CstpControlEvent::WriteFailed(_) | CstpControlEvent::SessionEnded(_)) => {
                    session_lost = true;
                }
                Some(CstpControlEvent::IoDiagnostics(_)) => {}
                None => {
                    pump::diag("control channel closed (CSTP read task ended)");
                    session_lost = true;
                }
            }},
            () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                ticks += 1;
                if ticks.is_multiple_of(15) {
                    pump::diag("keepalive sent");
                    if write_channel.send(keepalive.clone()).is_err() {
                        diagnostics.queue_failures += 1; session_lost = true;
                    } else { diagnostics.keepalive_queued += 1; }
                }
                // C2 延迟探测节拍：周期 ping / 超时回退。探测包经 write_channel 注入
                // 上行（CSTP Data 帧）；发送失败 = TLS 写任务已退出 → 尽力而为忽略
                // （对齐 win32），掉线由泵/控制面观察点上报。
                latency_probe
                    .lock()
                    .expect("latency probe lock")
                    .tick(std::time::Instant::now(), &mut |frame: Vec<u8>| {
                        let _ = write_channel.send(frame);
                    });
            }
        }
        diagnostics.periodic(&runtime.logs, &operation_id);
        if session_lost {
            break;
        }
    }

    // ---- 5. 逆序 teardown：泵 → CSTP → 路由 → utun。----
    // 有界收取已经到达的最终样本，不等待网络任务、不改变清理顺序。
    for _ in 0..256 {
        match control_rx.try_recv() {
            Ok(event) => diagnostics.observe(&event, &runtime.logs, &operation_id),
            Err(_) => break,
        }
    }
    diagnostics.emit(
        &runtime.logs,
        &operation_id,
        if session_lost {
            "session_lost"
        } else {
            "stop_requested"
        },
    );
    let teardown_started = std::time::Instant::now();
    runtime.logs.publish(
        "debug",
        "engine",
        "TEARDOWN_BEGIN",
        "开始清理隧道资源",
        &crate::session_diagnostics::identity(&operation_id),
    );
    pump::diag("teardown begin");
    runtime.stats.registry().set_phase(wire::StatsPhase::Idle);
    runtime.stats.registry().detach_pump();
    pumps.shutdown();
    drop(write_channel);
    drop(control_rx);
    let mut teardown_complete = true;
    for (index, route) in applied_routes.drain(..).enumerate() {
        if let Err(error) = platform_route::delete(&route) {
            pump::diag(&format!("teardown route #{index}: {error}"));
            runtime.remember_route_cleanup_failure(route);
            teardown_complete = false;
        }
    }
    // 只有业务路由均已删除时才允许把 utun 留给重连。若先前删除失败，先走完整
    // ledger 清理并放弃复用；不得让“空闲 utun”与残留 VPN 专用路由同时存在。
    let preserve_for_reconnect = session_lost && !is_cancelled() && teardown_complete;
    if preserve_for_reconnect {
        if let Err(error) = crate::platform::interface::clear_ipv4(&device, offer.ipv4_address) {
            runtime.logs.publish(
                "error",
                "platform",
                "IDLE_UTUN_ADDRESS_CLEAR_FAILED",
                format!("failed to remove address from retained utun: {error}"),
                &[],
            );
            // 保留带旧地址的 utun 会继续拥有源地址，违反空闲设备边界；失败时宁可
            // 释放设备，并由下一次重连重新创建。
            teardown_complete = false;
            drop(device);
            if let Err(cleanup_error) = runtime.complete_resource_cleanup() {
                runtime.logs.publish(
                    "error",
                    "engine",
                    "TEARDOWN_RESOURCE_REMAINS",
                    format!("resource cleanup incomplete: {cleanup_error}"),
                    &[],
                );
            } else {
                // 地址移除失败后不复用本设备；ledger 已成功收口时允许后续以新设备重连。
                teardown_complete = true;
            }
        } else {
            runtime.retain_device(device);
            pump::diag("teardown: retained idle device for pending reconnect");
        }
    } else {
        drop(device);
        if let Err(error) = runtime.complete_resource_cleanup() {
            teardown_complete = false;
            runtime.logs.publish(
                "error",
                "engine",
                "TEARDOWN_RESOURCE_REMAINS",
                format!("resource cleanup incomplete: {error}"),
                &[],
            );
        } else {
            // 先前一次逐条删除可以暂态失败；完整账本重试若成功，资源已如实清空，
            // 仅放弃本轮 utun 复用，下一次连接重新读取出口并创建设备即可。
            teardown_complete = true;
        }
        pump::diag("teardown: device dropped");
    }
    let mut teardown_fields = crate::session_diagnostics::identity(&operation_id);
    teardown_fields.push((
        "elapsed_ms",
        teardown_started.elapsed().as_millis().to_string(),
    ));
    runtime.logs.publish(
        if teardown_complete { "info" } else { "error" },
        "engine",
        if teardown_complete {
            "TEARDOWN_COMPLETED"
        } else {
            "TEARDOWN_INCOMPLETE"
        },
        if teardown_complete {
            "隧道资源已完成清理"
        } else {
            "隧道资源清理未完成，失败条目保留待重试"
        },
        &teardown_fields,
    );
    TERMINATION_TEARDOWN_DONE.store(true, Ordering::SeqCst);
    if let Ok(mut live) = runtime.live.lock() {
        *live = None;
    }
    if let Ok(mut slot) = runtime.attempt.lock() {
        *slot = None;
    }

    if session_lost && !is_cancelled() {
        TunnelOutcome::SessionLost
    } else {
        TunnelOutcome::Cancelled
    }
}

/// 解析 canonical IPv4 CIDR（Engine 侧对用户意图的再验证；拒绝 0.0.0.0/0）。
fn parse_cidr(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (address_text, prefix_text) = text.split_once('/')?;
    let prefix = prefix_text.parse::<u8>().ok()?;
    if !(1..=32).contains(&prefix) || prefix.to_string() != prefix_text {
        return None;
    }
    let address: Ipv4Addr = address_text.parse().ok()?;
    if address.to_string() != address_text || address.is_unspecified() {
        return None;
    }
    let mask = u32::MAX << (32 - u32::from(prefix));
    (u32::from(address) & mask == u32::from(address)).then_some((address, prefix))
}

/// 取消场景的 typed VpnError 组装辅助（保持 match 分支可读）。
struct ConnectCancelled;

impl ConnectCancelled {
    fn vpn_error() -> wire::VpnError {
        wire::VpnError {
            code: wire::ErrorCode::CancelledBeforeStart as i32,
            stage: wire::ErrorStage::ConnectingControl as i32,
            ..Default::default()
        }
    }

    /// 持有期间会话丢失（网关关闭/TLS 断链/泵死亡）的 typed 失败。
    ///
    /// W3-2/P3（主轨升级）：此错误只发生在 `establish` 成功、数据面曾真实运行之后
    /// （泵意外退出 / DPD 失败 / 控制通道关闭），故 `stage` 升级为 `DataPlane` 且
    /// `retry=RetrySameOperation`——Core 侧以「`stage=DataPlane + retry=
    /// RetrySameOperation`」为**可重试掉线**的唯一标记（镜像 win32 C2 语义）触发数据面
    /// 自动重连；与 Core 自身的 engine 失联终态（`ProtocolSession` +
    /// `UseNewOperation`，流 EOF 路径）靠 retry 字段明确区分。零 proto 改动
    /// （`ErrorStage`/`RetryAdvice` 枚举既有），`code=EffectUnknown` 维持平台类通用码
    /// 不参与判定。
    fn session_lost_vpn_error() -> wire::VpnError {
        wire::VpnError {
            code: wire::ErrorCode::EffectUnknown as i32,
            stage: wire::ErrorStage::DataPlane as i32,
            retry: wire::RetryAdvice::RetrySameOperation as i32,
            ..Default::default()
        }
    }

    /// 平台步骤失败的 typed 失败；code 区分步骤，errno/retry 为诊断值。
    fn platform_vpn_error(step: &'static str, errno: i32, marker: i32) -> wire::VpnError {
        use wire::ErrorCode;
        let (code, stage) = match step {
            "UTUN" => (
                ErrorCode::EffectUnknown,
                wire::ErrorStage::AttachingPacketBoundary,
            ),
            "PUMP" => (
                ErrorCode::DataPlaneBackpressure,
                wire::ErrorStage::AttachingPacketBoundary,
            ),
            "ADDRESS" | "SIOCSIFADDR" => (
                ErrorCode::ObservationFailed,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCSIFDSTADDR" => (
                ErrorCode::IdempotencyConflict,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCGIFFLAGS" => (
                ErrorCode::ConnectInProgress,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCSIFFLAGS" => (
                ErrorCode::SessionBusy,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCSIFMTU" => (
                ErrorCode::ReconnectAlreadyQueued,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "ROUTE" => (
                ErrorCode::ObservedConflict,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            _ => (
                ErrorCode::JournalCorrupt,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
        };
        wire::VpnError {
            code: code as i32,
            stage: stage as i32,
            certainty: wire::EffectCertainty::Applied as i32,
            retry: marker,
            subject: None,
            resource: None,
            native: Some(wire::RedactedNativeError {
                category: wire::NativeErrorCategory::Resource as i32,
                namespace: wire::NativeErrorNamespace::PosixErrno as i32,
                code: i64::from(errno),
            }),
        }
    }
}
