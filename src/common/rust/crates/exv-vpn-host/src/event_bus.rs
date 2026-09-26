
//! `WatchEvents` 的核心事件总线（W3-1/P1 S1a：自 win32 host
//! `kernel_control_service.rs` 的 `EventBus` 上提 common；win32 侧 wrapper 化保留
//! lane 装饰链，darwin 侧直接消费——两侧同时消费，终结 host crate R1 单宿主豁免）。
//!
//! 核心只承载与平台无关的总线事实：严格递增 monotonic tick（总线铸造）、最新事件
//! （断线/订阅重放路径）、多订阅者 fan-out broadcast 通道，以及统计 lane
//! （`publish_stats`/`subscribe_stats`）。**不做 lane 装饰**：stats / proxy_tun /
//! system_proxy / service_status / service_mode / self_heal 等伴生数据附加由调用方
//! 在组装快照时完成（win32 wrapper 在 `publish` 内完成装饰链；darwin 在各发布边界
//! 组装）。依赖仅 tokio + tokio-stream + exv-vpn-wire，零平台语义。
//!
//! lagged 处理对齐 win32 现状：`BroadcastStream` 的 `Lagged` 项被静默丢弃（订阅者
//! 只看到丢弃后的新事件，不收到错误、不重放）——P1.5 两侧同改挂账。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use exv_vpn_wire::generated::{self as wire, RuntimeEvent, RuntimeSnapshot};

/// `WatchEvents` 的核心事件总线。
///
/// 维护严格递增的 monotonic tick（总线铸造）、最新事件（断线/订阅重放路径）与
/// 多订阅者 fan-out broadcast 通道。`publish` 铸造下一 tick 并广播；`subscribe`
/// 先按 resume 语义发当前快照（SNAPSHOT）再转发现场事件（增量 tick）。
pub struct EventBus {
    /// live 事件 fan-out（多个 `WatchEvents` 订阅者）。
    live: broadcast::Sender<RuntimeEvent>,
    /// 串行化状态发布与已连接统计重发，避免 status / stats 两条后台转发器把较旧快照
    /// 在较新 tick 之后写回 `current`。
    publish_gate: Mutex<()>,
    /// 下一 monotonic tick（严格递增；0 保留为"无事件"）。
    tick: AtomicU64,
    /// 最新事件（含快照；resume / 断线重放路径）。`std::sync::Mutex`（同步访问，
    /// 不跨 await 持锁）。
    current: Mutex<Option<RuntimeEvent>>,
    /// 统计 fan-out（归一化后；供进程内订阅者消费，与事件 lane 互不干扰）。
    stats_live: broadcast::Sender<wire::RuntimeStats>,
    /// 最新统计（`publish_stats` 更新；已连接时与重发的快照使用同一 tick）。
    stats_current: Mutex<Option<wire::RuntimeStats>>,
}

impl EventBus {
    /// 构造空总线（无事件、无统计、tick=0）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            live: broadcast::channel(64).0,
            publish_gate: Mutex::new(()),
            tick: AtomicU64::new(0),
            current: Mutex::new(None),
            stats_live: broadcast::channel(64).0,
            stats_current: Mutex::new(None),
        }
    }

    /// 铸造下一 monotonic tick（严格递增，从 1 开始）。
    ///
    /// 仅供测试与观测断言 tick 轴；正常发布路径经 [`Self::publish`] /
    /// [`Self::publish_stats`] 在 `publish_gate` 内铸造，勿在发布间隙单独调用
    /// （会让 tick 轴出现空洞）。
    pub fn next_tick(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 当前已发布的最大 tick（0 = 尚无事件）。
    #[must_use]
    pub fn current_tick(&self) -> u64 {
        self.tick.load(Ordering::Relaxed)
    }

    /// 最新发布事件的快照（无事件 → `None`）。
    #[must_use]
    pub fn current_snapshot(&self) -> Option<RuntimeSnapshot> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|ev| ev.snapshot.clone())
    }

    /// 发布一个带快照的事件：铸造下一 tick → 更新 `current` → 广播。
    ///
    /// `kind`：`SNAPSHOT` = 完整快照刷新；`TRANSITION` = 状态过渡。返回发布的事件
    /// （含 minted tick），调用方可用于观测。**不做 lane 装饰**——伴生数据
    /// （stats/proxy_tun/...）由调用方在传入前组装进快照。
    pub fn publish(&self, kind: wire::RuntimeEventKind, snapshot: RuntimeSnapshot) -> RuntimeEvent {
        self.publish_composing(kind, |_| snapshot)
    }

    /// 与 [`Self::publish`] 同一发布路径，但快照由**铸造后的 tick** 组装。
    ///
    /// 供快照伴生数据必须与事件 tick 同轴同值的调用方使用（如 darwin Connected
    /// 首帧的 `sample_tick` 重铸：`stats.sample_tick == event.monotonic_tick`）。
    /// 组装闭包在 `publish_gate` 临界区内调用，铸造与组装原子发生。
    pub fn publish_composing(
        &self,
        kind: wire::RuntimeEventKind,
        compose: impl FnOnce(u64) -> RuntimeSnapshot,
    ) -> RuntimeEvent {
        let _publish_gate = self.publish_gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let tick = self.next_tick();
        let snapshot = compose(tick);
        let event = RuntimeEvent {
            monotonic_tick: tick,
            kind: kind as i32,
            snapshot: Some(snapshot.clone()),
            // R1：事件携带所属操作 id（镜像快照 operation_id）。
            operation_id: snapshot.operation_id,
        };
        *self.current.lock().unwrap() = Some(event.clone());
        // 无订阅者时 send 失败（无 receiver）——忽略；tick/current 已推进。
        let _ = self.live.send(event.clone());
        event
    }

    /// 订阅事件流（`WatchEvents` 语义）。
    ///
    /// `resume_tick == 0` → 先发当前快照 SNAPSHOT 事件（从当前开始）；
    /// `resume_tick > 0` 且已落后（当前 tick > resume）→ 重放当前快照（断线重放
    /// 语义：总线只保留最新快照，真事件日志属 P5）；随后转发 tick > resume 的
    /// 现场事件。
    ///
    /// BroadcastStream lagged 按 win32 现状静默丢（P1.5 挂账）。
    pub fn subscribe(&self, resume_tick: u64) -> impl Stream<Item = RuntimeEvent> + Send + 'static {
        let current = self
            .current
            .lock()
            .unwrap()
            .clone();
        let replay = match &current {
            Some(ev) if resume_tick == 0 || ev.monotonic_tick > resume_tick => Some(ev.clone()),
            _ => None,
        };
        let rx = self.live.subscribe();
        let live = BroadcastStream::new(rx).filter_map(move |ev| match ev {
            Ok(ev) if ev.monotonic_tick > resume_tick => Some(ev),
            // Lagged（订阅者落后超过通道容量）按 win32 现状静默丢——不终止流、
            // 不伪造错误；跨版本对齐处理挂账 P1.5。
            _ => None,
        });
        let init = replay.into_iter().collect::<Vec<_>>();
        tokio_stream::iter(init).chain(live)
    }

    // -----------------------------------------------------------------------
    // 统计 lane：与 wire 事件 lane 共存于同一总线（互不干扰；tick 对齐关联）。
    // -----------------------------------------------------------------------

    /// 发布一条归一化统计并更新 `stats_current`。若当前状态为已连接，则为该样本铸造
    /// 新 tick，并把携带该统计的 `SNAPSHOT` 重发到 `WatchEvents`；这使 UI 能持续
    /// 收到速率、累计量和会话起点，而不是只停在首次连接快照。其他状态不生成伪刷新，
    /// 样本沿用当前 tick。
    pub fn publish_stats(&self, mut stats: wire::RuntimeStats) -> wire::RuntimeStats {
        let _publish_gate = self.publish_gate.lock().unwrap();
        let current_snapshot = self.current_snapshot();
        let should_republish = matches!(
            current_snapshot.as_ref().and_then(|snapshot| snapshot.state.as_ref()),
            Some(wire::runtime_snapshot::State::Connected(_))
        );
        let tick = if should_republish {
            self.next_tick()
        } else {
            self.current_tick()
        };
        stats.sample_tick = tick;
        *self.stats_current.lock().unwrap() = Some(stats);
        // 无订阅者时 send 失败（无 receiver）——忽略；stats_current 已更新。
        let _ = self.stats_live.send(stats);

        // 统计随 RuntimeSnapshot 交付给 UI；若只更新内部 lane，已连接的 UI 会永久
        // 停在首次无样本快照。仅在 Connected 快照上重发，避免 Idle/Connecting 阶段
        // 因后台采样产生伪状态刷新。
        if let Some(snapshot) = current_snapshot.filter(|snapshot| {
            matches!(
                snapshot.state.as_ref(),
                Some(wire::runtime_snapshot::State::Connected(_))
            )
        }) {
            let mut snapshot = snapshot;
            snapshot.stats = Some(stats);
            let event = RuntimeEvent {
                monotonic_tick: tick,
                kind: wire::RuntimeEventKind::Snapshot as i32,
                snapshot: Some(snapshot.clone()),
                operation_id: snapshot.operation_id,
            };
            *self.current.lock().unwrap() = Some(event.clone());
            let _ = self.live.send(event);
        }
        stats
    }

    /// 最新归一化统计（无 → `None`）。
    #[must_use]
    pub fn current_stats(&self) -> Option<wire::RuntimeStats> {
        self.stats_current.lock().unwrap().clone()
    }

    /// 订阅归一化统计流（进程内消费）。
    ///
    /// `resume_tick == 0` 或落后 → 先发当前统计（含最新 `sample_tick`），随后转发
    /// 更新的统计。统计不独立铸造 tick，故 live 过滤以发布时刻的 `sample_tick` 判定。
    pub fn subscribe_stats(
        &self,
        resume_tick: u64,
    ) -> impl Stream<Item = wire::RuntimeStats> + Send + 'static {
        let current = self.stats_current.lock().unwrap().clone();
        let replay = match &current {
            Some(stats) if resume_tick == 0 || stats.sample_tick > resume_tick => Some(*stats),
            _ => None,
        };
        let rx = self.stats_live.subscribe();
        let live = BroadcastStream::new(rx).filter_map(move |stats| match stats {
            Ok(s) if s.sample_tick > resume_tick => Some(s),
            _ => None,
        });
        let init = replay.into_iter().collect::<Vec<_>>();
        tokio_stream::iter(init).chain(live)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 单元测试（自 win32 `kernel_control_service.rs` :8864+ 平移；snapshot 组装改为
// 本地 pure 辅助，不依赖 host composition）。
// ---------------------------------------------------------------------------
