
//! core 侧统计归一化（productization P5-2 已落地：消费 engine `StreamStats`，做权威速度归一化）。
//!
//! engine 经 `HelperControl.StreamStats` 推送 `StatsEvent`，其中
//! `rx_bytes`/`tx_bytes` 为**累计权威字节数**；`rx_rate`/`tx_rate` 是 engine 侧采样
//! 速率 convenience。core 不信任 engine 采样速率，改为以累计字节的
//! **增量采样**自行归一化速度（`delta_bytes / elapsed_seconds`）——累计计数为唯一
//! 权威，任何 engine 侧采样口径差异都在 core 归一化层抹平（真实流已由
//! `KernelControlService::spawn_stats_forwarder` 订阅并喂入本模块）。
//!
//! [`TrafficSample`] 维护跨样本的增量状态（上次累计计数 + 时间戳）；[`normalize_stats`]
//! 把一条 `StatsEvent` 归一化为 host 侧 [`RuntimeStats`]（归一化速率 + 累计流量 +
//! phase + latency 透传），并维护采样状态。速率计算镜像 engine 侧
//! `rate_bytes_per_sec`（`delta * 1000 / elapsed_ms`，饱和不溢出）——同口径、core
//! 为权威。
//!
//! `RuntimeStats.sample_tick` 由 [`crate::kernel_control_service::EventBus::publish_stats`]
//! 铸造（与 wire 事件同一 monotonic tick 对齐），经 `RuntimeSnapshot.stats` 随快照
//! 下发，供 tauri 侧统计镜像（`tauri/app/src/kernel/stats.rs`）在 UI 直接渲染。

use exv_vpn_wire::generated::{StatsEvent, StatsPhase};

/// 字节速率（bytes/sec）：`delta_bytes` 在 `elapsed_ms` 内的速率，饱和不溢出。
///
/// 镜像 engine `stats.rs::rate_bytes_per_sec` 同口径；`elapsed_ms == 0` 安全返回 0。
#[must_use]
fn rate_bytes_per_sec(delta_bytes: u64, elapsed_ms: u64) -> u64 {
    if elapsed_ms == 0 {
        return 0;
    }
    u64::try_from(u128::from(delta_bytes) * 1000 / u128::from(elapsed_ms)).unwrap_or(u64::MAX)
}

/// wire `StatsEvent.phase`（i32）→ `StatsPhase`（未知判别 → `Unspecified`）。
///
/// 生成的 wire 未提供 `TryFrom<i32>`（prost 0.14 本生成配置未产出），故手动判别；
/// 与 engine 侧 `phase_from_u8` 同构。W3-1/P1 S1b：EventBus wrapper 化后核心总线
/// 持 wire `RuntimeStats`，本函数兼供 host↔wire 往返转换复用。
#[must_use]
pub(crate) fn stats_phase_from_i32(value: i32) -> StatsPhase {
    match value {
        1 => StatsPhase::Idle,
        2 => StatsPhase::Connecting,
        3 => StatsPhase::Connected,
        4 => StatsPhase::Stopping,
        5 => StatsPhase::Failed,
        _ => StatsPhase::Unspecified,
    }
}

/// 一次 `feed` 的归一化产出（RT-SAMPLE-02：把"为什么速率是 0"的事实暴露给转发器）。
///
/// 速率语义与既有 `(u64, u64)` 返回值完全一致（首样本 0、时间戳未知/回退 0、
/// 计数回退 0、饱和封顶）；`timestamp_regression`/`counter_regression` 只是把既有
/// 边界规则里"速率被安全置 0"的原因作为显式布尔事实暴露，供统计转发器按冻结诊断表
/// 落码（`kernel.stats.timestamp_regression` / `kernel.stats.counter_regression`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleOutcome {
    /// 归一化接收速率（bytes/sec；与改前返回值第一元同义）。
    pub rx_bps: u64,
    /// 归一化发送速率（bytes/sec；与改前返回值第二元同义）。
    pub tx_bps: u64,
    /// 样本时间戳回退（两侧锚点已知且当前 < 上次）——该样本速率按 0 处理。
    pub timestamp_regression: bool,
    /// 同一订阅流内累计计数回退（engine 逻辑异常）——该样本速率按 0 处理。
    pub counter_regression: bool,
}

/// core 侧跨样本的流量增量采样状态（P5-2 权威速度归一化已落地：以累计字节增量计算）。
///
/// 每次 `feed` 一条 `StatsEvent`：以本次累计计数与上次的**差值**除以两次样本的
/// 时间戳间隔得到归一化速率（bytes/sec）；首条样本无上次参照 → 速率 0 并初始化状态。
/// 时间戳未知（0）/回退（engine 时钟调整）→ elapsed 0 → 速率 0（安全不误报）。
/// 累计计数回退（engine 重启/重置）→ `saturating_sub` 差值 0 → 速率 0（不虚构负流量）。
#[derive(Debug, Clone, Copy, Default)]
pub struct TrafficSample {
    /// 上次样本的累计接收字节。
    last_rx_bytes: u64,
    /// 上次样本的累计发送字节。
    last_tx_bytes: u64,
    /// 上次样本的 wall-clock epoch 毫秒（0 = 未知）。
    last_timestamp_ms: i64,
    /// 是否已有上次样本（首条样本无参照）。
    has_prior: bool,
}

impl TrafficSample {
    /// 建一个空采样状态（首条 `feed` 初始化）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 归一化本条样本：返回 [`SampleOutcome`]（速率 + 回退事实），并推进状态。
    ///
    /// 首条样本 → 速率 `(0, 0)`（无上次参照）；此后以累计字节差值与时间戳间隔计算
    /// 权威速率。速率规则与历史 `(u64, u64)` 返回值逐用例一致（RT-SAMPLE-02 只扩展
    /// 回退事实，不改任何数值语义）。
    #[must_use]
    pub fn feed(&mut self, ev: &StatsEvent) -> SampleOutcome {
        if !self.has_prior {
            self.last_rx_bytes = ev.rx_bytes;
            self.last_tx_bytes = ev.tx_bytes;
            self.last_timestamp_ms = ev.timestamp_ms;
            self.has_prior = true;
            return SampleOutcome {
                rx_bps: 0,
                tx_bps: 0,
                timestamp_regression: false,
                counter_regression: false,
            };
        }
        // 时间戳回退：两侧锚点已知且当前 < 上次（负间隔）——既有 `try_from` 失败路径
        // 的显式事实化；锚点未知（0）是"不可测"而非"回退"，不计入。
        let timestamp_regression =
            ev.timestamp_ms > 0 && self.last_timestamp_ms > 0 && ev.timestamp_ms < self.last_timestamp_ms;
        // 累计计数回退：任一方向小于上次参照（engine 重启/重置）。
        let counter_regression = ev.rx_bytes < self.last_rx_bytes || ev.tx_bytes < self.last_tx_bytes;
        let delta_rx = ev.rx_bytes.saturating_sub(self.last_rx_bytes);
        let delta_tx = ev.tx_bytes.saturating_sub(self.last_tx_bytes);
        let elapsed_ms = if ev.timestamp_ms > 0 && self.last_timestamp_ms > 0 {
            // 时钟回退 → try_from 失败 → 0（安全不误报）。
            u64::try_from(ev.timestamp_ms - self.last_timestamp_ms).unwrap_or(0)
        } else {
            0
        };
        self.last_rx_bytes = ev.rx_bytes;
        self.last_tx_bytes = ev.tx_bytes;
        self.last_timestamp_ms = ev.timestamp_ms;
        SampleOutcome {
            rx_bps: rate_bytes_per_sec(delta_rx, elapsed_ms),
            tx_bps: rate_bytes_per_sec(delta_tx, elapsed_ms),
            timestamp_regression,
            counter_regression,
        }
    }
}

/// host 侧归一化后的运行期统计（productization P5-2 产出；wire 走 `RuntimeSnapshot.stats`，
/// tauri 侧镜像消费）。
///
/// `rx_bytes`/`tx_bytes` 为 engine 累计权威字节（透传）；`rx_rate_bps`/`tx_rate_bps`
/// 为 core 归一化速度（累计增量 / 时间间隔，engine 的 `rx_rate`/`tx_rate` convenience
/// 不使用）；`latency_ms`/`phase` 透传；`engine_sequence` 为 engine 样本序号（关联用）；
/// `sample_tick` 由 `EventBus::publish_stats` 铸造（与 wire 事件同一 monotonic tick 对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStats {
    /// 累计接收字节（engine 权威）。
    pub rx_bytes: u64,
    /// 累计发送字节（engine 权威）。
    pub tx_bytes: u64,
    /// 归一化接收速率（bytes/sec；累计增量 / 时间间隔）。
    pub rx_rate_bps: u64,
    /// 归一化发送速率（bytes/sec；累计增量 / 时间间隔）。
    pub tx_rate_bps: u64,
    /// 往返延迟（ms；0 = 未知/不可得）。
    pub latency_ms: u64,
    /// engine 采样时刻的连接阶段。
    pub phase: StatsPhase,
    /// engine 样本序号（per engine boot）。
    pub engine_sequence: u64,
    /// 发布时的事件总线 tick（`publish_stats` 铸造；0 = 尚未发布）。
    pub sample_tick: u64,
}

/// 把一条 engine `StatsEvent` 归一化为 host 侧 [`RuntimeStats`] + 回退事实
/// （权威速度归一化；RT-SAMPLE-02 透传 outcome，`RuntimeStats` 本体不变）。
///
/// 速率由 `sample`（[`TrafficSample`]）的累计增量归一化；累计字节/latency/phase/
/// sequence 透传。`sample_tick` 初始 0，由 `EventBus::publish_stats` 铸造后覆盖。
/// 返回的 [`SampleOutcome`] 供统计转发器按冻结诊断表落 `kernel.stats.*` 码。
#[must_use]
pub fn normalize_stats(ev: &StatsEvent, sample: &mut TrafficSample) -> (RuntimeStats, SampleOutcome) {
    let outcome = sample.feed(ev);
    let stats = RuntimeStats {
        rx_bytes: ev.rx_bytes,
        tx_bytes: ev.tx_bytes,
        rx_rate_bps: outcome.rx_bps,
        tx_rate_bps: outcome.tx_bps,
        latency_ms: ev.latency_ms,
        phase: stats_phase_from_i32(ev.phase),
        engine_sequence: ev.sequence,
        sample_tick: 0,
    };
    (stats, outcome)
}

// ---------------------------------------------------------------------------
// 单元测试：增量/速度归一化正确性（权威口径）、边界（首条/时钟未知/时钟回退/
// 计数回退/饱和）、字段透传。
// ---------------------------------------------------------------------------
