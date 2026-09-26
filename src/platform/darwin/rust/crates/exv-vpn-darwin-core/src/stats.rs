//! Core 侧统计归一化（W-MVP-13；镜像 win32 host `stats.rs`：以累计字节增量做
//! 权威速度归一化）。
//!
//! Engine 经同一已认证控制通道的既有 `StreamStats` 推送 wire `StatsEvent`，
//! 其中 `rx_bytes`/`tx_bytes` 为**累计权威字节数**；`rx_rate`/`tx_rate` 是 Engine
//! 侧 convenience（Darwin Engine 恒填 0，无消费者）。Core 不信任 Engine 采样速率，
//! 以累计字节的**增量采样**自行归一化速度（`delta_bytes / elapsed_ms`）——累计计数
//! 为唯一权威，任何 Engine 侧采样口径差异都在 Core 归一化层抹平。
//!
//! [`TrafficSample`] 维护跨样本的增量状态（上次累计计数 + 时间戳）；[`normalize_stats`]
//! 把一条 `StatsEvent` 归一化为 wire `RuntimeStats`（供 `RuntimeSnapshot.stats` 携带）。
//! 速率计算镜像 engine/win32 同口径 `rate_bytes_per_sec`（`delta * 1000 / elapsed_ms`，
//! 饱和不溢出）。
//!
//! [`LiveStatsState`] 是 Live 连接投影的统计状态机：只有当前 operation 处于
//! 「Connected 且未终结」时，新样本才发布为带最新 `RuntimeStats` 的 Connected 快照
//! （经既有 `WatchEvents` `RuntimeEvent` 通道到达 UI；wire 已有 `snapshot.stats` 字段，
//! 本任务前恒为 `None`）。

use exv_vpn_wire::generated::{RuntimeStats, StatsEvent, StatsPhase};

/// 字节速率（bytes/sec）：`delta_bytes` 在 `elapsed_ms` 内的速率，饱和不溢出。
///
/// 镜像 engine/win32 host 同口径；`elapsed_ms == 0` 安全返回 0。
#[must_use]
fn rate_bytes_per_sec(delta_bytes: u64, elapsed_ms: u64) -> u64 {
    if elapsed_ms == 0 {
        return 0;
    }
    u64::try_from(u128::from(delta_bytes) * 1000 / u128::from(elapsed_ms)).unwrap_or(u64::MAX)
}

/// wire `StatsEvent.phase`（i32）→ `StatsPhase`（未知判别 → `Unspecified`）。
///
/// 生成的 wire 未提供 `TryFrom<i32>`，故手动判别（与 win32 host 同构）。
#[must_use]
fn stats_phase_from_i32(value: i32) -> StatsPhase {
    match value {
        1 => StatsPhase::Idle,
        2 => StatsPhase::Connecting,
        3 => StatsPhase::Connected,
        4 => StatsPhase::Stopping,
        5 => StatsPhase::Failed,
        _ => StatsPhase::Unspecified,
    }
}

/// Core 侧跨样本的流量增量采样状态（以累计字节增量做权威速度归一化）。
///
/// 每次 `feed` 一条 `StatsEvent`：以本次累计计数与上次的**差值**除以两次样本的
/// 时间戳间隔得到归一化速率（bytes/sec）；首条样本无上次参照 → 速率 0 并初始化状态。
/// 时间戳未知（0）/回退（Engine 时钟调整）→ elapsed 0 → 速率 0（安全不误报）。
/// 累计计数回退（Engine 重启/重置）→ `saturating_sub` 差值 0 → 速率 0（不虚构负流量）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TrafficSample {
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
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 归一化本条样本：返回 `(rx_bps, tx_bps)`（累计增量 / 时间间隔），并推进状态。
    ///
    /// 首条样本 → `(0, 0)`（无上次参照）；此后以累计字节差值与时间戳间隔计算权威速率。
    #[must_use]
    pub(crate) fn feed(&mut self, ev: &StatsEvent) -> (u64, u64) {
        if !self.has_prior {
            self.last_rx_bytes = ev.rx_bytes;
            self.last_tx_bytes = ev.tx_bytes;
            self.last_timestamp_ms = ev.timestamp_ms;
            self.has_prior = true;
            return (0, 0);
        }
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
        (
            rate_bytes_per_sec(delta_rx, elapsed_ms),
            rate_bytes_per_sec(delta_tx, elapsed_ms),
        )
    }
}

/// 把一条 Engine `StatsEvent` 归一化为 wire [`RuntimeStats`]（权威速度）。
///
/// 速率由 `sample`（[`TrafficSample`]）的累计增量归一化；累计字节/latency/phase/
/// sequence 透传；`sample_tick` 由调用方以发布 tick 铸造（与 `WatchEvents` 同一
/// monotonic tick 轴）。
#[must_use]
pub(crate) fn normalize_stats(
    ev: &StatsEvent,
    sample: &mut TrafficSample,
    sample_tick: u64,
) -> RuntimeStats {
    let (rx_rate_bps, tx_rate_bps) = sample.feed(ev);
    RuntimeStats {
        rx_bytes: ev.rx_bytes,
        tx_bytes: ev.tx_bytes,
        rx_rate_bps,
        tx_rate_bps,
        latency_ms: ev.latency_ms,
        // 未知判别归一为 Unspecified（wire 0），镜像 win32 host 同口径。
        phase: stats_phase_from_i32(ev.phase) as i32,
        engine_sequence: ev.sequence,
        sample_tick,
    }
}

/// Live 连接投影的统计状态机（当前 operation 的最新样本 + 发布门控）。
///
/// * `mark_connected`：状态投影发布 Connected 快照时置位（此后样本可发布）；
/// * `mark_finished`：失败终态 / 显式 Stop 后调用（停止发布并丢弃最新样本）；
/// * `observe`：每条 Engine 样本都归一化更新最新统计；仅在 Connected 且未终结时
///   返回 `Some(RuntimeStats)` 供发布为 Connected 快照；
/// * `connected_projection`：`GetSnapshot` 的 live 回放（B-09①）与统计发布共用
///   同一事实源——当前 operation 身份也记录在这里，终结时随之清空，不另设第二份
///   状态。
#[derive(Debug, Default)]
pub(crate) struct LiveStatsState {
    connected: bool,
    live: bool,
    latest: Option<RuntimeStats>,
    /// 当前 operation 身份（`begin_operation` 登记；终结时随门控一并清空）。
    operation_id: Option<Vec<u8>>,
}

impl LiveStatsState {
    /// 新 operation 的初始状态（未 Connected；允许消费样本）。
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            connected: false,
            live: true,
            latest: None,
            operation_id: None,
        }
    }

    /// 登记当前 operation 身份（Connect 受理后调用；`GetSnapshot` live 回放用）。
    pub(crate) fn begin_operation(&mut self, operation_id: Vec<u8>) {
        self.operation_id = Some(operation_id);
    }

    /// 状态投影已发布 Connected：后续样本发布为 Connected 快照。
    pub(crate) fn mark_connected(&mut self) {
        self.connected = true;
    }

    /// 当前 operation 终结（失败终态 / 显式 Stop）：停止发布并丢弃最新样本。
    pub(crate) fn mark_finished(&mut self) {
        self.connected = false;
        self.live = false;
        self.latest = None;
        self.operation_id = None;
    }

    /// 当前 operation 是否仍活着（未终结）；统计任务据此决定是否退出。
    #[must_use]
    pub(crate) fn is_live(&self) -> bool {
        self.live
    }

    /// 是否存在进行中的 operation（已受理 Connect 且未终结）——W2 退出清理包的
    /// 「在跑隧道」判据：空闲（未连接过或已显式 Stop/终态）为 false。
    #[must_use]
    pub(crate) fn has_live_operation(&self) -> bool {
        self.live && self.operation_id.is_some()
    }

    /// 状态投影发布 Connected 快照时可附带的最新统计（可能尚无样本）。
    #[must_use]
    pub(crate) fn latest(&self) -> Option<RuntimeStats> {
        self.latest
    }

    /// 归一化一条 Engine 样本并推进状态。
    ///
    /// 返回 `Some(RuntimeStats)` = 该样本应发布为 Connected 快照（Connected 且
    /// operation 未终结）；`None` = 仅更新最新统计，不发布。
    pub(crate) fn observe(
        &mut self,
        ev: &StatsEvent,
        traffic: &mut TrafficSample,
        sample_tick: u64,
    ) -> Option<RuntimeStats> {
        let sample = normalize_stats(ev, traffic, sample_tick);
        if !self.live {
            return None;
        }
        self.latest = Some(sample);
        self.connected.then_some(sample)
    }

    /// `GetSnapshot` 的 live 回放材料（B-09①）：当前 operation 已 Connected 且未
    /// 终结时返回 `(operation_id, 最新门控统计)`，供组装与 `WatchEvents` 推送同构的
    /// Connected 快照。connecting / 未连接 / 已终结返回 `None`（连接期阶段恢复由
    /// `WatchEvents` Transition 承担；connecting 无统计是 10c 门控契约）。
    #[must_use]
    pub(crate) fn connected_projection(&self) -> Option<(Vec<u8>, Option<RuntimeStats>)> {
        if !self.live || !self.connected {
            return None;
        }
        let operation_id = self.operation_id.clone()?;
        Some((operation_id, self.latest))
    }

    /// Connected 首帧的样本归一化（B-09②）：连接期缓存的样本携带 Engine 相位
    /// （如 `Idle`）与未铸造的 tick 0——直接附加会被前端 `hasUsableConnectedSample`
    /// 守卫拒绝（统计首帧延迟约 1s）。Connected 粗阶段是 Core 权威的数据面事实，
    /// 发布时把缓存样本归一化为 Connected 相位并铸造发布 tick（与统计发布同一
    /// monotonic tick 轴）；重铸结果同步回缓存，`GetSnapshot` live 回放与首帧发布
    /// 同源一致。
    #[must_use]
    pub(crate) fn connected_baseline(&mut self, sample_tick: u64) -> Option<RuntimeStats> {
        let mut stats = self.latest()?;
        stats.phase = StatsPhase::Connected as i32;
        stats.sample_tick = sample_tick;
        self.latest = Some(stats);
        Some(stats)
    }

    /// 统计发布后把铸造 tick 的样本同步进缓存（B-09①）：live 回放与已发布统计
    /// 一致；终结后不复活样本（只在 live 时写入）。
    pub(crate) fn cache_minted(&mut self, stats: RuntimeStats) {
        if self.live {
            self.latest = Some(stats);
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试：增量/速度归一化正确性（权威口径）、边界（首条/时钟未知/时钟回退/
// 计数回退/饱和）、字段透传；Live 状态机的发布门控与终结清理。
// ---------------------------------------------------------------------------
