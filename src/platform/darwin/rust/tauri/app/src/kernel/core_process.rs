//! 10b 唯一的普通用户 Core launcher。
//!
//! 本文件是 Rust-only guard 唯一允许创建子进程的位置。它只启动固定 Core（无签名
//! `.app` bundle 布局同目录 Core，或固定开发期 target/debug Core），把固定
//! bootstrap record 写入 stdin，并持有一次 authenticated UDS session；不接触
//! Engine、提权、网络或用户可配置的 executable/path。

use std::{
    env,
    ffi::OsStr,
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin},
    time::Duration,
};

use exv_vpn_darwin_ipc::{
    authenticate_client_to_tonic_channel,
    path::{RuntimeDir, RuntimeOwner, SocketPath},
    peer::{ExpectedPeer, SystemPeerLookup},
    ui_core_bootstrap::UiCoreBootstrapV1,
};
use exv_vpn_wire::generated::{
    RuntimeEvent, RuntimeEventKind, WatchEventsRequest, kernel_control_client::KernelControlClient,
    runtime_snapshot,
};
use tokio::{net::UnixStream, time};
use tonic::{Streaming, transport::Channel};

const CORE_READY_DEADLINE: Duration = Duration::from_secs(5);
const CORE_REAP_DEADLINE: Duration = Duration::from_secs(5);
/// 正常退出路径等待 Core 自行退出的上界。
///
/// 正常退出不 kill（既有 `CoreChild` 不做 Drop terminate 的语义不变）：Core 经 UI
/// 通道 EOF 走有序停机；超时即放弃目录清理并保留现场，不得阻塞退出或改退出码。
const CORE_SHUTDOWN_CLEANUP_DEADLINE: Duration = Duration::from_secs(5);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Tauri 侧稳定的普通用户 Core 错误码。
///
/// 所有 variant 都有固定显示文本；不得把 executable、socket、认证材料或底层 I/O 文本
/// 传给前端、日志或 Tauri event。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // variant 名与稳定错误码一一对应，Core 前缀是有意契约。
pub(crate) enum CoreProcessError {
    CoreBinaryMissing,
    CoreStdinWriteFailed,
    CoreReadyTimeout,
    CoreBootstrapEarlyExit,
    CoreAuthFailed,
    CoreChannelEof,
    CoreReapTimeout,
    CoreRuntimeCleanupRefused,
}

impl CoreProcessError {
    /// 面向 Tauri command 的稳定、无敏感信息错误码。
    #[must_use]
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::CoreBinaryMissing => "CORE_BINARY_MISSING",
            Self::CoreStdinWriteFailed => "CORE_STDIN_WRITE_FAILED",
            Self::CoreReadyTimeout => "CORE_READY_TIMEOUT",
            Self::CoreBootstrapEarlyExit => "CORE_BOOTSTRAP_EARLY_EXIT",
            Self::CoreAuthFailed => "CORE_AUTH_FAILED",
            Self::CoreChannelEof => "CORE_CHANNEL_EOF",
            Self::CoreReapTimeout => "CORE_REAP_TIMEOUT",
            Self::CoreRuntimeCleanupRefused => "CORE_RUNTIME_CLEANUP_REFUSED",
        }
    }
}

impl std::fmt::Display for CoreProcessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CoreProcessError {}

/// 已验证但尚未写入唯一 fixed launcher 的六个 argv 值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreLaunchArguments {
    ui_pid: u32,
    ui_uid: u32,
    socket: SocketPath,
}

impl CoreLaunchArguments {
    pub(super) fn new(
        ui_pid: u32,
        ui_uid: u32,
        socket: SocketPath,
    ) -> Result<Self, CoreProcessError> {
        if ui_pid == 0 || ui_pid > i32::MAX as u32 || ui_uid == 0 {
            return Err(CoreProcessError::CoreBootstrapEarlyExit);
        }
        Ok(Self {
            ui_pid,
            ui_uid,
            socket,
        })
    }

}

/// 仅用于 child wait/kill 的可注入 seam。
///
/// 生产实现包装 `std::process::Child`；测试可确定性覆盖已退出、未退出和查询失败，而不
/// 创建第二个 executable 或放宽固定 launcher。
pub(super) trait ReapableChild {
    fn try_wait(&mut self) -> Result<bool, ()>;

    fn kill(&mut self) -> Result<(), ()>;
}

struct CoreChild {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    pid: u32,
}

impl CoreChild {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn write_bootstrap(
        &mut self,
        record: &exv_vpn_darwin_ipc::ui_core_bootstrap::UiCoreBootstrapEncodedV1,
    ) -> Result<(), ()> {
        let mut stdin = self.stdin.take().ok_or(())?;
        let result = stdin.write_all(record.as_bytes()).map_err(|_| ());
        drop(stdin);
        result
    }

    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

}

impl ReapableChild for CoreChild {
    fn try_wait(&mut self) -> Result<bool, ()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(true);
        };
        match child.try_wait().map_err(|_| ())? {
            Some(_) => {
                self.child = None;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn kill(&mut self) -> Result<(), ()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        child.kill().map_err(|_| ())
    }
}

/// 等待 child 的可测有界循环；不会启动任何进程。
pub(super) async fn wait_for_child_exit<C: ReapableChild>(
    child: &mut C,
    deadline: Duration,
) -> Result<bool, ()> {
    let deadline = time::Instant::now() + deadline;
    loop {
        if child.try_wait()? {
            return Ok(true);
        }
        if time::Instant::now() >= deadline {
            return Ok(false);
        }
        time::sleep(CHILD_POLL_INTERVAL).await;
    }
}

/// 唯一 Core binary 解析入口：无签名 `.app` bundle 布局优先，其余保持唯一
/// 开发期路径。不接收用户路径、环境 override、PATH 搜索或任何 fallback。
fn canonical_core_binary() -> Result<PathBuf, CoreProcessError> {
    match canonical_bundle_core_binary() {
        Some(binary) => Ok(binary),
        None => canonical_dev_core_binary(),
    }
}

/// MAC-PACKAGE-12：无签名 `.app` bundle 布局的 Core 解析。
///
/// 打包后本 launcher 与 Core 同位于 `EXV.app/Contents/MacOS/`。只认 current
/// executable 所在目录下固定 basename 的可执行普通文件，且 parent 链必须呈现
/// `Contents/MacOS` 形状；不满足时返回 `None`，由开发期 resolver 接管。
fn canonical_bundle_core_binary() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    bundle_core_binary_at(&executable)
}

/// [`canonical_bundle_core_binary`] 的纯函数视图（测试注入 exe 路径）。
pub(crate) fn bundle_core_binary_at(executable: &Path) -> Option<PathBuf> {
    let macos_directory = executable.parent()?;
    if macos_directory.file_name() != Some(OsStr::new("MacOS")) {
        return None;
    }
    let contents_directory = macos_directory.parent()?;
    if contents_directory.file_name() != Some(OsStr::new("Contents")) {
        return None;
    }
    let candidate = macos_directory.join("exv-vpn-darwin-core");
    let metadata = fs::metadata(&candidate).ok()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return None;
    }
    Some(candidate)
}

/// 解析唯一开发期 Core binary，且拒绝任何 fallback。
fn canonical_dev_core_binary() -> Result<PathBuf, CoreProcessError> {
    let manifest_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // 治理 §7：唯一共享根仓 target。工作树层级不定（主仓与 `.worktrees/` 下深度不同），
    // 沿祖先目录向上寻找第一个持有当前 Core 的 `target/debug`——最近祖先优先，
    // 工作树内按纪律不囤积产物，故实际命中的总是根仓唯一 `target/`。
    let canonical_target_debug = manifest_directory
        .ancestors()
        .map(|ancestor| ancestor.join("target").join("debug"))
        .find(|candidate| candidate.join("exv-vpn-darwin-core").is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
        .ok_or(CoreProcessError::CoreBinaryMissing)?;
    let canonical_binary = canonical_target_debug
        .join("exv-vpn-darwin-core")
        .canonicalize()
        .map_err(|_| CoreProcessError::CoreBinaryMissing)?;
    validate_canonical_dev_core_binary(&canonical_target_debug, canonical_binary)
}

fn validate_canonical_dev_core_binary(
    canonical_target_debug: &Path,
    canonical_binary: PathBuf,
) -> Result<PathBuf, CoreProcessError> {
    let metadata =
        fs::metadata(&canonical_binary).map_err(|_| CoreProcessError::CoreBinaryMissing)?;
    if canonical_binary.file_name() != Some(OsStr::new("exv-vpn-darwin-core"))
        || canonical_binary.parent() != Some(canonical_target_debug)
        || !canonical_binary.starts_with(canonical_target_debug)
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(CoreProcessError::CoreBinaryMissing);
    }
    Ok(canonical_binary)
}

/// 该文件唯一允许的 Core 创建点。
fn spawn_fixed_core(
    binary: &Path,
    arguments: &CoreLaunchArguments,
) -> Result<CoreChild, CoreProcessError> {
    let mut child = std::process::Command::new(binary)
        .arg("--ui-pid")
        .arg(arguments.ui_pid.to_string())
        .arg("--ui-uid")
        .arg(arguments.ui_uid.to_string())
        .arg("--ui-socket")
        .arg(arguments.socket.as_path())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| CoreProcessError::CoreBootstrapEarlyExit)?;
    let pid = child.id();
    let stdin = child.stdin.take();
    Ok(CoreChild {
        child: Some(child),
        stdin,
        pid,
    })
}

/// Tauri 持有的单次 authenticated Core session。
///
/// win32 全量接入后本结构只承载进程生命周期（child + runtime 目录）与唯一
/// WatchEvents 流；全部 KernelControl RPC 由命令层经 [`Self::channel`] 的共享
/// 克隆独立发起（win32 「通道与会话分离」形态在 darwin 上的落点）。
pub(crate) struct CoreSession {
    /// 已认证的 UDS tonic 通道（Clone 副本供命令层/事件订阅独立使用）。
    channel: Channel,
    watch: Option<Streaming<RuntimeEvent>>,
    child: CoreChild,
    runtime: RuntimeDir,
}

impl CoreSession {
    /// 已认证通道的克隆（命令层与事件订阅的共享入口）。
    #[must_use]
    pub(crate) fn channel(&self) -> Channel {
        self.channel.clone()
    }

    /// 取出唯一 WatchEvents 流交给状态转发任务；只能取一次。
    pub(crate) fn take_watch(&mut self) -> Option<Streaming<RuntimeEvent>> {
        self.watch.take()
    }

    /// 释放唯一 watch/channel 后回收 Core，随后才尝试 guarded runtime cleanup。
    pub(crate) async fn close_and_reap(self) -> Result<(), CoreProcessError> {
        let (mut child, runtime) = self.release_for_teardown();

        if !wait_for_child_exit(&mut child, CORE_REAP_DEADLINE)
            .await
            .map_err(|_| CoreProcessError::CoreReapTimeout)?
        {
            child
                .kill()
                .map_err(|_| CoreProcessError::CoreReapTimeout)?;
            if !wait_for_child_exit(&mut child, CORE_REAP_DEADLINE)
                .await
                .map_err(|_| CoreProcessError::CoreReapTimeout)?
            {
                return Err(CoreProcessError::CoreReapTimeout);
            }
        }

        runtime
            .cleanup_empty()
            .map_err(|_| CoreProcessError::CoreRuntimeCleanupRefused)
    }

    /// 正常退出路径的有界 runtime 清理（L1 修复入口）。
    ///
    /// 与 [`Self::close_and_reap`] 的差异：**不 kill**。正常退出必须让 Core 经 UI
    /// 通道 EOF 走有序停机（保持 `CoreChild` 不做 Drop terminate 的既有语义）；
    /// 因此这里只能等待，超时即返回 typed 错误并**保留现场**（目录留给下一次启动或
    /// 卸载清理），不阻塞进程退出、不改退出码。
    ///
    /// **关键前提（真机实测得出）**：Core 的停机主路径是「认证 UDS 连接 EOF」
    /// （`kernel_control_service` 注释 + `ui_runner` 的
    /// `authenticated_channel_eof_without_watch_shuts_down_and_unlinks_socket`），
    /// 不是 stdin EOF。因此调用方必须在调用本函数**之前**释放所有 Channel 克隆
    /// （含 `CoreState` 里那一份），否则连接不关、Core 永不退出、本函数必然超时。
    /// 本函数自身释放会话持有的 watch 流与 channel（[`Self::release_for_teardown`]）。
    ///
    /// 同步实现：子进程退出探测本身就是同步 `try_wait`，有界轮询即可。调用方
    /// （[`crate::lifecycle::notify_core_shutdown`]）把它放到独立线程执行——退出入口
    /// 既可能在主线程事件循环、也可能在 tokio 任务里，而两者都不能在此 `block_on`。
    pub(crate) fn shutdown_and_cleanup_runtime_blocking(self) -> Result<(), CoreProcessError> {
        let (mut child, runtime) = self.release_for_teardown();
        let deadline = std::time::Instant::now() + CORE_SHUTDOWN_CLEANUP_DEADLINE;
        loop {
            if child
                .try_wait()
                .map_err(|()| CoreProcessError::CoreReapTimeout)?
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(CoreProcessError::CoreReapTimeout);
            }
            std::thread::sleep(CHILD_POLL_INTERVAL);
        }
        runtime
            .cleanup_empty()
            .map_err(|_| CoreProcessError::CoreRuntimeCleanupRefused)
    }

    /// 关闭唯一 stdin 并释放 channel/watch 克隆（Core 据此感知 EOF），返回回收子进程
    /// 与 runtime 目录所需的全部所有权。
    fn release_for_teardown(self) -> (CoreChild, RuntimeDir) {
        let Self {
            channel,
            watch,
            mut child,
            runtime,
        } = self;
        drop(watch);
        drop(channel);
        child.close_stdin();
        (child, runtime)
    }

}

/// 创建、认证并启动唯一 Core session。
pub(crate) async fn start_core_session() -> Result<CoreSession, CoreProcessError> {
    let runtime = RuntimeDir::create(RuntimeOwner::current())
        .map_err(|_| CoreProcessError::CoreBootstrapEarlyExit)?;
    let socket = runtime
        .engine_socket_path()
        .map_err(|_| CoreProcessError::CoreBootstrapEarlyExit)?;
    // SAFETY: `geteuid` has no arguments and only reads the current ordinary-user credential.
    let ui_uid = unsafe { libc::geteuid() };
    let arguments = CoreLaunchArguments::new(std::process::id(), ui_uid, socket)?;
    let binary = match canonical_core_binary() {
        Ok(binary) => binary,
        Err(error) => return cleanup_unstarted_runtime(runtime, error),
    };
    let mut child = match spawn_fixed_core(&binary, &arguments) {
        Ok(child) => child,
        Err(error) => return cleanup_unstarted_runtime(runtime, error),
    };

    let (bootstrap, client_key) = match UiCoreBootstrapV1::random_pair() {
        Ok(pair) => pair,
        Err(_) => {
            return abort_child_and_cleanup(child, runtime, CoreProcessError::CoreStdinWriteFailed)
                .await;
        }
    };
    let record = bootstrap.encode();
    if record.as_bytes().len() != exv_vpn_darwin_ipc::ui_core_bootstrap::UI_CORE_BOOTSTRAP_V1_LEN {
        drop(record);
        drop(client_key);
        return abort_child_and_cleanup(child, runtime, CoreProcessError::CoreStdinWriteFailed)
            .await;
    }
    if child.write_bootstrap(&record).is_err() {
        drop(record);
        drop(client_key);
        return abort_child_and_cleanup(child, runtime, CoreProcessError::CoreStdinWriteFailed)
            .await;
    }
    drop(record);

    let channel = match wait_for_core_channel(&mut child, &runtime, &arguments, &client_key).await {
        Ok(channel) => channel,
        Err(error) => {
            drop(client_key);
            return abort_child_and_cleanup(child, runtime, error).await;
        }
    };
    drop(client_key);

    let watch = match open_initial_watch(channel.clone()).await {
        Ok(watch) => watch,
        Err(error) => return abort_child_and_cleanup(child, runtime, error).await,
    };
    Ok(CoreSession {
        channel,
        watch: Some(watch),
        child,
        runtime,
    })
}

fn cleanup_unstarted_runtime(
    runtime: RuntimeDir,
    primary: CoreProcessError,
) -> Result<CoreSession, CoreProcessError> {
    runtime
        .cleanup_empty()
        .map_err(|_| CoreProcessError::CoreRuntimeCleanupRefused)?;
    Err(primary)
}

async fn abort_child_and_cleanup(
    mut child: CoreChild,
    runtime: RuntimeDir,
    primary: CoreProcessError,
) -> Result<CoreSession, CoreProcessError> {
    child.close_stdin();
    let _ = child.kill();
    if !wait_for_child_exit(&mut child, CORE_REAP_DEADLINE)
        .await
        .map_err(|_| CoreProcessError::CoreReapTimeout)?
    {
        return Err(CoreProcessError::CoreReapTimeout);
    }
    runtime
        .cleanup_empty()
        .map_err(|_| CoreProcessError::CoreRuntimeCleanupRefused)?;
    Err(primary)
}

async fn wait_for_core_channel(
    child: &mut CoreChild,
    runtime: &RuntimeDir,
    arguments: &CoreLaunchArguments,
    client_key: &exv_vpn_darwin_ipc::auth::AuthKey,
) -> Result<Channel, CoreProcessError> {
    let deadline = time::Instant::now() + CORE_READY_DEADLINE;
    loop {
        if child
            .try_wait()
            .map_err(|_| CoreProcessError::CoreBootstrapEarlyExit)?
        {
            return Err(CoreProcessError::CoreBootstrapEarlyExit);
        }
        if arguments.socket.as_path().exists() {
            runtime
                .revalidate_path()
                .map_err(|_| CoreProcessError::CoreAuthFailed)?;
            break;
        }
        if time::Instant::now() >= deadline {
            return Err(CoreProcessError::CoreReadyTimeout);
        }
        time::sleep(CHILD_POLL_INTERVAL).await;
    }

    let stream = UnixStream::connect(arguments.socket.as_path())
        .await
        .map_err(|_| CoreProcessError::CoreAuthFailed)?;
    let peer_lookup = SystemPeerLookup;
    let expected_core = ExpectedPeer::new(arguments.ui_uid, child.pid());
    let ui_identity = ExpectedPeer::new(arguments.ui_uid, arguments.ui_pid);
    authenticate_client_to_tonic_channel(
        stream,
        expected_core,
        ui_identity,
        &peer_lookup,
        client_key,
    )
    .await
    .map(|(channel, _verified_core)| channel)
    .map_err(|_| CoreProcessError::CoreAuthFailed)
}

async fn open_initial_watch(channel: Channel) -> Result<Streaming<RuntimeEvent>, CoreProcessError> {
    let mut client = KernelControlClient::new(channel);
    let mut watch = client
        .watch_events(WatchEventsRequest { resume_tick: 0 })
        .await
        .map_err(|_| CoreProcessError::CoreAuthFailed)?
        .into_inner();
    // 首个事件是 Core 发布的 initial Idle（tick=1 + Snapshot kind + Idle state）——
    // 只作就绪校验（语义等价旧 IdleSnapshot 载体；快照内容经 wire 层随事件流直达前端）。
    let initial_event = time::timeout(CORE_READY_DEADLINE, watch.message())
        .await
        .map_err(|_| CoreProcessError::CoreReadyTimeout)?
        .map_err(|_| CoreProcessError::CoreAuthFailed)?
        .ok_or(CoreProcessError::CoreChannelEof)?;
    if initial_event.monotonic_tick != 1
        || initial_event.kind != RuntimeEventKind::Snapshot as i32
        || !matches!(
            initial_event.snapshot.as_ref().map(|s| &s.state),
            Some(Some(runtime_snapshot::State::Idle(_)))
        )
    {
        return Err(CoreProcessError::CoreAuthFailed);
    }
    Ok(watch)
}
