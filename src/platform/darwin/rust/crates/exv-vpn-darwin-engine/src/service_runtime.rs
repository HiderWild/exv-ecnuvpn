//! W2.5：engine `--service` 常驻形态（权威规范 §二.2 service engine 目标态）。
//!
//! 与既有 per-core-session 会话形态（[`crate::bootstrap_runtime`]）的关键分叉：
//!
//! - **拉起与身份**：launchd 以 root 拉起（`RunAtLoad` + `KeepAlive={SuccessfulExit:
//!   false}`），引擎自检允许 uid=euid=0 起步，启动时自 `setreuid(owner_uid, 0)`
//!   （替代旧伴侣会话形态的 `pre_exec` trick），随后与现会话形态同一套管线。
//! - **端点与认证**：固定路径 `--control-socket`（生产为 root 控制目录内的
//!   `engine.sock`）；认证=单因子 uid 门（`getpeereid == --owner-uid`，无 HMAC
//!   ticket、无 pid 钉死——规范 §二.2.7 既定边界）。
//! - **顺序单会话循环**：`accept → arm 心跳 → 服务 → EOF/挂起超时 → 拆隧道
//!   （W1 收敛减去进程退出）→ 回 accept`；第二并发连接 typed 拒绝（既有
//!   `ATTEMPT_ACTIVE` 门词汇），不 evict。
//! - **退出权威分叉**：显式停止（`Shutdown` RPC）与 SIGTERM = 拆隧道 → exit(0)
//!   （`SuccessfulExit:false` 下不复活）；EOF 与心跳超时对 service 形态只拆隧道、
//!   不退出（E7 两层心跳：活性层共用，退出层专属 oneshot/会话形态）。

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicU64},
    time::Duration,
};

use async_trait::async_trait;
use exv_vpn_darwin_ipc::{
    AuthenticatedIncoming,
    path::SocketPath,
    service_listener::{ServiceConnection, ServiceListener, ServiceListenerConfig},
};
use exv_vpn_wire::generated::{
    self as wire,
    helper_control_server::{HelperControl, HelperControlServer},
};
use tokio_stream::Empty;
use tonic::{Request, Response, Status, transport::Server};

use crate::bootstrap_runtime::{
    ATTEMPT_ACTIVE_MESSAGE, EngineBootstrapError, EngineControlService, EngineRuntime,
    HEARTBEAT_TIMEOUT, HEARTBEAT_WATCHDOG_CHECK_INTERVAL, TERMINATION_REQUESTED,
    converge_and_await_teardown, poll_signal_flag, spawn_heartbeat_watchdog_with,
};
use crate::core_exit_policy::CoreExitPolicy;
use crate::file_log;

/// 会话代数计数器（进程内单调递增；取证时区分同一 engine 进程内的多个会话）。
static SESSION_GENERATION: AtomicU64 = AtomicU64::new(0);

/// 取下一会话代数（从 1 起）。
fn next_session_generation() -> u64 {
    SESSION_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// service 形态文件日志快捷入口（component 固定 `service`）。
///
/// 挂点纪律（S7）：事件字段只携带 pid/generation/决策布尔/错误类别一类取证
/// 元数据；凭据/cookie/token/session id 一律不进日志（`file_log` 的 key 脱敏是
/// 兜底防线，不是许可）。
fn service_log(level: &str, code: &str, message: &str, fields: &[(&str, String)]) {
    file_log::default_log().log(level, "service", code, message, fields);
}

/// [`EngineBootstrapError`] 的稳定短类别（日志字段用；不展开内部错误细节）。
fn bootstrap_error_category(error: &EngineBootstrapError) -> &'static str {
    match error {
        EngineBootstrapError::Arguments => "arguments",
        EngineBootstrapError::Credentials => "credentials",
        EngineBootstrapError::RuntimeDir(_) => "runtime_dir",
        EngineBootstrapError::Ticket(_) => "ticket",
        EngineBootstrapError::Listener => "listener",
        EngineBootstrapError::Pipe => "pipe",
        EngineBootstrapError::Server => "server",
    }
}

/// 非对端发起的会话终结（心跳超时/显式停止/信号）在强制收权底层 socket 前的
/// 应答排空宽限：本地 UDS 上的在途应答（含 Shutdown 的 ACCEPTED）flush 是微秒级，
/// 宽限只为覆盖调度抖动；宽限后无条件收权——挂起对端不得把常驻进程钉死在旧
/// 会话上（tonic 连接任务无法从外部 abort，只能内核层 shutdown）。
const REPLY_FLUSH_GRACE: Duration = Duration::from_millis(250);

/// 单会话的生命周期节奏（生产默认=W1/W2 常量；单测注入毫秒级小间隔加速真实
/// socket 集成——auto-advance 虚拟时钟与真实 IO 驱动互踩，见心跳超时测试注释）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionHeartbeatTiming {
    /// 看门狗检查节拍（生产 [`HEARTBEAT_WATCHDOG_CHECK_INTERVAL`]）。
    pub(crate) check_interval: Duration,
    /// 心跳超时上界（生产 [`HEARTBEAT_TIMEOUT`]）。
    pub(crate) timeout: Duration,
}

impl Default for SessionHeartbeatTiming {
    fn default() -> Self {
        Self {
            check_interval: HEARTBEAT_WATCHDOG_CHECK_INTERVAL,
            timeout: HEARTBEAT_TIMEOUT,
        }
    }
}

/// `--service` 形态的固定 argv 材料。
///
/// 形态：`--service --owner-uid N --control-socket P`（launchd engine job plist 的
/// `ProgramArguments` 原文；P 为固定端点路径，生产为
/// `/Library/Application Support/EXV/ServiceAgent/engine.sock`）。
pub(crate) struct ServiceArgs {
    owner_uid: u32,
    control_socket: PathBuf,
}

impl ServiceArgs {
    /// 解析 `--service` 形态 argv（`values[0]` 已由调用方确认为 `--service`）。
    ///
    /// # Errors
    ///
    /// 参数数量、flag 次序、root owner uid 或非法 socket 路径返回
    /// [`EngineBootstrapError::Arguments`]。
    pub(crate) fn from_values(values: &[OsString]) -> Result<Self, EngineBootstrapError> {
        // [--service, --owner-uid, N, --control-socket, P]
        if values.len() != 5
            || values[1] != "--owner-uid"
            || values[3] != "--control-socket"
        {
            return Err(EngineBootstrapError::Arguments);
        }
        let owner_uid = values[2]
            .to_str()
            .and_then(|value| value.parse().ok())
            .filter(|value: &u32| *value != 0)
            .ok_or(EngineBootstrapError::Arguments)?;
        let control_socket = PathBuf::from(&values[4]);
        // 端点路径进 listener 前先过 SocketPath 词法门（绝对、短于 sun_path、无 NUL）。
        SocketPath::new(&control_socket).map_err(|_| EngineBootstrapError::Arguments)?;
        Ok(Self {
            owner_uid,
            control_socket,
        })
    }
}

/// `--service` 形态进程入口：root 起步自检 → 自降权 → 顺序会话循环。
///
/// # Errors
///
/// argv、身份（非 root 起步/降权失败）、端点 bind 或 listener 级失败返回稳定
/// [`EngineBootstrapError`]；`Shutdown`/SIGTERM 的干净退出以 `Ok`（exit 0）返回。
pub(crate) fn run_service_from_process(
    values: &[OsString],
) -> Result<(), EngineBootstrapError> {
    let args = match ServiceArgs::from_values(values) {
        Ok(args) => args,
        Err(error) => {
            service_log(
                "error",
                "SERVICE_FORM_START_FAILED",
                "service form argv rejected",
                &[("error", bootstrap_error_category(&error).to_owned())],
            );
            return Err(error);
        }
    };
    if let Err(error) = demote_to_service_credentials(args.owner_uid) {
        service_log(
            "error",
            "SERVICE_FORM_START_FAILED",
            "service form credential demotion failed",
            &[
                ("error", bootstrap_error_category(&error).to_owned()),
                ("stage", "demote".to_owned()),
            ],
        );
        return Err(error);
    }
    service_log(
        "info",
        "SERVICE_FORM_STARTED",
        "service engine form started",
        &[
            ("owner_uid", args.owner_uid.to_string()),
            ("pid", std::process::id().to_string()),
        ],
    );
    let Ok(tokio_runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    else {
        service_log(
            "error",
            "SERVICE_FORM_START_FAILED",
            "service form tokio runtime build failed",
            &[(
                "error",
                bootstrap_error_category(&EngineBootstrapError::Server).to_owned(),
            )],
        );
        return Err(EngineBootstrapError::Server);
    };
    let outcome = tokio_runtime.block_on(run_service_loop(
        SocketPath::new(&args.control_socket).map_err(|_| EngineBootstrapError::Arguments)?,
        args.owner_uid,
    ));
    match &outcome {
        Ok(()) => service_log(
            "info",
            "SERVICE_FORM_EXIT",
            "service engine form exited cleanly",
            &[],
        ),
        Err(error) => service_log(
            "error",
            "SERVICE_FORM_EXIT",
            "service engine form exited with listener error",
            &[("error", bootstrap_error_category(error).to_owned())],
        ),
    }
    outcome
}

/// service 形态身份自检与自降权：允许 root 起步（launchd 拉起），随后
/// `setreuid(owner_uid, 0)` 达到与会话形态一致的 real=owner/euid=0 身份态。
///
/// # Errors
///
/// 非 uid=euid=0 起步或 setreuid 原操作失败返回
/// [`EngineBootstrapError::Credentials`]。
fn demote_to_service_credentials(owner_uid: u32) -> Result<(), EngineBootstrapError> {
    // SAFETY: these calls only read this process credentials.
    let (uid, euid) = unsafe { (libc::getuid(), libc::geteuid()) };
    if uid != 0 || euid != 0 {
        return Err(EngineBootstrapError::Credentials);
    }
    // SAFETY: setreuid 只改本进程身份；参数为固定 u32（owner 来自已校验 argv，
    // 非 root）；目标 real=owner/euid=0 与会话形态对齐，失败按原返回处理。
    if unsafe { libc::setreuid(owner_uid as libc::uid_t, 0) } != 0 {
        return Err(EngineBootstrapError::Credentials);
    }
    Ok(())
}

/// service 形态主循环的整体终态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceLoopOutcome {
    /// 干净退出（SIGTERM/显式停止/成功收敛）：exit(0)，KeepAlive 不复活。
    CleanExit,
    /// listener 级失败（端点不可再用）：按错误退出（launchd 视为崩溃可自愈重启）。
    ListenerError,
}

/// 单个会话的终态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionEnd {
    /// 会话结束但进程常驻：回 accept 等下一个 core（EOF/心跳超时）。
    BackToAccept,
    /// 干净退出（SIGTERM/显式停止）。
    CleanExit,
}

/// 会话终态 + 会话结束时 runtime 的归属（T1 keep 分支携带跨会话；teardown 路径
/// 返回 None——runtime 随收敛消亡）。
struct SessionOutcome {
    end: SessionEnd,
    kept_runtime: Option<Arc<EngineRuntime>>,
}

/// T1：service 会话该终结触发是否进入 keep 分支（与 oneshot 的
/// [`crate::bootstrap_runtime::core_exit_keeps_tunnel`] 同语义；显式停止/信号豁免）。
fn session_end_keeps_tunnel(trigger: SessionTrigger, policy: CoreExitPolicy) -> bool {
    matches!(
        trigger,
        SessionTrigger::CoreClosed | SessionTrigger::HeartbeatTimeout
    ) && policy == CoreExitPolicy::KeepTunnel
}

/// 触发会话结束的源（诊断标注；EOF/超时共用「拆隧道不退出」路径）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionTrigger {
    /// 已准入连接关闭（EOF/失联）。
    CoreClosed,
    /// SIGTERM/SIGINT（launchd 停止/用户信号）。
    PosixSignals,
    /// W2 心跳超时（core 挂起 15s 无 `KeepAlive`）。
    HeartbeatTimeout,
    /// `Shutdown` RPC 显式停止（service 形态退出权威）。
    ExplicitStop,
}

/// bind 固定端点并进入顺序会话循环；退出路径 guarded 清理端点。
async fn run_service_loop(
    socket_path: SocketPath,
    owner_uid: u32,
) -> Result<(), EngineBootstrapError> {
    run_service_loop_with_timing(
        socket_path,
        owner_uid,
        &TERMINATION_REQUESTED,
        SessionHeartbeatTiming::default(),
    )
    .await
}

/// [`run_service_loop`] 的可注入信号源+心跳节奏变体（单测专用入口）。
async fn run_service_loop_with_timing(
    socket_path: SocketPath,
    owner_uid: u32,
    termination_requested: &AtomicBool,
    timing: SessionHeartbeatTiming,
) -> Result<(), EngineBootstrapError> {
    let listener = Arc::new(
        ServiceListener::bind(ServiceListenerConfig::new(socket_path, owner_uid))
            .map_err(|_| EngineBootstrapError::Listener)?,
    );
    let outcome = service_session_loop_with_policy(
        Arc::clone(&listener),
        termination_requested,
        timing,
        CoreExitPolicy::default(),
        owner_uid,
    )
    .await;
    // 干净退出前 guarded 清理本端点；拒绝（被替换等）不阻断退出（stale 由下次
    // 启动的 guarded stale 清理兜底）。
    let _ = listener.cleanup();
    map_loop_outcome(outcome)
}

/// 会话循环终态 → 进程退出码语义（干净退出=Ok/exit 0；listener 级失败=错误退出，
/// launchd 视为崩溃可自愈）。
fn map_loop_outcome(outcome: ServiceLoopOutcome) -> Result<(), EngineBootstrapError> {
    match outcome {
        ServiceLoopOutcome::CleanExit => Ok(()),
        ServiceLoopOutcome::ListenerError => Err(EngineBootstrapError::Listener),
    }
}

/// 顺序单会话循环：空闲 accept（uid 门）→ 会话服务 → 拆隧道 → 回 accept。
///
/// 生产消费进程级 SIGTERM/SIGINT 标志（[`run_service_loop`] 以
/// [`TERMINATION_REQUESTED`] 调用）；单测注入独立标志+毫秒级心跳节奏（隔离并行
/// 测试对进程级静态的竞态）。
async fn service_session_loop_with_timing(
    listener: Arc<ServiceListener>,
    termination_requested: &AtomicBool,
    timing: SessionHeartbeatTiming,
) -> ServiceLoopOutcome {
    service_session_loop_with_policy(
        listener,
        termination_requested,
        timing,
        CoreExitPolicy::default(),
        // SAFETY: geteuid 只读取当前进程凭证。
        unsafe { libc::geteuid() },
    )
    .await
}

/// [`service_session_loop_with_timing`] 的策略注入变体（T1 测试入口；生产默认
/// Teardown=现状行为）。keep 分支的会话结束后 runtime 携带进入下一会话（管线
/// 跨会话存续），teardown 语义延迟到下一个默认策略会话尾或显式停止。
async fn service_session_loop_with_policy(
    listener: Arc<ServiceListener>,
    termination_requested: &AtomicBool,
    timing: SessionHeartbeatTiming,
    policy: CoreExitPolicy,
    owner_uid: u32,
) -> ServiceLoopOutcome {
    let mut kept_runtime: Option<Arc<EngineRuntime>> = None;
    loop {
        // 空闲段：等待下一条通过 uid 门的连接；SIGTERM 在空闲期干净退出。
        // T1 复审整改（发现 2）：默认路径空闲=无隧道（无需 teardown 等待），但
        // keep 携带期持有活管线——退出前必须经统一收敛的有界等待，否则进程退出
        // 与管线自拆竞速，可能残留 root 路由/utun。
        let connection = tokio::select! {
            () = poll_signal_flag(termination_requested) => {
                if let Some(runtime) = kept_runtime.as_ref() {
                    let converged = converge_and_await_teardown(runtime).await;
                    if !converged {
                        runtime.logs().publish(
                            "error",
                            "lifecycle",
                            "TEARDOWN_TIMEOUT",
                            "idle-exit with kept pipeline; teardown unconfirmed within the bound",
                            &[],
                        );
                    }
                }
                return ServiceLoopOutcome::CleanExit
            }
            admitted = listener.accept_uid_gated() => match admitted {
                Ok(connection) => connection,
                Err(_) => return ServiceLoopOutcome::ListenerError,
            },
        };
        let carried = kept_runtime.take();
        let outcome = run_one_service_session(
            &listener,
            connection,
            termination_requested,
            timing,
            policy,
            owner_uid,
            carried,
        )
        .await;
        match outcome.end {
            SessionEnd::BackToAccept => {
                kept_runtime = outcome.kept_runtime;
            }
            SessionEnd::CleanExit => return ServiceLoopOutcome::CleanExit,
        }
    }
}

/// 服务一个已准入会话直至其终结，返回会话终态与 runtime 归属（不进程退出）。
///
/// 会话生命周期：uid 门准入（=认证，=心跳零点）→ tonic 服务 → 四类触发之一
///（EOF/信号/心跳超时/显式停止）→ W1 统一收敛（拆隧道）→ 会话占用解除。
/// T1：`carried` 携带上一会话按 keep 策略保留的 runtime（管线跨会话存续；
/// None=全新 runtime——原"每会话全新"语义，keep 策略是唯一例外来源）。
#[allow(clippy::too_many_lines)]
async fn run_one_service_session(
    listener: &Arc<ServiceListener>,
    connection: ServiceConnection,
    termination_requested: &AtomicBool,
    timing: SessionHeartbeatTiming,
    fresh_policy: CoreExitPolicy,
    owner_uid: u32,
    carried: Option<Arc<EngineRuntime>>,
) -> SessionOutcome {
    let engine_runtime = prepare_service_session_runtime(
        carried,
        fresh_policy,
        || Arc::new(EngineRuntime::new_for_owner(owner_uid)),
        start_fresh_runtime_sweep,
    );
    let generation = next_session_generation();
    let service = EngineControlService::from_runtime(Arc::clone(&engine_runtime));
    // 底层 socket 强制关闭句柄：必须在 stream 移交 tonic 前取（tonic 经 incoming
    // 通道 spawn 的连接任务无法从外部 abort，挂起对端只能内核层收权）。
    let kill_handle = connection.socket_shutdown_handle().ok();
    let (sender, incoming) = AuthenticatedIncoming::channel(1);
    // server 任务分离持有（连接随会话终结自然结束；挂起对端由下方 socket 收权兜底）。
    tokio::spawn(async move {
        Server::builder()
            .add_service(HelperControlServer::new(service))
            .serve_with_incoming(incoming)
            .await
    });
    // incoming 已关等罕见交付失败：本会话无从服务，丢弃回 accept。
    // T1 复审整改（发现 3）：携带进来的 runtime 原样带出——丢弃会使已应用管线
    // 成为常驻进程内无主资源（自持 Arc 的管线任务无人再置位收敛）。
    let carried_through = Arc::clone(&engine_runtime);
    let Ok(close_signal) = sender
        .handoff_service_stream_with_close_signal(connection.into_authenticated_unix_stream())
        .await
    else {
        service_log(
            "warn",
            "SESSION_OPEN_FAILED",
            "admitted connection failed stream handoff; back to accept",
            &[("generation", generation.to_string())],
        );
        return SessionOutcome {
            end: SessionEnd::BackToAccept,
            kept_runtime: Some(carried_through),
        };
    };
    // uid 门准入完成 = 认证握手完成 = 心跳零点（对齐会话形态的零点语义）。
    // T1 复审整改：会话入口轮换生命周期触发槽（跨会话携带 runtime 的 permit/
    // 电平零残留——Notify permit 无法清空，唯一可靠手段是换新槽）。
    engine_runtime.rotate_session_trigger_slots();
    engine_runtime.arm_session_heartbeat();
    service_log(
        "info",
        "SESSION_OPENED",
        "core session admitted and served",
        &[("generation", generation.to_string())],
    );
    let watchdog = spawn_heartbeat_watchdog_with(
        Arc::clone(&engine_runtime),
        timing.check_interval,
        timing.timeout,
    );
    // 会话占用期：额外连接 accept 后 typed 拒绝（既有 ATTEMPT_ACTIVE 门词汇，
    // 不 evict、不抢占）。
    let busy = tokio::spawn(busy_reject_loop(Arc::clone(listener), generation));

    let trigger = tokio::select! {
        _signal = close_signal => SessionTrigger::CoreClosed,
        () = poll_signal_flag(termination_requested) => SessionTrigger::PosixSignals,
        () = engine_runtime.wait_heartbeat_timeout() => SessionTrigger::HeartbeatTimeout,
        () = engine_runtime.wait_explicit_stop() => SessionTrigger::ExplicitStop,
    };

    // T1 keep 分支：core 会话终结（EOF/心跳超时）且策略为 KeepTunnel 时不收敛
    // teardown——管线随 runtime 携带回 accept，等待下一会话；teardown 语义延迟
    // 到下一个默认策略会话尾或显式停止。显式停止与信号不受策略豁免。
    let keep_tunnel =
        session_end_keeps_tunnel(trigger, engine_runtime.core_exit_policy());
    service_log(
        "info",
        "SESSION_ENDED",
        "core session trigger resolved",
        &[
            ("trigger", format!("{trigger:?}")),
            ("keep_tunnel", keep_tunnel.to_string()),
            ("generation", generation.to_string()),
        ],
    );
    if keep_tunnel {
        engine_runtime.logs().publish(
            "info",
            "lifecycle",
            "CORE_EXIT_KEEP",
            "core session ended; pipeline kept per on_core_exit policy",
            &[("trigger", format!("{trigger:?}"))],
        );
        busy.abort();
        // T1 复审整改：看门狗是心跳槽唯一生产写入方——keep 转换必须 abort，
        // 否则空闲期触发会把 permit 落存到已无等待者的槽里（发现 1），且任务
        // 跨会话泄漏（发现 4）。
        watchdog.abort();
        drop(sender);
        // 心跳超时的挂起对端必须收权掐断（EOF 时连接已由对端关闭）。
        if trigger == SessionTrigger::HeartbeatTimeout {
            tokio::time::sleep(REPLY_FLUSH_GRACE).await;
            if let Some(handle) = kill_handle.as_ref() {
                handle.shutdown();
            }
        }
        return SessionOutcome {
            end: SessionEnd::BackToAccept,
            kept_runtime: Some(Arc::clone(&engine_runtime)),
        };
    }

    // W1 统一收敛执行体（service 形态分叉点：收敛后不进程退出）。收敛完成前不
    // drop(sender)/关 server——在途 handler（含 Shutdown 应答）需要活着的服务端。
    let converged = converge_and_await_teardown(&engine_runtime).await;
    service_log(
        if converged { "info" } else { "error" },
        "SESSION_TORN_DOWN",
        "service session teardown finished",
        &[
            ("converged", converged.to_string()),
            ("trigger", format!("{trigger:?}")),
            ("generation", generation.to_string()),
        ],
    );
    if !converged {
        engine_runtime.logs().publish(
            "error",
            "lifecycle",
            "TEARDOWN_TIMEOUT",
            format!(
                "service session teardown unconfirmed within the bound (trigger={trigger:?})"
            ),
            &[("trigger", format!("{trigger:?}"))],
        );
    }
    busy.abort();
    drop(sender);
    // 非对端发起的终结：应答排空宽限后对底层 socket 强制收权——tonic 经 incoming
    // 通道 spawn 的连接任务无法从外部 abort，挂起对端只能内核层 shutdown 收权
    //（僵尸连接不得复活为影子会话）。CoreClosed 时连接已由对端关闭，无需收权。
    if trigger != SessionTrigger::CoreClosed {
        tokio::time::sleep(REPLY_FLUSH_GRACE).await;
        if let Some(kill_handle) = kill_handle.as_ref() {
            kill_handle.shutdown();
        }
    }

    let end = match trigger {
        SessionTrigger::PosixSignals | SessionTrigger::ExplicitStop => SessionEnd::CleanExit,
        SessionTrigger::CoreClosed | SessionTrigger::HeartbeatTimeout => {
            SessionEnd::BackToAccept
        }
    };
    SessionOutcome {
        end,
        kept_runtime: None,
    }
}

/// 携带在用 runtime 时保留原实例与策略；只有新建实例才注入本次会话策略。
///
/// S2（2026-10-26 验收修复计划）：新建（fresh）runtime 挂启动清扫——managed
/// resolver 标记回收域（best-effort 后台执行；携带 runtime 仍拥有在用隧道与
/// resolver，其文件不是待回收残留，复用即跳过）。
fn prepare_service_session_runtime(
    carried: Option<Arc<EngineRuntime>>,
    fresh_policy: CoreExitPolicy,
    create_runtime: impl FnOnce() -> Arc<EngineRuntime>,
    start_sweep: impl FnOnce(&Arc<EngineRuntime>),
) -> Arc<EngineRuntime> {
    if let Some(runtime) = carried {
        return runtime;
    }
    let runtime = create_runtime();
    runtime.set_core_exit_policy(fresh_policy);
    start_sweep(&runtime);
    runtime
}

/// fresh runtime 的启动清扫：后台执行（会话服务先行——紧急断开 StopTunnel 立即
/// 可达），动作落 S7 文件日志（component=sweep）+ 会话日志流。
fn start_fresh_runtime_sweep(engine_runtime: &Arc<EngineRuntime>) {
    let owner_uid = engine_runtime.owner_uid();
    let logs = Arc::clone(engine_runtime.logs());
    let gate = crate::connect_gate::process_gate();
    tokio::task::spawn_blocking(move || {
        gate.sweep_startup_residue(owner_uid, &logs);
    });
}

/// 会话占用期的额外连接拒绝循环：每条 uid 门通过的连接由一个一次性拒绝服务
/// 以 `ATTEMPT_ACTIVE` typed 拒绝（全部 RPC；不 evict 当前会话）。拒绝连接随对端
/// 断开自然终结。`generation` 为当前占用会话的代数（文件日志标注）。
async fn busy_reject_loop(listener: Arc<ServiceListener>, generation: u64) {
    loop {
        let Ok(connection) = listener.accept_uid_gated().await else {
            return;
        };
        service_log(
            "info",
            "SESSION_BUSY_REJECTED",
            "second concurrent core connection typed rejected",
            &[("generation", generation.to_string())],
        );
        tokio::spawn(async move {
            let (sender, incoming) = AuthenticatedIncoming::channel(1);
            if sender
                .handoff_service_stream(connection.into_authenticated_unix_stream())
                .await
                .is_ok()
            {
                let _ = Server::builder()
                    .add_service(HelperControlServer::new(SessionOccupiedService))
                    .serve_with_incoming(incoming)
                    .await;
            }
        });
    }
}

/// 会话占用期的 typed 拒绝服务：全部 RPC 恒 `failed_precondition(ATTEMPT_ACTIVE)`
///（顺序单会话模型：第二 core 并发连接的既有门词汇）。
struct SessionOccupiedService;

#[async_trait]
impl HelperControl for SessionOccupiedService {
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
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn observe_owned_state(
        &self,
        _request: Request<wire::ObserveOwnedStateRequest>,
    ) -> Result<Response<wire::ObserveOwnedStateReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn acquire_lease(
        &self,
        _request: Request<wire::AcquireLeaseRequest>,
    ) -> Result<Response<wire::AcquireLeaseReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn apply_tunnel(
        &self,
        _request: Request<wire::ApplyTunnelRequest>,
    ) -> Result<Response<wire::ApplyTunnelReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn stop_tunnel(
        &self,
        _request: Request<wire::StopTunnelRequest>,
    ) -> Result<Response<wire::StopTunnelReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn get_operation(
        &self,
        _request: Request<wire::GetOperationRequest>,
    ) -> Result<Response<wire::GetOperationReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn release_lease(
        &self,
        _request: Request<wire::ReleaseLeaseRequest>,
    ) -> Result<Response<wire::ReleaseLeaseReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn stream_logs(
        &self,
        _request: Request<wire::StreamLogsRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn stream_stats(
        &self,
        _request: Request<wire::StreamStatsRequest>,
    ) -> Result<Response<Self::StreamStatsStream>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn stream_connect_status(
        &self,
        _request: Request<wire::StreamConnectStatusRequest>,
    ) -> Result<Response<Self::StreamConnectStatusStream>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn keep_alive(
        &self,
        _request: Request<wire::KeepAliveRequest>,
    ) -> Result<Response<wire::KeepAliveReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn trigger_latency_refresh(
        &self,
        _request: Request<wire::HelperLatencyRefreshRequest>,
    ) -> Result<Response<wire::HelperLatencyRefreshReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    // Busy connections retain ATTEMPT_ACTIVE; the admitted service session uses
    // EngineControlService and its real official receiver backend.
    async fn auth_receiver_control(
        &self, _request: Request<wire::AuthReceiverControlRequest>,
    ) -> Result<Response<wire::AuthReceiverControlReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn service_manage(
        &self,
        _request: Request<wire::ServiceManageRequest>,
    ) -> Result<Response<wire::ServiceManageReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }

    async fn shutdown(
        &self,
        _request: Request<wire::ShutdownRequest>,
    ) -> Result<Response<wire::ShutdownReply>, Status> {
        Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE))
    }
}
