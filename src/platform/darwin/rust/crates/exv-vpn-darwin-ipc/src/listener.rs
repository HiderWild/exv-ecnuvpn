//! 认证前 UDS listener 的 bind、发布与单连接准入。

use std::{
    fmt, fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    net::{UnixListener, UnixStream},
    time::{self, Instant},
};

use crate::{
    auth::{
        AuthKey, AuthenticatedBinding, OneTimeAuthenticator, PreauthError, authenticate_server,
    },
    path::{
        EndpointIdentity, RuntimeDir, RuntimeOwner, SocketPath, guarded_cleanup,
        publish_listener_barrier,
    },
    peer::{ExpectedPeer, PeerLookup, SystemPeerLookup},
};

/// publish barrier 前的可注入故障 seam。
///
/// 生产使用 [`AllowPublishBarrier`]；fixture 仅用它复现 bind 后路径被替换或发布失败。
pub trait PublishBarrierHook: Send + Sync {
    /// 在 UDS bind 完成、发布屏障开始前执行。
    ///
    /// # Errors
    ///
    /// 返回错误时 bind 后的当前 endpoint 会经过 guarded cleanup，且 listener 不会发布。
    fn before_publish(&self, socket_path: &SocketPath) -> Result<(), PreauthError>;
}

/// 生产默认的 publish barrier 策略。
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowPublishBarrier;

impl PublishBarrierHook for AllowPublishBarrier {
    fn before_publish(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        Ok(())
    }
}

/// root Engine socket 发布步骤的 crate 内测试故障 seam。
///
/// 生产配置固定使用 [`AllowRootPublish`]。fixture 只能在初始 held-dirfd identity 已记录后、最终
/// handoff 前注入失败或 inode 替换，以证明失败时不会创建可 accept 的
/// [`PreauthListener`]，不能借此改变认证 wire 或发布任意路径。
pub(crate) trait RootPublishHook: Send + Sync {
    /// 在 sealed runtime dirfd 的 `fchownat(..., AT_SYMLINK_NOFOLLOW)` 前执行。
    ///
    /// 该 seam 只用于模拟所有权变更失败或在其前替换 leaf；实现必须在 hook 返回后重新
    /// 通过 held dirfd 核对初始 socket identity。若已替换，guarded cleanup 必须保留替身。
    fn before_fchownat(&self, socket_path: &SocketPath) -> Result<(), PreauthError>;

    /// 在 socket/parent 最终 identity recheck 前执行。
    fn before_recheck(&self, socket_path: &SocketPath) -> Result<(), PreauthError>;
}

/// root publisher 的生产默认 hook。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AllowRootPublish;

impl RootPublishHook for AllowRootPublish {
    fn before_fchownat(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        Ok(())
    }

    fn before_recheck(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        Ok(())
    }
}

/// listener bind 所需的本地安全事实与认证材料。
pub struct ListenerConfig {
    socket_path: SocketPath,
    runtime_owner: RuntimeOwner,
    expected_core: ExpectedPeer,
    auth_key: AuthKey,
    global_deadline: Duration,
    peer_lookup: Arc<dyn PeerLookup>,
    publish_barrier_hook: Arc<dyn PublishBarrierHook>,
}

impl ListenerConfig {
    /// 用生产 peer lookup 和 30 秒全局认证 deadline 创建配置。
    #[must_use]
    pub fn new(
        socket_path: SocketPath,
        runtime_owner: RuntimeOwner,
        expected_core: ExpectedPeer,
        auth_key: AuthKey,
    ) -> Self {
        Self {
            socket_path,
            runtime_owner,
            expected_core,
            auth_key,
            global_deadline: Duration::from_secs(30),
            peer_lookup: Arc::new(SystemPeerLookup),
            publish_barrier_hook: Arc::new(AllowPublishBarrier),
        }
    }

    /// 为 fixture 注入 OS peer 读取器。
    #[must_use]
    pub fn with_peer_lookup(mut self, peer_lookup: Arc<dyn PeerLookup>) -> Self {
        self.peer_lookup = peer_lookup;
        self
    }

    /// 覆盖 listener 的全局认证 deadline。
    #[must_use]
    pub const fn with_global_deadline(mut self, global_deadline: Duration) -> Self {
        self.global_deadline = global_deadline;
        self
    }

    /// 为 fixture 注入 bind 后、发布前的 endpoint 变化。
    #[must_use]
    pub fn with_publish_barrier_hook(
        mut self,
        publish_barrier_hook: Arc<dyn PublishBarrierHook>,
    ) -> Self {
        self.publish_barrier_hook = publish_barrier_hook;
        self
    }
}

/// root Engine 发布已认证 UDS 所需的固定材料。
///
/// socket path 由 `runtime_dir` 的固定 `engine.sock` 名称派生；调用方不能注入任意
/// pathname。它必须已经完成 root-seal；成功返回的 listener 持有同一 `RuntimeDir`，使
/// accept/cleanup 一直基于同一个 sealed dirfd identity。
pub struct RootListenerConfig {
    runtime_dir: RuntimeDir,
    expected_core: ExpectedPeer,
    auth_key: AuthKey,
    global_deadline: Duration,
    peer_lookup: Arc<dyn PeerLookup>,
    root_publish_hook: Arc<dyn RootPublishHook>,
    #[cfg(test)]
    after_bind_before_initial_record_hook: Arc<dyn AfterBindBeforeInitialRecordHook>,
}

impl RootListenerConfig {
    /// 创建 root publisher 的生产配置。
    ///
    /// # Errors
    ///
    /// 未完成 root-seal 时返回 [`PreauthError::PathInvalid`]；`expected_core.uid` 不等于
    /// seal 前记录的 Core owner 时返回 [`PreauthError::EndpointOwnership`]。这样 root
    /// Engine 不能把 socket 交给和 ticket owner 不同的用户。
    pub fn new(
        runtime_dir: RuntimeDir,
        expected_core: ExpectedPeer,
        auth_key: AuthKey,
    ) -> Result<Self, PreauthError> {
        if !runtime_dir.is_root_sealed() {
            return Err(PreauthError::PathInvalid);
        }
        if expected_core.uid() != runtime_dir.owner().uid() {
            return Err(PreauthError::EndpointOwnership);
        }
        Ok(Self {
            runtime_dir,
            expected_core,
            auth_key,
            global_deadline: Duration::from_secs(30),
            peer_lookup: Arc::new(SystemPeerLookup),
            root_publish_hook: Arc::new(AllowRootPublish),
            #[cfg(test)]
            after_bind_before_initial_record_hook: Arc::new(AllowAfterBindBeforeInitialRecord),
        })
    }

    /// 为 fixture 注入 OS peer 读取器。
    #[must_use]
    pub fn with_peer_lookup(mut self, peer_lookup: Arc<dyn PeerLookup>) -> Self {
        self.peer_lookup = peer_lookup;
        self
    }

    /// 覆盖 listener 的全局认证 deadline。
    #[must_use]
    pub const fn with_global_deadline(mut self, global_deadline: Duration) -> Self {
        self.global_deadline = global_deadline;
        self
    }

}

/// 已通过认证、尚未交给 tonic bridge 的同一条 UDS stream。
pub struct AuthenticatedConnection {
    stream: UnixStream,
    binding: AuthenticatedBinding,
}

impl fmt::Debug for AuthenticatedConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedConnection")
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

impl PartialEq for AuthenticatedConnection {
    fn eq(&self, other: &Self) -> bool {
        self.binding == other.binding
    }
}

impl Eq for AuthenticatedConnection {}

impl AuthenticatedConnection {
    /// 返回认证绑定。
    #[must_use]
    pub const fn binding(&self) -> AuthenticatedBinding {
        self.binding
    }

    /// 取回认证成功的原始 stream；调用方只能接入 Common tonic bridge。
    #[must_use]
    pub fn into_stream(self) -> UnixStream {
        self.stream
    }
}

/// 一个 listener 只接受一个成功的 ClientProof，随后稳定拒绝 replay。
pub struct PreauthListener {
    listener: UnixListener,
    socket_path: SocketPath,
    endpoint: EndpointIdentity,
    /// 只有 root publisher 成功时存在；保持 dirfd 直到 listener cleanup 完成。
    runtime_dir: Option<RuntimeDir>,
    expected_core: ExpectedPeer,
    authenticator: OneTimeAuthenticator,
    peer_lookup: Arc<dyn PeerLookup>,
    global_deadline: Instant,
}

enum AdmissionFailure {
    Listener(PreauthError),
    Connection(PreauthError),
}

impl AdmissionFailure {
    const fn error(self) -> PreauthError {
        match self {
            Self::Listener(error) | Self::Connection(error) => error,
        }
    }
}

/// bind 后、publish 前记录的 endpoint 事实。
///
/// 当 publish 失败时，它只用于判断是否仍能安全 unlink 刚刚创建的同一 socket；它
/// 不替代成功 publish 后由 `path` 模块记录的 [`EndpointIdentity`]。
#[derive(Clone, Copy)]
struct BoundEndpointIdentity {
    device: u64,
    inode: u64,
    owner: RuntimeOwner,
    mode: u32,
}

impl PreauthListener {
    /// 验证路径后 bind，并完成 endpoint 的 owner-only 发布屏障。
    ///
    /// # Errors
    ///
    /// 当 runtime path 不安全、endpoint 已存在、bind/发布失败或 hook 拒绝发布时返回
    /// 相应的 [`PreauthError`]，且只会 guarded-cleanup 本次 bind 的 endpoint。
    pub fn bind(config: ListenerConfig) -> Result<Self, PreauthError> {
        config
            .socket_path
            .validate_runtime_dir(config.runtime_owner)?;
        match fs::symlink_metadata(config.socket_path.as_path()) {
            Ok(_) => return Err(PreauthError::EndpointExists),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(PreauthError::PathInvalid),
        }

        let listener = UnixListener::bind(config.socket_path.as_path())
            .map_err(|_| PreauthError::Transport)?;
        let bound_endpoint = match capture_bound_endpoint(&config.socket_path, config.runtime_owner)
        {
            Ok(identity) => identity,
            Err(error) => {
                drop(listener);
                return Err(error);
            }
        };
        if let Err(error) = config
            .publish_barrier_hook
            .before_publish(&config.socket_path)
        {
            drop(listener);
            let _ = guarded_cleanup_bound_endpoint(
                &config.socket_path,
                config.runtime_owner,
                bound_endpoint,
            );
            return Err(error);
        }
        let endpoint = match publish_listener_barrier(&config.socket_path, config.runtime_owner) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                // A failed barrier may have raced a replacement. Only unlink when the current
                // endpoint still matches the bind-time device/inode/type/owner/mode facts.
                drop(listener);
                let _ = guarded_cleanup_bound_endpoint(
                    &config.socket_path,
                    config.runtime_owner,
                    bound_endpoint,
                );
                return Err(error);
            }
        };

        Ok(Self {
            listener,
            socket_path: config.socket_path,
            endpoint,
            runtime_dir: None,
            expected_core: config.expected_core,
            authenticator: OneTimeAuthenticator::new(config.auth_key),
            peer_lookup: config.peer_lookup,
            global_deadline: Instant::now() + config.global_deadline,
        })
    }

    /// 在单线程 root Engine bootstrap 中发布已封印 runtime directory 内的固定 socket。
    ///
    /// 该函数不会检查 euid/ruid；E3 Engine entry 必须先完成进程身份和 ticket/argv
    /// 校验，再用 [`RuntimeDir::seal_for_root_publisher`] 封印 directory。这里拒绝
    /// unsealed directory；同 uid fixture 只验证发布后的非破坏性失败路径，不能替代 E4 的
    /// 真正 root/sticky 跨 uid 证据。
    ///
    /// # Safety
    ///
    /// `umask` 是进程全局状态。调用者必须保证当前 Engine bootstrap 是单线程、且没有任何
    /// 其他线程同时创建文件或 socket；函数在 bind 后立即恢复先前 umask。
    ///
    /// # Errors
    ///
    /// runtime directory 变化、endpoint 已存在、bind、`fchownat`、hook 或最终
    /// parent/socket recheck 失败时，返回 [`PreauthError`]，关闭 listener 且不返回可 accept
    /// 的对象。bind 后第一次 held-dirfd `fstatat` 成功记录 identity 前，所有失败只关闭并
    /// 保留 leaf；记录成功后才允许 sealed-dirfd guarded cleanup。任何 parent/runtime
    /// identity 不匹配均保留现场。
    pub unsafe fn bind_root_publisher_single_threaded(
        config: RootListenerConfig,
    ) -> Result<Self, PreauthError> {
        #[cfg(test)]
        let after_bind_before_initial_record_hook =
            Arc::clone(&config.after_bind_before_initial_record_hook);
        let RootListenerConfig {
            runtime_dir,
            expected_core,
            auth_key,
            global_deadline,
            peer_lookup,
            root_publish_hook,
            ..
        } = config;
        let socket_path = prepare_root_publisher_socket(&runtime_dir)?;

        // SAFETY: the caller upholds the documented single-threaded bootstrap requirement; the
        // helper restores the old process umask before returning.
        let listener = with_private_umask(|| {
            UnixListener::bind(socket_path.as_path()).map_err(|_| PreauthError::Transport)
        })?;
        #[cfg(test)]
        if let Err(error) =
            after_bind_before_initial_record_hook.after_bind_before_initial_record(&socket_path)
        {
            drop(listener);
            // The initial held-dirfd endpoint identity was not recorded. Preserve the leaf.
            return Err(error);
        }
        if runtime_dir.revalidate_path().is_err() {
            drop(listener);
            // A bind-time parent/runtime replacement can redirect the pathname. Preserve it;
            // no identity was recorded and no cleanup is safe here.
            return Err(PreauthError::PathInvalid);
        }
        let bound_endpoint = match runtime_dir.engine_socket_identity() {
            Ok(identity) => identity,
            Err(error) => {
                drop(listener);
                // The first held-dirfd record did not succeed, so preserve the leaf.
                return Err(error);
            }
        };
        if bound_endpoint.mode != 0o600 {
            return abort_root_publish(
                listener,
                runtime_dir,
                bound_endpoint,
                PreauthError::PathInvalid,
            );
        }

        if let Err(error) = root_publish_hook.before_fchownat(&socket_path) {
            return abort_root_publish(listener, runtime_dir, bound_endpoint, error);
        }
        if runtime_dir.revalidate_path().is_err() {
            drop(listener);
            // A parent/runtime mismatch is fail-closed and must preserve the runtime evidence.
            return Err(PreauthError::PathInvalid);
        }
        let before_fchown = match runtime_dir.engine_socket_identity() {
            Ok(identity) if identity == bound_endpoint => identity,
            Ok(_) | Err(_) => {
                return abort_root_publish(
                    listener,
                    runtime_dir,
                    bound_endpoint,
                    PreauthError::PathInvalid,
                );
            }
        };
        if let Err(error) = runtime_dir.fchown_engine_socket_nofollow(runtime_dir.owner()) {
            return abort_root_publish(listener, runtime_dir, before_fchown, error);
        }
        let published_endpoint = EndpointIdentity {
            device: bound_endpoint.device,
            inode: bound_endpoint.inode,
            owner: runtime_dir.owner(),
            mode: 0o600,
        };
        if let Err(error) = root_publish_hook.before_recheck(&socket_path) {
            return abort_root_publish(listener, runtime_dir, published_endpoint, error);
        }
        if runtime_dir.revalidate_path().is_err() {
            drop(listener);
            // A parent/runtime mismatch after bind must preserve the held filesystem evidence.
            return Err(PreauthError::PathInvalid);
        }
        let endpoint = match revalidate_published_root_endpoint(&runtime_dir, published_endpoint) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                return abort_root_publish(listener, runtime_dir, published_endpoint, error);
            }
        };

        Ok(Self {
            listener,
            socket_path,
            endpoint,
            runtime_dir: Some(runtime_dir),
            expected_core,
            authenticator: OneTimeAuthenticator::new(auth_key),
            peer_lookup,
            global_deadline: Instant::now() + global_deadline,
        })
    }

    /// 等待并认证一条连接。
    ///
    /// 单条失败连接只会关闭自身；只要全局 deadline 尚未到期，调用方可再次调用本方法，
    /// 而未成功的 key 仍保持可用。
    ///
    /// # Errors
    ///
    /// 当 listener accept、全局 deadline 或当前连接的 pre-auth 认证失败时返回相应的
    /// [`PreauthError`]；连接级失败不会消费 key。
    pub async fn accept_preface(&self) -> Result<AuthenticatedConnection, PreauthError> {
        self.accept_one().await.map_err(AdmissionFailure::error)
    }

    async fn accept_one(&self) -> Result<AuthenticatedConnection, AdmissionFailure> {
        let (mut stream, _) = time::timeout_at(self.global_deadline, self.listener.accept())
            .await
            .map_err(|_| AdmissionFailure::Listener(PreauthError::AuthTimeout))?
            .map_err(|_| AdmissionFailure::Listener(PreauthError::Transport))?;

        let remaining = self
            .global_deadline
            .saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AdmissionFailure::Connection(PreauthError::AuthTimeout));
        }
        let binding = time::timeout(
            remaining,
            authenticate_server(
                &mut stream,
                self.expected_core,
                self.peer_lookup.as_ref(),
                &self.authenticator,
            ),
        )
        .await
        .map_err(|_| AdmissionFailure::Connection(PreauthError::AuthTimeout))?
        .map_err(AdmissionFailure::Connection)?;
        Ok(AuthenticatedConnection { stream, binding })
    }

    /// 丢弃认证前失败连接，并在全局 deadline 内继续等待一个合法 Core。
    ///
    /// 该便捷入口让生产 accept loop 不会因坏 MAC、EOF、错误 peer 或单连接握手超时
    /// 停止；这些失败都只关闭当前 stream，且不会消费 listener key。需要断言具体拒绝
    /// 类别的测试可直接调用 [`Self::accept_preface`]。
    ///
    /// # Errors
    ///
    /// 仅在 listener 级 I/O/timeout、全局 deadline 到期或成功认证后检测到 replay 时返回
    /// [`PreauthError`]；其他连接级拒绝会继续等待。
    pub async fn accept_until_authenticated(
        &self,
    ) -> Result<AuthenticatedConnection, PreauthError> {
        loop {
            match self.accept_one().await {
                Ok(connection) => return Ok(connection),
                Err(AdmissionFailure::Listener(error)) => return Err(error),
                Err(AdmissionFailure::Connection(PreauthError::AuthReplay)) => {
                    return Err(PreauthError::AuthReplay);
                }
                Err(AdmissionFailure::Connection(PreauthError::AuthTimeout))
                    if Instant::now() >= self.global_deadline =>
                {
                    return Err(PreauthError::AuthTimeout);
                }
                Err(AdmissionFailure::Connection(_)) => {}
            }
        }
    }

    /// 把 listener 关闭后，只清理仍与 bind 后事实完全相符的 endpoint。
    ///
    /// # Errors
    ///
    /// 当 endpoint 已被替换、不再安全或无法 unlink 时返回 [`PreauthError::CleanupRefused`]。
    pub fn cleanup(self) -> Result<(), PreauthError> {
        let Self {
            listener,
            socket_path,
            endpoint,
            runtime_dir,
            ..
        } = self;
        drop(listener);
        if let Some(runtime_dir) = runtime_dir {
            runtime_dir.cleanup_sealed_engine_socket_and_runtime(endpoint)
        } else {
            guarded_cleanup(&socket_path, endpoint)
        }
    }

    /// 返回该 listener 的 key 是否已经成功消费，供非产品测试观察。
    ///
    /// # Errors
    ///
    /// 当认证器内部状态锁不可用时返回 [`PreauthError::Transport`]。
    pub fn key_is_consumed(&self) -> Result<bool, PreauthError> {
        self.authenticator.is_consumed()
    }
}

fn capture_bound_endpoint(
    socket_path: &SocketPath,
    owner: RuntimeOwner,
) -> Result<BoundEndpointIdentity, PreauthError> {
    let metadata =
        fs::symlink_metadata(socket_path.as_path()).map_err(|_| PreauthError::Transport)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(PreauthError::PathInvalid);
    }
    if metadata.uid() != owner.uid() || metadata.gid() != owner.gid() {
        return Err(PreauthError::EndpointOwnership);
    }
    Ok(BoundEndpointIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner,
        mode: metadata.mode() & 0o7777,
    })
}

fn guarded_cleanup_bound_endpoint(
    socket_path: &SocketPath,
    owner: RuntimeOwner,
    expected: BoundEndpointIdentity,
) -> Result<(), PreauthError> {
    let metadata =
        fs::symlink_metadata(socket_path.as_path()).map_err(|_| PreauthError::CleanupRefused)?;
    let mode = metadata.mode() & 0o7777;
    let mode_matches_bind_or_publish = mode == expected.mode || mode == 0o600;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != owner.uid()
        || metadata.gid() != owner.gid()
        || expected.owner != owner
        || metadata.dev() != expected.device
        || metadata.ino() != expected.inode
        || !mode_matches_bind_or_publish
    {
        return Err(PreauthError::CleanupRefused);
    }
    fs::remove_file(socket_path.as_path()).map_err(|_| PreauthError::CleanupRefused)
}

struct UmaskRestore(libc::mode_t);

impl Drop for UmaskRestore {
    fn drop(&mut self) {
        // SAFETY: restoring the value returned by the immediately preceding `umask` is the only
        // side effect of this guard; Drop runs exactly once.
        unsafe {
            libc::umask(self.0);
        }
    }
}

fn revalidate_published_root_endpoint(
    runtime_dir: &RuntimeDir,
    expected: EndpointIdentity,
) -> Result<EndpointIdentity, PreauthError> {
    match runtime_dir.engine_socket_identity() {
        Ok(identity) if identity == expected => Ok(identity),
        Ok(identity) if identity.owner == runtime_dir.owner() => Err(PreauthError::PathInvalid),
        Ok(_) => Err(PreauthError::EndpointOwnership),
        Err(error) => Err(error),
    }
}

fn prepare_root_publisher_socket(runtime_dir: &RuntimeDir) -> Result<SocketPath, PreauthError> {
    if !runtime_dir.is_root_sealed() {
        return Err(PreauthError::PathInvalid);
    }
    runtime_dir
        .revalidate_path()
        .map_err(|_| PreauthError::PathInvalid)?;
    runtime_dir.ensure_engine_socket_absent()?;
    runtime_dir
        .engine_socket_path()
        .map_err(|_| PreauthError::PathInvalid)
}

/// 进程内私有 umask 窗口锁（见 [`with_private_umask`]）。
static PRIVATE_UMASK_WINDOW: Mutex<()> = Mutex::new(());

/// 在**私有 umask 窗口**内执行 `create`（`umask(0o177)` → `create()` → 还原），且窗口在
/// 进程内互斥。
///
/// 为什么必须互斥：`umask(2)` 是**进程级**状态。若两处同时进入窗口，后进入者会把先进入者
/// 的临时 umask 当作自己的 previous，还原后把 0177 **永久留在进程**里；先进入者则可能以
/// ambient umask 创建 socket（例如 0755），随后 identity 复核按
/// [`PreauthError::PathInvalid`] 拒绝——真机表现为「随机的 bind service listener 失败」
/// （2026-09-20 复现：40 轮 ipc 套件里 2 次，失败用例各不相同）。
///
/// 生产调用点（engine 入口、root publisher、服务代理 daemon）本就在单线程 runtime 的
/// spawn 之前，但实现不再依赖该前提：任何调用点都可以并发调用本函数。
pub(crate) fn with_private_umask<T>(create: impl FnOnce() -> T) -> T {
    // 锁中毒只说明另一线程曾在窗口内 panic；umask 仍由 guard 还原，继续执行是安全的。
    let _window = PRIVATE_UMASK_WINDOW
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: 窗口由 `_window` 在本进程内独占；`umask` 返回值由 guard 在每条路径上还原。
    let previous = unsafe { libc::umask(0o177) };
    let restore = UmaskRestore(previous);
    let value = create();
    drop(restore);
    value
}

/// 关闭未发布 listener，并只在 initial held-dirfd identity 已成功记录后清理。
///
/// 该 helper 的调用点都发生在 `bind_root_publisher_single_threaded` 取得
/// `engine_socket_identity()` 之后。初始 record 前的错误路径只关闭 listener 并保留 leaf，
/// 绝不能调用本函数。
fn abort_root_publish(
    listener: UnixListener,
    runtime_dir: RuntimeDir,
    endpoint: EndpointIdentity,
    error: PreauthError,
) -> Result<PreauthListener, PreauthError> {
    drop(listener);
    let _ = runtime_dir.cleanup_sealed_engine_socket_and_runtime(endpoint);
    Err(error)
}
