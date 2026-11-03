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
    engine_client::{ObserveEndpoint, ObserveError, authenticate_connected_engine},
    service_lifecycle::run_oneshot_elevation,
    service_status::SERVICE_ENGINE_SOCKET_PATH,
};

pub(crate) const BOOTSTRAP_DEADLINE: Duration = Duration::from_secs(30);
const BOOTSTRAP_DIAL_INTERVAL: Duration = Duration::from_millis(20);

/// 固定 Engine 控制会话的稳定错误。
#[derive(Debug)]
pub enum EngineLifecycleError {
    ServiceAgent,
    ServiceConnect(std::io::Error),
    RuntimeDir(RuntimeDirError),
    Ticket(TicketError),
    Bootstrap,
    /// 原启动连接预算到期；保最后已返回的IO，pending不冒充OS超时。
    BootstrapConnectDeadline {
        last_connect: Option<std::io::Error>,
        connect_pending: bool,
    },
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
/// 会话持有已认证 client；[`Self::close`] 只释放该 client，不读取进程终态。
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
    client: exv_vpn_wire::generated::helper_control_client::HelperControlClient<
        tonic::transport::Channel,
    >,
}

/// 只克隆当前已认证控制通道；Core 在释放会话槽位锁后调用刷新 RPC。
pub struct EngineLatencyRefreshHandle {
    client: wire::helper_control_client::HelperControlClient<tonic::transport::Channel>,
}

/// 只克隆已认证通道；代理政策观察不持有连接/停止会话槽位。
pub(crate) struct EngineProxyObserveHandle {
    client: wire::helper_control_client::HelperControlClient<tonic::transport::Channel>,
}

impl EngineProxyObserveHandle {
    pub(crate) async fn observe(&mut self) -> Result<wire::ObserveOwnedStateReply, tonic::Status> {
        self.client
            .observe_owned_state(wire::ObserveOwnedStateRequest { lookup_key: None })
            .await
            .map(tonic::Response::into_inner)
    }
}

impl EngineLatencyRefreshHandle {
    pub async fn trigger_latency_refresh(
        &mut self,
        operation_id: Vec<u8>,
    ) -> Result<wire::HelperLatencyRefreshReply, tonic::Status> {
        self.client
            .trigger_latency_refresh(wire::HelperLatencyRefreshRequest { operation_id })
            .await
            .map(tonic::Response::into_inner)
    }
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
            .keep_alive(wire::KeepAliveRequest {
                monotonic_tick: tick,
            })
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Rpc(error)))
    }
}

impl EngineControlSession {
    pub(crate) fn proxy_observe_handle(&self) -> Option<EngineProxyObserveHandle> {
        self.client.as_ref().map(|client| EngineProxyObserveHandle { client: client.clone() })
    }
    #[must_use]
    pub fn latency_refresh_handle(&self) -> Option<EngineLatencyRefreshHandle> {
        self.client
            .as_ref()
            .map(|client| EngineLatencyRefreshHandle {
                client: client.clone(),
            })
    }
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

    /// 固定官方认证接收端的专用控制面，复用当前已认证通道。
    ///
    /// # Errors
    ///
    /// 返回本地认证通道或宿主动作失败。
    pub async fn auth_receiver_control(
        &mut self, request: wire::AuthReceiverControlRequest,
    ) -> Result<wire::AuthReceiverControlReply, EngineLifecycleError> {
        self.client_mut()?.auth_receiver_control(request).await
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
    /// 服务形态常驻 engine pid 未知，恒 0）。
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
        self.client.as_ref().map(|client| EngineHeartbeatHandle {
            client: client.clone(),
        })
    }

    /// 释放本对象的 Common client，不等待 Engine 进程或生成 Stop 回执。
    ///
    /// # Errors
    ///
    /// 当前实现释放 client 后固定返回 `Ok`，此返回不表示资源清理完成。
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
        if let Self::ServiceConnect(error) = self {
            return write!(formatter, "service Engine connect failed: {error} (errno={:?})", error.raw_os_error());
        }
        if let Self::BootstrapConnectDeadline {
            last_connect,
            connect_pending,
        } = self
        {
            write!(
                formatter,
                "Darwin Engine connect bootstrap deadline expired budget={}s connect_pending={connect_pending}",
                BOOTSTRAP_DEADLINE.as_secs(),
            )?;
            if let Some(error) = last_connect {
                write!(
                    formatter,
                    " last_connect kind={:?} errno={:?}: {error}",
                    error.kind(),
                    error.raw_os_error(),
                )?;
            } else {
                formatter.write_str("; no connect IO error returned before deadline")?;
            }
            return Ok(());
        }
        let message = match self {
            Self::ServiceConnect(_) | Self::BootstrapConnectDeadline { .. } => unreachable!(),
            Self::ServiceAgent => "Darwin service agent could not start Engine",
            Self::RuntimeDir(_) => "Darwin Engine runtime directory failed",
            Self::Ticket(_) => "Darwin Engine ticket bootstrap failed",
            Self::Bootstrap => "Darwin Engine bootstrap protocol failed",
            Self::Observe(_) => "Darwin Engine Observe failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for EngineLifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BootstrapConnectDeadline { last_connect: Some(error), .. } => Some(error),
            _ => None,
        }
    }
}

/// 启动固定 Engine，并返回与其唯一对应的已认证 Common 控制会话。
///
/// 调用者在完成本次业务请求后调用 [`EngineControlSession::close`] 释放本对象的client。
/// close不读取bootstrap终态；实际停止由既有Stop RPC回执报告。
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
        let launched =
            tokio::task::spawn_blocking(move || run_oneshot_elevation(&runtime_dir_path, core_pid))
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
    // 计时起点沿原启动等待位置；只在原预算内完成首次真实connect。
    let deadline = tokio::time::Instant::now() + BOOTSTRAP_DEADLINE;
    // 提权成功后Engine拥有ticket/runtime；释放本句柄不unlink，dial失败也不补删。
    drop(runtime_dir);
    let stream = dial_engine_until_bootstrap_deadline(&socket_path, deadline).await?;
    let client = authenticate_connected_engine(
        ObserveEndpoint::new(socket_path, ExpectedPeer::new(0, engine_pid), auth_key),
        stream,
    )
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
pub async fn connect_service_engine_session() -> Result<EngineControlSession, EngineLifecycleError>
{
    connect_service_engine_session_with_deadline(None).await
}

pub(crate) async fn connect_service_engine_session_with_deadline(
    deadline: Option<tokio::time::Instant>,
) -> Result<EngineControlSession, EngineLifecycleError> {
    let socket_path =
        SocketPath::new(SERVICE_ENGINE_SOCKET_PATH).map_err(|_| EngineLifecycleError::Bootstrap)?;
    let stream = match deadline {
        Some(deadline) => dial_engine_until_bootstrap_deadline(&socket_path, deadline).await?,
        None => tokio::net::UnixStream::connect(socket_path.as_path()).await
            .map_err(EngineLifecycleError::ServiceConnect)?,
    };
    let peer_lookup = SystemPeerLookup;
    let (channel, _verified_engine) =
        connect_service_authenticated_channel(stream, 0, &peer_lookup)
            .await
            .map_err(|error| EngineLifecycleError::Observe(ObserveError::Transport(error)))?;
    Ok(EngineControlSession {
        client: Some(
            exv_vpn_wire::generated::helper_control_client::HelperControlClient::new(channel),
        ),
        engine_pid: 0,
        kind: EngineSessionKind::Service,
    })
}

/// 同一原预算内只重试尚未完成的connect；首次成功后交原认证，不重发材料。
async fn dial_engine_until_bootstrap_deadline(
    socket: &SocketPath,
    deadline: tokio::time::Instant,
) -> Result<tokio::net::UnixStream, EngineLifecycleError> {
    dial_engine_until_bootstrap_deadline_with(deadline, || {
        tokio::net::UnixStream::connect(socket.as_path())
    })
    .await
}

/// 原dial动作的局部接缝，测试注入该次IO/时间；没有第二预算或重试层。
async fn dial_engine_until_bootstrap_deadline_with<T, F, Fut>(
    deadline: tokio::time::Instant,
    mut connect: F,
) -> Result<T, EngineLifecycleError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<T>>,
{
    connect_until_bootstrap_deadline_with(deadline, || {
        let attempt = connect();
        async move { attempt.await.map_err(EngineLifecycleError::ServiceConnect) }
    }).await.map_err(|error| match error {
        EngineLifecycleError::ServiceConnect(source) =>
            EngineLifecycleError::Observe(ObserveError::Connect(source)),
        other => other,
    })
}

/// 同一个实际连接的有限重试内核；服务恢复沿原请求期限复用，不重发恢复动作。
pub(crate) async fn connect_until_bootstrap_deadline_with<T, F, Fut>(
    deadline: tokio::time::Instant,
    mut connect: F,
) -> Result<T, EngineLifecycleError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, EngineLifecycleError>>,
{
    let mut last_connect = None;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(EngineLifecycleError::BootstrapConnectDeadline {
                last_connect,
                connect_pending: false,
            });
        }
        match tokio::time::timeout_at(deadline, connect()).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(EngineLifecycleError::ServiceConnect(error)))
                if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ECONNREFUSED)) =>
            {
                last_connect = Some(error);
            }
            Ok(Err(error)) => {
                return Err(error);
            }
            Err(_) => {
                return Err(EngineLifecycleError::BootstrapConnectDeadline {
                    last_connect,
                    connect_pending: true,
                });
            }
        }
        let next_attempt = tokio::time::Instant::now() + BOOTSTRAP_DIAL_INTERVAL;
        tokio::time::sleep_until(next_attempt.min(deadline)).await;
    }
}
