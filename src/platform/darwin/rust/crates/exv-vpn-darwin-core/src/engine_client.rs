//! Core 到 Engine 的最小 Observe-only 客户端。
//!
//! 本模块只消费 Darwin IPC 已认证 UDS 的 production handoff，并在认证成功后的同一条
//! stream 上调用既有 Common `HelperControl`。它不管理 Engine 生命周期，也不处理任何
//! 网络、packet 或提升权限行为。

use std::fmt;

use exv_vpn_darwin_ipc::{
    auth::{AuthKey, PreauthError},
    authenticate_client_to_tonic_channel,
    path::SocketPath,
    peer::{ExpectedPeer, SystemPeerLookup},
};
use exv_vpn_wire::generated::{self as wire, helper_control_client::HelperControlClient};
use tokio::net::UnixStream;
use tonic::{Status, transport::Channel};

/// Core 建立一次 Observe 请求所需的已校验 endpoint 与认证材料。
///
/// `auth_key` 只属于当前 listener；调用本函数会消费本配置，不能把它复用于第二个
/// listener 或第二次认证。
pub struct ObserveEndpoint {
    socket_path: SocketPath,
    expected_engine: ExpectedPeer,
    auth_key: AuthKey,
}

impl ObserveEndpoint {
    /// 创建一次性 Engine Observe endpoint。
    #[must_use]
    pub const fn new(
        socket_path: SocketPath,
        expected_engine: ExpectedPeer,
        auth_key: AuthKey,
    ) -> Self {
        Self {
            socket_path,
            expected_engine,
            auth_key,
        }
    }
}

impl fmt::Debug for ObserveEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObserveEndpoint")
            .field("socket_path", &self.socket_path.as_path())
            .field("expected_engine", &self.expected_engine)
            .finish_non_exhaustive()
    }
}

/// Observe-only 调用的稳定错误边界。
///
/// 认证前和 transport 失败沿用 IPC 的固定 [`PreauthError`]；已经进入 Common tonic
/// handler 的错误保持其原始 [`Status`]，绝不能重新解释为认证失败。
#[derive(Debug)]
pub enum ObserveError {
    /// 本地 socket、peer 或认证阶段的失败。
    Transport(PreauthError),
    /// 已认证 Common `HelperControl` handler 的 tonic 状态。
    Rpc(Status),
}

impl fmt::Display for ObserveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "Darwin Engine Observe transport: {error}"),
            Self::Rpc(status) => write!(
                formatter,
                "Darwin Engine Observe RPC ({code}): {message}",
                code = status.code(),
                message = status.message(),
            ),
        }
    }
}

impl std::error::Error for ObserveError {}

/// 在已认证的同一条 UDS 上观察 Engine 的当前状态。
///
/// 请求固定为 Common `ObserveOwnedStateRequest { lookup_key: None }`；本只读探测不能
/// 虚构 operation 或 lease key。
///
/// # Errors
///
/// 本地连接、peer 校验、`DarwinAuthV1` 或 warm tonic channel 失败返回
/// [`ObserveError::Transport`]。Common handler 返回的 tonic 状态不作翻译，返回
/// [`ObserveError::Rpc`]。
pub async fn observe_owned_state(
    endpoint: ObserveEndpoint,
) -> Result<wire::ObserveOwnedStateReply, ObserveError> {
    let mut client = connect_authenticated_engine(endpoint).await?;
    client
        .observe_owned_state(wire::ObserveOwnedStateRequest { lookup_key: None })
        .await
        .map(tonic::Response::into_inner)
        .map_err(ObserveError::Rpc)
}

/// 连接固定 Engine，并把唯一已认证 tonic client 交给调用方持有。
///
/// 此函数不发起任何业务请求。调用方只可在这个已经完成 `DarwinAuthV1` 的通道上调用
/// Common `HelperControl`；它不会创建第二个本地控制协议。
///
/// # Errors
///
/// 返回连接、对端认证或 tonic 建立失败。
pub async fn connect_authenticated_engine(
    endpoint: ObserveEndpoint,
) -> Result<HelperControlClient<Channel>, ObserveError> {
    let stream = UnixStream::connect(endpoint.socket_path.as_path())
        .await
        .map_err(|_| ObserveError::Transport(PreauthError::Transport))?;
    let core_identity = ExpectedPeer::current_process();
    let peer_lookup = SystemPeerLookup;
    let (channel, _verified_engine) = authenticate_client_to_tonic_channel(
        stream,
        endpoint.expected_engine,
        core_identity,
        &peer_lookup,
        &endpoint.auth_key,
    )
    .await
    .map_err(ObserveError::Transport)?;

    Ok(HelperControlClient::new(channel))
}
