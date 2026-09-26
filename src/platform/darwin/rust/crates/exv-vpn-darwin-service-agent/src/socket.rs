//! 固定 Unix socket 的 daemon/client 执行层。
//!
//! daemon 只接受 enrolled owner uid 的 macOS `getpeereid` 事实；frame 中的 owner uid
//! 只是二次一致性检查，绝不单独信任。每条连接只允许一个固定 16-byte request 后 EOF，
//! 正常用户只能通过它请求 `status` 或 `cleanup`。

use std::{
    io::{Read, Write},
    net::Shutdown,
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    time::Duration,
};

use crate::{
    ServiceAgentAction, ServiceAgentError, ServiceAgentOutcome, ServiceAgentRequestV1,
    ServiceAgentResponseV1, ExecutionIdentity, authorize_request,
    macos::{
        ArtifactIdentity, MacosPlatform, UmaskGuard, ensure_socket_parent, peer_identity,
        remove_safe_stale_socket, remove_socket_if_identity, seal_bound_socket,
        verify_root_socket_server,
    },
    platform::{EnrolledOwner, PlatformError, PrivilegedPlatform, cleanup_with},
};

const SOCKET_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// 在 root daemon 中服务固定 socket，直到系统服务管理器停止该进程。
///
/// daemon 构造平台时读取固定 state 叶解析 serve 端 Engine 路径：存在且校验通过
/// （绝对路径、长度受限、常规文件、非 symlink）即采用；缺失或不合法回退固定
/// canonical 路径（服务代理安装时写下的已装 Engine 路径）。
///
/// # Errors
///
/// daemon 不是 root、socket parent/bind/seal 失败或 listener 无法继续接受连接时返回稳定
/// [`PlatformError`]。单个 client 的 peer/frame/EOF/timeout/cleanup 失败只关闭该连接，
/// 不停止 listener。
pub fn serve_forever(owner: EnrolledOwner) -> Result<(), PlatformError> {
    MacosPlatform::require_root_process()?;
    let socket = BoundSocket::bind(owner)?;
    let mut platform = MacosPlatform::for_serve();

    loop {
        let (stream, _) = socket
            .listener
            .accept()
            .map_err(|_| PlatformError::SocketProtocolFailed)?;
        let _ = serve_one_connection(&mut platform, stream, owner);
    }
}

/// 普通用户通过固定 socket 请求 `status` 或 `cleanup`。
///
/// # Errors
///
/// root caller、非 `status|cleanup` action、固定 socket、root server peer credential、
/// 固定 frame 或 EOF/timeout 任一不符合时返回稳定 [`PlatformError`]。
pub fn request_from_ordinary_user(
    action: ServiceAgentAction,
) -> Result<ServiceAgentOutcome, PlatformError> {
    if !matches!(
        action,
        ServiceAgentAction::Status | ServiceAgentAction::Cleanup
    ) {
        return Err(PlatformError::CliUsage);
    }
    let owner_uid = MacosPlatform::ordinary_user_uid()?;
    let request = ServiceAgentRequestV1::new(action, owner_uid)
        .map_err(|_| PlatformError::SocketProtocolFailed)?;
    let mut stream = UnixStream::connect(crate::platform::SERVICE_AGENT_SOCKET_PATH)
        .map_err(|_| PlatformError::SocketProtocolFailed)?;
    configure_stream(&stream)?;
    verify_root_socket_server(stream.as_raw_fd())?;
    stream
        .write_all(&request.encode())
        .and_then(|()| stream.shutdown(Shutdown::Write))
        .map_err(|_| PlatformError::SocketProtocolFailed)?;

    let response_bytes = read_exact_frame(&mut stream)?;
    require_eof(&mut stream)?;
    let response = ServiceAgentResponseV1::decode(&response_bytes)
        .map_err(|_| PlatformError::SocketProtocolFailed)?;
    if response.action() != action {
        return Err(PlatformError::SocketProtocolFailed);
    }
    Ok(response.outcome())
}

/// 纯逻辑 daemon handler；测试通过 fake 平台验证 cleanup 和拒绝规则。
///
/// # Errors
///
/// peer uid 不是 enrolled owner，或 frame owner uid 与已验证 peer uid 不一致时返回
/// [`PlatformError::SocketPeerRejected`]，调用方必须关闭连接且不写 response。
pub fn handle_daemon_request<P>(
    platform: &mut P,
    request: ServiceAgentRequestV1,
    peer: EnrolledOwner,
    enrolled_owner: EnrolledOwner,
) -> Result<ServiceAgentResponseV1, PlatformError>
where
    P: PrivilegedPlatform,
{
    if peer.uid() != enrolled_owner.uid() || request.owner_uid() != peer.uid() {
        return Err(PlatformError::SocketPeerRejected);
    }

    match request.action() {
        ServiceAgentAction::Status => {
            Ok(ServiceAgentResponseV1::accepted(ServiceAgentAction::Status))
        }
        ServiceAgentAction::Cleanup => {
            let identity = ExecutionIdentity::from_verified_os_identity(0, peer.uid());
            authorize_request(request, identity).map_err(|_| PlatformError::SocketPeerRejected)?;
            match cleanup_with(platform) {
                Ok(()) => Ok(ServiceAgentResponseV1::accepted(
                    ServiceAgentAction::Cleanup,
                )),
                Err(_) => Ok(ServiceAgentResponseV1::rejected(
                    ServiceAgentAction::Cleanup,
                    ServiceAgentError::CleanupFailed,
                )),
            }
        }
        ServiceAgentAction::Install | ServiceAgentAction::Uninstall => {
            Ok(ServiceAgentResponseV1::rejected(
                request.action(),
                ServiceAgentError::DaemonActionRejected,
            ))
        }
        // W2.5：幂等确保 service engine job 已 bootstrap（产出受 launchd 监护的
        // engine，不做裸 spawn）——修复分层中 daemon 在场时的免提权拉起通道。
        ServiceAgentAction::EnsureEngine => {
            let identity = ExecutionIdentity::from_verified_os_identity(0, peer.uid());
            authorize_request(request, identity).map_err(|_| PlatformError::SocketPeerRejected)?;
            match platform.control_engine_job(crate::platform::EngineJobAction::EnsureBootstrap) {
                Ok(()) => Ok(ServiceAgentResponseV1::accepted(
                    ServiceAgentAction::EnsureEngine,
                )),
                Err(_) => Ok(ServiceAgentResponseV1::rejected(
                    ServiceAgentAction::EnsureEngine,
                    ServiceAgentError::EnsureEngineFailed,
                )),
            }
        }
    }
}

struct BoundSocket {
    listener: UnixListener,
    identity: ArtifactIdentity,
    owner: EnrolledOwner,
}

impl BoundSocket {
    fn bind(owner: EnrolledOwner) -> Result<Self, PlatformError> {
        ensure_socket_parent()?;
        remove_safe_stale_socket(owner)?;
        let listener = {
            let umask = UmaskGuard::restrictive();
            let listener = UnixListener::bind(crate::platform::SERVICE_AGENT_SOCKET_PATH)
                .map_err(|_| PlatformError::SocketSetupFailed)?;
            drop(umask);
            listener
        };
        // Before `seal_bound_socket` records the identity, any error intentionally leaves the
        // leaf in place; a later root startup may only remove a verified stale socket.
        let identity = seal_bound_socket(owner)?;
        Ok(Self {
            listener,
            identity,
            owner,
        })
    }
}

impl Drop for BoundSocket {
    fn drop(&mut self) {
        let _ = remove_socket_if_identity(self.identity, self.owner);
    }
}

fn serve_one_connection<P>(
    platform: &mut P,
    mut stream: UnixStream,
    enrolled_owner: EnrolledOwner,
) -> Result<(), PlatformError>
where
    P: PrivilegedPlatform,
{
    configure_stream(&stream)?;
    let peer = peer_identity(stream.as_raw_fd())?;
    if peer.uid() != enrolled_owner.uid() {
        return Err(PlatformError::SocketPeerRejected);
    }
    let request_bytes = read_exact_frame(&mut stream)?;
    let request = ServiceAgentRequestV1::decode(&request_bytes)
        .map_err(|_| PlatformError::SocketProtocolFailed)?;
    require_eof(&mut stream)?;
    let response = handle_daemon_request(platform, request, peer, enrolled_owner)?.encode();
    stream
        .write_all(&response)
        .and_then(|()| stream.shutdown(Shutdown::Write))
        .map_err(|_| PlatformError::SocketProtocolFailed)
}

fn configure_stream(stream: &UnixStream) -> Result<(), PlatformError> {
    stream
        .set_read_timeout(Some(SOCKET_IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT)))
        .map_err(|_| PlatformError::SocketProtocolFailed)
}

fn read_exact_frame(
    stream: &mut UnixStream,
) -> Result<[u8; crate::SERVICE_AGENT_REQUEST_V1_LEN], PlatformError> {
    let mut bytes = [0_u8; crate::SERVICE_AGENT_REQUEST_V1_LEN];
    stream
        .read_exact(&mut bytes)
        .map_err(|_| PlatformError::SocketProtocolFailed)?;
    Ok(bytes)
}

fn require_eof(stream: &mut UnixStream) -> Result<(), PlatformError> {
    let mut extra = [0_u8; 1];
    match stream.read(&mut extra) {
        Ok(0) => Ok(()),
        Ok(_) | Err(_) => Err(PlatformError::SocketProtocolFailed),
    }
}
