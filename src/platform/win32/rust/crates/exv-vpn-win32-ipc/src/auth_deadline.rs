//! 本机认证获取的单调绝对期限；Windows 两进程共用开机计时，不传 Instant。
use std::time::Duration;
use windows::Win32::System::SystemInformation::GetTickCount64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthDeadline(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthDeadlineError {
    Invalid,
    Expired,
}

impl AuthDeadline {
    pub fn after(duration: Duration) -> Result<Self, AuthDeadlineError> {
        if duration.is_zero() {
            return Err(AuthDeadlineError::Expired);
        }
        if duration > Duration::from_secs(300) {
            return Err(AuthDeadlineError::Invalid);
        }
        let nanos = u64::try_from(duration.as_nanos()).map_err(|_| AuthDeadlineError::Invalid)?;
        monotonic_ns()
            .checked_add(nanos)
            .map(Self)
            .ok_or(AuthDeadlineError::Invalid)
    }

    pub fn from_monotonic_ns(value: u64) -> Result<Self, AuthDeadlineError> {
        let deadline = Self(value);
        deadline.remaining()?;
        Ok(deadline)
    }

    pub fn monotonic_ns(self) -> u64 {
        self.0
    }

    pub fn remaining(self) -> Result<Duration, AuthDeadlineError> {
        self.remaining_at(monotonic_ns())
    }

    fn remaining_at(self, now: u64) -> Result<Duration, AuthDeadlineError> {
        let nanos = self
            .0
            .checked_sub(now)
            .filter(|n| *n != 0)
            .ok_or(AuthDeadlineError::Expired)?;
        let duration = Duration::from_nanos(nanos);
        if duration > Duration::from_secs(300) {
            return Err(AuthDeadlineError::Invalid);
        }
        Ok(duration)
    }

    /// 起点先取本进程时钟，换算不会延长跨进程截止。
    pub fn tokio_deadline(self) -> Result<tokio::time::Instant, AuthDeadlineError> {
        let now = tokio::time::Instant::now();
        now.checked_add(self.remaining()?)
            .ok_or(AuthDeadlineError::Invalid)
    }
}

fn monotonic_ns() -> u64 {
    // GetTickCount64 无参数；Windows 在本机各进程使用同一开机原点。
    unsafe { GetTickCount64() }.saturating_mul(1_000_000)
}
