
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::status::{StatusEvent, StatusPublisher};
use crate::tunnel_runtime::TunnelRuntime;

/// 默认 core 侧心跳发送周期（10s，可调——`spawn_keepalive_ticker` 参数）。
pub const HEARTBEAT_PERIOD_MS: u64 = 10_000;
/// 默认 engine 侧心跳超时上界（15s，硬时间界；可调——watchdog 参数）。
pub const HEARTBEAT_TIMEOUT_MS: u64 = 15_000;

/// 退出前等待在途组装响应取消的既有预算；SCM 停止等待必须覆盖此窗口。
pub const ASSEMBLY_CANCEL_WAIT: Duration = Duration::from_secs(5);
const ASSEMBLY_CANCEL_POLL: Duration = Duration::from_millis(50);

/// engine 侧心跳监视状态（单调计时，无跨线程锁：仅原子读改写）。
///
/// `last_ms` 是自引擎启动以来的单调毫秒；**0 = 引擎启动时刻**（尚无任何心跳）。watchdog
/// 以「距启动/最近心跳 > 超时上界」判超时——core 自引擎启动后 15s 内不发心跳即触发
/// 自清理+自退出（硬时间界，不依赖进程句柄）。
pub struct HeartbeatWatch {
    /// 单调参考零点（进程启动）。
    start: Instant,
    /// 最近一次收到 KeepAlive 的单调毫秒（0 = 启动时刻，尚无心跳）。
    last_ms: AtomicU64,
}

impl HeartbeatWatch {
    /// 建一个监视（计时从构造开始；`last_ms = 0` = 引擎启动）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    /// 当前单调毫秒（自构造起）。
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// 刷新最近心跳时间戳（`keep_alive` RPC handler 调用）。
    pub fn touch(&self) {
        self.last_ms.store(self.now_ms(), Ordering::SeqCst);
    }

    /// 距最近一次心跳已过的毫秒（0 = 引擎启动，尚无心跳）。
    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        self.now_ms().saturating_sub(self.last_ms.load(Ordering::SeqCst))
    }

    /// 是否已超过指定超时上界（毫秒；硬时间界判据的通用形态）。
    #[must_use]
    pub fn elapsed_exceeds(&self, timeout_ms: u64) -> bool {
        self.elapsed_ms() > timeout_ms
    }

    /// 是否已超过心跳超时上界（硬时间界判据，默认 [`HEARTBEAT_TIMEOUT_MS`]）。
    #[must_use]
    pub fn timed_out(&self) -> bool {
        self.elapsed_exceeds(HEARTBEAT_TIMEOUT_MS)
    }
}

impl Default for HeartbeatWatch {
    fn default() -> Self {
        Self::new()
    }
}

/// 心跳超时 watchdog：周期检查 `elapsed_ms() > timeout_ms`，超时即 resolve（驱动 engine
/// 自退路径）。`check_period` 是检查节拍（默认 500ms；测试注入更小值加速）；`timeout_ms`
/// 是硬时间界上界（默认 [`HEARTBEAT_TIMEOUT_MS`]；测试注入小值）。
#[must_use]
pub fn heartbeat_timeout_watcher(
    heartbeat: Arc<HeartbeatWatch>,
    check_period: Duration,
    timeout_ms: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(check_period).await;
            if heartbeat.elapsed_ms() > timeout_ms {
                break;
            }
        }
    })
}

/// 心跳超时自清理（**复用 teardown 路径**，plan D8）：cancel 在途组装 → 有界等待组装在
/// 段边界自清理（5s 上限，镜像 `stop_tunnel`）→ teardown 数据面/特权资源 → post Idle。
///
/// 调用方随后 300ms flush + 进程退出——完整顺序：**teardown → post Idle → flush → 退**
///（「关机瞬间 connect 中」也满足：cancel 中止在途组装、teardown 撤网卡/路由，再发 Idle）。
///
/// oneshot 专属：服务 engine 不受影响（SCM 生命周期自管，无服务形态代码）。
pub async fn heartbeat_shutdown(runtime: Arc<dyn TunnelRuntime>, status: Arc<StatusPublisher>) {
    // 1. 置位取消令牌 → 有界等待在途组装在段边界取消并自清理（5s 上限，镜像 stop_tunnel）。
    runtime.cancel();
    // 2. teardown 数据面/特权资源（阻塞操作 → spawn_blocking 隔离，不占异步 worker）。
    let _ = tokio::task::spawn_blocking(move || {
        for _ in 0..(ASSEMBLY_CANCEL_WAIT.as_millis() / ASSEMBLY_CANCEL_POLL.as_millis()) {
            if !runtime.is_assembling() {
                break;
            }
            std::thread::sleep(ASSEMBLY_CANCEL_POLL);
        }
        runtime.teardown()
    })
    .await;
    // 3. post Idle（coarse Idle 终态；空 operation_id——心跳超时无外部操作关联，host 侧
    //    R3-C2 陈旧过滤在无在途操作时保守透传/在途不符时丢弃，均为安全）。
    status.publish(StatusEvent::idle(Vec::new()));
}

// ---------------------------------------------------------------------------
// 单元测试：监视计时 / 超时判据 / watchdog 触发 / 自清理顺序（teardown→Idle）。
// ---------------------------------------------------------------------------
