//! W2.5 service 形态固定端点监听器：uid 门准入、无 HMAC 握手。
//!
//! 权威规范（2026-09-08 darwin 引擎生命周期权威规范 §二.2.7）：常驻 service engine 的
//! 端点认证是**单因子 uid 门**——固定 root 控制目录内的 UDS + `getpeereid` 校验对端
//! uid == owner uid；「同 uid 可信」为既定信任边界，per-install PSK 方案已作废。
//! 本模块与既有 [`crate::listener`]（per-session HMAC ticket）并列：不共享一次性 key
//! 语义，共享 guarded 清理纪律与 tonic bridge 交付路径。
//!
//! 监听前 guarded 清 stale：既有 leaf 必须是 socket ∧ 属主 == owner uid ∧ mode 0600
//! 才允许回收（对齐服务代理 `macos.rs` 的 `remove_safe_stale_socket` 先例——属主
//! 严格校验、绝不删除其他文件）；任何不匹配保留现场并拒绝 bind。

use std::{
    ffi::CString,
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt},
        },
    },
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::net::UnixListener;

use crate::{
    auth::PreauthError,
    listener::with_private_umask,
    path::{EndpointIdentity, MAX_UNIX_SOCKET_PATH_BYTES, RuntimeOwner, SocketPath},
    peer::{PeerLookup, SystemPeerLookup, VerifiedLocalPeer},
    tonic_bridge::AuthenticatedUnixStream,
};

/// service 形态固定端点 socket 的固定 mode（umask 0177 下 bind 产物；属主为
/// owner uid、组位与其它位全零——目录本身 root 0755）。
const SERVICE_SOCKET_MODE: u32 = 0o600;

/// 仅 service 端点建立使用的原错误；认证和 peer 错误仍沿用 PreauthError。
#[derive(Debug)]
pub enum ServiceEndpointError {
    /// 既有路径、身份或清理拒绝。
    Existing(PreauthError),
    /// 本次 connect 或 bind 的原 IO；不从 errno 重建错误。
    Io {
        step: &'static str,
        source: io::Error,
    },
}

impl From<PreauthError> for ServiceEndpointError {
    fn from(error: PreauthError) -> Self {
        Self::Existing(error)
    }
}

impl std::fmt::Display for ServiceEndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Existing(error) => std::fmt::Display::fmt(error, formatter),
            Self::Io { step, source } => write!(
                formatter,
                "Darwin service endpoint {step} failed: kind={:?} errno={:?}: {source}",
                source.kind(), source.raw_os_error(),
            ),
        }
    }
}

impl std::error::Error for ServiceEndpointError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Existing(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// service 形态监听器的构造材料。
pub struct ServiceListenerConfig {
    socket_path: SocketPath,
    owner_uid: u32,
    peer_lookup: Arc<dyn PeerLookup>,
}

impl ServiceListenerConfig {
    /// 创建生产配置：固定端点路径 + uid 门的期望 owner（来自 engine `--owner-uid`）。
    #[must_use]
    pub fn new(socket_path: SocketPath, owner_uid: u32) -> Self {
        Self {
            socket_path,
            owner_uid,
            peer_lookup: Arc::new(SystemPeerLookup),
        }
    }

    /// 为 fixture 注入 OS peer 读取器。
    #[must_use]
    pub fn with_peer_lookup(mut self, peer_lookup: Arc<dyn PeerLookup>) -> Self {
        self.peer_lookup = peer_lookup;
        self
    }
}

/// uid 门准入后的连接：持有 OS 验证过的 peer 事实，可直入 tonic bridge。
pub struct ServiceConnection {
    stream: tokio::net::UnixStream,
    peer: VerifiedLocalPeer,
}

/// 底层 socket 的强制关闭句柄（`dup` 的独立 fd）。
///
/// 用途：service 会话循环对挂起对端的最终收权——tonic 经 incoming 通道 spawn 的
/// 连接任务无法从外部 abort，`shutdown(SHUT_RDWR)` 在内核层掐断 socket，立即
/// 唤醒两侧在途 IO（对端见 EOF）。`dup` fd 独立存活，无 fd 复用误伤窗口；drop
/// 自动 close。
#[derive(Debug)]
pub struct SocketShutdownHandle {
    descriptor: std::os::fd::OwnedFd,
}

impl SocketShutdownHandle {
    /// 强制关闭底层 socket（幂等；失败忽略——socket 可能已由对端关闭）。
    pub fn shutdown(&self) {
        use std::os::fd::AsRawFd;
        // SAFETY: fd 是本 handle 独立持有的 dup 副本；shutdown 只影响该 socket，
        // 不触碰调用方任何状态。
        unsafe {
            libc::shutdown(self.descriptor.as_raw_fd(), libc::SHUT_RDWR);
        }
    }
}

impl ServiceConnection {
    /// uid 门验证过的对端事实（OS credential，非 wire 字段）。
    #[must_use]
    pub fn verified_peer(&self) -> VerifiedLocalPeer {
        self.peer
    }

    /// 取底层 socket 的强制关闭句柄（必须在 stream 移交 tonic 前调用）。
    ///
    /// # Errors
    ///
    /// `dup` 失败返回 [`PreauthError::Transport`]。
    pub fn socket_shutdown_handle(&self) -> Result<SocketShutdownHandle, PreauthError> {
        use std::os::fd::{AsRawFd, FromRawFd};
        // SAFETY: stream 活着且 fd 属于它；dup 成功返回独立拥有的新 fd。
        let descriptor = unsafe { libc::dup(self.stream.as_raw_fd()) };
        if descriptor < 0 {
            return Err(PreauthError::Transport);
        }
        // SAFETY: 非负 fd 来自本次 dup，所有权唯一移交 OwnedFd。
        Ok(SocketShutdownHandle {
            descriptor: unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptor) },
        })
    }

    /// 把已通过 uid 门的 stream 交给 tonic bridge（peer metadata 来自 OS 事实）。
    #[must_use]
    pub fn into_authenticated_unix_stream(self) -> AuthenticatedUnixStream {
        AuthenticatedUnixStream::from_verified_service_peer(self.stream, self.peer)
    }
}

/// service 形态的常驻固定端点监听器：一次 bind，跨会话顺序 accept。
pub struct ServiceListener {
    listener: UnixListener,
    socket_path: SocketPath,
    endpoint: EndpointIdentity,
    owner_uid: u32,
    peer_lookup: Arc<dyn PeerLookup>,
}

impl std::fmt::Debug for ServiceListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServiceListener")
            .field("socket_path", &self.socket_path.as_path())
            .field("owner_uid", &self.owner_uid)
            .finish_non_exhaustive()
    }
}

impl ServiceListener {
    /// guarded 清 stale 后 bind 固定端点，并把 socket 属主置为 owner uid、0600。
    ///
    /// 顺序契约：
    /// 1. 校验 parent directory：非 symlink 目录、属主 == 当前 euid（生产 root 的
    ///    root-controlled 固定安装目录）、无组/其他写位（服务代理
    ///    `ensure_root_controlled_directory` 的同款纪律）；
    /// 2. 若 leaf 已存在：socket ∧ 属主 == owner uid ∧ 0600，且原 connect 返回
    ///    ECONNREFUSED 才回收；ENOENT 不删除而继续 bind，其他 IO 保留目标返回；
    ///    否则保留现场并拒绝（[`PreauthError::EndpointOwnership`]/
    ///    [`PreauthError::PathInvalid`]）；
    /// 3. 私有 umask（0177）下在同目录短临时路径bind，产物mode恒0600；
    /// 4. `fchownat(parent_fd, leaf, uid=owner, gid 不变, AT_SYMLINK_NOFOLLOW)`
    ///    （socket leaf 不能 open，属主变更走服务代理 `seal_bound_socket` 同款
    ///    parent-dirfd 路径；生产引擎 euid=0 可跨 uid 赋属，同 uid 测试自然成立）；
    /// 5. 复验identity，再以不覆盖目标的原子rename发布正式路径；cleanup沿该identity。
    ///
    /// # Errors
    ///
    /// parent、身份或清理拒绝保留既有 [`PreauthError`]；connect/bind 失败携带原 IO。
    /// 不因任意连接失败删除目标。
    pub fn bind(config: ServiceListenerConfig) -> Result<Self, ServiceEndpointError> {
        Self::bind_with_connect(config, |path| std::os::unix::net::UnixStream::connect(path))
    }

    fn bind_with_connect(
        config: ServiceListenerConfig,
        connect: impl FnOnce(&Path) -> io::Result<std::os::unix::net::UnixStream>,
    ) -> Result<Self, ServiceEndpointError> {
        Self::bind_with_publication(config, connect, seal_service_socket, publish_service_socket)
    }

    // 局部阶段接缝用于暂停封印/发布；生产始终使用原封印与独占原子发布动作。
    fn bind_with_publication(
        config: ServiceListenerConfig,
        connect: impl FnOnce(&Path) -> io::Result<std::os::unix::net::UnixStream>,
        seal: impl FnOnce(&Path, u32) -> Result<(), PreauthError>,
        publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> Result<Self, ServiceEndpointError> {
        let ServiceListenerConfig { socket_path, owner_uid, peer_lookup } = config;
        validate_service_parent(socket_path.as_path())?;
        remove_safe_stale_service_socket_with_connect(socket_path.as_path(), owner_uid, connect)?;
        let (listener, mut pending) = bind_pending_service_socket(socket_path.as_path())?;
        seal(&pending.path, owner_uid)?;
        let endpoint = service_endpoint_identity(&pending.path, owner_uid)?;
        publish(&pending.path, socket_path.as_path())
            .map_err(|source| ServiceEndpointError::Io { step: "publish", source })?;
        // rename后device/inode不变；后续失败仍只回收本次实际发布的端点。
        pending.path = socket_path.as_path().to_path_buf();
        if service_endpoint_identity(socket_path.as_path(), owner_uid)? != endpoint {
            return Err(PreauthError::EndpointOwnership.into());
        }
        pending.published = true;
        Ok(Self { listener, socket_path, endpoint, owner_uid, peer_lookup })
    }

    /// 本端点期望的 owner uid（uid 门参数）。
    #[must_use]
    pub const fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    /// accept 下一条连接并执行 uid 门：门失败的连接只关闭自身，继续等待。
    ///
    /// # Errors
    ///
    /// 仅 listener 级 accept 失败返回 [`PreauthError::Transport`]；uid 门失败是
    /// 连接级拒绝，不终止监听。
    pub async fn accept_uid_gated(&self) -> Result<ServiceConnection, PreauthError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|_| PreauthError::Transport)?;
            let Ok(peer) = self.peer_lookup.inspect(&stream) else {
                // OS credential 不可读：连接级拒绝，关闭后继续 accept。
                continue;
            };
            if peer.uid() == self.owner_uid {
                return Ok(ServiceConnection { stream, peer });
            }
            // uid 门拒绝：关闭该连接，保持监听（不回写任何诊断，避免向陌生 uid 泄密）。
        }
    }

    /// 关闭监听并 guarded 清理本端点（仅当 leaf 仍与 bind 后记录的 identity 一致）。
    ///
    /// 取 `&self`：常驻进程的 listener 经 `Arc` 共享（busy 拒绝循环与会话循环），
    /// 清理只做 identity 校验 + unlink；listener fd 随所属 `Arc` 析构关闭。
    ///
    /// # Errors
    ///
    /// leaf 缺失、被替换或属主/mode 不符时返回 [`PreauthError::CleanupRefused`] 并保留现场。
    pub fn cleanup(&self) -> Result<(), PreauthError> {
        let current = service_endpoint_identity(self.socket_path.as_path(), self.owner_uid)
            .map_err(|_| PreauthError::CleanupRefused)?;
        if current != self.endpoint {
            return Err(PreauthError::CleanupRefused);
        }
        std::fs::remove_file(self.socket_path.as_path())
            .map_err(|_| PreauthError::CleanupRefused)
    }
}

// 同目录临时socket只属于一次发布尝试。正常失败按创建inode回收；意外进程退出的
// 临时leaf不作为后续启动前置，新的短名继续发布。正式路径与持久数据位置沿用。
struct PendingServiceSocket {
    path: PathBuf,
    device: u64,
    inode: u64,
    published: bool,
}

impl Drop for PendingServiceSocket {
    fn drop(&mut self) {
        if self.published { return; }
        let Ok(metadata) = std::fs::symlink_metadata(&self.path) else { return; };
        if metadata.file_type().is_socket()
            && metadata.dev() == self.device && metadata.ino() == self.inode
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn pending_socket_path(target: &Path, random: &[u8; 16]) -> Result<PathBuf, ServiceEndpointError> {
    let parent = target.parent().ok_or(PreauthError::PathInvalid)?;
    // 原正式路径允许103字节；极长parent同样保留单字节leaf的能力。
    let budget = MAX_UNIX_SOCKET_PATH_BYTES.checked_sub(parent.as_os_str().as_bytes().len() + 1)
        .filter(|budget| *budget > 0).ok_or(PreauthError::PathInvalid)?;
    let length = budget.min(random.len());
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut name: Vec<u8> = random[..length].iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()]).collect();
    if length >= 4 { name[..2].copy_from_slice(b".s"); }
    Ok(parent.join(std::ffi::OsStr::from_bytes(&name)))
}

fn bind_pending_service_socket(target: &Path) -> Result<(UnixListener, PendingServiceSocket), ServiceEndpointError> {
    let mut last_collision = None;
    for _ in 0..16 {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| ServiceEndpointError::Io {
            step: "temporary.name", source: io::Error::other(error.to_string()),
        })?;
        let path = pending_socket_path(target, &random)?;
        if path == target { continue; }
        let result = with_private_umask(|| UnixListener::bind(&path));
        match result {
            Ok(listener) => {
                let metadata = std::fs::symlink_metadata(&path)
                    .map_err(|source| ServiceEndpointError::Io { step: "temporary.identity", source })?;
                let pending = PendingServiceSocket {
                    path, device: metadata.dev(), inode: metadata.ino(), published: false,
                };
                return Ok((listener, pending));
            }
            Err(error) if error.raw_os_error() == Some(libc::EADDRINUSE) => last_collision = Some(error),
            Err(source) => return Err(ServiceEndpointError::Io { step: "bind", source }),
        }
    }
    Err(ServiceEndpointError::Io {
        step: "bind", source: last_collision.unwrap_or_else(|| io::Error::from_raw_os_error(libc::EADDRINUSE)),
    })
}

fn publish_service_socket(source: &Path, target: &Path) -> io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: 两个CString持有NUL结尾路径；同目录独占rename保留已存在的正式端点。
    let result = unsafe {
        libc::renameatx_np(libc::AT_FDCWD, source.as_ptr(), libc::AT_FDCWD, target.as_ptr(), libc::RENAME_EXCL)
    };
    if result != 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}

/// 当前进程 effective uid（bind/封印时刻的属主锚点；生产 = 0）。
fn current_euid() -> u32 {
    // SAFETY: geteuid 只读取当前进程 credential。
    unsafe { libc::geteuid() }
}

/// 校验端点 parent：非 symlink 目录、属主 == 当前 euid、无组/其他写位。
fn validate_service_parent(socket_path: &Path) -> Result<(), PreauthError> {
    let parent = socket_path.parent().ok_or(PreauthError::PathInvalid)?;
    let metadata = std::fs::symlink_metadata(parent).map_err(|_| PreauthError::PathInvalid)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PreauthError::PathInvalid);
    }
    if metadata.uid() != current_euid() || metadata.mode() & 0o022 != 0 {
        return Err(PreauthError::PathInvalid);
    }
    Ok(())
}

/// 仅回收「socket ∧ 属主 == owner uid ∧ 0600」的 stale leaf；其他任何形态保留现场。
fn remove_safe_stale_service_socket(socket_path: &Path, owner_uid: u32) -> Result<(), ServiceEndpointError> {
    remove_safe_stale_service_socket_with_connect(socket_path, owner_uid, |path| {
        std::os::unix::net::UnixStream::connect(path)
    })
}

fn remove_safe_stale_service_socket_with_connect(
    socket_path: &Path,
    owner_uid: u32,
    connect: impl FnOnce(&Path) -> io::Result<std::os::unix::net::UnixStream>,
) -> Result<(), ServiceEndpointError> {
    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PreauthError::PathInvalid.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(PreauthError::PathInvalid.into());
    }
    if metadata.uid() != owner_uid || metadata.mode() & 0o7777 != SERVICE_SOCKET_MODE {
        return Err(PreauthError::EndpointOwnership.into());
    }
    // 只消费原本这一次 connect；其他错误不是 stale，不能据此删除目标。
    match connect(socket_path) {
        Ok(_) => return Err(PreauthError::EndpointInUse.into()),
        Err(error) if error.raw_os_error() == Some(libc::ECONNREFUSED) => {}
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(()),
        Err(source) => return Err(ServiceEndpointError::Io { step: "stale.connect", source }),
    }
    std::fs::remove_file(socket_path).map_err(|_| PreauthError::CleanupRefused)?;
    Ok(())
}

/// bind 后以 `fchownat(uid=owner, gid 不变, NOFOLLOW)` 设置属主；最终identity由bind统一记录。
fn seal_service_socket(socket_path: &Path, owner_uid: u32) -> Result<(), PreauthError> {
    let parent = socket_path.parent().ok_or(PreauthError::PathInvalid)?;
    let leaf = socket_path.file_name().ok_or(PreauthError::PathInvalid)?;
    let leaf = CString::new(leaf.as_bytes()).map_err(|_| PreauthError::PathInvalid)?;
    let parent_fd = File::open(parent).map_err(|_| PreauthError::Transport)?;
    // SAFETY: parent_fd 为本函数持有的目录 fd；leaf 是固定 NUL 结尾 basename；
    // AT_SYMLINK_NOFOLLOW 防止通过替换 symlink 改属；gid 传 -1 表示不变。
    let chown_result = unsafe {
        libc::fchownat(
            parent_fd.as_raw_fd(),
            leaf.as_ptr(),
            owner_uid,
            -1_i32 as libc::gid_t,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    drop(parent_fd);
    if chown_result != 0 {
        return Err(PreauthError::EndpointOwnership);
    }
    Ok(())
}

/// 读取并校验端点 identity：socket ∧ 属主 uid == owner uid ∧ 0600（gid 不钉死——
/// 生产为 root egid 0、测试为进程 egid，属主 uid 与 mode 才是契约字段）。
fn service_endpoint_identity(
    socket_path: &Path,
    owner_uid: u32,
) -> Result<EndpointIdentity, PreauthError> {
    let metadata = std::fs::symlink_metadata(socket_path).map_err(|_| PreauthError::Transport)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(PreauthError::PathInvalid);
    }
    if metadata.uid() != owner_uid {
        return Err(PreauthError::EndpointOwnership);
    }
    let mode = metadata.mode() & 0o7777;
    if mode != SERVICE_SOCKET_MODE {
        return Err(PreauthError::PathInvalid);
    }
    Ok(EndpointIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: RuntimeOwner::new(owner_uid, metadata.gid()),
        mode,
    })
}
