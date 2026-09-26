//! Darwin UDS runtime path 的安全校验和发布屏障。

use std::{
    ffi::CString,
    fs,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use crate::auth::PreauthError;

/// Darwin `sockaddr_un::sun_path` 预留 NUL 后允许的最大路径字节数。
pub const MAX_UNIX_SOCKET_PATH_BYTES: usize = 103;

/// Engine 专用 root socket 的固定文件名。
pub const ENGINE_SOCKET_FILE_NAME: &str = "engine.sock";
const ENGINE_SOCKET_FILE_NAME_C: &[u8] = b"engine.sock\0";

const RUNTIME_DIR_PARENT: &str = "/private/tmp";
const RUNTIME_DIR_CREATE_ATTEMPTS: usize = 16;

/// 持有 runtime directory 期间的稳定错误类别。
///
/// 这些错误只描述本地 bootstrap 文件系统边界；Core/Engine 在各自边界把它们映射为
/// 已批准的稳定产品错误，不能把它们编码成 Common 业务错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeDirError {
    /// 路径不是允许的短绝对 owner-only runtime directory。
    PathInvalid,
    /// 无法原子创建新的 runtime directory。
    Create,
    /// 无法以 `O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC` 打开 directory。
    Open,
    /// 目录的 fd 元数据不再满足目录/owner/0700 契约。
    Metadata,
    /// 目录 uid/gid 与预期 owner 不一致。
    Ownership,
    /// 路径对应的目录已替换或不再与持有的 dirfd 一致。
    PathChanged,
    /// 当前目录身份不足以安全删除它。
    CleanupRefused,
}

impl std::fmt::Display for RuntimeDirError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::PathInvalid => "Darwin runtime directory path is invalid",
            Self::Create => "Darwin runtime directory creation failed",
            Self::Open => "Darwin runtime directory open failed",
            Self::Metadata => "Darwin runtime directory metadata is invalid",
            Self::Ownership => "Darwin runtime directory ownership is invalid",
            Self::PathChanged => "Darwin runtime directory path changed",
            Self::CleanupRefused => "Darwin runtime directory cleanup was refused",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RuntimeDirError {}

/// runtime directory 与 endpoint 都必须匹配的本机 owner。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeOwner {
    uid: u32,
    gid: u32,
}

/// root-seal 后由 `fstat` 读取的 runtime directory 元数据。
///
/// 这是 path 模块内部的 root-seal 测试接缝：生产实现只从 held dirfd 的 `fstat` 生成它，
/// fixture 可以注入读取失败或不一致的事实，验证调用方不会继续发布 socket。它不包含
/// pathname，不能用来选择或重新打开任何文件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RootSealMetadata {
    device: u64,
    inode: u64,
    owner: RuntimeOwner,
    mode: u32,
    is_directory: bool,
}

impl RootSealMetadata {
    /// 用已读取的 metadata 构造 root-seal 测试事实。
    #[must_use]
    const fn new(
        device: u64,
        inode: u64,
        owner: RuntimeOwner,
        mode: u32,
        is_directory: bool,
    ) -> Self {
        Self {
            device,
            inode,
            owner,
            mode,
            is_directory,
        }
    }
}

/// root publisher 对 runtime directory 的最小系统调用接缝。
///
/// 生产实现固定把 directory 封印为 `root:root`、`0711`；该 trait 仅让同 uid fixture
/// 注入等价的 owner，以验证封印失败不会发布 listener。它不改变 ticket、认证 wire 或
/// 任意 pathname 的选择。
trait RootSealOps: Send + Sync {
    /// 对 held runtime dirfd 执行封印所需的 owner 变更。
    ///
    /// # Errors
    ///
    /// 当 `fchown` 失败时返回 [`RuntimeDirError`]；调用方必须停止发布流程。
    fn fchown_runtime_dir(&self, runtime_dir_fd: RawFd) -> Result<(), RuntimeDirError>;

    /// 对 held runtime dirfd 执行 `0711` mode 变更。
    ///
    /// # Errors
    ///
    /// 当 `fchmod` 失败时返回 [`RuntimeDirError`]；调用方必须停止发布流程。
    fn fchmod_runtime_dir(&self, runtime_dir_fd: RawFd) -> Result<(), RuntimeDirError>;

    /// 从 held runtime dirfd 读取 `fchown`/`fchmod` 后的 metadata。
    ///
    /// # Errors
    ///
    /// 当 `fstat` 不可用或读取结果无法满足 root-seal identity 时，返回
    /// [`RuntimeDirError`]；调用方必须停止发布流程。
    fn fstat_runtime_dir(&self, runtime_dir_fd: RawFd)
    -> Result<RootSealMetadata, RuntimeDirError>;

    /// 返回上述两步成功后必须由 `fstat` 观察到的 owner。
    #[must_use]
    fn sealed_owner(&self) -> RuntimeOwner;
}

/// 真实 root Engine 使用的固定 root-seal 实现。
#[derive(Clone, Copy, Debug, Default)]
struct SystemRootSeal;

impl RootSealOps for SystemRootSeal {
    fn fchown_runtime_dir(&self, runtime_dir_fd: RawFd) -> Result<(), RuntimeDirError> {
        // SAFETY: the fd is held by RuntimeDir for the whole bootstrap transaction. The fixed
        // root uid/gid are the only production owner values accepted by this implementation.
        if unsafe { libc::fchown(runtime_dir_fd, 0, 0) } == 0 {
            Ok(())
        } else {
            Err(RuntimeDirError::Ownership)
        }
    }

    fn fchmod_runtime_dir(&self, runtime_dir_fd: RawFd) -> Result<(), RuntimeDirError> {
        // SAFETY: the fd is the held runtime directory fd and the mode is the approved sealed
        // directory mode. This never operates through a pathname.
        if unsafe { libc::fchmod(runtime_dir_fd, 0o711) } == 0 {
            Ok(())
        } else {
            Err(RuntimeDirError::Metadata)
        }
    }

    fn fstat_runtime_dir(
        &self,
        runtime_dir_fd: RawFd,
    ) -> Result<RootSealMetadata, RuntimeDirError> {
        root_seal_metadata_from_fd(runtime_dir_fd)
    }

    fn sealed_owner(&self) -> RuntimeOwner {
        RuntimeOwner::new(0, 0)
    }
}

impl RuntimeOwner {
    /// 构造期望的 uid/gid。
    #[must_use]
    pub const fn new(uid: u32, gid: u32) -> Self {
        Self { uid, gid }
    }

    /// 返回当前用户的 uid/gid。
    #[must_use]
    pub fn current() -> Self {
        // SAFETY: both calls only read the calling process credentials and have no pointer
        // arguments or mutable global state visible to Rust.
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        Self::new(uid, gid)
    }

    /// owner uid。
    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// owner gid。
    #[must_use]
    pub const fn gid(self) -> u32 {
        self.gid
    }
}

/// 由持有 dirfd 记录的 runtime directory 身份。
///
/// 调用方不能自行构造该值；所有可写 child 都必须经 [`RuntimeDir`] 的 dirfd 操作，
/// 而不是在 bootstrap 期间重新按字符串打开目录。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RuntimeDirIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
    runtime_gid: u32,
    mode: u32,
}

/// root-seal 后由 held runtime dirfd 记录的 directory 身份。
///
/// 它与 [`RuntimeDirIdentity`] 分开保存：后者固定 Core 的原 owner 与
/// `pre_seal_runtime_gid`，供 socket `fchownat` 使用；本结构固定 directory 已经是
/// root-owned `0711` 的事实，供 bind 后复验与 cleanup 使用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SealedRuntimeDirIdentity {
    device: u64,
    inode: u64,
    owner: RuntimeOwner,
    mode: u32,
}

/// 固定 `/private/tmp` parent 的 fd identity。
///
/// 它与 runtime dir 的 owner 规则不同：parent 必须是 root:root、严格 mode 01777；只接受
/// 这个已记录 parent 下的直接 child，不能把 `/tmp` alias 或任意绝对路径当成等价路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RuntimeParentIdentity {
    device: u64,
    inode: u64,
    mode: u32,
}

/// 一个已验证、持有 dirfd 的 owner-only runtime directory。
///
/// `parent_fd` 与 `name` 只用于针对记录 identity 的 guarded directory cleanup；ticket
/// 的 `openat`/`fstatat`/`unlinkat` 则始终使用 `fd`。该类型不实现 `Clone`，避免在
/// bootstrap 生命周期中丢失哪一个 fd 是权威目录句柄的事实。
pub struct RuntimeDir {
    path: PathBuf,
    name: CString,
    fd: OwnedFd,
    parent_fd: OwnedFd,
    parent_identity: RuntimeParentIdentity,
    /// seal 前从 held fd 记录的 Core owner 和实际 gid；seal 后仍是 socket 的唯一 owner
    /// 与 gid 来源。
    identity: RuntimeDirIdentity,
    /// 只有 root publisher 成功完成 held-dirfd seal 后才记录；此后所有路径与 cleanup
    /// 校验都必须匹配该 identity。
    sealed_identity: Option<SealedRuntimeDirIdentity>,
}

impl std::fmt::Debug for RuntimeDir {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeDir")
            .field("path", &self.path)
            .field("owner_uid", &self.identity.owner_uid)
            .field("runtime_gid", &self.identity.runtime_gid)
            .field("device", &self.identity.device)
            .field("inode", &self.identity.inode)
            .field("parent_device", &self.parent_identity.device)
            .field("parent_inode", &self.parent_identity.inode)
            .field("sealed_identity", &self.sealed_identity)
            .finish_non_exhaustive()
    }
}

impl RuntimeDir {
    /// 在固定 `/private/tmp` parent 下创建短、随机、0700 的当前 owner runtime directory。
    ///
    /// 该入口只允许当前进程的有效 uid/gid 作为 owner，避免非特权 Core 伪造别人的
    /// directory ownership。每次尝试通过同一个 parent dirfd 的 `mkdirat` 原子创建，
    /// 随后立刻以 `openat(..., O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC)` 持有目录，并只经该
    /// Core-owned dirfd 显式 `fchmod(0700)`；这使创建结果不受另一个单线程 root
    /// publisher 临时 `umask(0177)` 的影响。
    ///
    /// # Errors
    ///
    /// 当 owner 不是当前进程、随机源或目录创建不可用，或新目录无法满足 0700 identity
    /// 契约时返回 [`RuntimeDirError`]。
    pub fn create(owner: RuntimeOwner) -> Result<Self, RuntimeDirError> {
        if owner.uid() != RuntimeOwner::current().uid() {
            return Err(RuntimeDirError::Ownership);
        }

        let parent_path = Path::new(RUNTIME_DIR_PARENT);
        let (parent_fd, parent_identity) = open_fixed_runtime_parent()?;
        for _ in 0..RUNTIME_DIR_CREATE_ATTEMPTS {
            let name = create_runtime_dir_name()?;
            // SAFETY: `parent_fd` is a live directory fd owned by this function and `name` is
            // a NUL-terminated basename produced locally. The requested mode exposes no group
            // or other permissions even before a restrictive process umask is applied.
            let result = unsafe { libc::mkdirat(parent_fd.as_raw_fd(), name.as_ptr(), 0o700) };
            if result == 0 {
                let path = parent_path.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
                return Self::from_created_parent_and_name(
                    path,
                    parent_fd,
                    parent_identity,
                    name,
                    owner.uid(),
                );
            }

            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EEXIST) {
                continue;
            }
            return Err(RuntimeDirError::Create);
        }
        Err(RuntimeDirError::Create)
    }

    /// 以固定 parent 下的 owner uid 0700 dirfd 打开既有 runtime directory。
    ///
    /// Engine 必须在启动后通过这个入口取得且一直持有返回的 dirfd；不得在之后按字符串
    /// 重新打开 directory。
    ///
    /// # Errors
    ///
    /// 当路径不是短绝对目录、出现 symlink、fd 或路径 identity 不匹配时返回
    /// [`RuntimeDirError`]。
    pub fn open_existing_for_uid(
        path: impl Into<PathBuf>,
        owner_uid: u32,
    ) -> Result<Self, RuntimeDirError> {
        let path = path.into();
        let (parent_path, name) = split_runtime_dir_path(&path)?;
        if parent_path != Path::new(RUNTIME_DIR_PARENT) {
            return Err(RuntimeDirError::PathInvalid);
        }
        let (parent_fd, parent_identity) = open_fixed_runtime_parent()?;
        Self::from_open_parent_and_name(path, parent_fd, parent_identity, name, owner_uid)
    }

    /// 返回原始、已验证的 runtime directory path。
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// 返回从 held runtime dirfd 读取的 owner uid 与实际 gid。
    #[must_use]
    pub const fn owner(&self) -> RuntimeOwner {
        RuntimeOwner::new(self.identity.owner_uid, self.identity.runtime_gid)
    }

    /// 返回经 held runtime dirfd 验证的实际 gid。
    ///
    /// macOS/BSD 的 sticky `/private/tmp` parent 可以让新 child 继承 parent gid；0700
    /// directory 仍只对 owner uid 可访问。因此 root Engine 唯一允许的 socket ownership
    /// 变更必须在 sealed held runtime dirfd 上通过
    /// `fchownat(..., AT_SYMLINK_NOFOLLOW)` 使用此处事实 gid，绝不从 argv、环境或 Core
    /// egid 接收。
    #[must_use]
    pub const fn runtime_gid(&self) -> u32 {
        self.identity.runtime_gid
    }

    /// 仅供 root publisher 在 ticket/argv 校验成功后、任何 bind 前封印 held directory。
    ///
    /// 流程严格为：先验证 unsealed Core-owned `0700` identity，再通过 held dirfd
    /// `fchown`、`fchmod(0711)`，随后 `fstat` 与 held parent dirfd 的
    /// `fstatat(AT_SYMLINK_NOFOLLOW)` 共同确认同一 root-owned leaf。任何失败都不发布
    /// socket，且本函数不进行 pathname cleanup。
    ///
    /// # Errors
    ///
    /// 当 held directory、固定 parent 或 seal 系统调用不满足 root-seal identity 时返回
    /// [`RuntimeDirError`]，并保留现场供调用方作受控诊断。
    pub fn seal_for_root_publisher(&mut self) -> Result<(), RuntimeDirError> {
        self.seal_for_root_publisher_with(&SystemRootSeal)
    }

    /// 使用 fixture 注入的 held-fd root-seal 操作完成同一 root-seal transition。
    ///
    /// 生产 Engine 必须调用 [`Self::seal_for_root_publisher`]，不可使用此 test seam
    /// 选择非 root owner。该入口只为不需要真实 root 的契约测试而存在。
    ///
    /// # Errors
    ///
    /// 当 held directory、固定 parent 或注入的 seal 操作不满足 root-seal identity 时
    /// 返回 [`RuntimeDirError`]，并保留现场供调用方作受控诊断。
    fn seal_for_root_publisher_with(
        &mut self,
        ops: &dyn RootSealOps,
    ) -> Result<(), RuntimeDirError> {
        if self.sealed_identity.is_some() {
            return Err(RuntimeDirError::Metadata);
        }
        self.revalidate_path()?;
        ops.fchown_runtime_dir(self.fd.as_raw_fd())?;
        ops.fchmod_runtime_dir(self.fd.as_raw_fd())?;

        let sealed_owner = ops.sealed_owner();
        let sealed = sealed_runtime_dir_identity_from_metadata(
            ops.fstat_runtime_dir(self.fd.as_raw_fd())?,
            sealed_owner,
        )?;
        if sealed.device != self.identity.device || sealed.inode != self.identity.inode {
            return Err(RuntimeDirError::Metadata);
        }

        self.revalidate_parent()?;
        let child_identity = sealed_runtime_dir_identity_from_stat_at(
            self.parent_fd.as_raw_fd(),
            &self.name,
            sealed_owner,
        )
        .map_err(|_| RuntimeDirError::PathChanged)?;
        if child_identity != sealed {
            return Err(RuntimeDirError::PathChanged);
        }
        let path_identity = sealed_runtime_dir_identity_from_path(&self.path, sealed_owner)
            .map_err(|_| RuntimeDirError::PathChanged)?;
        if path_identity != sealed {
            return Err(RuntimeDirError::PathChanged);
        }

        self.sealed_identity = Some(sealed);
        Ok(())
    }

    /// 返回 held directory 是否已通过 root-seal identity 校验。
    ///
    /// 后续 root socket publisher 只能接收已封印的 [`RuntimeDir`]；该状态只会在
    /// [`Self::seal_for_root_publisher`] 全部 held-fd/parent-fd 检查成功后出现。
    #[must_use]
    pub const fn is_root_sealed(&self) -> bool {
        self.sealed_identity.is_some()
    }

    /// 返回固定 Engine root socket 的短路径。
    ///
    /// # Errors
    ///
    /// 当组合后的 path 超出 `sockaddr_un::sun_path` 或不再是绝对路径时返回
    /// [`RuntimeDirError::PathInvalid`]。
    pub fn engine_socket_path(&self) -> Result<SocketPath, RuntimeDirError> {
        SocketPath::new(self.path.join(ENGINE_SOCKET_FILE_NAME))
            .map_err(|_| RuntimeDirError::PathInvalid)
    }

    /// 重新验证持有 fd 与 pathname 的 directory identity。
    ///
    /// Root publisher 的 pathname `bind` 前后必须调用该方法；ticket 路径操作则只可使用
    /// `dirfd`，不需要也不得用 pathname 打开 child。
    ///
    /// # Errors
    ///
    /// 当 fd 元数据改变、pathname 成为 symlink/其他目录或 `(dev, ino)` 与原记录不一致时
    /// 返回 [`RuntimeDirError`]。
    pub fn revalidate_path(&self) -> Result<(), RuntimeDirError> {
        self.revalidate_parent()?;
        match self.sealed_identity {
            Some(sealed) => self.revalidate_sealed_path(sealed),
            None => self.revalidate_unsealed_path(),
        }
    }

    fn revalidate_parent(&self) -> Result<(), RuntimeDirError> {
        let parent_fd_identity = runtime_parent_identity_from_fd(self.parent_fd.as_raw_fd())
            .map_err(|_| RuntimeDirError::Metadata)?;
        if parent_fd_identity != self.parent_identity {
            return Err(RuntimeDirError::Metadata);
        }
        let parent_path_identity = runtime_parent_identity_from_path(Path::new(RUNTIME_DIR_PARENT))
            .map_err(|_| RuntimeDirError::PathChanged)?;
        if parent_path_identity != self.parent_identity {
            return Err(RuntimeDirError::PathChanged);
        }
        Ok(())
    }

    fn revalidate_unsealed_path(&self) -> Result<(), RuntimeDirError> {
        let fd_identity =
            runtime_dir_identity_from_fd(self.fd.as_raw_fd(), self.identity.owner_uid)
                .map_err(|_| RuntimeDirError::Metadata)?;
        if fd_identity != self.identity {
            return Err(RuntimeDirError::Metadata);
        }
        let path_identity = runtime_dir_identity_from_path(&self.path, self.identity.owner_uid)
            .map_err(|_| RuntimeDirError::PathChanged)?;
        if path_identity != self.identity {
            return Err(RuntimeDirError::PathChanged);
        }

        let parent_identity = runtime_dir_identity_from_stat_at(
            self.parent_fd.as_raw_fd(),
            &self.name,
            self.identity.owner_uid,
        )
        .map_err(|_| RuntimeDirError::PathChanged)?;
        if parent_identity != self.identity {
            return Err(RuntimeDirError::PathChanged);
        }
        Ok(())
    }

    fn revalidate_sealed_path(
        &self,
        sealed: SealedRuntimeDirIdentity,
    ) -> Result<(), RuntimeDirError> {
        let fd_identity = sealed_runtime_dir_identity_from_fd(self.fd.as_raw_fd(), sealed.owner)
            .map_err(|_| RuntimeDirError::Metadata)?;
        if fd_identity != sealed {
            return Err(RuntimeDirError::Metadata);
        }
        let path_identity = sealed_runtime_dir_identity_from_path(&self.path, sealed.owner)
            .map_err(|_| RuntimeDirError::PathChanged)?;
        if path_identity != sealed {
            return Err(RuntimeDirError::PathChanged);
        }
        let parent_identity = sealed_runtime_dir_identity_from_stat_at(
            self.parent_fd.as_raw_fd(),
            &self.name,
            sealed.owner,
        )
        .map_err(|_| RuntimeDirError::PathChanged)?;
        if parent_identity != sealed {
            return Err(RuntimeDirError::PathChanged);
        }
        Ok(())
    }

    /// 仅在 path 和 parent dirfd 下的 directory identity 都仍匹配时删除一个空目录。
    ///
    /// E1 只用它清理自身创建的测试 runtime；产品生命周期只能在 socket/ticket 都已经
    /// guarded-cleanup 后调用。目录非空、被替换或 parent path 不一致时一律保留现场。
    ///
    /// # Errors
    ///
    /// 当目录不再是本对象记录的 identity，或 `unlinkat(AT_REMOVEDIR)` 失败时返回
    /// [`RuntimeDirError::CleanupRefused`]。
    pub fn cleanup_empty(self) -> Result<(), RuntimeDirError> {
        self.revalidate_path()
            .map_err(|_| RuntimeDirError::CleanupRefused)?;
        // SAFETY: `parent_fd` is held by `self`; `name` is the original NUL-terminated basename.
        // The preceding fd/path/fstatat checks make this a guarded removal. They do not promise
        // atomic exclusion of a same-owner replacement between the check and `unlinkat`.
        let result = unsafe {
            libc::unlinkat(
                self.parent_fd.as_raw_fd(),
                self.name.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(RuntimeDirError::CleanupRefused)
        }
    }

    pub(crate) fn dirfd(&self) -> std::os::fd::RawFd {
        self.fd.as_raw_fd()
    }

    /// 从 held runtime dirfd 对固定 Engine socket 读取 leaf identity。
    ///
    /// 这是 root publisher bind 后与 `fchownat` 前后的唯一 socket metadata 读取路径；
    /// `AT_SYMLINK_NOFOLLOW` 保证 leaf symlink 不会被当作 socket 使用。
    pub(crate) fn engine_socket_identity(&self) -> Result<EndpointIdentity, PreauthError> {
        if self.sealed_identity.is_none() {
            return Err(PreauthError::PathInvalid);
        }
        engine_socket_identity_from_stat_at(self.fd.as_raw_fd())
    }

    /// 仅在 sealed directory 中判断固定 socket basename 仍不存在。
    pub(crate) fn ensure_engine_socket_absent(&self) -> Result<(), PreauthError> {
        if self.sealed_identity.is_none() {
            return Err(PreauthError::PathInvalid);
        }
        // SAFETY: the held dirfd and fixed NUL-terminated basename make this a directory-relative
        // lstat. It is used only before the one permitted pathname bind.
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        let result = unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                ENGINE_SOCKET_FILE_NAME_C.as_ptr().cast(),
                &raw mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            return Err(PreauthError::EndpointExists);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(())
        } else {
            Err(PreauthError::PathInvalid)
        }
    }

    /// 仅通过 held runtime dirfd 改写固定 Engine socket 的 owner/gid。
    ///
    /// 调用方必须在此前后分别检查该 socket 的完整 identity；该操作绝不接受 pathname，
    /// 也不会跟随 leaf symlink。
    pub(crate) fn fchown_engine_socket_nofollow(
        &self,
        owner: RuntimeOwner,
    ) -> Result<(), PreauthError> {
        if self.sealed_identity.is_none() {
            return Err(PreauthError::PathInvalid);
        }
        // SAFETY: `fd` is held by `self`; the basename is fixed and NUL-terminated; Darwin
        // `fchownat` receives AT_SYMLINK_NOFOLLOW so a leaf symlink is never followed.
        if unsafe {
            libc::fchownat(
                self.fd.as_raw_fd(),
                ENGINE_SOCKET_FILE_NAME_C.as_ptr().cast(),
                owner.uid(),
                owner.gid(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            Ok(())
        } else {
            Err(PreauthError::EndpointOwnership)
        }
    }

    /// 仅在 held runtime dirfd 中的固定 `engine.sock` 仍完全等于 `expected` 时删除它。
    ///
    /// 这是 root publisher 的唯一 socket cleanup 路径。它先重新验证 runtime directory，
    /// 再以同一个 held dirfd 的 `fstatat(AT_SYMLINK_NOFOLLOW)` 比较 type/owner/mode/
    /// `(dev, ino)`，最后 `unlinkat` 固定 basename；绝不以 socket pathname 删除对象。
    /// 检查与删除之间不承诺排除同 owner 的并发替换，因此任一不匹配或系统错误均保留现场。
    pub(crate) fn guarded_cleanup_engine_socket(
        &self,
        expected: EndpointIdentity,
    ) -> Result<(), PreauthError> {
        if self.sealed_identity.is_none() {
            return Err(PreauthError::CleanupRefused);
        }
        self.revalidate_path()
            .map_err(|_| PreauthError::CleanupRefused)?;
        let current = engine_socket_identity_from_stat_at(self.fd.as_raw_fd())?;
        if current != expected {
            return Err(PreauthError::CleanupRefused);
        }
        // SAFETY: `fd` is held by `self`; the name is the fixed NUL-terminated Engine basename.
        // The immediately preceding fstatat compared the complete expected identity. This is a
        // guarded deletion, not a claim that a check/unlink pair is atomic against replacement.
        if unsafe {
            libc::unlinkat(
                self.fd.as_raw_fd(),
                ENGINE_SOCKET_FILE_NAME_C.as_ptr().cast(),
                0,
            )
        } == 0
        {
            Ok(())
        } else {
            Err(PreauthError::CleanupRefused)
        }
    }

    /// root publisher 的完整 guarded cleanup：先清理固定 socket，再只在 held parent
    /// dirfd 中 leaf 仍匹配 sealed identity 时删除空 runtime directory。
    ///
    /// 任何 socket 或 directory 不匹配都保留现场；本方法不声称 check/unlink 对 root 或
    /// 内核级替换具有原子排他性。
    pub(crate) fn cleanup_sealed_engine_socket_and_runtime(
        self,
        expected: EndpointIdentity,
    ) -> Result<(), PreauthError> {
        self.guarded_cleanup_engine_socket(expected)?;
        self.cleanup_empty()
            .map_err(|_| PreauthError::CleanupRefused)
    }

    fn from_open_parent_and_name(
        path: PathBuf,
        parent_fd: OwnedFd,
        parent_identity: RuntimeParentIdentity,
        name: CString,
        owner_uid: u32,
    ) -> Result<Self, RuntimeDirError> {
        let fd = open_directory_at(parent_fd.as_raw_fd(), &name)?;
        Self::from_held_parent_and_name(path, parent_fd, parent_identity, name, owner_uid, fd)
    }

    /// 把刚由 Core 在 held parent dirfd 中创建的 child 打开并固定为 0700。
    ///
    /// 此步骤只适用于 `RuntimeDir::create`：Engine 的 `open_existing_for_uid` 必须保持
    /// 只读验证，绝不能把已有 directory 的 mode 改写为自身预期。
    fn from_created_parent_and_name(
        path: PathBuf,
        parent_fd: OwnedFd,
        parent_identity: RuntimeParentIdentity,
        name: CString,
        owner_uid: u32,
    ) -> Result<Self, RuntimeDirError> {
        let fd = open_directory_at(parent_fd.as_raw_fd(), &name)?;
        // SAFETY: `fd` is the newly opened, held directory fd for the just-created direct
        // child. This never follows or selects a pathname and fixes the approved Core mode
        // before any identity metadata is accepted.
        if unsafe { libc::fchmod(fd.as_raw_fd(), 0o700) } != 0 {
            return Err(RuntimeDirError::Metadata);
        }
        Self::from_held_parent_and_name(path, parent_fd, parent_identity, name, owner_uid, fd)
    }

    fn from_held_parent_and_name(
        path: PathBuf,
        parent_fd: OwnedFd,
        parent_identity: RuntimeParentIdentity,
        name: CString,
        owner_uid: u32,
        fd: OwnedFd,
    ) -> Result<Self, RuntimeDirError> {
        let identity = runtime_dir_identity_from_fd(fd.as_raw_fd(), owner_uid)?;
        let path_identity = runtime_dir_identity_from_path(&path, owner_uid)?;
        let child_path_identity =
            runtime_dir_identity_from_stat_at(parent_fd.as_raw_fd(), &name, owner_uid)?;
        if identity != path_identity || identity != child_path_identity {
            return Err(RuntimeDirError::PathChanged);
        }
        Ok(Self {
            path,
            name,
            fd,
            parent_fd,
            parent_identity,
            identity,
            sealed_identity: None,
        })
    }
}

/// 已通过长度和绝对路径检查的 Unix socket path。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketPath(PathBuf);

impl SocketPath {
    /// 验证可用于 `UnixListener::bind` 的短绝对 socket path。
    ///
    /// # Errors
    ///
    /// 当路径不是短绝对 Unix socket path、没有文件名或包含 NUL 时返回
    /// [`PreauthError::PathInvalid`]。
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, PreauthError> {
        let path = path.into();
        let bytes = path.as_os_str().as_bytes();
        if !path.is_absolute()
            || path.file_name().is_none()
            || bytes.contains(&0)
            || bytes.len() > MAX_UNIX_SOCKET_PATH_BYTES
        {
            return Err(PreauthError::PathInvalid);
        }
        Ok(Self(path))
    }

    /// 返回已验证的 filesystem path。
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// 验证 parent 是目标 owner 的非 symlink 0700 runtime directory。
    ///
    /// # Errors
    ///
    /// 当 parent 缺失、不安全或不属于 `owner` 时返回相应的 [`PreauthError`]。
    pub fn validate_runtime_dir(&self, owner: RuntimeOwner) -> Result<(), PreauthError> {
        let parent = self.0.parent().ok_or(PreauthError::PathInvalid)?;
        validate_runtime_dir(parent, owner)
    }
}

/// listener bind 后记录的 endpoint 事实，用于 guarded cleanup。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) owner: RuntimeOwner,
    pub(crate) mode: u32,
}

/// 验证 runtime directory 严格为 owner 的 0700 非 symlink directory。
///
/// # Errors
///
/// 当路径缺失、不安全、不是 0700 directory 或 owner 不匹配时返回相应的
/// [`PreauthError`]。
pub fn validate_runtime_dir(path: &Path, owner: RuntimeOwner) -> Result<(), PreauthError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| PreauthError::PathInvalid)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PreauthError::PathInvalid);
    }
    if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
        return Err(PreauthError::EndpointOwnership);
    }
    if metadata.mode() & 0o7777 != 0o700 {
        return Err(PreauthError::PathInvalid);
    }
    Ok(())
}

/// 把刚 bind 的 UDS 发布为 owner-only socket，并立刻复核 type/owner/mode/inode。
///
/// 这只是非特权 Core fixture 的发布屏障；root Engine 的 socket ownership 只能在 sealed
/// held runtime dirfd 上以 `fchownat(..., AT_SYMLINK_NOFOLLOW)` 完成，属于后续
/// `MAC-ELEVATE-02`。本函数绝不尝试变更 socket owner 或创建 ticket。
///
/// # Errors
///
/// 当 socket 权限无法设为 0600，或复核发现 type/owner/mode 不安全时返回相应的
/// [`PreauthError`]。
pub fn publish_listener_barrier(
    socket_path: &SocketPath,
    owner: RuntimeOwner,
) -> Result<EndpointIdentity, PreauthError> {
    fs::set_permissions(socket_path.as_path(), fs::Permissions::from_mode(0o600))
        .map_err(|_| PreauthError::Transport)?;
    endpoint_identity(socket_path, owner)
}

/// 仅在 endpoint 仍完全等于 bind 后记录事实时清理它。
///
/// # Errors
///
/// 当 endpoint 缺失、被替换、不安全或无法 unlink 时返回
/// [`PreauthError::CleanupRefused`]。
pub fn guarded_cleanup(
    socket_path: &SocketPath,
    expected: EndpointIdentity,
) -> Result<(), PreauthError> {
    let current =
        endpoint_identity(socket_path, expected.owner).map_err(|_| PreauthError::CleanupRefused)?;
    if current != expected {
        return Err(PreauthError::CleanupRefused);
    }
    fs::remove_file(socket_path.as_path()).map_err(|_| PreauthError::CleanupRefused)
}

pub(crate) fn endpoint_identity(
    socket_path: &SocketPath,
    owner: RuntimeOwner,
) -> Result<EndpointIdentity, PreauthError> {
    let metadata =
        fs::symlink_metadata(socket_path.as_path()).map_err(|_| PreauthError::Transport)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(PreauthError::PathInvalid);
    }
    if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
        return Err(PreauthError::EndpointOwnership);
    }
    let mode = metadata.mode() & 0o7777;
    if mode != 0o600 {
        return Err(PreauthError::PathInvalid);
    }
    Ok(EndpointIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner,
        mode,
    })
}

fn engine_socket_identity_from_stat_at(
    runtime_dir_fd: std::os::fd::RawFd,
) -> Result<EndpointIdentity, PreauthError> {
    // SAFETY: zeroed is valid initialization for C `stat`, which fstatat fully writes before use.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: `runtime_dir_fd` remains held by RuntimeDir; the fixed basename is NUL-terminated;
    // AT_SYMLINK_NOFOLLOW makes this a relative lstat rather than a symlink traversal.
    if unsafe {
        libc::fstatat(
            runtime_dir_fd,
            ENGINE_SOCKET_FILE_NAME_C.as_ptr().cast(),
            &raw mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(PreauthError::CleanupRefused);
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Err(PreauthError::CleanupRefused);
    }
    let mode = u32::from(stat.st_mode) & 0o7777;
    Ok(EndpointIdentity {
        device: u64::try_from(stat.st_dev).map_err(|_| PreauthError::CleanupRefused)?,
        inode: stat.st_ino,
        owner: RuntimeOwner::new(stat.st_uid, stat.st_gid),
        mode,
    })
}

fn create_runtime_dir_name() -> Result<CString, RuntimeDirError> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|_| RuntimeDirError::Create)?;
    let mut suffix = String::with_capacity(random.len() * 2);
    for byte in random {
        use std::fmt::Write as _;
        let _ = write!(&mut suffix, "{byte:02x}");
    }
    CString::new(format!("exv-vpn-{:x}-{suffix}", std::process::id()))
        .map_err(|_| RuntimeDirError::Create)
}

fn split_runtime_dir_path(path: &Path) -> Result<(PathBuf, CString), RuntimeDirError> {
    if !path.is_absolute()
        || path.as_os_str().as_bytes().contains(&0)
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
    {
        return Err(RuntimeDirError::PathInvalid);
    }
    let parent = path.parent().ok_or(RuntimeDirError::PathInvalid)?;
    let name = path.file_name().ok_or(RuntimeDirError::PathInvalid)?;
    if parent != Path::new(RUNTIME_DIR_PARENT) || name.as_bytes().is_empty() {
        return Err(RuntimeDirError::PathInvalid);
    }
    let socket_path = SocketPath::new(path.join(ENGINE_SOCKET_FILE_NAME))
        .map_err(|_| RuntimeDirError::PathInvalid)?;
    if socket_path.as_path().parent() != Some(path) {
        return Err(RuntimeDirError::PathInvalid);
    }
    CString::new(name.as_bytes())
        .map_err(|_| RuntimeDirError::PathInvalid)
        .map(|name| (parent.to_path_buf(), name))
}

fn open_fixed_runtime_parent() -> Result<(OwnedFd, RuntimeParentIdentity), RuntimeDirError> {
    let parent_fd = open_directory(Path::new(RUNTIME_DIR_PARENT))?;
    let identity = runtime_parent_identity_from_fd(parent_fd.as_raw_fd())?;
    let path_identity = runtime_parent_identity_from_path(Path::new(RUNTIME_DIR_PARENT))?;
    if identity != path_identity {
        return Err(RuntimeDirError::PathChanged);
    }
    Ok((parent_fd, identity))
}

fn open_directory(path: &Path) -> Result<OwnedFd, RuntimeDirError> {
    let path =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| RuntimeDirError::PathInvalid)?;
    // SAFETY: `path` is NUL-terminated and remains alive for the call. The flags request a
    // directory fd without following a final-component symlink or inheriting it across exec.
    let raw_fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    owned_fd_from_open(raw_fd)
}

fn open_directory_at(
    parent_fd: std::os::fd::RawFd,
    name: &CString,
) -> Result<OwnedFd, RuntimeDirError> {
    // SAFETY: `parent_fd` is owned by the caller, and `name` is an original NUL-terminated
    // basename. `O_NOFOLLOW` prevents the final component from being a symlink.
    let raw_fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    owned_fd_from_open(raw_fd)
}

fn owned_fd_from_open(raw_fd: libc::c_int) -> Result<OwnedFd, RuntimeDirError> {
    if raw_fd < 0 {
        return Err(RuntimeDirError::Open);
    }
    // SAFETY: a non-negative fd returned by `open`/`openat` is newly owned by this call and is
    // transferred exactly once to `OwnedFd`.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

fn runtime_dir_identity_from_fd(
    fd: std::os::fd::RawFd,
    owner_uid: u32,
) -> Result<RuntimeDirIdentity, RuntimeDirError> {
    // SAFETY: `zeroed` is valid initialization for C `stat`, which is immediately populated by
    // `fstat` before any field is read.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: `fd` is borrowed for the duration of this call and `stat` is a valid writable C
    // `stat` buffer.
    if unsafe { libc::fstat(fd, &raw mut stat) } != 0 {
        return Err(RuntimeDirError::Metadata);
    }
    runtime_dir_identity_from_stat(&stat, owner_uid)
}

fn root_seal_metadata_from_fd(fd: RawFd) -> Result<RootSealMetadata, RuntimeDirError> {
    // SAFETY: `zeroed` is valid initialization for C `stat`, which `fstat` fully writes before
    // any field is read.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: `fd` is borrowed for the duration of this call and `stat` is valid writable C
    // storage.
    if unsafe { libc::fstat(fd, &raw mut stat) } != 0 {
        return Err(RuntimeDirError::Metadata);
    }
    root_seal_metadata_from_stat(&stat)
}

fn root_seal_metadata_from_stat(stat: &libc::stat) -> Result<RootSealMetadata, RuntimeDirError> {
    let device = u64::try_from(stat.st_dev).map_err(|_| RuntimeDirError::Metadata)?;
    let inode = stat.st_ino;
    let mode = u32::from(stat.st_mode);
    Ok(RootSealMetadata::new(
        device,
        inode,
        RuntimeOwner::new(stat.st_uid, stat.st_gid),
        mode & 0o7777,
        stat.st_mode & libc::S_IFMT == libc::S_IFDIR,
    ))
}

fn sealed_runtime_dir_identity_from_metadata(
    metadata: RootSealMetadata,
    expected_owner: RuntimeOwner,
) -> Result<SealedRuntimeDirIdentity, RuntimeDirError> {
    if !metadata.is_directory {
        return Err(RuntimeDirError::Metadata);
    }
    if metadata.owner != expected_owner {
        return Err(RuntimeDirError::Ownership);
    }
    if metadata.mode != 0o711 {
        return Err(RuntimeDirError::Metadata);
    }
    Ok(SealedRuntimeDirIdentity {
        device: metadata.device,
        inode: metadata.inode,
        owner: metadata.owner,
        mode: metadata.mode,
    })
}

fn sealed_runtime_dir_identity_from_fd(
    fd: RawFd,
    expected_owner: RuntimeOwner,
) -> Result<SealedRuntimeDirIdentity, RuntimeDirError> {
    sealed_runtime_dir_identity_from_metadata(root_seal_metadata_from_fd(fd)?, expected_owner)
}

fn sealed_runtime_dir_identity_from_stat_at(
    parent_fd: RawFd,
    name: &CString,
    expected_owner: RuntimeOwner,
) -> Result<SealedRuntimeDirIdentity, RuntimeDirError> {
    // SAFETY: `zeroed` is valid initialization for C `stat`, which `fstatat` fully writes before
    // any field is read.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: `parent_fd` is held by the caller, `name` is the original NUL-terminated basename,
    // and `stat` is valid writable storage. `AT_SYMLINK_NOFOLLOW` prevents leaf traversal.
    if unsafe {
        libc::fstatat(
            parent_fd,
            name.as_ptr(),
            &raw mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(RuntimeDirError::PathChanged);
    }
    sealed_runtime_dir_identity_from_metadata(root_seal_metadata_from_stat(&stat)?, expected_owner)
}

fn sealed_runtime_dir_identity_from_path(
    path: &Path,
    expected_owner: RuntimeOwner,
) -> Result<SealedRuntimeDirIdentity, RuntimeDirError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeDirError::PathChanged)?;
    let stat = RootSealMetadata::new(
        metadata.dev(),
        metadata.ino(),
        RuntimeOwner::new(metadata.uid(), metadata.gid()),
        metadata.mode() & 0o7777,
        !metadata.file_type().is_symlink() && metadata.is_dir(),
    );
    sealed_runtime_dir_identity_from_metadata(stat, expected_owner)
}

fn runtime_parent_identity_from_fd(
    fd: std::os::fd::RawFd,
) -> Result<RuntimeParentIdentity, RuntimeDirError> {
    // SAFETY: `zeroed` is valid initialization for C `stat`, which is immediately populated by
    // `fstat` before any field is read.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: `fd` is borrowed for the duration of this call and `stat` is valid writable C
    // `stat` storage.
    if unsafe { libc::fstat(fd, &raw mut stat) } != 0 {
        return Err(RuntimeDirError::Metadata);
    }
    runtime_parent_identity_from_stat(&stat)
}

fn runtime_parent_identity_from_path(
    path: &Path,
) -> Result<RuntimeParentIdentity, RuntimeDirError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeDirError::PathChanged)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RuntimeDirError::PathInvalid);
    }
    if metadata.uid() != 0 || metadata.gid() != 0 {
        return Err(RuntimeDirError::Ownership);
    }
    let mode = metadata.mode() & 0o7777;
    if mode != 0o1777 {
        return Err(RuntimeDirError::Metadata);
    }
    Ok(RuntimeParentIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode,
    })
}

fn runtime_dir_identity_from_stat_at(
    parent_fd: std::os::fd::RawFd,
    name: &CString,
    owner_uid: u32,
) -> Result<RuntimeDirIdentity, RuntimeDirError> {
    // SAFETY: `zeroed` is valid initialization for C `stat`, which is immediately populated by
    // `fstatat` before any field is read.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: `parent_fd` is held by the caller, `name` is a NUL-terminated basename, and `stat`
    // is valid writable storage. `AT_SYMLINK_NOFOLLOW` makes this a relative `lstat`.
    if unsafe {
        libc::fstatat(
            parent_fd,
            name.as_ptr(),
            &raw mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(RuntimeDirError::PathChanged);
    }
    runtime_dir_identity_from_stat(&stat, owner_uid)
}

fn runtime_dir_identity_from_path(
    path: &Path,
    owner_uid: u32,
) -> Result<RuntimeDirIdentity, RuntimeDirError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeDirError::PathChanged)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RuntimeDirError::PathInvalid);
    }
    if metadata.uid() != owner_uid {
        return Err(RuntimeDirError::Ownership);
    }
    let mode = metadata.mode() & 0o7777;
    if mode != 0o700 {
        return Err(RuntimeDirError::Metadata);
    }
    Ok(RuntimeDirIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner_uid,
        runtime_gid: metadata.gid(),
        mode,
    })
}

fn runtime_dir_identity_from_stat(
    stat: &libc::stat,
    owner_uid: u32,
) -> Result<RuntimeDirIdentity, RuntimeDirError> {
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(RuntimeDirError::Metadata);
    }
    if stat.st_uid != owner_uid {
        return Err(RuntimeDirError::Ownership);
    }
    let mode = u32::from(stat.st_mode) & 0o7777;
    if mode != 0o700 {
        return Err(RuntimeDirError::Metadata);
    }
    Ok(RuntimeDirIdentity {
        device: u64::try_from(stat.st_dev).map_err(|_| RuntimeDirError::Metadata)?,
        inode: stat.st_ino,
        owner_uid,
        runtime_gid: stat.st_gid,
        mode,
    })
}

fn runtime_parent_identity_from_stat(
    stat: &libc::stat,
) -> Result<RuntimeParentIdentity, RuntimeDirError> {
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(RuntimeDirError::Metadata);
    }
    if stat.st_uid != 0 || stat.st_gid != 0 {
        return Err(RuntimeDirError::Ownership);
    }
    let mode = u32::from(stat.st_mode) & 0o7777;
    if mode != 0o1777 {
        return Err(RuntimeDirError::Metadata);
    }
    Ok(RuntimeParentIdentity {
        device: u64::try_from(stat.st_dev).map_err(|_| RuntimeDirError::Metadata)?,
        inode: stat.st_ino,
        mode,
    })
}

