//! tonic client 对已认证 Unix stream 的一次性 connector。
//!
//! 本模块刻意不保存 UDS path，也不调用 `UnixStream::connect`。因此 channel 只能消费认证
//! 车道交付的 warm stream；tonic 试图重连时稳定失败，不能经内建 `unix:` connector 绕过
//! peer credential 或 `DarwinAuthV1`。

use std::fmt;
use std::future::{Ready, ready};
use std::task::{Context, Poll};

use hyper_util::rt::TokioIo;
use tonic::codegen::Service;
use tonic::codegen::http::Uri;
use tonic::transport::{Channel, Endpoint};

use crate::{
    auth::{AuthKey, PreauthError, authenticate_client},
    peer::{ExpectedPeer, PeerLookup},
    tonic_bridge::{AuthenticatedUnixStream, DarwinConnectInfo},
};
use tokio::net::UnixStream;

/// warm stream 已被 channel 消费后的稳定错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarmStreamConsumed;

impl fmt::Display for WarmStreamConsumed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("authenticated Unix stream already consumed; automatic redial is forbidden")
    }
}

impl std::error::Error for WarmStreamConsumed {}

/// 只交付一次预先认证 stream 的 tonic connector。
#[derive(Debug)]
pub(crate) struct WarmAuthenticatedConnector {
    stream: Option<AuthenticatedUnixStream>,
}

impl WarmAuthenticatedConnector {
    pub(crate) const fn new(stream: AuthenticatedUnixStream) -> Self {
        Self {
            stream: Some(stream),
        }
    }
}

impl Service<Uri> for WarmAuthenticatedConnector {
    type Response = TokioIo<AuthenticatedUnixStream>;
    type Error = WarmStreamConsumed;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        ready(
            self.stream
                .take()
                .map(TokioIo::new)
                .ok_or(WarmStreamConsumed),
        )
    }
}

/// 用已经认证的唯一 stream 建立 tonic channel。
///
/// 返回值同时保留认证车道确认的 server uid/pid。connector 不持有 path，channel 断开后
/// 不会自动重拨；上层必须重新启动完整 peer/auth 流程并取得新的 key 与 stream。
///
/// # Errors
///
/// tonic 无法从该 warm stream 建立 channel 时返回 transport error。
pub async fn connect_authenticated_channel(
    stream: AuthenticatedUnixStream,
) -> Result<(Channel, DarwinConnectInfo), tonic::transport::Error> {
    let peer = stream.peer();
    let connector = WarmAuthenticatedConnector::new(stream);
    let channel = Endpoint::from_static("http://darwin-engine.local")
        .connect_with_connector(connector)
        .await?;
    Ok((channel, peer))
}

/// W2.5 service 形态直连：固定端点 + 单因子 uid 门（无 HMAC ticket、无 pid 钉死）。
///
/// 认证语义（权威规范 §二.2.7）：固定 root 控制路径内的常驻端点，对端（service
/// engine，euid=0）的 OS credential uid 必须等于 `expected_server_uid`（生产恒 0，
/// 与既有会话形态 `ExpectedPeer(0, pid)` 的 uid 事实同源）；pid 不钉——常驻形态
/// engine pid 未知。uid 门 + root-owned 固定路径即既定信任边界。
///
/// # Errors
///
/// OS credential 读取失败或 uid 不匹配返回 [`PreauthError`]（PeerMismatch/Transport）；
/// tonic 无法建立 channel 返回 [`PreauthError::Transport`]。
pub async fn connect_service_authenticated_channel(
    stream: UnixStream,
    expected_server_uid: u32,
    peer_lookup: &dyn PeerLookup,
) -> Result<(Channel, DarwinConnectInfo), PreauthError> {
    let peer = peer_lookup.inspect(&stream)?;
    if peer.uid() != expected_server_uid {
        return Err(PreauthError::PeerMismatch);
    }
    let authenticated_stream = AuthenticatedUnixStream::from_verified_service_peer(stream, peer);
    connect_authenticated_channel(authenticated_stream)
        .await
        .map_err(|_| PreauthError::Transport)
}

/// 在已连接 UDS 上完成 client 认证，并把同一条 stream 交给 warm tonic connector。
///
/// 认证成功后，metadata 只从 [`crate::auth::AuthenticatedBinding::remote_peer`] 派生；
/// 没有可传入 uid/pid 的旁路，也不会重连或根据 socket path 重新拨号。
///
/// # Errors
///
/// 认证失败保持其稳定 [`PreauthError`]；tonic 无法从已认证 warm stream 建立 channel 时
/// 映射为 `PreauthError::Transport`。
pub async fn authenticate_client_to_tonic_channel(
    mut stream: UnixStream,
    expected_server: ExpectedPeer,
    core_identity: ExpectedPeer,
    peer_lookup: &dyn PeerLookup,
    key: &AuthKey,
) -> Result<(Channel, DarwinConnectInfo), PreauthError> {
    let binding = authenticate_client(
        &mut stream,
        expected_server,
        core_identity,
        peer_lookup,
        key,
    )
    .await?;
    let remote_peer = binding.remote_peer;
    let authenticated_stream = AuthenticatedUnixStream::from_verified_peer(
        stream,
        DarwinConnectInfo::from_verified_peer(remote_peer.uid(), remote_peer.pid()),
    );
    connect_authenticated_channel(authenticated_stream)
        .await
        .map_err(|_| PreauthError::Transport)
}
