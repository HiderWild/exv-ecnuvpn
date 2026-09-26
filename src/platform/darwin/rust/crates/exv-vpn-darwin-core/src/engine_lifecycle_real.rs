//! Core 对固定 root Engine 的受控 bootstrap 与同通道控制会话。

use std::time::Duration;

use exv_vpn_darwin_ipc::{
    auth::{AUTH_KEY_LEN, AuthKey},
    bootstrap::{EngineTicketV1, TicketError, create_engine_ticket, guarded_remove_engine_ticket},
    connect_service_authenticated_channel,
    path::{RuntimeDir, RuntimeDirError, RuntimeOwner, SocketPath},
    peer::{ExpectedPeer, SystemPeerLookup},
};
use exv_vpn_wire::generated as wire;
use zeroize::Zeroize;

use crate::{
    elevation::current_core_credentials,
    engine_client::{ObserveEndpoint, ObserveError, connect_authenticated_engine},
    service_lifecycle::run_oneshot_elevation,
    service_status::SERVICE_ENGINE_SOCKET_PATH,
};

const BOOTSTRAP_DEADLINE: Duration = Duration::from_secs(30);

/// 固定 Engine 控制会话的稳定错误。
#[derive(Debug)]
pub enum EngineLifecycleError {
    ServiceAgent,
    RuntimeDir(RuntimeDirError),
    Ticket(TicketError),
    Bootstrap,
    Observe(ObserveError),
}

/// Engine 会话的形态标记（W2.5 起 `snapshot.mode` 的事实源）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineSessionKind {
    /// W3 oneshot 形态：用户手势经系统提权弹窗一次性直拉（pid 来自守护式
    /// spawn 的 stdout 解析；`snapshot.mode="oneshot"`）。生命周期对齐本次 core
    /// 会话——core 退出请求 + 保活超时兜底 + EOF 收敛。
    Session,
    /// W2.5 服务形态：常驻固定端点直连（engine pid 未知；`snapshot.mode="service"`）。
    Service,
}

impl EngineSessionKind {
    /// `snapshot.mode` 的取值（权威规范 §四：darwin 恒 `"service"|"oneshot"`）。
    #[must_use]
    pub const fn as_mode_str(self) -> &'static str {
        match self {
            Self::Session => "oneshot",
            Self::Service => "service",
        }
    }
}

/// Core 在一次 Engine 生命周期中持有的唯一 Common 控制通道。
///
/// 该会话从 Engine 的 `Ready` record 建立，到 [`Self::close`] 读取终态 record 为止。
/// 它只承载已经批准的 Common `ObserveOwnedState`、`ApplyTunnel` 和 `StopTunnel`；不在
/// 此处实现 CSTP、utun 或网络数据通路。
pub struct EngineControlSession {
    client: Option<
        exv_vpn_wire::generated::helper_control_client::HelperControlClient<
            tonic::transport::Channel,
        >,
    >,
    engine_pid: u32,
    kind: EngineSessionKind,
}

/// W2 keepalive 心跳句柄：克隆自 [`EngineControlSession`] 的已认证 client。
///
/// 会话槽位被 `&mut` 借用（连接/停止路径）期间 ticker 仍可独立发 `KeepAlive`
/// ——同一 channel 上的独立 RPC，不与槽位互斥。
pub struct EngineHeartbeatHandle {
    client:
        exv_vpn_wire::generated::helper_control_client::HelperControlClient<
            tonic::transport::Channel,
        >,
}

impl EngineHeartbeatHandle {
    /// 发送一条 `KeepAlive`（W2 活性层；Engine 侧 touch 心跳时间戳并回显 tick）。
    ///
    /// # Errors
    ///
    /// 返回 Engine RPC 的 tonic 状态（传输失败由调用方 best-effort 忽略）。
    pub async fn keep_alive(
        &mut self,
        tick: u64,
    ) -> Result<wire::KeepAliveReply, EngineLifecycleError> {
        self.client
            .keep_alive(wire::KeepAliveRequest { monotonic_tick: tick })
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }
}

impl EngineControlSession {
    /// 读取当前 Engine owned-state。
    ///
    /// # Errors
    ///
    /// 返回本地认证通道或 Engine RPC 失败。
    pub async fn observe_owned_state(
        &mut self,
    ) -> Result<wire::ObserveOwnedStateReply, EngineLifecycleError> {
        self.client_mut()?
            .observe_owned_state(wire::ObserveOwnedStateRequest { lookup_key: None })
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }

    /// 把连接请求交给同一已认证 Engine 通道。
    ///
    /// # Errors
    ///
    /// 返回本地认证通道或 Engine RPC 失败。
    pub async fn apply_tunnel(
        &mut self,
        request: wire::ApplyTunnelRequest,
    ) -> Result<wire::ApplyTunnelReply, EngineLifecycleError> {
        self.client_mut()?
            .apply_tunnel(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }

    /// 把断开请求交给同一已认证 Engine 通道。
    ///
    /// # Errors
    ///
    /// 返回本地认证通道或 Engine RPC 失败。
    pub async fn stop_tunnel(
        &mut self,
        request: wire::StopTunnelRequest,
    ) -> Result<wire::StopTunnelReply, EngineLifecycleError> {
        self.client_mut()?
            .stop_tunnel(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }

    /// 挂接 Engine 状态流并返回该流；调用方（Core 连接投影）负责消费。
    ///
    /// Windows 对齐的 attach-before-apply 顺序：必须先挂本流，再 `ApplyTunnel`。
    ///
    /// # Errors
    ///
    /// 返回本地认证通道或 Engine RPC 失败。
    pub async fn attach_connect_status(
        &mut self,
    ) -> Result<tonic::Streaming<wire::ConnectStatusEvent>, EngineLifecycleError> {
        self.client_mut()?
            .stream_connect_status(wire::StreamConnectStatusRequest {})
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }

    /// 挂接 Engine 统计流并返回该流（W-MVP-13；同一已认证控制通道上的既有
    /// `StreamStats` 接缝，不新建通道）。`sample_interval_ms == 0` 使用 Engine 默认间隔。
    ///
    /// # Errors
    ///
    /// 返回本地认证通道或 Engine RPC 失败。
    pub async fn attach_stats(
        &mut self,
    ) -> Result<tonic::Streaming<wire::StatsEvent>, EngineLifecycleError> {
        self.client_mut()?
            .stream_stats(wire::StreamStatsRequest {
                sample_interval_ms: 0,
            })
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }

    /// 挂接 Engine 日志流并返回该流（MAC-OBS-13 S1；同一已认证控制通道上的既有
    /// `StreamLogs` 接缝，不新建通道）。`resume_tick` 契约为 0 = 从当前流位置开始
    /// （Engine 无离线缓冲补拉）。
    ///
    /// # Errors
    ///
    /// 返回本地认证通道或 Engine RPC 失败。
    pub async fn attach_logs(
        &mut self,
    ) -> Result<tonic::Streaming<wire::LogEvent>, EngineLifecycleError> {
        self.client_mut()?
            .stream_logs(wire::StreamLogsRequest { resume_tick: 0 })
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }

    /// 本次会话对应 Engine 进程的 pid（bootstrap `Ready` record 中的真实 pid；
    /// 服务形态常驻 engine pid 未知，恒 0——liveness 走端点探测）。
    #[must_use]
    pub fn engine_pid(&self) -> u32 {
        self.engine_pid
    }

    /// 本次会话的形态（W2.5：`snapshot.mode` 事实源）。
    #[must_use]
    pub const fn kind(&self) -> EngineSessionKind {
        self.kind
    }

    /// W2 心跳句柄：克隆的已认证 tonic client（channel 基，tonic client 可 Clone）
    /// ——core 侧 keepalive ticker 独立持有，不借用会话槽的 `&mut`，也不打开第二条
    /// 控制协议（同一已认证通道上的既有 `KeepAlive` RPC）。
    ///
    /// client 已关闭（`close` 后）时返回 `None`。
    #[must_use]
    pub fn heartbeat_handle(&self) -> Option<EngineHeartbeatHandle> {
        self.client
            .as_ref()
            .map(|client| EngineHeartbeatHandle {
                client: client.clone(),
            })
    }

    /// 关闭 Common client；Engine 会在唯一已认证连接结束后自行退出并清理运行时目录。
    ///
    /// # Errors
    ///
    /// 当前实现释放 client 后固定返回 `Ok`；保留 `Result` 以承载后续有界终态读取。
    pub fn close(mut self) -> Result<(), EngineLifecycleError> {
        drop(self.client.take());
        Ok(())
    }

    fn client_mut(
        &mut self,
    ) -> Result<
        &mut exv_vpn_wire::generated::helper_control_client::HelperControlClient<
            tonic::transport::Channel,
        >,
        EngineLifecycleError,
    > {
        self.client.as_mut().ok_or(EngineLifecycleError::Bootstrap)
    }
}

impl std::fmt::Display for EngineLifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::ServiceAgent => "Darwin service agent could not start Engine",
            Self::RuntimeDir(_) => "Darwin Engine runtime directory failed",
            Self::Ticket(_) => "Darwin Engine ticket bootstrap failed",
            Self::Bootstrap => "Darwin Engine bootstrap protocol failed",
            Self::Observe(_) => "Darwin Engine Observe failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for EngineLifecycleError {}

/// 启动固定 Engine，并返回与其唯一对应的已认证 Common 控制会话。
///
/// 调用者必须在完成本次业务请求后调用 [`EngineControlSession::close`]，以读取固定
/// bootstrap 的终态 record。该函数不发送连接、CSTP、utun 或网络请求。
///
/// # Errors
///
/// 返回提升启动、bootstrap、运行目录或已认证通道建立失败。
pub async fn launch_fixed_engine_session() -> Result<EngineControlSession, EngineLifecycleError> {
    let credentials = current_core_credentials();
    let runtime_dir =
        RuntimeDir::create(RuntimeOwner::current()).map_err(EngineLifecycleError::RuntimeDir)?;
    let socket_path = runtime_dir
        .engine_socket_path()
        .map_err(EngineLifecycleError::RuntimeDir)?;
    let mut key_bytes = [0_u8; AUTH_KEY_LEN];
    getrandom::fill(&mut key_bytes).map_err(|_| EngineLifecycleError::Bootstrap)?;
    let auth_key = AuthKey::from_bytes(key_bytes);
    let ticket = EngineTicketV1::new(credentials.uid(), credentials.pid(), key_bytes)
        .map_err(EngineLifecycleError::Ticket)?;
    key_bytes.zeroize();
    let ticket_identity =
        create_engine_ticket(&runtime_dir, ticket).map_err(EngineLifecycleError::Ticket)?;

    // W3：经系统提权弹窗一次性直拉（service agent `start-engine-once` 守护式
    // spawn；daemon StartEngine 帧已随 E8 退役）。取消/失败/SourceMissing/pid
    // 不合规统一按会话建立失败上报（细节码进 `engine_session_status` 的 message
    // 与日志；MVP 面=elevate 失败同款错误面，e2e 轮再按需分化「取消=安静失败」面）。
    let engine_pid = {
        let runtime_dir_path = runtime_dir.as_path().to_path_buf();
        let core_pid = credentials.pid();
        let launched = tokio::task::spawn_blocking(move || {
            run_oneshot_elevation(&runtime_dir_path, core_pid)
        })
        .await
        .map_err(|_| EngineLifecycleError::ServiceAgent)
        .and_then(|outcome| outcome.map_err(|_| EngineLifecycleError::ServiceAgent));
        match launched {
            Ok(pid) => pid,
            Err(error) => {
                let _ = guarded_remove_engine_ticket(&runtime_dir, ticket_identity);
                let _ = runtime_dir.cleanup_empty();
                return Err(error);
            }
        }
    };
    wait_for_engine_socket(socket_path.as_path()).await?;
    // The root Engine owns the sealed runtime and its final cleanup from this point.
    drop(runtime_dir);

    let client = connect_authenticated_engine(ObserveEndpoint::new(
        socket_path,
        ExpectedPeer::new(0, engine_pid),
        auth_key,
    ))
    .await
    .map_err(EngineLifecycleError::Observe)?;
    Ok(EngineControlSession {
        client: Some(client),
        engine_pid,
        kind: EngineSessionKind::Session,
    })
}

/// W2.5：直连常驻 service engine 的固定端点并建立已认证控制会话。
///
/// 与 [`launch_fixed_engine_session`] 的差异：无 ticket/runtime-dir/bootstrap——
/// engine 已由 launchd 常驻拉起；认证=单因子 uid 门（core 侧校验 server peer
/// uid==0——engine `setreuid(owner, 0)` 后 euid=0，与会话形态 `ExpectedPeer(0,
/// pid)` 的 uid 事实同源；pid 不钉——常驻形态 pid 未知）。
///
/// # Errors
///
/// 固定端点不可连、server peer uid 不符或 tonic channel 建立失败返回
/// [`EngineLifecycleError::Observe`]（调用方按过渡兼容回退会话路径）。
pub async fn connect_service_engine_session() -> Result<EngineControlSession, EngineLifecycleError> {
    let socket_path = SocketPath::new(SERVICE_ENGINE_SOCKET_PATH)
        .map_err(|_| EngineLifecycleError::Bootstrap)?;
    let stream = tokio::net::UnixStream::connect(socket_path.as_path())
        .await
        .map_err(|_| {
            EngineLifecycleError::Observe(ObserveError::Transport(
                exv_vpn_darwin_ipc::auth::PreauthError::Transport,
            ))
        })?;
    let peer_lookup = SystemPeerLookup;
    let (channel, _verified_engine) =
        connect_service_authenticated_channel(stream, 0, &peer_lookup)
            .await
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Transport(error)))?;
    Ok(EngineControlSession {
        client: Some(exv_vpn_wire::generated::helper_control_client::HelperControlClient::new(
            channel,
        )),
        engine_pid: 0,
        kind: EngineSessionKind::Service,
    })
}

async fn wait_for_engine_socket(socket: &std::path::Path) -> Result<(), EngineLifecycleError> {
    let deadline = tokio::time::Instant::now() + BOOTSTRAP_DEADLINE;
    while !socket.exists() {
        if tokio::time::Instant::now() >= deadline {
            return Err(EngineLifecycleError::Bootstrap);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}
