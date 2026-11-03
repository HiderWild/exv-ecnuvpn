//! 同一宿主认证请求的单调绝对期限；不持久化、不跨重启或跨宿主使用。
//! CLOCK_MONOTONIC 纳秒值只在已认证本机 IPC 传递，进程内 Instant 仅由剩余量换算。

use std::time::Duration;
use tonic::metadata::MetadataMap;

pub const AUTH_DEADLINE_METADATA_KEY: &str = "x-exv-auth-deadline-monotonic-ns";
const MAX_AUTH_DURATION: Duration = Duration::from_secs(300);
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// 本机单调时钟的绝对截止值；不暴露或序列化进程内 Instant。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthDeadline(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthDeadlineError {
    Invalid,
    Expired,
    Clock,
}

impl AuthDeadline {
    /// 运输同一个 CLOCK_MONOTONIC 绝对值；不重新分配认证预算。
    pub fn from_monotonic_ns(value: u64) -> Result<Self, AuthDeadlineError> {
        let deadline = Self(value);
        deadline.remaining()?;
        Ok(deadline)
    }

    pub const fn monotonic_ns(self) -> u64 {
        self.0
    }

    /// 从本机当前单调时间固定一次期限，最长 300 秒。
    pub fn after(duration: Duration) -> Result<Self, AuthDeadlineError> {
        Self::after_at(duration, monotonic_ns()?)
    }

    /// 缺头保留旧客户端兼容；有头时必须唯一且为合法 ASCII 十进制 u64。
    pub fn from_metadata(metadata: &MetadataMap) -> Result<Option<Self>, AuthDeadlineError> {
        let mut values = metadata.get_all(AUTH_DEADLINE_METADATA_KEY).iter();
        let Some(value) = values.next() else {
            return Ok(None);
        };
        if values.next().is_some() {
            return Err(AuthDeadlineError::Invalid);
        }
        let value = value.to_str().map_err(|_| AuthDeadlineError::Invalid)?;
        let deadline = Self::parse_value(value)?;
        deadline.remaining()?;
        Ok(Some(deadline))
    }

    /// 将同一绝对截止附到新请求；不覆盖已存在的期限或生成重复值。
    pub fn write_metadata(self, metadata: &mut MetadataMap) -> Result<(), AuthDeadlineError> {
        self.remaining()?;
        if metadata.contains_key(AUTH_DEADLINE_METADATA_KEY) {
            return Err(AuthDeadlineError::Invalid);
        }
        let value = self
            .0
            .to_string()
            .parse()
            .map_err(|_| AuthDeadlineError::Invalid)?;
        metadata.insert(AUTH_DEADLINE_METADATA_KEY, value);
        Ok(())
    }

    /// 读取当前剩余量；已到期与时钟故障分别报告，不能转成认证回退。
    pub fn remaining(self) -> Result<Duration, AuthDeadlineError> {
        self.remaining_at(monotonic_ns()?)
    }

    /// 将当前剩余量换算到本进程 Tokio 原点；不把 Instant 内部值用于 IPC。
    pub fn tokio_deadline(self) -> Result<tokio::time::Instant, AuthDeadlineError> {
        // 先捕获Tokio起点，再读CLOCK_MONOTONIC，避免换算操作本身延长截止。
        let now = tokio::time::Instant::now();
        now.checked_add(self.remaining()?)
            .ok_or(AuthDeadlineError::Invalid)
    }

    fn after_at(duration: Duration, now: u64) -> Result<Self, AuthDeadlineError> {
        if duration.is_zero() {
            return Err(AuthDeadlineError::Expired);
        }
        if duration > MAX_AUTH_DURATION {
            return Err(AuthDeadlineError::Invalid);
        }
        let nanos = u64::try_from(duration.as_nanos()).map_err(|_| AuthDeadlineError::Invalid)?;
        now.checked_add(nanos)
            .map(Self)
            .ok_or(AuthDeadlineError::Invalid)
    }

    fn parse_value(value: &str) -> Result<Self, AuthDeadlineError> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(AuthDeadlineError::Invalid);
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_| AuthDeadlineError::Invalid)
    }

    fn remaining_at(self, now: u64) -> Result<Duration, AuthDeadlineError> {
        let nanos = self
            .0
            .checked_sub(now)
            .filter(|remaining| *remaining != 0)
            .ok_or(AuthDeadlineError::Expired)?;
        let remaining = Duration::from_nanos(nanos);
        if remaining > MAX_AUTH_DURATION {
            return Err(AuthDeadlineError::Invalid);
        }
        Ok(remaining)
    }
}

fn monotonic_ns() -> Result<u64, AuthDeadlineError> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // 安全说明：time是可写timespec，CLOCK_MONOTONIC不依赖指针之外的输入。
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(AuthDeadlineError::Clock);
    }
    timespec_ns(time.tv_sec, time.tv_nsec)
}

fn timespec_ns(seconds: i64, nanoseconds: i64) -> Result<u64, AuthDeadlineError> {
    let seconds = u64::try_from(seconds).map_err(|_| AuthDeadlineError::Clock)?;
    let nanoseconds = u64::try_from(nanoseconds).map_err(|_| AuthDeadlineError::Clock)?;
    if nanoseconds >= NANOS_PER_SECOND {
        return Err(AuthDeadlineError::Clock);
    }
    seconds
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|seconds| seconds.checked_add(nanoseconds))
        .ok_or(AuthDeadlineError::Clock)
}
