
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use exv_vpn_wire::generated;
use generated::ConnectPhase;
use generated::StatsPhase;
use tokio::sync::mpsc;
use tonic::Status;

/// status 推送 mpsc 通道容量（低频事件，32 足够；满则不阻塞业务路径）。
const STREAM_CAPACITY: usize = 32;

/// 一次连接状态事件（独立 status 通道的领域形态；R1b 由真实数据面驱动）。
#[derive(Debug, Clone)]
pub struct StatusEvent {
    /// 16 字节 operation_id（= `OperationLookupKey.operation_id`；与 ApplyAccepted 关联）。
    pub operation_id: Vec<u8>,
    /// 细粒度 8 级连接阶段（common.proto `ConnectPhase`）。
    pub connect_phase: ConnectPhase,
    /// 粗粒度生命周期阶段（common.proto `StatsPhase`）。
    pub coarse_phase: StatsPhase,
    /// 失败时携带（coarse_phase = Failed）；成功时 `None`。
    pub error: Option<generated::VpnError>,
    /// engine 首次确认数据面可用的 wall-clock epoch 毫秒；只在 `Connected` 终态
    /// 非零。它是展示用真实会话起点，不参与授权、路由或资源所有权决策。
    pub session_established_at_ms: i64,
}

impl StatusEvent {
    /// 一个「连接进行中」事件（coarse = Connecting）。
    #[must_use]
    pub fn connecting(operation_id: Vec<u8>, connect_phase: ConnectPhase) -> Self {
        Self {
            operation_id,
            connect_phase,
            coarse_phase: StatsPhase::Connecting,
            error: None,
            session_established_at_ms: 0,
        }
    }

    /// 一个「已连接」终态事件（coarse = Connected，无错误）。
    #[must_use]
    pub fn connected(operation_id: Vec<u8>) -> Self {
        Self {
            operation_id,
            connect_phase: ConnectPhase::StartingDataPlane,
            coarse_phase: StatsPhase::Connected,
            error: None,
            session_established_at_ms: wall_clock_ms(),
        }
    }

    /// 一个「失败」终态事件（coarse = Failed，携带结构化错误）。
    #[must_use]
    pub fn failed(operation_id: Vec<u8>, connect_phase: ConnectPhase, error: generated::VpnError) -> Self {
        Self {
            operation_id,
            connect_phase,
            coarse_phase: StatsPhase::Failed,
            error: Some(error),
            session_established_at_ms: 0,
        }
    }

    /// 一个「已停止 / 回 idle」终态事件（coarse = Idle，无连接阶段语义）。
    #[must_use]
    pub fn idle(operation_id: Vec<u8>) -> Self {
        Self {
            operation_id,
            connect_phase: ConnectPhase::Unspecified,
            coarse_phase: StatsPhase::Idle,
            error: None,
            session_established_at_ms: 0,
        }
    }

    /// 转 wire `ConnectStatusEvent`。
    #[must_use]
    pub fn to_wire(&self) -> generated::ConnectStatusEvent {
        generated::ConnectStatusEvent {
            own_tunnel_if_index: 0,
            operation_id: self.operation_id.clone(),
            connect_phase: self.connect_phase as i32,
            coarse_phase: self.coarse_phase as i32,
            error: self.error.clone(),
            session_established_at_ms: self.session_established_at_ms,
        }
    }
}

/// 当前 UTC epoch 毫秒。系统时钟不可用或溢出时返回 0，保持 wire 中的“未知”约定。
fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

/// `StreamConnectStatus` 推送端内部可变态。
///
/// 注：不 derive `Debug`——内部终态观察者（`dyn Fn`）非 `Debug`。
struct StatusPublisherInner {
    /// 最近一次真实状态，用于查询及订阅恢复；不是凭据或操作回执。
    latest: Option<StatusEvent>,
    /// 当前挂接的 `StreamConnectStatus` 推送通道（`last-writer-wins`：core 是唯一消费方）。
    push: Option<mpsc::Sender<Result<generated::ConnectStatusEvent, Status>>>,
    /// engine 内部终态观察者（R2/P2-1）：`publish` 每次调用它；`grpc_server` 用它把
    /// 异步 apply 的运行时终态写回 `core.terminals`（`GetOperation(apply)` 可答真实
    /// 终态）。独立于推送通道（观察者是内部记账，不挂接外部流）。`None` = 未注册。
    observer: Option<Arc<dyn Fn(&StatusEvent) + Send + Sync>>,
}

/// engine 状态事件推送端点：持有推送通道（对齐 `StatsPublisher` 的 last-writer-wins）。
///
/// 注：不 derive `Debug`——内部终态观察者（`dyn Fn`）非 `Debug`。
pub struct StatusPublisher {
    inner: Mutex<StatusPublisherInner>,
    /// 观察者与快照/推送必须同序；观察者仍在 inner 锁外，允许查询最新状态。
    publish_order: Mutex<()>,
}

impl StatusPublisher {
    /// 建一个空推送端点（未挂接流；发布为 no-op；无内部观察者）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            publish_order: Mutex::new(()),
            inner: Mutex::new(StatusPublisherInner {
                latest: None,
                push: None,
                observer: None,
            }),
        }
    }

    /// 挂接一条 `StreamConnectStatus` 推送流，返回接收端（由 gRPC server 转成
    /// `ReceiverStream`）。注册为当前唯一推送通道（替换任何残留旧通道）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    #[must_use]
    pub fn open_stream(&self) -> mpsc::Receiver<Result<generated::ConnectStatusEvent, Status>> {
        let (tx, rx) = mpsc::channel(STREAM_CAPACITY);
        {
            let mut inner = self.inner.lock().expect("status publisher lock");
            if let Some(latest) = &inner.latest {
                let _ = tx.try_send(Ok(latest.to_wire()));
            }
            inner.push = Some(tx);
        }
        rx
    }

    /// 发布并保存最新状态；新订阅重放最新值，满队列关闭旧流以便重新订阅。
    ///
    /// R2/P2-1：无论是否挂接外部流，都先调用内部终态观察者（`grpc_server` 用它把
    /// 异步 apply 的运行时终态写回 `core.terminals`）。观察者在锁外调用（避免重入
    /// 持锁；观察者回调里只取 `core` 锁，与发布器锁无环）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    pub fn publish(&self, event: StatusEvent) {
        let _order = self.publish_order.lock().expect("status publish order");
        let observer = {
            let inner = self.inner.lock().expect("status publisher lock");
            if inner.latest.as_ref().is_some_and(|latest| {
                latest.operation_id == event.operation_id &&
                    matches!(latest.coarse_phase, StatsPhase::Failed | StatsPhase::Idle) &&
                    matches!(event.coarse_phase, StatsPhase::Connecting | StatsPhase::Connected)
            }) { return; }
            inner.observer.clone()
        };
        if let Some(observer) = observer {
            observer(&event);
        }
        let mut inner = self.inner.lock().expect("status publisher lock");
        inner.latest = Some(event.clone());
        if let Some(tx) = &inner.push {
            let wire = event.to_wire();
            if tx.try_send(Ok(wire)).is_err() {
                // 满队列不能静默丢终态：关掉本次流，Core EOF 后重订阅会重放最新值。
                inner.push = None;
            }
        }
    }

    pub fn latest(&self) -> Option<StatusEvent> {
        self.inner.lock().expect("status publisher lock").latest.clone()
    }

    /// 注册内部终态观察者（R2/P2-1）：每次 `publish` 调用一次（替换任何先前的观察者）。
    /// `grpc_server` 在构造时调用；观察者只做 engine 内部记账（写 `core.terminals`），
    /// 不得触碰推送通道（外部流仍由 core 唯一消费）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    pub fn set_terminal_observer(&self, observer: Arc<dyn Fn(&StatusEvent) + Send + Sync>) {
        let mut inner = self.inner.lock().expect("status publisher lock");
        inner.observer = Some(observer);
    }

    /// 是否挂接着推送通道（测试/观测）。
    ///
    /// # Panics
    /// 内部互斥锁中毒（另一线程持锁时 panic）→ panic。
    #[must_use]
    pub fn is_push_attached(&self) -> bool {
        self.inner.lock().expect("status publisher lock").push.is_some()
    }
}

impl Default for StatusPublisher {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 单元测试：事件形态 / 挂接 / 发布。
// 集成契约测试（StreamConnectStatus RPC over named pipe）在 tests/grpc_server.rs。
// ---------------------------------------------------------------------------
