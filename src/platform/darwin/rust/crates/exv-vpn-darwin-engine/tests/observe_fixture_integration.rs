//! Core 到非特权 fixture Engine 的最小 Observe-only 纵切。
//!
//! 此测试只证明当前普通用户在真实 UDS 上经 Darwin IPC 的同一已认证 stream 调用 Common
//! `HelperControl.ObserveOwnedState`。fixture 不启动 Engine binary，不创建 operation、lease、
//! 平台资源、网络或 packet 数据通路。

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use exv_vpn_darwin_core::{ObserveEndpoint, ObserveError, observe_owned_state};
use exv_vpn_darwin_ipc::{
    AuthenticatedIncoming, AuthenticatedIncomingSender, DarwinConnectInfo,
    auth::{AUTH_KEY_LEN, AuthKey, PreauthError},
    listener::{ListenerConfig, PreauthListener},
    path::{RuntimeOwner, SocketPath},
    peer::ExpectedPeer,
};
use exv_vpn_wire::generated::{
    self as wire,
    helper_control_server::{HelperControl, HelperControlServer},
};
use tokio::{sync::oneshot, task::JoinHandle, time};
use tonic::{
    Code, Request, Response, Status, Streaming,
    codegen::{async_trait, tokio_stream},
    transport::Server,
};

const FIXTURE_STATUS_MESSAGE: &str = "fixture observe refusal";

/// 在真实 fixture 验收中拒绝把 root 执行当作普通用户证据。
fn require_non_root_fixture_user() {
    assert_ne!(
        RuntimeOwner::current().uid(),
        0,
        "MAC-IPC-01 fixture evidence must run as a normal user, not root"
    );
}

struct TestRuntime {
    directory: PathBuf,
    socket: SocketPath,
    auth_key: [u8; AUTH_KEY_LEN],
}

impl TestRuntime {
    fn new(label: char) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "exv-i2-{label}-{:x}-{sequence:x}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create fixture runtime directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("set fixture runtime directory mode");
        let socket = SocketPath::new(directory.join("control.sock"))
            .expect("fixture socket path stays within sun_path");
        let mut auth_key = [0x5A; AUTH_KEY_LEN];
        auth_key[..8].copy_from_slice(&sequence.to_be_bytes());
        auth_key[8..16].copy_from_slice(&u64::from(std::process::id()).to_be_bytes());
        auth_key[16] = label as u8;
        Self {
            directory,
            socket,
            auth_key,
        }
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.socket.as_path());
        let _ = fs::remove_dir(&self.directory);
    }
}

#[derive(Default)]
struct FixtureCounters {
    observe_handlers: AtomicUsize,
    operation_mutations: AtomicUsize,
    lease_mutations: AtomicUsize,
    platform_mutations: AtomicUsize,
    network_mutations: AtomicUsize,
}

impl FixtureCounters {
    fn assert_no_mutation(&self) {
        assert_eq!(
            self.operation_mutations.load(Ordering::SeqCst),
            0,
            "Observe fixture must not create an operation"
        );
        assert_eq!(
            self.lease_mutations.load(Ordering::SeqCst),
            0,
            "Observe fixture must not create a lease"
        );
        assert_eq!(
            self.platform_mutations.load(Ordering::SeqCst),
            0,
            "Observe fixture must not mutate a platform resource"
        );
        assert_eq!(
            self.network_mutations.load(Ordering::SeqCst),
            0,
            "Observe fixture must not perform a network mutation"
        );
    }
}

#[derive(Clone, Copy)]
enum ObserveOutcome {
    IdleReply,
    StatusError,
}

struct ObserveFixture {
    expected_core: ExpectedPeer,
    counters: Arc<FixtureCounters>,
    outcome: ObserveOutcome,
}

impl ObserveFixture {
    fn new(
        expected_core: ExpectedPeer,
        counters: Arc<FixtureCounters>,
        outcome: ObserveOutcome,
    ) -> Self {
        Self {
            expected_core,
            counters,
            outcome,
        }
    }
}

fn identity_free_idle_reply() -> wire::ObserveOwnedStateReply {
    wire::ObserveOwnedStateReply {
        snapshot: Some(wire::RuntimeSnapshot {
            system_proxy: None, reconnect: None, self_heal: None,
            state: Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
                last_cleanup: None,
            })),
            stats: None,
            proxy_tun: None,
            operation_id: Vec::new(),
            service_status: None,
            mode: String::new(),
        }),
        authority_fence: Some(wire::AuthorityFence {
            authority_epoch: 1,
            // 固定非 nil fixture 标记；它不是 uid、pid、operation 或宿主身份。
            platform_authority_instance_id: vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            admission_watermark: 0,
            journal_revision: 0,
        }),
    }
}

fn fixture_unimplemented<T>() -> Result<Response<T>, Status> {
    Err(Status::unimplemented("fixture is Observe-only"))
}

#[async_trait]
impl HelperControl for ObserveFixture {
    type MaintainOwnerLeaseStream = tokio_stream::Empty<Result<wire::HelperLeaseMessage, Status>>;
    type StreamLogsStream = tokio_stream::Empty<Result<wire::LogEvent, Status>>;
    type StreamStatsStream = tokio_stream::Empty<Result<wire::StatsEvent, Status>>;
    type StreamConnectStatusStream = tokio_stream::Empty<Result<wire::ConnectStatusEvent, Status>>;

    async fn maintain_owner_lease(
        &self,
        _request: Request<Streaming<wire::HostLeaseMessage>>,
    ) -> Result<Response<Self::MaintainOwnerLeaseStream>, Status> {
        fixture_unimplemented()
    }

    async fn observe_owned_state(
        &self,
        request: Request<wire::ObserveOwnedStateRequest>,
    ) -> Result<Response<wire::ObserveOwnedStateReply>, Status> {
        self.counters
            .observe_handlers
            .fetch_add(1, Ordering::SeqCst);

        let peer = request
            .extensions()
            .get::<DarwinConnectInfo>()
            .copied()
            .ok_or_else(|| Status::unauthenticated("missing verified Darwin peer"))?;
        if peer.uid() != self.expected_core.uid() || peer.pid() != self.expected_core.pid() {
            return Err(Status::unauthenticated("unexpected verified Darwin peer"));
        }
        if request.get_ref().lookup_key.is_some() {
            return Err(Status::invalid_argument(
                "fixture Observe requires an empty lookup_key",
            ));
        }

        match self.outcome {
            ObserveOutcome::IdleReply => Ok(Response::new(identity_free_idle_reply())),
            ObserveOutcome::StatusError => Err(Status::failed_precondition(FIXTURE_STATUS_MESSAGE)),
        }
    }

    async fn acquire_lease(
        &self,
        _request: Request<wire::AcquireLeaseRequest>,
    ) -> Result<Response<wire::AcquireLeaseReply>, Status> {
        fixture_unimplemented()
    }

    async fn apply_tunnel(
        &self,
        _request: Request<wire::ApplyTunnelRequest>,
    ) -> Result<Response<wire::ApplyTunnelReply>, Status> {
        fixture_unimplemented()
    }

    async fn stop_tunnel(
        &self,
        _request: Request<wire::StopTunnelRequest>,
    ) -> Result<Response<wire::StopTunnelReply>, Status> {
        fixture_unimplemented()
    }

    async fn get_operation(
        &self,
        _request: Request<wire::GetOperationRequest>,
    ) -> Result<Response<wire::GetOperationReply>, Status> {
        fixture_unimplemented()
    }

    async fn release_lease(
        &self,
        _request: Request<wire::ReleaseLeaseRequest>,
    ) -> Result<Response<wire::ReleaseLeaseReply>, Status> {
        fixture_unimplemented()
    }

    async fn stream_logs(
        &self,
        _request: Request<wire::StreamLogsRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        fixture_unimplemented()
    }

    async fn stream_stats(
        &self,
        _request: Request<wire::StreamStatsRequest>,
    ) -> Result<Response<Self::StreamStatsStream>, Status> {
        fixture_unimplemented()
    }

    async fn stream_connect_status(
        &self,
        _request: Request<wire::StreamConnectStatusRequest>,
    ) -> Result<Response<Self::StreamConnectStatusStream>, Status> {
        fixture_unimplemented()
    }

    async fn keep_alive(
        &self,
        _request: Request<wire::KeepAliveRequest>,
    ) -> Result<Response<wire::KeepAliveReply>, Status> {
        fixture_unimplemented()
    }

    async fn service_manage(
        &self,
        _request: Request<wire::ServiceManageRequest>,
    ) -> Result<Response<wire::ServiceManageReply>, Status> {
        fixture_unimplemented()
    }

    async fn shutdown(
        &self,
        _request: Request<wire::ShutdownRequest>,
    ) -> Result<Response<wire::ShutdownReply>, Status> {
        // 2026-09-05 方案 B（win32 Shutdown RPC）的 darwin fixture stub：darwin 无
        // SCM 服务形态——与生产 stub 同语义回 NOT_APPLICABLE（非 fixture_unimplemented），
        // 无停机业务逻辑（T7）。
        Ok(Response::new(wire::ShutdownReply {
            outcome: wire::ShutdownOutcome::NotApplicable as i32,
        }))
    }
}


struct FixtureServer {
    sender: AuthenticatedIncomingSender,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<Result<(), tonic::transport::Error>>,
}

impl FixtureServer {
    fn sender(&self) -> AuthenticatedIncomingSender {
        self.sender.clone()
    }

    async fn stop(self) {
        let Self {
            sender,
            shutdown,
            task,
        } = self;
        drop(sender);
        let _ = shutdown.send(());
        let joined = time::timeout(Duration::from_secs(1), task)
            .await
            .expect("fixture tonic server must stop");
        let server_result = joined.expect("fixture tonic task must not panic");
        server_result.expect("fixture tonic server must exit cleanly");
    }
}

fn start_fixture(
    expected_core: ExpectedPeer,
    counters: Arc<FixtureCounters>,
    outcome: ObserveOutcome,
) -> FixtureServer {
    let (sender, incoming) = AuthenticatedIncoming::channel(1);
    let (shutdown, shutdown_rx) = oneshot::channel();
    let fixture = ObserveFixture::new(expected_core, counters, outcome);
    let task = tokio::spawn(async move {
        Server::builder()
            .add_service(HelperControlServer::new(fixture))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
    });
    FixtureServer {
        sender,
        shutdown,
        task,
    }
}

fn bind_listener(runtime: &TestRuntime, expected_core: ExpectedPeer) -> PreauthListener {
    PreauthListener::bind(
        ListenerConfig::new(
            runtime.socket.clone(),
            RuntimeOwner::current(),
            expected_core,
            AuthKey::from_bytes(runtime.auth_key),
        )
        .with_global_deadline(Duration::from_secs(5)),
    )
    .expect("bind normal-user fixture listener")
}

async fn handoff_authenticated_connection(
    listener: &PreauthListener,
    sender: AuthenticatedIncomingSender,
) -> Result<(), PreauthError> {
    let connection = listener.accept_until_authenticated().await?;
    sender.handoff(connection).await
}

async fn stop_fixture(listener: PreauthListener, fixture: FixtureServer) {
    fixture.stop().await;
    listener
        .cleanup()
        .expect("cleanup unchanged fixture socket");
}

fn observe_endpoint(runtime: &TestRuntime, expected_engine: ExpectedPeer) -> ObserveEndpoint {
    ObserveEndpoint::new(
        runtime.socket.clone(),
        expected_engine,
        AuthKey::from_bytes(runtime.auth_key),
    )
}

fn assert_identity_free_idle_reply(reply: wire::ObserveOwnedStateReply) {
    let snapshot = reply.snapshot.expect("fixture reply has a snapshot");
    assert!(snapshot.stats.is_none());
    assert!(snapshot.proxy_tun.is_none());
    assert!(snapshot.operation_id.is_empty());
    assert!(snapshot.service_status.is_none());
    assert!(snapshot.mode.is_empty());
    match snapshot.state {
        Some(wire::runtime_snapshot::State::Idle(idle)) => {
            assert!(idle.last_cleanup.is_none());
        }
        _ => panic!("fixture reply must be Idle"),
    }

    let fence = reply.authority_fence.expect("fixture reply has a fence");
    assert_eq!(fence.authority_epoch, 1);
    assert_eq!(
        fence.platform_authority_instance_id,
        vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
    );
    assert_eq!(fence.admission_watermark, 0);
    assert_eq!(fence.journal_revision, 0);
}

#[tokio::test]
async fn normal_user_core_observe_returns_idle_without_side_effects() {
    require_non_root_fixture_user();
    let runtime = TestRuntime::new('i');
    let current_process = ExpectedPeer::current_process();
    let listener = bind_listener(&runtime, current_process);
    let counters = Arc::new(FixtureCounters::default());
    let fixture = start_fixture(
        current_process,
        Arc::clone(&counters),
        ObserveOutcome::IdleReply,
    );

    let admission = handoff_authenticated_connection(&listener, fixture.sender());
    let observe = observe_owned_state(observe_endpoint(&runtime, current_process));
    let (admission, reply) = tokio::join!(admission, observe);
    admission.expect("fixture must receive one authenticated Core stream");
    assert_identity_free_idle_reply(reply.expect("Core Observe must succeed"));
    assert_eq!(counters.observe_handlers.load(Ordering::SeqCst), 1);
    counters.assert_no_mutation();

    stop_fixture(listener, fixture).await;
}

#[tokio::test]
async fn authenticated_fixture_status_reaches_core_unchanged() {
    require_non_root_fixture_user();
    let runtime = TestRuntime::new('s');
    let current_process = ExpectedPeer::current_process();
    let listener = bind_listener(&runtime, current_process);
    let counters = Arc::new(FixtureCounters::default());
    let fixture = start_fixture(
        current_process,
        Arc::clone(&counters),
        ObserveOutcome::StatusError,
    );

    let admission = handoff_authenticated_connection(&listener, fixture.sender());
    let observe = observe_owned_state(observe_endpoint(&runtime, current_process));
    let (admission, result) = tokio::join!(admission, observe);
    admission.expect("fixture must receive one authenticated Core stream");
    let ObserveError::Rpc(status) = result.expect_err("fixture Status must reach Core") else {
        panic!("authenticated handler Status must not become a transport error");
    };
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(status.message(), FIXTURE_STATUS_MESSAGE);
    assert_eq!(counters.observe_handlers.load(Ordering::SeqCst), 1);
    counters.assert_no_mutation();

    stop_fixture(listener, fixture).await;
}

#[tokio::test]
async fn absent_endpoint_is_stable_transport_error() {
    require_non_root_fixture_user();
    let runtime = TestRuntime::new('t');
    let error = observe_owned_state(observe_endpoint(&runtime, ExpectedPeer::current_process()))
        .await
        .expect_err("an absent UDS endpoint must fail before Common tonic");

    assert!(matches!(
        error,
        ObserveError::Transport(PreauthError::Transport)
    ));
}
