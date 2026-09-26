//! 已认证 Unix stream 到 tonic server 的纯桥接。
//!
//! 本模块不认证 peer，也不打开 socket。调用方必须先完成 Darwin peer credential 与
//! `DarwinAuthV1` 校验，再构造这里的类型。tonic handler 只会从 [`DarwinConnectInfo`]
//! request extension 读取已经验证的远端 uid/pid。

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tonic::codegen::tokio_stream::Stream;
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::transport::server::Connected;

use crate::{auth::PreauthError, listener::AuthenticatedConnection};

/// tonic request extension 中的 Darwin 认证 peer。
///
/// 字段只来自认证车道已经核验的 OS peer credential，不能从 protobuf 字段构造。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DarwinConnectInfo {
    uid: u32,
    pid: u32,
}

impl DarwinConnectInfo {
    /// 从认证车道已经核验的 OS uid/pid 建立 metadata。
    ///
    /// 该构造器只对当前 crate 可见，避免未认证的外部调用方伪造 tonic extension。
    pub(crate) const fn from_verified_peer(uid: u32, pid: u32) -> Self {
        Self { uid, pid }
    }

    /// 已认证远端进程的 uid。
    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// 已认证远端进程的 pid。
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.pid
    }
}

/// 已完成认证、可以交给 tonic 的单条 Unix stream。
///
/// 本类型不执行认证；它只保留认证结果、转发 Tokio I/O，并通过 [`Connected`] 把
/// [`DarwinConnectInfo`] 注入每个 server request。
#[derive(Debug)]
pub struct AuthenticatedUnixStream {
    io: UnixStream,
    peer: DarwinConnectInfo,
    delivery_ack: Option<oneshot::Sender<()>>,
    // 必须排在 `io` 后面：Rust 按字段声明顺序析构，确保通知发生时底层 UDS 已释放。
    close_notifier: AuthenticatedStreamCloseNotifier,
}

impl AuthenticatedUnixStream {
    /// 包装认证车道交付的 stream 与 OS peer metadata。
    pub(crate) const fn from_verified_peer(io: UnixStream, peer: DarwinConnectInfo) -> Self {
        Self {
            io,
            peer,
            delivery_ack: None,
            close_notifier: AuthenticatedStreamCloseNotifier::none(),
        }
    }

    /// W2.5 service 形态：包装 uid 门验证过的 stream（无 HMAC 车道）。
    ///
    /// 与 [`Self::from_verified_peer`] 的唯一差异是准入车道——peer 事实同样来自
    /// `ServiceListener::accept_uid_gated` 的 OS credential，不接受调用方自报字段；
    /// 服务形态连接不需要 close 信号装饰（会话循环持有 stream 生命周期）。
    pub(crate) fn from_verified_service_peer(
        io: UnixStream,
        peer: crate::peer::VerifiedLocalPeer,
    ) -> Self {
        Self::from_verified_peer(
            io,
            DarwinConnectInfo::from_verified_peer(peer.uid(), peer.pid()),
        )
    }

    /// 返回已经验证的远端 uid/pid。
    #[must_use]
    pub const fn peer(&self) -> DarwinConnectInfo {
        self.peer
    }

    fn with_close_signal(mut self, close_sender: oneshot::Sender<()>) -> Self {
        self.close_notifier = AuthenticatedStreamCloseNotifier::new(close_sender);
        self
    }

    fn with_delivery_ack(mut self, delivery_sender: oneshot::Sender<()>) -> Self {
        self.delivery_ack = Some(delivery_sender);
        self
    }

    fn acknowledge_delivery(&mut self) {
        if let Some(delivery_sender) = self.delivery_ack.take() {
            let _ = delivery_sender.send(());
        }
    }
}

/// 已成功 handoff 的认证连接关闭通知。
///
/// 此 Future 只会在对应 [`AuthenticatedUnixStream`] 被实际析构后完成；它不会主动关闭
/// 连接，也不能由调用方触发。若将一个尚未完成的 `&mut` 借用交给可取消的等待（例如
/// `timeout`）后取消，仍可继续等待本 handle。
///
/// 与 Tokio 的 one-shot receiver 一样，本 handle 只能完成一次。`Err` 表示内部 sender
/// 未发送通知即被丢弃，属于 bridge 实现错误或异常终止，不能解释为远端正常 EOF。
#[must_use = "成功 handoff 后必须持有或等待关闭通知，才能维护连接生命周期"]
pub struct AuthenticatedConnectionCloseSignal {
    receiver: oneshot::Receiver<()>,
}

impl std::fmt::Debug for AuthenticatedConnectionCloseSignal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedConnectionCloseSignal")
            .finish_non_exhaustive()
    }
}

impl std::future::Future for AuthenticatedConnectionCloseSignal {
    type Output = Result<(), oneshot::error::RecvError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().receiver).poll(context)
    }
}

/// stream 析构时只发送一次的私有通知器。
///
/// 它必须是 [`AuthenticatedUnixStream`] 的最后一个字段，使 `UnixStream` 先析构，再发出关闭
/// 通知。通知 receiver 被调用方提前丢弃时，发送失败是预期且无副作用的。
#[derive(Debug)]
struct AuthenticatedStreamCloseNotifier {
    sender: Option<oneshot::Sender<()>>,
}

impl AuthenticatedStreamCloseNotifier {
    const fn none() -> Self {
        Self { sender: None }
    }

    const fn new(sender: oneshot::Sender<()>) -> Self {
        Self {
            sender: Some(sender),
        }
    }
}

impl Drop for AuthenticatedStreamCloseNotifier {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
    }
}

impl AuthenticatedConnection {
    /// 把同一条已认证 UDS 交给 tonic server。
    ///
    /// 此转换只能消费 [`AuthenticatedConnection`]；request extension 的 peer metadata
    /// 直接取自认证阶段保存的 `binding.remote_peer`，不会读取 protobuf 或调用方参数。
    #[must_use]
    pub fn into_authenticated_unix_stream(self) -> AuthenticatedUnixStream {
        let remote_peer = self.binding().remote_peer;
        AuthenticatedUnixStream::from_verified_peer(
            self.into_stream(),
            DarwinConnectInfo::from_verified_peer(remote_peer.uid(), remote_peer.pid()),
        )
    }
}

impl Connected for AuthenticatedUnixStream {
    type ConnectInfo = DarwinConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.peer
    }
}

impl AsyncRead for AuthenticatedUnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_read(context, buffer)
    }
}

impl AsyncWrite for AuthenticatedUnixStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().io).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(context)
    }
}

/// `tonic::transport::Server::serve_with_incoming` 可直接消费的认证连接流。
///
/// 认证失败由 listener/auth 车道在发送前丢弃，绝不能 enqueue。本流的 `Some(Err)` 只表示
/// 一次可恢复的 accept/incoming I/O 错误，下一条已认证 stream 仍可产出；只有所有 sender
/// 关闭、底层 receiver 返回 `None` 才表示 incoming EOF。
#[derive(Debug)]
pub struct AuthenticatedIncoming {
    inner: ReceiverStream<Result<AuthenticatedUnixStream, io::Error>>,
}

/// 向 tonic server 交付已认证连接的唯一 sender。
///
/// 其内部 channel 不接受裸 UDS、调用方自报 peer 或认证失败错误；只能消费
/// [`AuthenticatedConnection`] 并由 crate 从其 `binding().remote_peer` 建立 extension metadata。
#[derive(Clone, Debug)]
pub struct AuthenticatedIncomingSender {
    inner: mpsc::Sender<Result<AuthenticatedUnixStream, io::Error>>,
}

impl AuthenticatedIncoming {
    /// 创建只接收已认证连接的 tonic incoming 和配对 handoff sender。
    ///
    /// 容量为零时仍创建一个最小有界队列，避免测试或调用方意外触发 Tokio panic。
    #[must_use]
    pub fn channel(capacity: usize) -> (AuthenticatedIncomingSender, Self) {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        (
            AuthenticatedIncomingSender { inner: sender },
            Self::from_receiver(receiver),
        )
    }

    /// 把认证 accept loop 的有界接收端包装为 tonic incoming。
    pub(crate) fn from_receiver(
        receiver: mpsc::Receiver<Result<AuthenticatedUnixStream, io::Error>>,
    ) -> Self {
        Self {
            inner: ReceiverStream::new(receiver),
        }
    }
}

impl AuthenticatedIncomingSender {
    /// 将同一条已认证 UDS 交付给 tonic server。
    ///
    /// # Errors
    ///
    /// 当 server incoming 已关闭时返回 `Transport`；认证失败从不会进入本 sender。
    pub async fn handoff(&self, connection: AuthenticatedConnection) -> Result<(), PreauthError> {
        self.inner
            .send(Ok(connection.into_authenticated_unix_stream()))
            .await
            .map_err(|_| PreauthError::Transport)
    }

    /// 将同一条已认证 UDS 交给 tonic server，并返回其实际关闭的一次性通知。
    ///
    /// 只有 [`AuthenticatedIncoming`] 已实际取走本次 stream 后才返回 close handle；因此
    /// 认证前失败、已关闭 incoming，或入队后 server 尚未取走即关闭的 handoff 都不会向 Core
    /// 交付伪造的关闭通知。通知由 stream 析构触发，既不改变 wire，也不会发起重连。
    ///
    /// # Errors
    ///
    /// 当 server incoming 已关闭，或在取走本次 stream 前关闭时返回 `Transport`，且不会创建
    /// close handle。
    pub async fn handoff_with_close_signal(
        &self,
        connection: AuthenticatedConnection,
    ) -> Result<AuthenticatedConnectionCloseSignal, PreauthError> {
        // 先 reserve，确保一个已经关闭的 incoming 不会得到 signal 或转换连接。拿到 permit
        // 后 `send` 无失败返回，且这一条消息已经占用队列容量；delivery ack 再排除 receiver
        // 在 reserve 与实际 poll 之间关闭的竞态。
        let permit = self
            .inner
            .reserve()
            .await
            .map_err(|_| PreauthError::Transport)?;
        let (delivery_sender, delivery_receiver) = oneshot::channel();
        let (close_sender, close_receiver) = oneshot::channel();
        let stream = connection
            .into_authenticated_unix_stream()
            .with_delivery_ack(delivery_sender)
            .with_close_signal(close_sender);
        permit.send(Ok(stream));

        delivery_receiver
            .await
            .map_err(|_| PreauthError::Transport)?;

        Ok(AuthenticatedConnectionCloseSignal {
            receiver: close_receiver,
        })
    }

    /// W2.5 service 形态：把 uid 门验证过的 stream 交给 tonic server 并返回关闭通知。
    ///
    /// 与 [`Self::handoff_with_close_signal`] 同一交付/竞态纪律，唯一差异是准入
    /// 车道——stream 已由 `ServiceListener::accept_uid_gated` 完成 OS credential
    /// 验证（无 HMAC 车道）。
    ///
    /// # Errors
    ///
    /// 当 server incoming 已关闭，或在取走本次 stream 前关闭时返回 `Transport`。
    pub async fn handoff_service_stream_with_close_signal(
        &self,
        stream: AuthenticatedUnixStream,
    ) -> Result<AuthenticatedConnectionCloseSignal, PreauthError> {
        let permit = self
            .inner
            .reserve()
            .await
            .map_err(|_| PreauthError::Transport)?;
        let (delivery_sender, delivery_receiver) = oneshot::channel();
        let (close_sender, close_receiver) = oneshot::channel();
        permit.send(Ok(stream.with_delivery_ack(delivery_sender).with_close_signal(close_sender)));
        delivery_receiver
            .await
            .map_err(|_| PreauthError::Transport)?;
        Ok(AuthenticatedConnectionCloseSignal {
            receiver: close_receiver,
        })
    }

    /// W2.5 service 形态：把 uid 门验证过的 stream 交给 tonic server（无关闭信号；
    /// busy 拒绝连接等无需观察关闭的交付点）。
    ///
    /// # Errors
    ///
    /// 当 server incoming 已关闭时返回 `Transport`。
    pub async fn handoff_service_stream(
        &self,
        stream: AuthenticatedUnixStream,
    ) -> Result<(), PreauthError> {
        self.inner
            .send(Ok(stream))
            .await
            .map_err(|_| PreauthError::Transport)
    }
}

impl Stream for AuthenticatedIncoming {
    type Item = Result<AuthenticatedUnixStream, io::Error>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.get_mut().inner).poll_next(context) {
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(Some(Ok(mut stream))) => {
                stream.acknowledge_delivery();
                Poll::Ready(Some(Ok(stream)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
