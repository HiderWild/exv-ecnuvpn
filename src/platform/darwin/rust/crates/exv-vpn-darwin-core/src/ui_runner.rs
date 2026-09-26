//! 普通用户 UI→Core 启动前置校验。
//!
//! 本模块拥有 Core 侧 argv、进程身份、runtime socket 与 stdin bootstrap record 的组合
//! 契约。它不创建 listener，也不启动 Engine；只有全部前置事实已经成立后，后续 runner 才可
//! 取得 [`PreparedUiCore`] 并绑定唯一 UDS。

use std::{
    ffi::{OsStr, OsString},
    fmt,
    io::{self, Read},
    path::PathBuf,
    time::Duration,
};

use exv_vpn_darwin_ipc::{
    AuthenticatedConnectionCloseSignal, AuthenticatedIncoming, AuthenticatedIncomingSender,
    auth::AuthKey,
    listener::{ListenerConfig, PreauthListener},
    path::{ENGINE_SOCKET_FILE_NAME, RuntimeDir, SocketPath},
    peer::ExpectedPeer,
    ui_core_bootstrap::{UiCoreBootstrapError, UiCoreBootstrapV1},
};
use exv_vpn_wire::generated::kernel_control_server::KernelControlServer;
use tokio::{task::JoinHandle, time};
use tonic::transport::Server;

use crate::kernel_control_service::{
    DarwinKernelControlService, LiveConnectionProjection, SessionShutdown,
};

/// 普通用户 Core 接受的唯一 argv 形状（不含 binary 自身）。
const UI_CORE_ARG_COUNT: usize = 6;
const UI_PID_FLAG: &str = "--ui-pid";
const UI_UID_FLAG: &str = "--ui-uid";
const UI_SOCKET_FLAG: &str = "--ui-socket";

/// 由 Core 在 bind 之前报告的稳定启动失败类别。
///
/// 每个 variant 的显示文本都不包含 argv 路径、record 字节或认证 key；调用者可安全将它
/// 写到受控的本地 stderr。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCoreRunnerError {
    /// argv 不是固定六 token 形状，或其中的整数/socket basename 非法。
    Arguments,
    /// uid/euid/ppid 与 UI 声明不一致，或任一方尝试 root 身份。
    Identity,
    /// socket 的 runtime directory 不再满足当前普通用户的 owner-only 事实。
    RuntimeDirectory,
    /// stdin record 不是唯一有效的 [`UiCoreBootstrapV1`]。
    Bootstrap(UiCoreBootstrapError),
    /// Tokio runtime 无法启动。
    Runtime,
    /// socket listener bind 或发布失败。
    Listener,
    /// 唯一一次 pre-auth accept 失败、超时或认证失败。
    Admission,
    /// 已认证 connection 无法 handoff 给 tonic server。
    Handoff,
    /// listener endpoint 无法在已验证 runtime identity 下清理。
    Cleanup,
    /// tonic server 发生不可恢复 transport 或 task 错误。
    Server,
    /// 已触发 shutdown 后 server 未能在 5 秒内退出。
    DrainTimeout,
    /// bridge 在实际 stream 析构前异常丢失 close notification。
    CloseSignal,
}

impl UiCoreRunnerError {
    /// 不含用户输入或 secret 的稳定错误码。
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Arguments => "DARWIN_UI_CORE_ARGS_INVALID",
            Self::Identity => "DARWIN_UI_CORE_IDENTITY_INVALID",
            Self::RuntimeDirectory => "DARWIN_UI_CORE_RUNTIME_INVALID",
            Self::Bootstrap(_) => "DARWIN_UI_CORE_BOOTSTRAP_INVALID",
            Self::Runtime => "DARWIN_UI_CORE_RUNTIME_START_FAILED",
            Self::Listener => "DARWIN_UI_CORE_LISTENER_FAILED",
            Self::Admission => "DARWIN_UI_CORE_ADMISSION_FAILED",
            Self::Handoff => "DARWIN_UI_CORE_HANDOFF_FAILED",
            Self::Cleanup => "DARWIN_UI_CORE_CLEANUP_FAILED",
            Self::Server => "DARWIN_UI_CORE_SERVER_FAILED",
            Self::DrainTimeout => "DARWIN_UI_CORE_DRAIN_TIMEOUT",
            Self::CloseSignal => "DARWIN_UI_CORE_CLOSE_SIGNAL_FAILED",
        }
    }
}

impl fmt::Display for UiCoreRunnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for UiCoreRunnerError {}

/// 来自当前 Core 进程、而不是 UI wire 的最小 OS 身份事实。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CoreProcessFacts {
    uid: u32,
    euid: u32,
    parent_pid: u32,
}

impl CoreProcessFacts {
    /// 为测试或受控进程入口构造身份快照。
    #[must_use]
    pub(crate) const fn new(uid: u32, euid: u32, parent_pid: u32) -> Self {
        Self {
            uid,
            euid,
            parent_pid,
        }
    }

    /// 读取实际 Core 进程的 uid/euid/ppid。
    #[must_use]
    pub(crate) fn current() -> Self {
        // SAFETY: these calls only read this process's credentials and parent pid; they accept
        // no Rust references or caller-controlled pointers.
        let (uid, euid, parent_pid) = unsafe { (libc::getuid(), libc::geteuid(), libc::getppid()) };
        Self::new(uid, euid, u32::try_from(parent_pid).unwrap_or_default())
    }
}

/// 已严格解析但尚未读取 stdin 的固定 UI→Core 启动参数。
#[derive(Clone, Debug)]
pub(crate) struct UiCoreLaunchArgs {
    ui_pid: u32,
    ui_uid: u32,
    socket_path: SocketPath,
}

/// 可以安全交给唯一 UDS runner 的已验证启动材料。
///
/// `auth_key` 没有 getter、没有 `Debug`，并由 IPC 的 [`AuthKey`] 在 drop 时清零。此类型
/// 只能由 [`prepare_ui_core`] 构造，从而保证不存在“先 bind 后校验 stdin”的旁路。
pub(crate) struct PreparedUiCore {
    /// 持有经 dirfd 验证的 UI runtime identity，直至后续唯一 listener 完成 bind/cleanup。
    runtime_dir: RuntimeDir,
    ui_peer: ExpectedPeer,
    auth_key: AuthKey,
}

impl PreparedUiCore {
    /// 把唯一 runtime identity 与认证材料交给后续 listener runner。
    ///
    /// runner 必须从返回的 [`RuntimeDir`] 派生 socket path 和 owner；不得复用已经验证前
    /// 的 pathname 或手工构造 owner。
    pub(crate) fn into_parts(self) -> (RuntimeDir, ExpectedPeer, AuthKey) {
        (self.runtime_dir, self.ui_peer, self.auth_key)
    }
}

/// 解析生产入口的 argv；输入不包括 binary 自身。
///
/// 非 UTF-8 token、额外 token、flag 乱序或自由形式整数都会在任何 socket/handler 创建前
/// 返回 [`UiCoreRunnerError::Arguments`]。
pub(crate) fn parse_ui_core_args(
    arguments: &[OsString],
) -> Result<UiCoreLaunchArgs, UiCoreRunnerError> {
    if arguments.len() != UI_CORE_ARG_COUNT {
        return Err(UiCoreRunnerError::Arguments);
    }
    let [
        pid_flag,
        pid_token,
        uid_flag,
        uid_token,
        socket_flag,
        socket,
    ] = arguments
    else {
        return Err(UiCoreRunnerError::Arguments);
    };
    if pid_flag != OsStr::new(UI_PID_FLAG)
        || uid_flag != OsStr::new(UI_UID_FLAG)
        || socket_flag != OsStr::new(UI_SOCKET_FLAG)
    {
        return Err(UiCoreRunnerError::Arguments);
    }

    let parent_process_id = parse_decimal(pid_token)?;
    if parent_process_id == 0 || parent_process_id > i32::MAX as u32 {
        return Err(UiCoreRunnerError::Arguments);
    }
    let caller_uid = parse_decimal(uid_token)?;
    let socket = socket
        .to_str()
        .ok_or(UiCoreRunnerError::Arguments)
        .map(PathBuf::from)?;
    let socket_path = SocketPath::new(socket).map_err(|_| UiCoreRunnerError::Arguments)?;
    if socket_path.as_path().file_name() != Some(OsStr::new(ENGINE_SOCKET_FILE_NAME)) {
        return Err(UiCoreRunnerError::Arguments);
    }

    Ok(UiCoreLaunchArgs {
        ui_pid: parent_process_id,
        ui_uid: caller_uid,
        socket_path,
    })
}

/// 校验所有 bind 前事实，并从 stdin 消费唯一 bootstrap record。
///
/// 本函数没有 listener、handler、Engine 或 Authorization 依赖。调用者只有获得返回的
/// [`PreparedUiCore`] 后才能进入 socket bind 车道，因此任何 `Err` 都证明零 bind。
pub(crate) fn prepare_ui_core<R: Read>(
    args: &UiCoreLaunchArgs,
    facts: CoreProcessFacts,
    reader: &mut R,
) -> Result<PreparedUiCore, UiCoreRunnerError> {
    validate_identity(args, facts)?;
    let runtime_path = args
        .socket_path
        .as_path()
        .parent()
        .ok_or(UiCoreRunnerError::RuntimeDirectory)?;
    let runtime = RuntimeDir::open_existing_for_uid(runtime_path, args.ui_uid)
        .map_err(|_| UiCoreRunnerError::RuntimeDirectory)?;
    let expected_socket = runtime
        .engine_socket_path()
        .map_err(|_| UiCoreRunnerError::RuntimeDirectory)?;
    if expected_socket != args.socket_path {
        return Err(UiCoreRunnerError::RuntimeDirectory);
    }
    let record = UiCoreBootstrapV1::decode(reader).map_err(UiCoreRunnerError::Bootstrap)?;

    Ok(PreparedUiCore {
        runtime_dir: runtime,
        ui_peer: ExpectedPeer::new(args.ui_uid, args.ui_pid),
        auth_key: record.into_auth_key(),
    })
}

fn parse_decimal(value: &OsString) -> Result<u32, UiCoreRunnerError> {
    let value = value.to_str().ok_or(UiCoreRunnerError::Arguments)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UiCoreRunnerError::Arguments);
    }
    value
        .parse::<u32>()
        .map_err(|_| UiCoreRunnerError::Arguments)
}

fn validate_identity(
    args: &UiCoreLaunchArgs,
    facts: CoreProcessFacts,
) -> Result<(), UiCoreRunnerError> {
    if args.ui_uid == 0
        || facts.uid == 0
        || facts.euid == 0
        || facts.uid != facts.euid
        || facts.uid != args.ui_uid
        || facts.parent_pid != args.ui_pid
    {
        return Err(UiCoreRunnerError::Identity);
    }
    Ok(())
}

const ACCEPT_DEADLINE: Duration = Duration::from_secs(30);
const SERVER_DRAIN_DEADLINE: Duration = Duration::from_secs(5);

/// 生产 Core 的同步入口：先完成全部可阻塞前置校验，再启动唯一 Tokio UDS session。
///
/// stdin 的 40-byte bootstrap record 在构造 runtime 前已被完整消费且确认 EOF；因此 argv、
/// 身份、runtime directory 或 record 任一失败都不会创建 listener 或 handler。
pub(crate) fn run_from_process() -> Result<(), UiCoreRunnerError> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let args = parse_ui_core_args(&arguments)?;
    let prepared = {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        prepare_ui_core(&args, CoreProcessFacts::current(), &mut reader)?
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| UiCoreRunnerError::Runtime)?;
    runtime.block_on(run_prepared(prepared))
}

/// 运行已经通过所有 bind 前校验的唯一普通用户 Core session。
async fn run_prepared(prepared: PreparedUiCore) -> Result<(), UiCoreRunnerError> {
    run_prepared_with_deadlines(prepared, ACCEPT_DEADLINE, SERVER_DRAIN_DEADLINE).await
}

/// 仅将 deadline 参数化以便确定性验证，不改变生产的单次 accept 语义。
async fn run_prepared_with_deadlines(
    prepared: PreparedUiCore,
    accept_deadline: Duration,
    drain_deadline: Duration,
) -> Result<(), UiCoreRunnerError> {
    run_prepared_with_deadlines_inner(
        prepared,
        accept_deadline,
        drain_deadline,
        #[cfg(test)]
        None,
        #[cfg(test)]
        None,
    )
    .await
}

async fn run_prepared_with_deadlines_inner(
    prepared: PreparedUiCore,
    accept_deadline: Duration,
    drain_deadline: Duration,
    #[cfg(test)] listener_ready: Option<tokio::sync::oneshot::Sender<()>>,
    #[cfg(test)] handoff_cleanup_barrier: Option<HandoffCleanupBarrier>,
) -> Result<(), UiCoreRunnerError> {
    let (runtime, ui_peer, auth_key) = prepared.into_parts();
    runtime
        .revalidate_path()
        .map_err(|_| UiCoreRunnerError::RuntimeDirectory)?;
    let socket_path = runtime
        .engine_socket_path()
        .map_err(|_| UiCoreRunnerError::RuntimeDirectory)?;
    let listener = PreauthListener::bind(
        ListenerConfig::new(socket_path, runtime.owner(), ui_peer, auth_key)
            .with_global_deadline(accept_deadline),
    )
    .map_err(|_| UiCoreRunnerError::Listener)?;

    #[cfg(test)]
    if let Some(listener_ready) = listener_ready {
        let _ = listener_ready.send(());
    }

    // 10b 刻意只调用这一次；无论 timeout、认证失败或连接提前 EOF 都不会重试 accept。
    let Ok(connection) = listener.accept_preface().await else {
        return Err(cleanup_after_listener_failure(
            listener,
            &runtime,
            UiCoreRunnerError::Admission,
        ));
    };

    let session = CoreUiSession::start();
    let Ok(close_signal) = session.handoff(connection).await else {
        let primary_error =
            cleanup_after_listener_failure(listener, &runtime, UiCoreRunnerError::Handoff);
        return Err(prioritize_drain_failure(
            primary_error,
            session.shutdown_and_drain(drain_deadline).await,
        ));
    };

    #[cfg(test)]
    if let Some(handoff_cleanup_barrier) = handoff_cleanup_barrier {
        handoff_cleanup_barrier.wait().await;
    }

    // pathname cleanup 只能发生在同一条已认证 stream 已真正 handoff 后；sender/task 仍由
    // `session` 持有，因而 unlink 不会终止既有 HTTP/2 connection。
    if let Err(cleanup_error) = cleanup_listener(&runtime, listener) {
        return Err(prioritize_drain_failure(
            cleanup_error,
            session.shutdown_and_drain(drain_deadline).await,
        ));
    }

    // Core 绝不 cleanup_empty：UI/Tauri 只在确认 child 已退出后才拥有 runtime directory
    // 的最终 cleanup 权限。
    session
        .wait_for_terminal(close_signal, drain_deadline)
        .await
}

/// 与 held runtime identity 对齐地关闭 listener 并 guarded-cleanup 其 endpoint。
///
/// identity 在 cleanup 前发生变化时仅关闭 listener、保留路径现场；绝不尝试按 pathname
/// 删除 UI 拥有的 runtime directory。
fn cleanup_listener(
    runtime: &RuntimeDir,
    listener: PreauthListener,
) -> Result<(), UiCoreRunnerError> {
    if runtime.revalidate_path().is_err() {
        drop(listener);
        return Err(UiCoreRunnerError::RuntimeDirectory);
    }
    listener.cleanup().map_err(|_| UiCoreRunnerError::Cleanup)
}

fn cleanup_after_listener_failure(
    listener: PreauthListener,
    runtime: &RuntimeDir,
    primary: UiCoreRunnerError,
) -> UiCoreRunnerError {
    match cleanup_listener(runtime, listener) {
        Ok(()) => primary,
        Err(cleanup_error) => cleanup_error,
    }
}

/// 在已启动 tonic task 的失败路径中，不能把无法完成 drain 伪装成较早的业务错误。
///
/// listener cleanup/handoff/close-signal 的原始失败仅在 server 已确定退出时保留；若
/// `Server` 或 `DrainTimeout` 阻止确定性收束，则其优先级更高。
#[must_use]
fn prioritize_drain_failure(
    primary: UiCoreRunnerError,
    drain_result: Result<(), UiCoreRunnerError>,
) -> UiCoreRunnerError {
    match drain_result {
        Ok(()) => primary,
        Err(drain_error) => drain_error,
    }
}

/// 保持 authenticated incoming sender、close signal 与 tonic task 的一次性 Core session。
///
/// sender 必须在 server task 退出后才释放，避免 incoming EOF 提前驱逐同一条 HTTP/2 stream。
struct CoreUiSession {
    _power_monitor: Option<crate::power::PowerMonitor>,
    shutdown: SessionShutdown,
    sender: AuthenticatedIncomingSender,
    server_task: JoinHandle<Result<(), tonic::transport::Error>>,
    engine: std::sync::Arc<LiveConnectionProjection>,
}

impl CoreUiSession {
    fn start() -> Self {
        let (shutdown, shutdown_receiver) = SessionShutdown::new();
        let (service, engine) = DarwinKernelControlService::new_with_live_engine();
        let power_monitor = engine.observe_power();
        let (sender, incoming) = AuthenticatedIncoming::channel(1);
        let server_task = tokio::spawn(async move {
            Server::builder()
                .add_service(KernelControlServer::new(service))
                .serve_with_incoming_shutdown(incoming, async move {
                    let _ = shutdown_receiver.await;
                })
                .await
        });
        Self {
            _power_monitor: power_monitor,
            shutdown,
            sender,
            server_task,
            engine,
        }
    }

    async fn handoff(
        &self,
        connection: exv_vpn_darwin_ipc::listener::AuthenticatedConnection,
    ) -> Result<AuthenticatedConnectionCloseSignal, UiCoreRunnerError> {
        self.sender
            .handoff_with_close_signal(connection)
            .await
            .map_err(|_| UiCoreRunnerError::Handoff)
    }

    async fn wait_for_terminal(
        mut self,
        mut close_signal: AuthenticatedConnectionCloseSignal,
        drain_deadline: Duration,
    ) -> Result<(), UiCoreRunnerError> {
        enum Terminal {
            ShutdownRequested,
            ConnectionClosed,
            CloseSignalFailed,
            ServerExitedCleanly,
            ServerFailed,
        }

        let shutdown = self.shutdown.clone();
        let terminal = tokio::select! {
            () = shutdown.wait_for_trigger() => Terminal::ShutdownRequested,
            close_result = &mut close_signal => {
                if close_result.is_ok() {
                    Terminal::ConnectionClosed
                } else {
                    Terminal::CloseSignalFailed
                }
            }
            server_result = &mut self.server_task => {
                if matches!(server_result, Ok(Ok(()))) {
                    Terminal::ServerExitedCleanly
                } else {
                    Terminal::ServerFailed
                }
            }
        };

        let result = match terminal {
            Terminal::ShutdownRequested | Terminal::ConnectionClosed => {
                self.shutdown.trigger();
                self.drain_after_shutdown(drain_deadline).await
            }
            Terminal::CloseSignalFailed => {
                self.shutdown.trigger();
                Err(prioritize_drain_failure(
                    UiCoreRunnerError::CloseSignal,
                    self.drain_after_shutdown(drain_deadline).await,
                ))
            }
            Terminal::ServerExitedCleanly => {
                self.shutdown.trigger();
                Ok(())
            }
            Terminal::ServerFailed => {
                self.shutdown.trigger();
                Err(UiCoreRunnerError::Server)
            }
        };
        self.engine
            .close()
            .await
            .map_err(|_| UiCoreRunnerError::Server)?;
        result
    }

    async fn shutdown_and_drain(
        mut self,
        drain_deadline: Duration,
    ) -> Result<(), UiCoreRunnerError> {
        self.shutdown.trigger();
        let result = self.drain_after_shutdown(drain_deadline).await;
        self.engine
            .close()
            .await
            .map_err(|_| UiCoreRunnerError::Server)?;
        result
    }

    async fn drain_after_shutdown(
        &mut self,
        drain_deadline: Duration,
    ) -> Result<(), UiCoreRunnerError> {
        match time::timeout(drain_deadline, &mut self.server_task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(_)) | Err(_)) => Err(UiCoreRunnerError::Server),
            Err(_) => {
                self.server_task.abort();
                let _ = (&mut self.server_task).await;
                Err(UiCoreRunnerError::DrainTimeout)
            }
        }
    }
}
