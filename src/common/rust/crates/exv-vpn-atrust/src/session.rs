//! 会话关闭语义：幂等关闭、关闭后排空、迟到结果无法进入新会话。
//!
//! 这是纯逻辑契约，不涉及真实网络。计划 §3.1 要求“会话关闭”幂等终止心跳、刷新、所有节点
//! 连接和排队数据，且旧连接结果不能进入新会话；本模块把该契约固定成可测试的状态机，供
//! T06/T09 的会话循环直接依赖：
//!
//! - **重复关闭**：`close` 以互斥状态为唯一权威，第一次调用返回
//!   [`CloseOutcome::Closed`]，后续调用返回 [`CloseOutcome::AlreadyClosed`]，不产生第二次副作用。
//! - **关闭后排空**：第一次关闭在同一临界区内丢弃排队数据并随结果返回；关闭后 `enqueue`
//!   一律拒绝，`drain` 也不会再拿到数据。
//! - **有界背压**：队列带显式的 [`QueueBudget`]（条数 + 字节）上限，超预算的 [`Session::enqueue`]
//!   返回 [`SessionError::QueueBudgetExceeded`] 且不改变队列状态（拒绝而不是无限增长）。
//!   该预算同时被连接层复用：`crate::transport::l3::connection` 的读、写两个方向各持一份，
//!   写侧在途帧持续占用预算；帧级水位属于那里，不在本模块。
//! - **迟到结果 fence**：结果用 [`TaggedResult`] 携带所属 `SessionId`；[`Session::admit`]
//!   只接受属于本会话的结果，其它会话（尤其是上一个会话）的结果被拒绝。配合
//!   [`SessionFactory`] 的单调 ID，新会话不可能接受旧会话的结果。
//! - **取消**：[`Session::cancelled`] 在关闭时完成，宿主可以用它中断等待中的任务。
//!
//! 关闭的**执行**（停心跳、关节点连接、撤销宿主网络资源）属于宿主与会话循环；本模块只提供
//! 单一权威的关闭状态和 fence，不自己去碰网络。

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;

use crate::error::SessionError;

/// 会话排队的显式预算：条数上限 + 字节上限（设计 §3.1/§4.3「有界背压」）。
///
/// 两个上限对一条入队都必须是“接受后不超过”，否则 [`Session::enqueue`] 返回
/// [`SessionError::QueueBudgetExceeded`]。语义参考同仓 CSTP mux 的 `data_byte_budget`
/// / `DataBudgetExceeded`：预算是**显式参数**，不是“越大越好”的实现细节——连接层用同一份预算
/// 驱动读写水位。
///
/// 取值口径：条数按“排队单元个数”（不是字节）算，字节按载荷长度之和算；已 `drain` 或随关闭
/// 排空的单元立即释放预算。`crate::transport::l3::connection` 的读、写两个方向各持一份预算，
/// 写侧在途帧一直到写出都占用预算（CSTP mux 的 `data_byte_budget` 同样把 in-flight 计入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueBudget {
    max_units: usize,
    max_bytes: usize,
}

impl QueueBudget {
    /// 默认预算：4096 个单元 / 8 MiB。
    ///
    /// 取值的用途是“挡住无界增长”，不是流量整形：连接层与宿主应按真实节点吞吐与延迟预算显式设置。
    pub const DEFAULT: Self = Self {
        max_units: 4096,
        max_bytes: 8 * 1024 * 1024,
    };

    /// 显式预算。两个上限都允许为 0：那表示该维度不允许任何排队（入队即被拒绝）。
    #[must_use]
    pub const fn new(max_units: usize, max_bytes: usize) -> Self {
        Self {
            max_units,
            max_bytes,
        }
    }

    /// 条数上限。
    #[must_use]
    pub const fn max_units(self) -> usize {
        self.max_units
    }

    /// 字节上限。
    #[must_use]
    pub const fn max_bytes(self) -> usize {
        self.max_bytes
    }
}

impl Default for QueueBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 会话标识。由 [`SessionFactory`] 单调分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(u64);

impl SessionId {
    /// 原始值，用于日志与关联 ID（不含敏感材料）。
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session#{}", self.0)
    }
}

/// 单调分配会话 ID 的工厂。
///
/// 新会话总是拿到比之前更大的 ID，因此“旧会话的结果进入新会话”在 ID 层面就不可能成立，
/// 剩下的判断由 [`Session::admit`] 完成。
#[derive(Debug)]
pub struct SessionFactory {
    next: AtomicU64,
}

impl Default for SessionFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionFactory {
    /// 从 1 开始分配。
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
        }
    }

    /// 开一个新会话（排队预算取 [`QueueBudget::DEFAULT`]）。
    pub fn open(&self) -> Session {
        self.open_with_budget(QueueBudget::DEFAULT)
    }

    /// 开一个使用指定排队预算的新会话。
    pub fn open_with_budget(&self, budget: QueueBudget) -> Session {
        Session::with_budget(
            SessionId(self.next.fetch_add(1, Ordering::Relaxed)),
            budget,
        )
    }
}

/// 排队等待发送的工作单元（例如待写出的帧）。
///
/// `Debug` 只打印长度：载荷可能承载用户流量，不进入日志。
///
/// 载荷是**裸 `Vec<u8>` 且可 `Clone`**（数据面重连重发需要克隆），不进入
/// [`crate::error::Secret`]、不做零化；边界说明见 [`crate::error`] 模块文档。
#[derive(Clone, PartialEq, Eq)]
pub struct PendingUnit {
    sequence: u64,
    payload: Vec<u8>,
}

impl PendingUnit {
    /// 会话内单调序号。
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// 载荷。
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// 载荷长度。
    #[must_use]
    pub fn len(&self) -> usize {
        self.payload.len()
    }

    /// 载荷是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.payload.is_empty()
    }
}

impl fmt::Debug for PendingUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingUnit")
            .field("sequence", &self.sequence)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

/// `close` 的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// 本次调用是关闭的第一责任人：排队数据已在关闭时排空并随结果返回。
    Closed {
        /// 被丢弃的排队数据，按入队顺序。
        drained: Vec<PendingUnit>,
    },
    /// 会话此前已经关闭；重复关闭没有第二次副作用，也不再返回数据。
    AlreadyClosed,
}

impl CloseOutcome {
    /// 本次调用是否是第一次关闭。
    #[must_use]
    pub fn is_first_close(&self) -> bool {
        matches!(self, Self::Closed { .. })
    }

    /// 被排空的排队数据；重复关闭时为空（第一次关闭已经取走）。
    #[must_use]
    pub fn drained(&self) -> &[PendingUnit] {
        match self {
            Self::Closed { drained } => drained,
            Self::AlreadyClosed => &[],
        }
    }
}

/// 带会话归属的结果：只有属于本会话的结果才能被本会话消费。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResult<T> {
    session: SessionId,
    value: T,
}

impl<T> TaggedResult<T> {
    /// 打上归属标记。
    pub fn new(session: SessionId, value: T) -> Self {
        Self { session, value }
    }

    /// 归属会话。
    #[must_use]
    pub fn session(&self) -> SessionId {
        self.session
    }

    /// 取出结果值。
    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }
}

#[derive(Debug, Default)]
struct SessionState {
    closed: bool,
    queue: VecDeque<PendingUnit>,
    queued_bytes: usize,
    next_sequence: u64,
}

#[derive(Debug)]
struct SessionInner {
    id: SessionId,
    state: Mutex<SessionState>,
    budget: QueueBudget,
    closed_signal: watch::Sender<bool>,
}

/// 一个会话的句柄（可克隆，共享同一关闭状态）。
#[derive(Debug, Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

impl Session {
    /// 用指定 ID 构造，排队预算取 [`QueueBudget::DEFAULT`]。ID 的分配交给 [`SessionFactory`]。
    #[must_use]
    pub fn new(id: SessionId) -> Self {
        Self::with_budget(id, QueueBudget::DEFAULT)
    }

    /// 用指定 ID 与排队预算构造。ID 的分配交给 [`SessionFactory`]。
    #[must_use]
    pub fn with_budget(id: SessionId, budget: QueueBudget) -> Self {
        let (closed_signal, _receiver) = watch::channel(false);
        Self {
            inner: Arc::new(SessionInner {
                id,
                state: Mutex::new(SessionState::default()),
                budget,
                closed_signal,
            }),
        }
    }

    /// 会话 ID。
    #[must_use]
    pub fn id(&self) -> SessionId {
        self.inner.id
    }

    /// 本会话的排队预算。
    #[must_use]
    pub fn budget(&self) -> QueueBudget {
        self.inner.budget
    }

    /// 是否已关闭。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.lock_state().closed
    }

    /// 入队一个待发送单元，返回会话内单调序号。
    ///
    /// # Errors
    ///
    /// 会话已关闭时返回 [`SessionError::Closed`]；接受本次入队会超过 [`QueueBudget`] 时返回
    /// [`SessionError::QueueBudgetExceeded`]，两种拒绝都不改动队列（序号也不会被消耗）。
    pub fn enqueue(&self, payload: Vec<u8>) -> Result<u64, SessionError> {
        let mut state = self.lock_state();
        if state.closed {
            return Err(SessionError::Closed {
                session: self.inner.id.get(),
            });
        }
        let would_hold_units = state.queue.len() + 1;
        let would_hold_bytes = state.queued_bytes + payload.len();
        if would_hold_units > self.inner.budget.max_units()
            || would_hold_bytes > self.inner.budget.max_bytes()
        {
            return Err(SessionError::QueueBudgetExceeded {
                session: self.inner.id.get(),
                would_hold_units,
                would_hold_bytes,
                max_units: self.inner.budget.max_units(),
                max_bytes: self.inner.budget.max_bytes(),
            });
        }
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        state.queued_bytes = would_hold_bytes;
        state.queue.push_back(PendingUnit { sequence, payload });
        Ok(sequence)
    }

    /// 当前排队数量。
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.lock_state().queue.len()
    }

    /// 当前排队数据的字节数（`drain` 与 `close` 之后归零）。
    #[must_use]
    pub fn queued_bytes(&self) -> usize {
        self.lock_state().queued_bytes
    }

    /// 取走全部排队数据（同时释放它们占用的预算）。
    ///
    /// 关闭后调用返回空：第一次 `close` 已经把队列排空。
    #[must_use]
    pub fn drain(&self) -> Vec<PendingUnit> {
        let mut state = self.lock_state();
        let drained = state.queue.drain(..).collect();
        state.queued_bytes = 0;
        drained
    }

    /// 关闭会话（幂等）。
    ///
    /// 第一次调用在同一个临界区里标记关闭、排空队列并发出取消信号；重复调用直接返回
    /// [`CloseOutcome::AlreadyClosed`]。
    #[must_use]
    pub fn close(&self) -> CloseOutcome {
        let drained = {
            let mut state = self.lock_state();
            if state.closed {
                return CloseOutcome::AlreadyClosed;
            }
            state.closed = true;
            let drained = state.queue.drain(..).collect::<Vec<_>>();
            state.queued_bytes = 0;
            drained
        };
        // 无接收者时 send_replace 仍然成功（watch 的语义），因此关闭信号不会因为
        // 调用点还没开始等待而丢失。
        let _ = self.inner.closed_signal.send_replace(true);
        CloseOutcome::Closed { drained }
    }

    /// 等待会话被关闭（取消信号）。
    pub async fn cancelled(&self) {
        let mut receiver = self.inner.closed_signal.subscribe();
        if *receiver.borrow_and_update() {
            return;
        }
        // 关闭信号发出后返回；发送者被丢弃（会话句柄全部消失）同样视为已结束。
        let _ = receiver.changed().await;
    }

    /// 消费一个带归属的结果。
    ///
    /// # Errors
    ///
    /// 结果属于其它会话时返回 [`SessionError::StaleResult`]；本会话已关闭时返回
    /// [`SessionError::Closed`]。
    pub fn admit<T>(&self, result: TaggedResult<T>) -> Result<T, SessionError> {
        if result.session != self.inner.id {
            return Err(SessionError::StaleResult {
                belongs_to: result.session.get(),
                offered_to: self.inner.id.get(),
            });
        }
        if self.is_closed() {
            return Err(SessionError::Closed {
                session: self.inner.id.get(),
            });
        }
        Ok(result.value)
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

