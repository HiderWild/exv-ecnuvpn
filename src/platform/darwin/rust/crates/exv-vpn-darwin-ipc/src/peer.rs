//! 从 Darwin 内核读取 Unix-domain peer credential。

use std::{mem, os::fd::AsRawFd};

use tokio::net::UnixStream;

use crate::auth::PreauthError;

/// 认证中唯一可信的 uid/pid 对；它来自 OS peer credential，而非任何 wire 字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedPeer {
    uid: u32,
    pid: u32,
}

impl ExpectedPeer {
    /// 构造预期的 uid/pid。
    #[must_use]
    pub const fn new(uid: u32, pid: u32) -> Self {
        Self { uid, pid }
    }

    /// 当前 Core 进程可写入 HMAC transcript 的本机身份。
    #[must_use]
    pub fn current_process() -> Self {
        // SAFETY: `geteuid` has no arguments, no aliasing preconditions, and only reads the
        // calling process credential supplied by Darwin.
        let uid = unsafe { libc::geteuid() };
        Self::new(uid, std::process::id())
    }

    /// 预期 uid。
    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// 预期 pid。
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }

    /// 认证后的 OS peer 是否满足该预期。
    #[must_use]
    pub const fn matches(self, peer: VerifiedLocalPeer) -> bool {
        self.uid == peer.uid && self.pid == peer.pid
    }
}

/// 已由 Darwin 内核提供的 UDS peer credential。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedLocalPeer {
    uid: u32,
    gid: u32,
    pid: u32,
}

impl VerifiedLocalPeer {
    /// 构造已验证 peer，供测试 seam 使用。
    #[must_use]
    pub const fn new(uid: u32, gid: u32, pid: u32) -> Self {
        Self { uid, gid, pid }
    }

    /// 该 peer 的 uid。
    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// 该 peer 的 gid。
    #[must_use]
    pub const fn gid(self) -> u32 {
        self.gid
    }

    /// 该 peer 的 pid。
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }

    /// 抹去 gid 后用于 HMAC transcript 的身份。
    #[must_use]
    pub const fn identity(self) -> ExpectedPeer {
        ExpectedPeer::new(self.uid, self.pid)
    }
}

/// peer credential 的可注入读取 seam。
///
/// 生产使用 [`SystemPeerLookup`]；测试可返回确定的 credential，而无需改变宿主 uid。
pub trait PeerLookup: Send + Sync {
    /// 读取指定已连接 UDS 的 OS peer credential。
    ///
    /// # Errors
    ///
    /// 当宿主无法读取该 stream 的 OS credential 时返回 [`PreauthError`]。
    fn inspect(&self, stream: &UnixStream) -> Result<VerifiedLocalPeer, PreauthError>;
}

/// Darwin `getpeereid` 加 `LOCAL_PEERPID` 的生产读取器。
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPeerLookup;

impl PeerLookup for SystemPeerLookup {
    fn inspect(&self, stream: &UnixStream) -> Result<VerifiedLocalPeer, PreauthError> {
        inspect_system_peer(stream)
    }
}

/// 用 OS 事实校验而非 wire 字段校验 peer。
///
/// # Errors
///
/// 当读取 OS credential 失败，或其 uid/pid 与 `expected` 不匹配时返回相应的
/// [`PreauthError`]。
pub fn verify_peer(
    lookup: &dyn PeerLookup,
    stream: &UnixStream,
    expected: ExpectedPeer,
) -> Result<VerifiedLocalPeer, PreauthError> {
    let peer = lookup.inspect(stream)?;
    if expected.matches(peer) {
        Ok(peer)
    } else {
        Err(PreauthError::PeerMismatch)
    }
}

#[cfg(target_os = "macos")]
fn inspect_system_peer(stream: &UnixStream) -> Result<VerifiedLocalPeer, PreauthError> {
    let file_descriptor = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `file_descriptor` is borrowed from a live `UnixStream`; the uid/gid output
    // pointers are valid mutable storage for the duration of this synchronous syscall.
    let peer_result = unsafe { libc::getpeereid(file_descriptor, &raw mut uid, &raw mut gid) };
    if peer_result != 0 {
        return Err(PreauthError::Transport);
    }

    let mut pid: libc::pid_t = 0;
    let pid_size =
        libc::socklen_t::try_from(mem::size_of_val(&pid)).map_err(|_| PreauthError::Transport)?;
    let mut pid_length = pid_size;
    // SAFETY: the descriptor is a live Unix socket; `pid` and `pid_length` are writable
    // storage of the exact types and lengths required by Darwin's `LOCAL_PEERPID` option.
    let pid_result = unsafe {
        libc::getsockopt(
            file_descriptor,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast::<libc::c_void>(),
            &raw mut pid_length,
        )
    };
    if pid_result != 0 || pid_length != pid_size || pid < 1 {
        return Err(PreauthError::Transport);
    }

    Ok(VerifiedLocalPeer::new(
        uid,
        gid,
        u32::try_from(pid).map_err(|_| PreauthError::Transport)?,
    ))
}

#[cfg(not(target_os = "macos"))]
fn inspect_system_peer(stream: &UnixStream) -> Result<VerifiedLocalPeer, PreauthError> {
    let _ = stream;
    Err(PreauthError::Transport)
}
