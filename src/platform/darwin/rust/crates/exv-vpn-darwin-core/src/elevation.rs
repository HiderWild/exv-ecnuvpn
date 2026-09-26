//! Core 身份前置条件（授权启动走 `engine_lifecycle_real`）。

use std::fmt;

/// Core 在发起受控流程前必须具备的进程身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreCredentials {
    uid: u32,
    euid: u32,
    pid: u32,
}

impl CoreCredentials {
    /// 构造 Core 的身份快照。
    #[must_use]
    pub const fn new(uid: u32, euid: u32, pid: u32) -> Self {
        Self { uid, euid, pid }
    }

    /// owner uid。
    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// Core pid。
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }
}

/// 本模块在当前阶段可报告的失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevationError {
    /// Core 不是普通 owner 进程。
    CoreMustBeUnprivileged,
}

impl fmt::Display for ElevationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoreMustBeUnprivileged => formatter.write_str("Core 必须保持普通身份"),
        }
    }
}

impl std::error::Error for ElevationError {}

/// 验证 Core 没有获得特权，也没有处于身份不一致状态。
///
/// # Errors
///
/// `uid != euid` 或 `uid == 0` 时返回 [`ElevationError::CoreMustBeUnprivileged`]。
pub const fn validate_core_credentials(credentials: CoreCredentials) -> Result<(), ElevationError> {
    if credentials.uid == 0 || credentials.uid != credentials.euid {
        Err(ElevationError::CoreMustBeUnprivileged)
    } else {
        Ok(())
    }
}

/// 读取当前 Core 进程的身份快照。
#[must_use]
pub fn current_core_credentials() -> CoreCredentials {
    // SAFETY: these calls only read the current process credentials and pid.
    let (uid, euid, pid) = unsafe { (libc::getuid(), libc::geteuid(), libc::getpid()) };
    CoreCredentials::new(uid, euid, pid.unsigned_abs())
}
