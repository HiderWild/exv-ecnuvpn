
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use exv_vpn_wire::generated;
use generated::ConnectPhase;
use generated::StatsPhase;
use tokio::sync::mpsc;
use tonic::Status;

/// status 推送 mpsc 通道容量（低频事件，32 足够；满则不阻塞业务路径）。
const STREAM_CAPACITY: usize = 32;

/// 推送通道发送端类型（wire 事件 + tonic 状态的 Result 载体）。
type StatusPushSender = mpsc::Sender<Result<generated::ConnectStatusEvent, Status>>;

/// 内部终态观察者类型（R2/P2-1：`grpc_server` 注册的 engine 内部记账回调）。
type StatusTerminalObserver = Arc<dyn Fn(&StatusEvent) + Send + Sync>;

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
    /// T5 善后验证结论（EXV_UNFREEZE tag 7）：只在 Idle 终态事件携带语义——
    /// true = 断开/善后已复验干净（痕已删，core/UI 可清除本地会话状态）；
    /// false = 未验证 / 残留 / 不适用。非 Idle 事件恒 false。
    pub aftermath_verified: bool,
    /// T6a R1 重连进度遥测（EXV_UNFREEZE tag 8）：只在 engine 自愈事件
    /// （reconnecting / reconnected / R1 终态）携带；`None` = 无重连语义。
    pub reconnect_state: Option<ReconnectState>,
}

/// T6a R1 重连进度遥测（`ConnectStatusEvent.reconnect_state` 的领域形态）。
///
/// 稳定 reason id 契约见 [`crate::reconnect::reason`]；只作 UI 文案路由，
/// 绝不是授权材料。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectState {
    /// 本次事件描述的第几次尝试（1 起）。
    pub attempt: u32,
    /// 下次重试的 wall-clock epoch 毫秒；0 = 无（尝试在途 / 已恢复 / 终态）。
    pub next_retry_at_ms: i64,
    /// 当前重连步骤的稳定 reason id（空串 = 不适用）。
    pub reason: &'static str,
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
            aftermath_verified: false,
            reconnect_state: None,
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
            aftermath_verified: false,
            reconnect_state: None,
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
            aftermath_verified: false,
            reconnect_state: None,
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
            aftermath_verified: false,
            reconnect_state: None,
        }
    }

    /// 一个「已停止 / 回 idle」终态事件 + T5 善后验证结论（EXV_UNFREEZE tag 7）。
    ///
    /// 断开路径（StopTunnel）与 engine 兜底善后（core 死亡 teardown / 崩溃环 /
    /// 无头 R1 终态）完成验证后发布：`aftermath_verified` = 复验是否干净
    /// （干净时连接痕迹已删——core 侧删除权前提"善后已验证"由此满足）。
    #[must_use]
    pub fn idle_with_aftermath(operation_id: Vec<u8>, aftermath_verified: bool) -> Self {
        Self {
            aftermath_verified,
            ..Self::idle(operation_id)
        }
    }

    /// T6a：一个「重连进行中」事件（coarse = Connecting + 重连遥测 tag 8）。
    ///
    /// `next_retry_at_ms > 0` = 退避已排定（事件预告下一次尝试）；`= 0` = 该次
    /// 尝试在途。携带新铸造的尝试 `operation_id`。
    #[must_use]
    pub fn reconnecting(
        operation_id: Vec<u8>,
        attempt: u32,
        reason: &'static str,
        next_retry_at_ms: i64,
    ) -> Self {
        Self {
            connect_phase: ConnectPhase::ConnectingControl,
            reconnect_state: Some(ReconnectState {
                attempt,
                next_retry_at_ms,
                reason,
            }),
            ..Self::connecting(operation_id, ConnectPhase::ConnectingControl)
        }
    }

    /// T6a：一个「重连成功」事件（coarse = Connected + 重连遥测 tag 8；携带
    /// 成功的尝试序号——组装路径自己的 Connected 事件不带重连语义，本事件补充）。
    #[must_use]
    pub fn reconnected(operation_id: Vec<u8>, attempt: u32) -> Self {
        Self {
            reconnect_state: Some(ReconnectState {
                attempt,
                next_retry_at_ms: 0,
                reason: "healed",
            }),
            ..Self::connected(operation_id)
        }
    }

    /// T6a：一个「R1 终态失败」事件（coarse = Failed + 重连遥测 tag 8——
    /// 认证终态失效或尝试耗尽，不循环；core 在场由 core 驱动后续善后）。
    #[must_use]
    pub fn reconnect_terminal(
        operation_id: Vec<u8>,
        attempt: u32,
        reason: &'static str,
        error: generated::VpnError,
    ) -> Self {
        Self {
            reconnect_state: Some(ReconnectState {
                attempt,
                next_retry_at_ms: 0,
                reason,
            }),
            ..Self::failed(operation_id, ConnectPhase::ConnectingControl, error)
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
            aftermath_verified: self.aftermath_verified,
            reconnect_state: self.reconnect_state.map(|state| generated::ReconnectState {
                attempt: state.attempt,
                next_retry_at_ms: state.next_retry_at_ms,
                reason: state.reason.to_owned(),
            }),
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
    push: Option<StatusPushSender>,
    /// engine 内部终态观察者（R2/P2-1）：`publish` 每次调用它；`grpc_server` 用它把
    /// 异步 apply 的运行时终态写回 `core.terminals`（`GetOperation(apply)` 可答真实
    /// 终态）。独立于推送通道（观察者是内部记账，不挂接外部流）。`None` = 未注册。
    observer: Option<StatusTerminalObserver>,
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
    pub fn set_terminal_observer(&self, observer: StatusTerminalObserver) {
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
