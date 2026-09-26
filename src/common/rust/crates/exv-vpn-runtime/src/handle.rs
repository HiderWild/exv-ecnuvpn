
use exv_vpn_domain::identity::AttemptId;
use exv_vpn_domain::limits::MvpLimits;

/// 并发 stop waiter 已达 `stop_waiters` 上限时，注册被拒绝。
#[derive(Debug)]
pub struct WaiterLimitExceeded;

/// `StopLatch::new()` 使用的默认 stop_waiters 预算（与 MVP 基线一致）。
const DEFAULT_STOP_WAITERS: usize = 8;

/// 单槽 Stop latch。
///
/// - 单 slot：`latched` 至多为一个 `Some`；`latch_stop` 在首次之后只是 coalesce 计数。
/// - 有界：`try_add_waiter` 受 `stop_waiters` 约束，超限返回 `WaiterLimitExceeded`。
/// - RPC waiter 记账不触发 Stop：`rpc_waiter_drop` 只归还占用，不 latch 任何 token。
#[derive(Debug)]
pub struct StopLatch {
    stop_waiters: usize,
    waiters: usize,
    rpc_waiters: usize,
    latched: Option<AttemptId>,
    coalesced: u64,
}

impl StopLatch {
    pub fn new() -> Self {
        Self {
            stop_waiters: DEFAULT_STOP_WAITERS,
            waiters: 0,
            rpc_waiters: 0,
            latched: None,
            coalesced: 0,
        }
    }

    pub fn with_limits(limits: &MvpLimits) -> Self {
        let mut latch = Self::new();
        latch.stop_waiters = limits.stop_waiters;
        latch
    }

    /// latch 一次 Stop：首个子句把单 token 锚定到当前 attempt，返回该 attempt；
    /// 后续 Stop 仅 coalesce 计数（`coalesced_count` 递增），不 mint 新 token。
    pub fn latch_stop(&mut self, attempt: &AttemptId) -> AttemptId {
        if self.latched.is_none() {
            self.latched = Some(attempt.clone());
        }
        self.coalesced += 1;
        attempt.clone()
    }

    pub fn latched_attempt(&self) -> Option<&AttemptId> {
        self.latched.as_ref()
    }

    /// 已持有的 token 数：0 或 1（单 slot）。
    pub fn token_count(&self) -> u64 {
        u64::from(self.latched.is_some())
    }

    /// 已被 coalesce 观测到的 Stop 总数。
    pub fn coalesced_count(&self) -> u64 {
        self.coalesced
    }

    pub fn try_add_waiter(&mut self) -> Result<(), WaiterLimitExceeded> {
        if self.waiters >= self.stop_waiters {
            return Err(WaiterLimitExceeded);
        }
        self.waiters += 1;
        Ok(())
    }

    pub fn register_rpc_waiter(&mut self) {
        self.rpc_waiters += 1;
    }

    /// 归还一个 RPC waiter 占用。它绝不 emit Stop：取消 RPC 不是 Stop 请求。
    pub fn rpc_waiter_drop(&mut self) {
        self.rpc_waiters = self.rpc_waiters.saturating_sub(1);
    }
}

impl Default for StopLatch {
    fn default() -> Self {
        Self::new()
    }
}

