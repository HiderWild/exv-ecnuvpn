//! Engine 统计注册表 + `StreamStats` 推送（W-MVP-13 对齐；镜像 win32 engine
//! `stats.rs` 的注册表/采样推送设计，采样实现适配 Darwin 泵）。
//!
//! 数据面（utun ↔ CSTP 双线程泵）在 [`crate::packet::pump`] 内以 lock-free
//! 原子计数器累计字节；本模块在 packet 边界之外聚合这些计数器：
//!
//! * [`StatsRegistry::attach_pump`] 把本次连接的双泵计数器挂进注册表
//!   （每次连接一个新 `PumpSet`，挂接即完成重置）；
//! * [`StatsPublisher::open_stream`] 建立一条 mpsc 推送通道并 spawn 采样 task
//!   （last-writer-wins：Core 是唯一统计消费方；镜像 win32 的 `LogSink` 形态）；
//! * 采样 task 按 interval 读注册表快照组 wire `StatsEvent` 推送；客户端断线
//!   （接收端 drop）→ 发送失败 → task 自行退出；重连重新 `open_stream` 即可，
//!   累计计数器即天然断点。
//!
//! 速率口径：Engine 只发布**累计权威字节**（`rx_rate`/`tx_rate` convenience 恒 0）；
//! Core 以累计增量归一化速度为权威口径（同 win32 host P5-2）。因此 Engine 不维护
//! 上一采样状态，改动面最小。延迟源（C2）= 隧道内 ICMP/DPD 探测
//! （[`crate::packet::probe`]）经 `record_latency` 写入的最近一次 RTT；无探测结果时
//! `latency_ms` 恒 0（wire 约定 0 = 未知）。
//!
//! 数据面安全：采样只读原子计数与 `RwLock` 计数器句柄（读取不阻塞 packet 路径）；
//! `record` 侧本来就是 pump 内 relaxed 原子累加。

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use exv_vpn_wire::generated::{self as wire, StatsPhase};
use tokio::sync::mpsc;
use tonic::Status;

/// Engine 默认 `StreamStats` 推送间隔（ms；`StreamStatsRequest.sample_interval_ms == 0` 时）。
pub const DEFAULT_SAMPLE_INTERVAL_MS: u32 = 1000;

/// 采样 mpsc 通道容量（低频推送，16 足够；满则不阻塞业务路径）。
const STREAM_CAPACITY: usize = 16;

/// 一次统计快照（注册表聚合读出的不可变态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsSample {
    /// 累计接收字节（隧道方向：CSTP → utun）。
    pub rx_bytes: u64,
    /// 累计发送字节（隧道方向：utun → CSTP）。
    pub tx_bytes: u64,
    /// 最近一次往返延迟（ms；0 = 未知/不可得；延迟探测经 `record_latency` 写入）。
    pub latency_ms: u64,
    /// 采样时刻的连接粗阶段。
    pub phase: StatsPhase,
    /// 采样时刻 wall-clock epoch 毫秒（UTC；0 = 未知）。
    pub timestamp_ms: i64,
}

/// 当前挂接的双泵字节计数器；无连接时为清零占位（样本恒 0）。
#[derive(Debug, Clone)]
struct PumpCounters {
    rx_bytes: Arc<AtomicU64>,
    tx_bytes: Arc<AtomicU64>,
}

impl PumpCounters {
    fn detached() -> Self {
        Self {
            rx_bytes: Arc::new(AtomicU64::new(0)),
            tx_bytes: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// 数据面计数器与阶段的可读注册表。
///
/// `attach_pump` 只在管线建立/拆除时调用；采样 task 以短临界区读计数器句柄并立即
/// 释放锁，再做 relaxed 原子读，永不阻塞 packet 路径。延迟探测（C2，
/// [`crate::packet::probe`]）在回包匹配点调用 `record_latency`（relaxed 原子写，
/// 同 win32）。
#[derive(Debug)]
pub struct StatsRegistry {
    counters: RwLock<PumpCounters>,
    latency_ms: AtomicU64,
    phase: AtomicU8,
}

impl StatsRegistry {
    /// 建一个清零注册表（默认阶段 `Idle`，无挂接计数器）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            counters: RwLock::new(PumpCounters::detached()),
            latency_ms: AtomicU64::new(0),
            phase: AtomicU8::new(StatsPhase::Idle as u8),
        }
    }

    /// 挂接本次连接的双泵计数器（每次连接的 `PumpSet` 计数器从 0 起，挂接即重置）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    pub fn attach_pump(&self, rx_bytes: Arc<AtomicU64>, tx_bytes: Arc<AtomicU64>) {
        let mut counters = self.counters.write().expect("stats registry counters lock");
        counters.rx_bytes = rx_bytes;
        counters.tx_bytes = tx_bytes;
    }

    /// 拆除挂接（teardown 后样本回到恒 0 占位，不泄露旧连接计数）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    pub fn detach_pump(&self) {
        let mut counters = self.counters.write().expect("stats registry counters lock");
        *counters = PumpCounters::detached();
    }

    /// 设置当前连接粗阶段（`StatsPhase`）。
    pub fn set_phase(&self, phase: StatsPhase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    /// 记录一次往返延迟（ms；延迟探测回包匹配点调用；0 = 清除为未知）。
    pub fn record_latency(&self, ms: u64) {
        self.latency_ms.store(ms, Ordering::Relaxed);
    }

    /// 读取一次聚合快照（短临界区换出计数器句柄 + relaxed 原子读）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    #[must_use]
    pub fn snapshot(&self) -> StatsSample {
        let (rx_bytes, tx_bytes) = {
            let counters = self.counters.read().expect("stats registry counters lock");
            (
                counters.rx_bytes.load(Ordering::Relaxed),
                counters.tx_bytes.load(Ordering::Relaxed),
            )
        };
        StatsSample {
            rx_bytes,
            tx_bytes,
            latency_ms: self.latency_ms.load(Ordering::Relaxed),
            phase: phase_from_u8(self.phase.load(Ordering::Relaxed)),
            timestamp_ms: wall_clock_ms(),
        }
    }
}

impl Default for StatsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// u8 阶段判别 → `StatsPhase`（未知判别 → `Unspecified`）。
fn phase_from_u8(value: u8) -> StatsPhase {
    match value {
        1 => StatsPhase::Idle,
        2 => StatsPhase::Connecting,
        3 => StatsPhase::Connected,
        4 => StatsPhase::Stopping,
        5 => StatsPhase::Failed,
        _ => StatsPhase::Unspecified,
    }
}

/// 当前 wall-clock epoch 毫秒（UTC；proto 约定 0 = unknown，本实现恒填真实值）。
fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `StreamStats` 推送端内部可变态。
#[derive(Debug)]
struct StatsPublisherInner {
    /// 当前挂接的推送通道（last-writer-wins：替换即弃用旧通道）。
    #[allow(dead_code, reason = "挂接状态仅由 open_stream 覆盖，读取留给观测扩展")]
    push: Option<mpsc::Sender<Result<wire::StatsEvent, Status>>>,
}

/// Engine 统计推送端点：持有注册表 + 全局样本序列（对齐 win32 `StatsPublisher`）。
#[derive(Debug)]
pub struct StatsPublisher {
    registry: Arc<StatsRegistry>,
    /// 全局单调样本序列（per engine boot；`StatsEvent.sequence` 的序列基础）。
    next_sequence: Arc<AtomicU64>,
    inner: std::sync::Mutex<StatsPublisherInner>,
}

impl StatsPublisher {
    /// 建一个空推送端点（未挂接流；注册表可供管线立即挂接计数器）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Arc::new(StatsRegistry::new()),
            next_sequence: Arc::new(AtomicU64::new(0)),
            inner: std::sync::Mutex::new(StatsPublisherInner { push: None }),
        }
    }

    /// 共享的统计注册表（管线通过它挂接泵计数器与阶段）。
    #[must_use]
    pub fn registry(&self) -> &Arc<StatsRegistry> {
        &self.registry
    }

    /// 挂接一条 `StreamStats` 推送流，返回接收端（gRPC server 转成 receiver stream）。
    ///
    /// `sample_interval_ms == 0` 使用 Engine 默认间隔（1000 ms）。注册为当前唯一
    /// 推送通道（替换任何残留旧通道），并 spawn 采样 task。无挂接流时无采样 task
    /// （无空转）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    #[must_use]
    pub fn open_stream(
        &self,
        sample_interval_ms: u32,
    ) -> mpsc::Receiver<Result<wire::StatsEvent, Status>> {
        let interval_ms = if sample_interval_ms == 0 {
            DEFAULT_SAMPLE_INTERVAL_MS
        } else {
            sample_interval_ms
        };
        let (tx, rx) = mpsc::channel(STREAM_CAPACITY);
        let sampler = sample_loop(
            Arc::clone(&self.registry),
            tx.clone(),
            Arc::clone(&self.next_sequence),
            interval_ms,
        );
        {
            let mut inner = self.inner.lock().expect("stats publisher lock");
            inner.push = Some(tx);
        }
        tokio::spawn(sampler);
        rx
    }
}

impl Default for StatsPublisher {
    fn default() -> Self {
        Self::new()
    }
}

/// 采样循环：按 interval 读注册表快照 → 推累计权威字节样本。
///
/// 发送失败（接收端 drop = 客户端断线）即退出；无外部取消信号——断线是唯一终止源。
async fn sample_loop(
    registry: Arc<StatsRegistry>,
    tx: mpsc::Sender<Result<wire::StatsEvent, Status>>,
    next_sequence: Arc<AtomicU64>,
    interval_ms: u32,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(u64::from(interval_ms)));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let sample = registry.snapshot();
        let sequence = next_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let event = wire::StatsEvent {
            sequence,
            timestamp_ms: sample.timestamp_ms,
            phase: sample.phase as i32,
            rx_bytes: sample.rx_bytes,
            tx_bytes: sample.tx_bytes,
            // 速率 convenience 不使用：Core 以累计增量归一化（权威口径）。
            rx_rate: 0,
            tx_rate: 0,
            // 延迟 = 注册表最近一次探测 RTT（C2）；无结果时 0 = 未知（wire 约定）。
            latency_ms: sample.latency_ms,
        };
        if tx.send(Ok(event)).await.is_err() {
            // 客户端断线：采样 task 自行退出；重连由 open_stream 重新挂接。
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试：挂接/累计正确性、阶段反映、事件形状与断线退出。
// Core 侧权威速率归一化的契约测试在 exv-vpn-darwin-core stats.rs。
// ---------------------------------------------------------------------------
