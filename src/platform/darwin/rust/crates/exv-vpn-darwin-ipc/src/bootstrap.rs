//! 无签名 Core→root Engine bootstrap 的固定材料和无业务 pipe record。
//!
//! 本模块只处理 owner-only runtime directory 中的一次性 ticket，以及 Authorization
//! communications pipe 的固定 20-byte record。它不定义 JSON、私有业务 wire、自由 argv
//! 或任何网络/packet 能力。

use std::{
    fmt,
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

use zeroize::{Zeroize, Zeroizing};

use crate::{
    auth::{AUTH_KEY_LEN, AuthKey},
    path::{RuntimeDir, RuntimeDirError},
};

const ENGINE_TICKET_FILE_NAME: &[u8] = b"engine.ticket\0";
const ENGINE_TICKET_VERSION: u32 = 1;
/// `version(u32) || owner_uid(u32) || core_pid(u32) || auth_key(32)`。
pub const ENGINE_TICKET_V1_LEN: usize = 4 + 4 + 4 + AUTH_KEY_LEN;

const BOOTSTRAP_MAGIC: [u8; 4] = *b"EXVB";
const BOOTSTRAP_VERSION: u8 = 1;
/// `magic(4) || version(1) || tag(1) || reserved(u16) || pid(u32) || code(u32) || reserved(u32)`。
pub const ENGINE_BOOTSTRAP_V1_LEN: usize = 20;

/// ticket 文件操作的稳定本地错误类别。
///
/// Core/Engine 各自把这些类别映射为其批准的 `Ticket*` 错误；这里绝不产生 Common
/// `VpnError`，也不会把 secret 放入错误信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketError {
    /// runtime directory 无法保持已验证 identity。
    RuntimeDir(RuntimeDirError),
    /// `engine.ticket` 已存在，不能覆盖或先删除。
    Exists,
    /// 无法以固定 `openat` flags 创建 ticket。
    Create,
    /// 无法设置 0600、写入或关闭 ticket。
    Write,
    /// ticket 的 type/owner/mode/size 不满足 V1。
    Metadata,
    /// 无法从 held dirfd 打开 ticket。
    Open,
    /// 固定长度字段或版本非法。
    Malformed,
    /// fstatat identity 不再匹配，或 guarded unlink 失败。
    CleanupRefused,
}

impl fmt::Display for TicketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::RuntimeDir(_) => "Darwin runtime directory is invalid for bootstrap ticket",
            Self::Exists => "Darwin bootstrap ticket already exists",
            Self::Create => "Darwin bootstrap ticket creation failed",
            Self::Write => "Darwin bootstrap ticket write failed",
            Self::Metadata => "Darwin bootstrap ticket metadata is invalid",
            Self::Open => "Darwin bootstrap ticket open failed",
            Self::Malformed => "Darwin bootstrap ticket is malformed",
            Self::CleanupRefused => "Darwin bootstrap ticket cleanup was refused",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TicketError {}

/// 已创建的 ticket inode 事实；Core 在授权未开始或失败时只能以此执行 guarded cleanup。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TicketIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
    mode: u32,
    size: u64,
}

/// 只在 ticket 内存中存在的 V1 内容。
///
/// `Debug`、错误和日志都不会暴露 `auth_key`。值在 drop 时清零；序列化缓冲也使用
/// [`Zeroizing`]，避免认证材料在常规错误路径遗留。
pub struct EngineTicketV1 {
    owner_uid: u32,
    core_pid: u32,
    auth_key: [u8; AUTH_KEY_LEN],
}

impl fmt::Debug for EngineTicketV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EngineTicketV1")
            .field("owner_uid", &self.owner_uid)
            .field("core_pid", &self.core_pid)
            .field("auth_key", &"[REDACTED]")
            .finish()
    }
}

impl EngineTicketV1 {
    /// 构造固定 V1 ticket 内容。
    ///
    /// # Errors
    ///
    /// Core pid 不在 Darwin 有效正 pid 范围时返回 [`TicketError::Malformed`]。
    pub fn new(
        owner_uid: u32,
        core_pid: u32,
        mut auth_key: [u8; AUTH_KEY_LEN],
    ) -> Result<Self, TicketError> {
        if !valid_pid(core_pid) {
            auth_key.zeroize();
            return Err(TicketError::Malformed);
        }
        Ok(Self {
            owner_uid,
            core_pid,
            auth_key,
        })
    }

    /// ticket 中的 Core owner uid。
    #[must_use]
    pub const fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    /// ticket 中的 Core pid。
    #[must_use]
    pub const fn core_pid(&self) -> u32 {
        self.core_pid
    }

    /// 把已消费 ticket 的 auth key 移交给现有 `DarwinAuthV1` listener。
    #[must_use]
    pub fn into_auth_key(mut self) -> AuthKey {
        AuthKey::from_bytes(std::mem::take(&mut self.auth_key))
    }

    fn into_encoded(self) -> Zeroizing<[u8; ENGINE_TICKET_V1_LEN]> {
        let mut bytes = Zeroizing::new([0_u8; ENGINE_TICKET_V1_LEN]);
        bytes[0..4].copy_from_slice(&ENGINE_TICKET_VERSION.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.owner_uid.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.core_pid.to_be_bytes());
        bytes[12..].copy_from_slice(&self.auth_key);
        bytes
    }

    fn decode(bytes: &[u8; ENGINE_TICKET_V1_LEN]) -> Result<Self, TicketError> {
        let version = u32::from_be_bytes(bytes[0..4].try_into().expect("fixed V1 version span"));
        if version != ENGINE_TICKET_VERSION {
            return Err(TicketError::Malformed);
        }
        let owner_uid = u32::from_be_bytes(bytes[4..8].try_into().expect("fixed V1 uid span"));
        let core_pid = u32::from_be_bytes(bytes[8..12].try_into().expect("fixed V1 pid span"));
        let mut auth_key = Zeroizing::new([0_u8; AUTH_KEY_LEN]);
        auth_key.copy_from_slice(&bytes[12..]);
        Self::new(owner_uid, core_pid, std::mem::take(&mut *auth_key))
    }
}

impl Drop for EngineTicketV1 {
    fn drop(&mut self) {
        self.auth_key.zeroize();
    }
}

/// 通过 held runtime dirfd 原子创建 `engine.ticket`。
///
/// 这个函数不接受 ticket pathname，避免 Core 向 Engine 传递任意路径。文件以
/// `O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW` 创建，随后由 fd 设置/验证 0600 并写入固定
/// 长度 V1。创建后再次相对同一 dirfd `fstatat(AT_SYMLINK_NOFOLLOW)`，只有 inode 仍匹配
/// 才把 identity 交给调用方。
///
/// # Errors
///
/// 已存在路径、metadata 不安全、I/O 失败或目录 identity 改变时返回 [`TicketError`]；不会
/// 覆盖、删除或跟随已有 `engine.ticket`。
pub fn create_engine_ticket(
    runtime_dir: &RuntimeDir,
    ticket: EngineTicketV1,
) -> Result<TicketIdentity, TicketError> {
    runtime_dir
        .revalidate_path()
        .map_err(TicketError::RuntimeDir)?;
    let raw_fd = open_ticket_at(
        runtime_dir,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        Some(0o600),
    )?;
    // SAFETY: `open_ticket_at` returned a fresh, uniquely owned non-negative fd.
    let mut file = unsafe { File::from(OwnedFd::from_raw_fd(raw_fd)) };
    if set_ticket_mode(file.as_raw_fd()).is_err() {
        return Err(TicketError::Write);
    }
    let initial_identity = ticket_identity_from_fd(file.as_raw_fd(), runtime_dir.owner().uid())?;
    if initial_identity.size != 0 {
        return Err(TicketError::Metadata);
    }
    let bytes = ticket.into_encoded();
    if file.write_all(&bytes[..]).is_err() || file.flush().is_err() {
        drop(file);
        let _ = guarded_remove_engine_ticket(runtime_dir, initial_identity);
        return Err(TicketError::Write);
    }
    let identity = ticket_identity_from_fd(file.as_raw_fd(), runtime_dir.owner().uid())?;
    if identity.size != ENGINE_TICKET_V1_LEN as u64 {
        drop(file);
        let _ = guarded_remove_engine_ticket(runtime_dir, identity);
        return Err(TicketError::Metadata);
    }
    drop(file);

    if runtime_dir.revalidate_path().is_err() {
        let _ = guarded_remove_engine_ticket(runtime_dir, identity);
        return Err(TicketError::RuntimeDir(RuntimeDirError::PathChanged));
    }
    let current = ticket_identity_from_path(runtime_dir)?;
    if current != identity {
        return Err(TicketError::CleanupRefused);
    }
    Ok(identity)
}

/// Engine 用 held dirfd 读取、验证并一次性消费 `engine.ticket`。
///
/// 读取前先对 fd 检查 regular file、owner、0600、固定长度及 `(dev, ino)`。解析缓存会在
/// 所有成功和错误路径清零。解析成功后，只有同一 dirfd `fstatat(AT_SYMLINK_NOFOLLOW)` 仍
/// 逐字段匹配才执行 `unlinkat`；任一步失败都不会让调用方继续发布 listener。
///
/// # Errors
///
/// symlink、非 regular 文件、owner/mode/size 不匹配、截断/版本错误或 inode 替换分别返回
/// [`TicketError`]，且不尝试普通 pathname cleanup。
pub fn consume_engine_ticket(runtime_dir: &RuntimeDir) -> Result<EngineTicketV1, TicketError> {
    consume_engine_ticket_after_read(runtime_dir, || {})
}

/// 实现一次性消费；测试可在 fd 验证/精确读取后、dirfd `fstatat` 前验证替换拒绝。
///
/// 该函数保持私有，生产入口始终使用空 hook，因而不会引入新的 bootstrap 接口或 wire。
fn consume_engine_ticket_after_read<F>(
    runtime_dir: &RuntimeDir,
    after_read: F,
) -> Result<EngineTicketV1, TicketError>
where
    F: FnOnce(),
{
    runtime_dir
        .revalidate_path()
        .map_err(TicketError::RuntimeDir)?;
    let raw_fd = open_ticket_at(runtime_dir, libc::O_RDONLY, None)?;
    // SAFETY: `open_ticket_at` returned an owned fd that is transferred once to `File`.
    let mut file = unsafe { File::from(OwnedFd::from_raw_fd(raw_fd)) };
    let identity = ticket_identity_from_fd(file.as_raw_fd(), runtime_dir.owner().uid())?;
    if identity.size != ENGINE_TICKET_V1_LEN as u64 {
        return Err(TicketError::Metadata);
    }
    let mut bytes = Zeroizing::new([0_u8; ENGINE_TICKET_V1_LEN]);
    if file.read_exact(&mut *bytes).is_err() {
        return Err(TicketError::Malformed);
    }
    let ticket = EngineTicketV1::decode(&bytes)?;
    drop(file);

    after_read();

    let current = ticket_identity_from_path(runtime_dir)?;
    if current != identity {
        return Err(TicketError::CleanupRefused);
    }
    unlink_ticket_at(runtime_dir)?;
    Ok(ticket)
}

/// 仅在 `engine.ticket` 仍与 `expected` 完全相等时删除它。
///
/// Core 在授权没有成功启动 Engine 时可调用这个函数；不能以路径存在为理由删除未知文件。
///
/// # Errors
///
/// directory、type、owner、mode、size 或 inode 不一致时返回
/// [`TicketError::CleanupRefused`] 并保留现场。
pub fn guarded_remove_engine_ticket(
    runtime_dir: &RuntimeDir,
    expected: TicketIdentity,
) -> Result<(), TicketError> {
    runtime_dir
        .revalidate_path()
        .map_err(|_| TicketError::CleanupRefused)?;
    let current = ticket_identity_from_path(runtime_dir)?;
    if current != expected {
        return Err(TicketError::CleanupRefused);
    }
    unlink_ticket_at(runtime_dir)
}

/// Engine 提前失败路径（ticket 校验失败）唯一允许的 ticket 清理。
///
/// 与 [`guarded_remove_engine_ticket`] 的差异：调用方没有先前记录的
/// [`TicketIdentity`]（消费/校验在读盘后失败）。这里先在 held dirfd 上读取当前
/// `engine.ticket` 的完整 identity（固定 basename、regular file、owner、0600、固定
/// 长度），再以同一次 identity 做 guarded `unlinkat`——路径字符串永不参与删除，
/// 任何不匹配（含 symlink）都返回 [`TicketError::CleanupRefused`] 并保留现场。
///
/// # Errors
///
/// `engine.ticket` 缺失、类型/owner/mode/size 不符，或目录 identity 改变时返回
/// [`TicketError`]。
pub fn remove_engine_ticket_for_failed_bootstrap(
    runtime_dir: &RuntimeDir,
) -> Result<(), TicketError> {
    runtime_dir
        .revalidate_path()
        .map_err(|_| TicketError::CleanupRefused)?;
    let identity = ticket_identity_from_path(runtime_dir)?;
    guarded_remove_engine_ticket(runtime_dir, identity)
}

/// 固定授权启动 communications pipe 的单 record。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineBootstrapRecord {
    /// socket 已发布且可 accept；Core 可以仅以此 pid 建立 `ExpectedPeer(0, pid)`。
    Ready { pid: u32 },
    /// Observe-only Engine 已完成并 clean exit。
    ExitOk { pid: u32 },
    /// Observe-only Engine 已退出；code 仅是非零、非 secret bootstrap 分类。
    ExitError { pid: u32, code: u32 },
}

/// 固定 pipe record 的 protocol/sequence 错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapProtocolError {
    /// read/write 不是正常固定帧 I/O。
    Io,
    /// EOF 或读取不足 20 bytes。
    Truncated,
    /// magic 不是 `EXVB`。
    Magic,
    /// version 不是 1。
    Version,
    /// tag 不是 Ready/ExitOk/ExitError。
    Tag,
    /// 保留字段不是零。
    Reserved,
    /// pid 不是 `1..=i32::MAX`。
    Pid,
    /// Ready/ExitOk code 非零，或 `ExitError` code 非法。
    Code,
    /// record 顺序、pid 关联或额外 record 非法。
    Sequence,
}

impl fmt::Display for BootstrapProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Darwin Engine bootstrap protocol is invalid")
    }
}

impl std::error::Error for BootstrapProtocolError {}

impl EngineBootstrapRecord {
    /// 以固定 20-byte big-endian `EngineBootstrapV1` 编码 record。
    ///
    /// # Errors
    ///
    /// pid/code 不符合固定范围时返回 [`BootstrapProtocolError`]；不会产生部分 record。
    pub fn encode(self) -> Result<[u8; ENGINE_BOOTSTRAP_V1_LEN], BootstrapProtocolError> {
        let (tag, pid, code) = match self {
            Self::Ready { pid } => (1, pid, 0),
            Self::ExitOk { pid } => (2, pid, 0),
            Self::ExitError { pid, code } => (3, pid, code),
        };
        if !valid_pid(pid) {
            return Err(BootstrapProtocolError::Pid);
        }
        if (tag == 3 && !valid_nonzero_code(code)) || (tag != 3 && code != 0) {
            return Err(BootstrapProtocolError::Code);
        }
        let mut bytes = [0_u8; ENGINE_BOOTSTRAP_V1_LEN];
        bytes[0..4].copy_from_slice(&BOOTSTRAP_MAGIC);
        bytes[4] = BOOTSTRAP_VERSION;
        bytes[5] = tag;
        bytes[8..12].copy_from_slice(&pid.to_be_bytes());
        bytes[12..16].copy_from_slice(&code.to_be_bytes());
        Ok(bytes)
    }

    /// 解析恰好一个固定 20-byte `EngineBootstrapV1` record。
    ///
    /// # Errors
    ///
    /// magic、version、tag、保留字段、pid 或 code 不满足协议时返回对应的
    /// [`BootstrapProtocolError`]。
    pub fn decode(bytes: &[u8; ENGINE_BOOTSTRAP_V1_LEN]) -> Result<Self, BootstrapProtocolError> {
        if bytes[0..4] != BOOTSTRAP_MAGIC {
            return Err(BootstrapProtocolError::Magic);
        }
        if bytes[4] != BOOTSTRAP_VERSION {
            return Err(BootstrapProtocolError::Version);
        }
        if bytes[6..8] != [0, 0] || bytes[16..20] != [0, 0, 0, 0] {
            return Err(BootstrapProtocolError::Reserved);
        }
        let pid = u32::from_be_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| BootstrapProtocolError::Truncated)?,
        );
        if !valid_pid(pid) {
            return Err(BootstrapProtocolError::Pid);
        }
        let code = u32::from_be_bytes(
            bytes[12..16]
                .try_into()
                .map_err(|_| BootstrapProtocolError::Truncated)?,
        );
        match bytes[5] {
            1 if code == 0 => Ok(Self::Ready { pid }),
            2 if code == 0 => Ok(Self::ExitOk { pid }),
            3 if valid_nonzero_code(code) => Ok(Self::ExitError { pid, code }),
            1..=3 => Err(BootstrapProtocolError::Code),
            _ => Err(BootstrapProtocolError::Tag),
        }
    }

    /// 精确写出一个 record；调用方负责 communications pipe 的 deadline 和 EOF 语义。
    ///
    /// # Errors
    ///
    /// record 无效或底层 writer 无法完整写入时返回相应错误。
    pub fn write_to<W: Write>(self, writer: &mut W) -> Result<(), BootstrapProtocolError> {
        let bytes = self.encode()?;
        writer
            .write_all(&bytes)
            .map_err(|_| BootstrapProtocolError::Io)
    }

    /// 从 reader 精确读取一个 record；外层必须对本次读取施加 bootstrap deadline。
    ///
    /// # Errors
    ///
    /// EOF/短读返回 [`BootstrapProtocolError::Truncated`]；其他 I/O 或字段错误返回对应类别。
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self, BootstrapProtocolError> {
        let mut bytes = [0_u8; ENGINE_BOOTSTRAP_V1_LEN];
        match reader.read_exact(&mut bytes) {
            Ok(()) => Self::decode(&bytes),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                Err(BootstrapProtocolError::Truncated)
            }
            Err(_) => Err(BootstrapProtocolError::Io),
        }
    }
}

/// 已被完整消费的 Engine bootstrap 终态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineBootstrapExit {
    /// Engine 正常结束。
    Ok { pid: u32 },
    /// Engine 以固定非零 bootstrap code 结束。
    Error { pid: u32, code: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootstrapSequenceState {
    AwaitingReady,
    AwaitingExit { pid: u32 },
    Finished(EngineBootstrapExit),
}

/// 强制唯一合法 `Ready(pid) → ExitOk(pid)|ExitError(pid, code)` 的状态机。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineBootstrapSequence {
    state: BootstrapSequenceState,
}

impl Default for EngineBootstrapSequence {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineBootstrapSequence {
    /// 创建等待首个 Ready 的 sequence。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: BootstrapSequenceState::AwaitingReady,
        }
    }

    /// 接收并验证下一个 record。
    ///
    /// # Errors
    ///
    /// Ready 前 Exit、重复 Ready、不同 pid 的 Exit，或完成后的额外 record 都返回
    /// [`BootstrapProtocolError::Sequence`]。
    pub fn accept(
        &mut self,
        record: EngineBootstrapRecord,
    ) -> Result<Option<EngineBootstrapExit>, BootstrapProtocolError> {
        match (self.state, record) {
            (BootstrapSequenceState::AwaitingReady, EngineBootstrapRecord::Ready { pid }) => {
                self.state = BootstrapSequenceState::AwaitingExit { pid };
                Ok(None)
            }
            (
                BootstrapSequenceState::AwaitingExit { pid },
                EngineBootstrapRecord::ExitOk { pid: exit_pid },
            ) if pid == exit_pid => {
                let exit = EngineBootstrapExit::Ok { pid };
                self.state = BootstrapSequenceState::Finished(exit);
                Ok(Some(exit))
            }
            (
                BootstrapSequenceState::AwaitingExit { pid },
                EngineBootstrapRecord::ExitError {
                    pid: exit_pid,
                    code,
                },
            ) if pid == exit_pid => {
                let exit = EngineBootstrapExit::Error { pid, code };
                self.state = BootstrapSequenceState::Finished(exit);
                Ok(Some(exit))
            }
            _ => Err(BootstrapProtocolError::Sequence),
        }
    }

    /// 返回唯一已完成终态；在 Ready 前或缺少 Exit 时 EOF 都是 protocol error。
    ///
    /// # Errors
    ///
    /// sequence 未收到合法 Exit 时返回 [`BootstrapProtocolError::Sequence`]。
    pub const fn finish_on_eof(self) -> Result<EngineBootstrapExit, BootstrapProtocolError> {
        match self.state {
            BootstrapSequenceState::Finished(exit) => Ok(exit),
            BootstrapSequenceState::AwaitingReady | BootstrapSequenceState::AwaitingExit { .. } => {
                Err(BootstrapProtocolError::Sequence)
            }
        }
    }
}

fn open_ticket_at(
    runtime_dir: &RuntimeDir,
    flags: libc::c_int,
    create_mode: Option<libc::mode_t>,
) -> Result<libc::c_int, TicketError> {
    let flags = flags | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    // SAFETY: runtime_dir owns the directory fd for this entire call and the ticket name is a
    // fixed NUL-terminated basename. The optional mode is supplied only with O_CREAT.
    let raw_fd = unsafe {
        match create_mode {
            Some(mode) => libc::openat(
                runtime_dir.dirfd(),
                ENGINE_TICKET_FILE_NAME.as_ptr().cast(),
                flags,
                libc::c_uint::from(mode),
            ),
            None => libc::openat(
                runtime_dir.dirfd(),
                ENGINE_TICKET_FILE_NAME.as_ptr().cast(),
                flags,
            ),
        }
    };
    if raw_fd >= 0 {
        return Ok(raw_fd);
    }
    let error = std::io::Error::last_os_error();
    if flags & libc::O_EXCL != 0 && error.raw_os_error() == Some(libc::EEXIST) {
        Err(TicketError::Exists)
    } else if flags & libc::O_CREAT != 0 {
        Err(TicketError::Create)
    } else {
        Err(TicketError::Open)
    }
}

fn set_ticket_mode(fd: libc::c_int) -> Result<(), TicketError> {
    // SAFETY: `fd` is an open ticket file descriptor; fchmod affects only that inode.
    if unsafe { libc::fchmod(fd, 0o600) } == 0 {
        Ok(())
    } else {
        Err(TicketError::Write)
    }
}

fn ticket_identity_from_fd(fd: libc::c_int, owner_uid: u32) -> Result<TicketIdentity, TicketError> {
    // SAFETY: zeroed is valid initialization for C stat, which fstat fully writes before use.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: `fd` is open and `stat` is valid writable storage.
    if unsafe { libc::fstat(fd, &raw mut stat) } != 0 {
        return Err(TicketError::Metadata);
    }
    ticket_identity_from_stat(&stat, owner_uid)
}

fn ticket_identity_from_path(runtime_dir: &RuntimeDir) -> Result<TicketIdentity, TicketError> {
    // SAFETY: zeroed is valid initialization for C stat, which fstatat fully writes before use.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: dirfd remains held by `runtime_dir`; the fixed ticket basename is NUL-terminated;
    // AT_SYMLINK_NOFOLLOW makes this a relative lstat rather than a symlink traversal.
    if unsafe {
        libc::fstatat(
            runtime_dir.dirfd(),
            ENGINE_TICKET_FILE_NAME.as_ptr().cast(),
            &raw mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(TicketError::CleanupRefused);
    }
    ticket_identity_from_stat(&stat, runtime_dir.owner().uid())
}

fn ticket_identity_from_stat(
    stat: &libc::stat,
    owner_uid: u32,
) -> Result<TicketIdentity, TicketError> {
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(TicketError::Metadata);
    }
    if stat.st_uid != owner_uid {
        return Err(TicketError::Metadata);
    }
    let mode = u32::from(stat.st_mode) & 0o7777;
    if mode != 0o600 || stat.st_size < 0 {
        return Err(TicketError::Metadata);
    }
    Ok(TicketIdentity {
        device: u64::try_from(stat.st_dev).map_err(|_| TicketError::Metadata)?,
        inode: stat.st_ino,
        owner_uid,
        mode,
        size: stat.st_size.cast_unsigned(),
    })
}

fn unlink_ticket_at(runtime_dir: &RuntimeDir) -> Result<(), TicketError> {
    // SAFETY: dirfd is held and the ticket name is a fixed NUL-terminated basename. Callers have
    // already checked the same dirfd's fstatat identity immediately before this unlinkat.
    if unsafe {
        libc::unlinkat(
            runtime_dir.dirfd(),
            ENGINE_TICKET_FILE_NAME.as_ptr().cast(),
            0,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(TicketError::CleanupRefused)
    }
}

const fn valid_pid(pid: u32) -> bool {
    pid >= 1 && pid <= i32::MAX as u32
}

const fn valid_nonzero_code(code: u32) -> bool {
    code >= 1 && code <= i32::MAX as u32
}
