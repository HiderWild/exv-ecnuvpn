//! 持续 L3 帧连接：接收宿主已验证 TLS 的流，管理握手、双向队列、心跳与停止。
//! 不解析资源或授予流权限；调用者须用每流授权结果组装 `DataRequest`。
//! 两个方向各使用 Session 的 QueueBudget，写侧预算包含在途帧；读侧另有一个
//! 最大线格式帧及 4096 字节解码余量。外部调用者持有的帧不计入连接内部预算。
use super::{
    L3Decoder, L3Frame,
    codec::encode_frame,
    handshake::{IpRequestError, IpRequestOptions, open_ipv4_tunnel},
};
use crate::{
    Secret,
    error::L3CodecError,
    session::{QueueBudget, Session},
};
use std::{
    net::Ipv4Addr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinHandle,
};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("L3 connection closed")]
    Closed,
    #[error("invalid L3 connection options")]
    InvalidOptions,
    #[error("L3 frame exceeds queue budget")]
    BudgetExceeded,
    #[error("unexpected L3 frame direction")]
    UnexpectedDirection,
    #[error("L3 heartbeat response timeout")]
    HeartbeatTimeout,
    #[error("L3 I/O failed: {0:?}")]
    Io(std::io::ErrorKind),
    #[error("L3 handshake: {0}")]
    Handshake(#[from] IpRequestError),
    #[error("L3 codec: {0}")]
    Codec(#[from] L3CodecError),
    #[error("L3 worker stopped unexpectedly")]
    WorkerStopped,
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectionOptions {
    pub handshake: IpRequestOptions,
    pub heartbeat_interval: Duration,
    pub heartbeat_miss_limit: u32,
}
impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            handshake: IpRequestOptions::default(),
            // 固定上游默认值；真实连续心跳仍需独立验证。
            heartbeat_interval: Duration::from_secs(5),
            heartbeat_miss_limit: 3,
        }
    }
}

struct Slot {
    _unit: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}
struct Queued<T> {
    value: T,
    _slot: Slot,
}
#[derive(Clone)]
struct Budget {
    limits: QueueBudget,
    units: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
}
impl Budget {
    fn new(limits: QueueBudget) -> Self {
        Self {
            limits,
            units: Arc::new(Semaphore::new(limits.max_units())),
            bytes: Arc::new(Semaphore::new(limits.max_bytes())),
        }
    }
    async fn acquire(&self, bytes: usize) -> Result<Slot, ConnectionError> {
        if bytes > self.limits.max_bytes() {
            return Err(ConnectionError::BudgetExceeded);
        }
        let count = u32::try_from(bytes).map_err(|_| ConnectionError::BudgetExceeded)?;
        let unit = self
            .units
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ConnectionError::Closed)?;
        let bytes = self
            .bytes
            .clone()
            .acquire_many_owned(count)
            .await
            .map_err(|_| ConnectionError::Closed)?;
        Ok(Slot {
            _unit: unit,
            _bytes: bytes,
        })
    }
}

/// 可克隆的发送入口；成功表示入队，写入失败由连接的 recv/close 返回。
#[derive(Clone)]
pub struct L3Sender {
    tx: mpsc::Sender<Queued<Zeroizing<Vec<u8>>>>,
    budget: Budget,
    lifecycle: Session,
}
impl L3Sender {
    /// 只接受上行认证/数据帧；心跳由连接维护。等待预算及队列时均可取消。
    /// # Errors
    /// 关闭、方向错误、帧超限或超出单条预算返回对应错误。
    pub async fn send(&self, frame: L3Frame) -> Result<(), ConnectionError> {
        if self.lifecycle.is_closed() {
            return Err(ConnectionError::Closed);
        }
        if !matches!(frame, L3Frame::AuthRequest { .. } | L3Frame::DataRequest(_)) {
            return Err(ConnectionError::UnexpectedDirection);
        }
        tokio::select! {
            biased;
            () = self.lifecycle.cancelled() => Err(ConnectionError::Closed),
            result = async {
                let slot = self.budget.acquire(frame_size(&frame)?).await?;
                let bytes = Zeroizing::new(encode_frame(&frame)?);
                self.tx.send(Queued { value: bytes, _slot: slot }).await.map_err(|_| ConnectionError::Closed)
            } => result,
        }
    }
}

/// 独占接收与生命周期句柄。显式 close 等待唯一工作任务释放流；Drop 取消任务。
pub struct L3Connection {
    vip: Ipv4Addr,
    sender: L3Sender,
    incoming: mpsc::Receiver<Queued<L3Frame>>,
    failure: Arc<Mutex<Option<ConnectionError>>>,
    worker: Option<JoinHandle<()>>,
}
impl L3Connection {
    /// 从宿主拥有且已验证的流建立持续连接，不自行拨号或改变证书策略。
    /// # Errors
    /// 无效配置、已关闭的会话、握手失败或握手被取消时返回错误。
    pub async fn open<S>(
        stream: S,
        sid: &Secret,
        lifecycle: Session,
        options: ConnectionOptions,
    ) -> Result<Self, ConnectionError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let limits = lifecycle.budget();
        if limits.max_units() == 0
            || limits.max_bytes() == 0
            || limits.max_units() > Semaphore::MAX_PERMITS
            || limits.max_bytes() > Semaphore::MAX_PERMITS
            || u32::try_from(limits.max_bytes()).is_err()
            || options.heartbeat_interval.is_zero()
            || options.heartbeat_miss_limit == 0
        {
            return Err(ConnectionError::InvalidOptions);
        }
        let (stream, vip) = tokio::select! {
            biased;
            () = lifecycle.cancelled() => return Err(ConnectionError::Closed),
            result = open_ipv4_tunnel(stream, sid, options.handshake) => result?,
        };
        let (tx, outgoing) = mpsc::channel(limits.max_units());
        let (incoming_tx, incoming) = mpsc::channel(limits.max_units());
        let failure = Arc::new(Mutex::new(None));
        let sender = L3Sender {
            tx,
            budget: Budget::new(limits),
            lifecycle: lifecycle.clone(),
        };
        let guard = CloseOnDrop(lifecycle);
        let failure_copy = Arc::clone(&failure);
        let worker = tokio::spawn(async move {
            let result = run(
                stream,
                outgoing,
                incoming_tx,
                Budget::new(limits),
                &guard.0,
                options,
            )
            .await;
            if let Err(error) = result {
                *failure_copy
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
            }
            drop(guard);
        });
        Ok(Self {
            vip,
            sender,
            incoming,
            failure,
            worker: Some(worker),
        })
    }
    #[must_use]
    pub fn vip(&self) -> Ipv4Addr {
        self.vip
    }
    #[must_use]
    pub fn sender(&self) -> L3Sender {
        self.sender.clone()
    }
    /// 现有连接会话的取消句柄，关闭它会停止本连接。
    #[must_use]
    pub fn session(&self) -> Session {
        self.sender.lifecycle.clone()
    }
    /// 返回下一条授权、数据或 VIP 更新帧；心跳在内部处理。主动关闭排空旧帧；
    /// 远端故障前已解码的完整帧先交付，再报告错误。
    /// # Errors
    /// 连接停止时返回其实际错误一次，之后返回 Closed。
    pub async fn recv(&mut self) -> Result<L3Frame, ConnectionError> {
        if self.sender.lifecycle.is_closed() {
            return self.terminal_frame();
        }
        tokio::select! {
            biased;
            () = self.sender.lifecycle.cancelled() => self.terminal_frame(),
            item = self.incoming.recv() => if let Some(item) = item { Ok(item.value) } else {
                    // read/write future 先丢发送端，外层任务随后发布错误并关闭会话。
                    self.sender.lifecycle.cancelled().await;
                    self.terminal_frame()
            },
        }
    }
    /// 幂等关闭，等待读写任务退出并排空队列。
    /// # Errors
    /// 返回尚未消费的连接错误或工作任务异常退出；主动取消本身成功。
    pub async fn close(&mut self) -> Result<(), ConnectionError> {
        let _ = self.sender.lifecycle.close();
        if let Some(worker) = self.worker.as_mut() {
            worker.await.map_err(|_| ConnectionError::WorkerStopped)?;
        }
        self.worker = None;
        self.incoming.close();
        while self.incoming.try_recv().is_ok() {}
        self.take_failure().map_or(Ok(()), Err)
    }
    fn take_failure(&self) -> Option<ConnectionError> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
    fn terminal_frame(&mut self) -> Result<L3Frame, ConnectionError> {
        let failed = self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        if failed && let Ok(item) = self.incoming.try_recv() {
            return Ok(item.value);
        }
        self.incoming.close();
        while self.incoming.try_recv().is_ok() {}
        Err(self.take_failure().unwrap_or(ConnectionError::Closed))
    }
}
impl Drop for L3Connection {
    fn drop(&mut self) {
        let _ = self.sender.lifecycle.close();
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}
struct CloseOnDrop(Session);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.close();
    }
}

#[derive(Default)]
struct Health {
    misses: AtomicU32,
    read_paused: AtomicBool,
}
async fn run<S>(
    stream: S,
    outgoing: mpsc::Receiver<Queued<Zeroizing<Vec<u8>>>>,
    incoming: mpsc::Sender<Queued<L3Frame>>,
    budget: Budget,
    lifecycle: &Session,
    options: ConnectionOptions,
) -> Result<(), ConnectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, write) = tokio::io::split(stream);
    let health = Health::default();
    tokio::select! {
        biased;
        () = lifecycle.cancelled() => Ok(()),
        result = read_loop(read, incoming, budget, &health) => result,
        result = write_loop(write, outgoing, &health, options) => result,
    }
}
async fn read_loop<R: AsyncRead + Unpin>(
    mut read: R,
    incoming: mpsc::Sender<Queued<L3Frame>>,
    budget: Budget,
    health: &Health,
) -> Result<(), ConnectionError> {
    let mut decoder = L3Decoder::new();
    let mut bytes = [0; 4096];
    loop {
        if let Some(frame) = decoder.decode()? {
            match frame {
                L3Frame::HeartbeatResponse { .. } => {
                    health.misses.store(0, Ordering::Relaxed);
                    continue;
                }
                L3Frame::AuthResponse { .. }
                | L3Frame::DataResponse { .. }
                | L3Frame::SecondVipResponse { .. } => {}
                _ => return Err(ConnectionError::UnexpectedDirection),
            }
            // 由本地消费者背压造成的暂停不能被报告为远端心跳故障。
            health.read_paused.store(true, Ordering::Relaxed);
            let slot = budget.acquire(frame_size(&frame)?).await?;
            incoming
                .send(Queued {
                    value: frame,
                    _slot: slot,
                })
                .await
                .map_err(|_| ConnectionError::Closed)?;
            health.read_paused.store(false, Ordering::Relaxed);
            continue;
        }
        let size = read
            .read(&mut bytes)
            .await
            .map_err(|e| ConnectionError::Io(e.kind()))?;
        if size == 0 {
            decoder.finish()?;
            return Err(ConnectionError::Io(std::io::ErrorKind::UnexpectedEof));
        }
        decoder.feed(&bytes[..size]);
    }
}
async fn write_loop<W: AsyncWrite + Unpin>(
    mut write: W,
    mut outgoing: mpsc::Receiver<Queued<Zeroizing<Vec<u8>>>>,
    health: &Health,
    options: ConnectionOptions,
) -> Result<(), ConnectionError> {
    let mut ticks = tokio::time::interval_at(
        tokio::time::Instant::now() + options.heartbeat_interval,
        options.heartbeat_interval,
    );
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            _ = ticks.tick() => {
                if health.read_paused.load(Ordering::Relaxed) { health.misses.store(0, Ordering::Relaxed); }
                else if health.misses.fetch_add(1, Ordering::Relaxed) >= options.heartbeat_miss_limit { return Err(ConnectionError::HeartbeatTimeout); }
                write_bytes(&mut write, &[5, 0x15, 0, 0]).await?;
            }
            item = outgoing.recv() => {
                let Some(item) = item else {return Ok(());};
                write_bytes(&mut write, &item.value).await?;
                // item 在 write/flush 完成后才释放预算。
                drop(item);
            }
        }
    }
}
async fn write_bytes<W: AsyncWrite + Unpin>(
    write: &mut W,
    bytes: &[u8],
) -> Result<(), ConnectionError> {
    write
        .write_all(bytes)
        .await
        .map_err(|e| ConnectionError::Io(e.kind()))?;
    write
        .flush()
        .await
        .map_err(|e| ConnectionError::Io(e.kind()))
}
fn frame_size(frame: &L3Frame) -> Result<usize, ConnectionError> {
    let size = match frame {
        L3Frame::AuthRequest { payload }
        | L3Frame::DataResponse { payload }
        | L3Frame::HeartbeatResponse { payload } => payload.len().checked_add(4),
        L3Frame::AuthResponse { payload, .. } | L3Frame::SecondVipResponse { payload, .. } => {
            payload.len().checked_add(5)
        }
        L3Frame::DataRequest(data) => data.packets.iter().try_fold(
            6usize
                .checked_add(data.token.len())
                .ok_or(ConnectionError::BudgetExceeded)?,
            |size, packet| size.checked_add(2)?.checked_add(packet.len()),
        ),
        L3Frame::HeartbeatRequest => Some(4),
    };
    size.ok_or(ConnectionError::BudgetExceeded)
}
