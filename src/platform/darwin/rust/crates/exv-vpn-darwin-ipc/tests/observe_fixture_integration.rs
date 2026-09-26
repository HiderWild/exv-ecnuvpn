//! `MAC-IPC-01` 的最小 Common tonic 纵向 fixture。
//!
//! 这里故意只证明 `Core → 已认证 UDS → fixture Engine → Observe`：没有 root、
//! 网络、packet 或平台 mutation。peer uid/pid 通过本文件的 lookup seam 注入，因此
//! 不把 fixture 结果表述为真实跨 uid 或 root 运行证据。

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

use exv_vpn_darwin_ipc::{
    AuthenticatedConnectionCloseSignal, AuthenticatedIncoming, AuthenticatedIncomingSender,
    DarwinConnectInfo,
    auth::{AUTH_KEY_LEN, AuthKey, PreauthError, authenticate_client},
    authenticate_client_to_tonic_channel,
    listener::{ListenerConfig, PreauthListener},
    path::{RuntimeOwner, SocketPath},
    peer::{ExpectedPeer, PeerLookup, VerifiedLocalPeer},
};
use exv_vpn_wire::generated::{
    self as wire,
    helper_control_client::HelperControlClient,
    helper_control_server::{HelperControl, HelperControlServer},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::oneshot,
    task::JoinHandle,
    time,
};
use tonic::{
    Code, Request, Response, Status, Streaming,
    codegen::{
        async_trait,
        tokio_stream::{self, StreamExt},
    },
    transport::{Channel, Server},
};

const CORE: ExpectedPeer = ExpectedPeer::new(501, 4_101);
const ENGINE: ExpectedPeer = ExpectedPeer::new(0, 1);
const AUTH_KEY: [u8; AUTH_KEY_LEN] = [0x5A; AUTH_KEY_LEN];
const FIXTURE_STATUS_MESSAGE: &str = "fixture observe status";

#[derive(Clone)]
struct FixedPeerLookup {
    peer: VerifiedLocalPeer,
}

impl FixedPeerLookup {
    const fn new(peer: VerifiedLocalPeer) -> Self {
        Self { peer }
    }
}

impl PeerLookup for FixedPeerLookup {
    fn inspect(&self, _stream: &UnixStream) -> Result<VerifiedLocalPeer, PreauthError> {
        Ok(self.peer)
    }
}

struct TestRuntime {
    directory: PathBuf,
    socket: SocketPath,
}

impl TestRuntime {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let label = label.chars().next().unwrap_or('x');
        let directory =
            std::env::temp_dir().join(format!("e{label}-{:x}-{sequence:x}", std::process::id()));
        fs::create_dir(&directory).expect("create fixture runtime directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("set fixture runtime directory mode");
        let socket = SocketPath::new(directory.join("control.sock"))
            .expect("fixture socket path stays within sun_path");
        Self { directory, socket }
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.socket.as_path());
        let _ = fs::remove_dir(&self.directory);
    }
}

fn core_peer() -> VerifiedLocalPeer {
    VerifiedLocalPeer::new(CORE.uid(), 20, CORE.pid())
}

fn engine_peer() -> VerifiedLocalPeer {
    VerifiedLocalPeer::new(ENGINE.uid(), 0, ENGINE.pid())
}

fn bind_listener(runtime: &TestRuntime, peer: VerifiedLocalPeer) -> PreauthListener {
    PreauthListener::bind(
        ListenerConfig::new(
            runtime.socket.clone(),
            RuntimeOwner::current(),
            CORE,
            AuthKey::from_bytes(AUTH_KEY),
        )
        .with_peer_lookup(Arc::new(FixedPeerLookup::new(peer)))
        .with_global_deadline(Duration::from_secs(5)),
    )
    .expect("bind fixture listener")
}

#[derive(Default)]
struct FixtureCounters {
    observe_handlers: AtomicUsize,
    operation_mutations: AtomicUsize,
    lease_mutations: AtomicUsize,
    platform_mutations: AtomicUsize,
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
    }
}

#[derive(Clone, Copy)]
enum ObserveOutcome {
    IdleReply,
    StatusError,
}

struct ObserveFixture {
    counters: Arc<FixtureCounters>,
    outcome: ObserveOutcome,
}

impl ObserveFixture {
    fn new(counters: Arc<FixtureCounters>, outcome: ObserveOutcome) -> Self {
        Self { counters, outcome }
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
            // 固定非 nil fixture 标记，不是 uid、pid、operation 或宿主身份。
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
        if peer.uid() != CORE.uid() || peer.pid() != CORE.pid() {
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

fn start_fixture(counters: Arc<FixtureCounters>, outcome: ObserveOutcome) -> FixtureServer {
    let (sender, incoming) = AuthenticatedIncoming::channel(1);
    let (shutdown, shutdown_rx) = oneshot::channel();
    let fixture = ObserveFixture::new(counters, outcome);
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

async fn handoff_authenticated_connection(
    listener: &PreauthListener,
    sender: AuthenticatedIncomingSender,
) -> Result<(), PreauthError> {
    let connection = listener.accept_preface().await?;
    sender.handoff(connection).await
}

async fn handoff_authenticated_connection_with_close_signal(
    listener: &PreauthListener,
    sender: AuthenticatedIncomingSender,
) -> Result<AuthenticatedConnectionCloseSignal, PreauthError> {
    let connection = listener.accept_preface().await?;
    sender.handoff_with_close_signal(connection).await
}

async fn authenticated_core_channel(socket: &SocketPath) -> Result<Channel, PreauthError> {
    let stream = UnixStream::connect(socket.as_path())
        .await
        .map_err(|_| PreauthError::Transport)?;
    let (channel, peer) = authenticate_client_to_tonic_channel(
        stream,
        ENGINE,
        CORE,
        &FixedPeerLookup::new(engine_peer()),
        &AuthKey::from_bytes(AUTH_KEY),
    )
    .await?;
    assert_eq!(peer.uid(), ENGINE.uid());
    assert_eq!(peer.pid(), ENGINE.pid());
    Ok(channel)
}

async fn authenticated_raw_core(socket: &SocketPath) -> Result<UnixStream, PreauthError> {
    let mut stream = UnixStream::connect(socket.as_path())
        .await
        .map_err(|_| PreauthError::Transport)?;
    authenticate_client(
        &mut stream,
        ENGINE,
        CORE,
        &FixedPeerLookup::new(engine_peer()),
        &AuthKey::from_bytes(AUTH_KEY),
    )
    .await?;
    Ok(stream)
}

async fn connect_fixture_client(
    listener: &PreauthListener,
    fixture: &FixtureServer,
    socket: &SocketPath,
) -> HelperControlClient<Channel> {
    let admission = handoff_authenticated_connection(listener, fixture.sender());
    let client = authenticated_core_channel(socket);
    let (admission, channel) = tokio::join!(admission, client);
    admission.expect("fixture must receive one authenticated UDS stream");
    HelperControlClient::new(channel.expect("Core must build a warm tonic channel"))
}

async fn stop_fixture(listener: PreauthListener, fixture: FixtureServer) {
    fixture.stop().await;
    listener
        .cleanup()
        .expect("cleanup unchanged fixture socket");
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
async fn authenticated_core_observe_returns_idle_without_side_effects() {
    let runtime = TestRuntime::new("observe");
    let listener = bind_listener(&runtime, core_peer());
    let counters = Arc::new(FixtureCounters::default());
    let fixture = start_fixture(Arc::clone(&counters), ObserveOutcome::IdleReply);

    let mut client = connect_fixture_client(&listener, &fixture, &runtime.socket).await;
    let reply = client
        .observe_owned_state(wire::ObserveOwnedStateRequest { lookup_key: None })
        .await
        .expect("authenticated Observe must succeed")
        .into_inner();

    assert_identity_free_idle_reply(reply);
    assert_eq!(counters.observe_handlers.load(Ordering::SeqCst), 1);
    counters.assert_no_mutation();

    drop(client);
    stop_fixture(listener, fixture).await;
}

#[tokio::test]
async fn preauth_failures_never_enter_the_fixture_handler() {
    let counters = Arc::new(FixtureCounters::default());
    let fixture = start_fixture(Arc::clone(&counters), ObserveOutcome::IdleReply);

    let peer_runtime = TestRuntime::new("peer");
    let wrong_peer = VerifiedLocalPeer::new(CORE.uid() + 1, 20, CORE.pid());
    let peer_listener = bind_listener(&peer_runtime, wrong_peer);
    let peer_server = peer_listener.accept_preface();
    let peer_client = UnixStream::connect(peer_runtime.socket.as_path());
    let (peer_server, peer_client) = tokio::join!(peer_server, peer_client);
    drop(peer_client.expect("raw peer client connects"));
    assert_eq!(peer_server, Err(PreauthError::PeerMismatch));
    peer_listener
        .cleanup()
        .expect("cleanup peer rejection socket");

    let malformed_runtime = TestRuntime::new("bytes");
    let malformed_listener = bind_listener(&malformed_runtime, core_peer());
    let malformed_server = malformed_listener.accept_preface();
    let malformed_client = async {
        let mut stream = UnixStream::connect(malformed_runtime.socket.as_path())
            .await
            .expect("connect malformed client");
        stream
            .write_all(b"not-authenticated")
            .await
            .expect("write malformed bytes");
        stream.shutdown().await.expect("close malformed client");
    };
    let (malformed_server, ()) = tokio::join!(malformed_server, malformed_client);
    assert_eq!(malformed_server, Err(PreauthError::AuthFrame));
    malformed_listener
        .cleanup()
        .expect("cleanup malformed rejection socket");

    let key_runtime = TestRuntime::new("key");
    let key_listener = bind_listener(&key_runtime, core_peer());
    let key_server = key_listener.accept_preface();
    let bad_key_client = async {
        let mut stream = UnixStream::connect(key_runtime.socket.as_path())
            .await
            .expect("connect bad-key client");
        authenticate_client(
            &mut stream,
            ENGINE,
            CORE,
            &FixedPeerLookup::new(engine_peer()),
            &AuthKey::from_bytes([0xA5; AUTH_KEY_LEN]),
        )
        .await
    };
    let (key_server, key_client) = tokio::join!(key_server, bad_key_client);
    assert_eq!(key_server, Err(PreauthError::AuthFrame));
    assert_eq!(key_client, Err(PreauthError::AuthFailed));
    assert!(
        !key_listener
            .key_is_consumed()
            .expect("bad key must remain available")
    );
    key_listener
        .cleanup()
        .expect("cleanup bad-key rejection socket");

    assert_eq!(counters.observe_handlers.load(Ordering::SeqCst), 0);
    counters.assert_no_mutation();
    fixture.stop().await;
}

#[tokio::test]
async fn authenticated_non_tonic_bytes_never_enter_the_fixture_handler() {
    let runtime = TestRuntime::new("nontonic");
    let listener = bind_listener(&runtime, core_peer());
    let counters = Arc::new(FixtureCounters::default());
    let fixture = start_fixture(Arc::clone(&counters), ObserveOutcome::IdleReply);

    let admission = handoff_authenticated_connection(&listener, fixture.sender());
    let raw_client = authenticated_raw_core(&runtime.socket);
    let (admission, raw_client) = tokio::join!(admission, raw_client);
    admission.expect("fixture receives authenticated non-tonic stream");
    let mut raw_client = raw_client.expect("raw Core authentication succeeds");
    raw_client
        .write_all(b"not-tonic")
        .await
        .expect("send non-tonic bytes after authentication");
    raw_client
        .shutdown()
        .await
        .expect("finish non-tonic request");

    let mut response = [0_u8; 1];
    let observed = time::timeout(Duration::from_secs(1), raw_client.read(&mut response))
        .await
        .expect("tonic must reject the completed non-tonic stream")
        .expect("tonic rejection must not become a client I/O failure");
    assert!(observed <= response.len());

    assert_eq!(counters.observe_handlers.load(Ordering::SeqCst), 0);
    counters.assert_no_mutation();

    drop(raw_client);
    stop_fixture(listener, fixture).await;
}

#[tokio::test]
async fn authenticated_handoff_close_signal_waits_for_peer_eof_and_keeps_metadata() {
    let runtime = TestRuntime::new("closeeof");
    let listener = bind_listener(&runtime, core_peer());
    let (sender, mut incoming) = AuthenticatedIncoming::channel(1);

    let server_task = tokio::spawn(async move {
        let mut stream = incoming
            .next()
            .await
            .expect("server must receive the authenticated stream")
            .expect("authenticated stream must not be an incoming error");
        assert_eq!(stream.peer().uid(), CORE.uid());
        assert_eq!(stream.peer().pid(), CORE.pid());

        let mut byte = [0_u8; 1];
        assert_eq!(
            stream
                .read(&mut byte)
                .await
                .expect("server must observe authenticated peer EOF"),
            0,
            "the raw peer must not send application bytes before EOF"
        );
    });

    let handoff = handoff_authenticated_connection_with_close_signal(&listener, sender);
    let raw_client = authenticated_raw_core(&runtime.socket);
    let (close_signal, raw_client) = tokio::join!(handoff, raw_client);
    let mut close_signal = close_signal.expect("authenticated handoff must succeed");
    let mut raw_client = raw_client.expect("raw Core authentication succeeds");

    assert!(
        time::timeout(Duration::from_millis(20), &mut close_signal)
            .await
            .is_err(),
        "a successfully handed-off stream must not report close before peer EOF"
    );

    raw_client
        .shutdown()
        .await
        .expect("raw Core must send its UDS EOF");
    assert!(
        time::timeout(Duration::from_secs(1), &mut close_signal)
            .await
            .expect("peer EOF must eventually drop the server stream")
            .is_ok(),
        "the close signal must be sent when the peer EOF releases the stream"
    );
    server_task.await.expect("EOF server task must not panic");

    drop(raw_client);
    listener
        .cleanup()
        .expect("cleanup unchanged EOF fixture socket");
}

#[tokio::test]
async fn authenticated_handoff_close_signal_fires_when_server_drops_stream() {
    let runtime = TestRuntime::new("closesrv");
    let listener = bind_listener(&runtime, core_peer());
    let (sender, mut incoming) = AuthenticatedIncoming::channel(1);
    let (stream_taken, stream_taken_rx) = oneshot::channel();
    let (release_stream, release_stream_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let stream = incoming
            .next()
            .await
            .expect("server must receive the authenticated stream")
            .expect("authenticated stream must not be an incoming error");
        stream_taken
            .send(stream.peer())
            .expect("test must wait until the server owns the stream");
        release_stream_rx
            .await
            .expect("test must request the server stream drop");
        drop(stream);
    });

    let handoff = handoff_authenticated_connection_with_close_signal(&listener, sender);
    let raw_client = authenticated_raw_core(&runtime.socket);
    let (close_signal, raw_client) = tokio::join!(handoff, raw_client);
    let mut close_signal = close_signal.expect("authenticated handoff must succeed");
    let raw_client = raw_client.expect("raw Core authentication succeeds");

    let peer = time::timeout(Duration::from_secs(1), stream_taken_rx)
        .await
        .expect("server must take the handed-off stream")
        .expect("server task must publish verified metadata");
    assert_eq!(peer.uid(), CORE.uid());
    assert_eq!(peer.pid(), CORE.pid());
    assert!(
        time::timeout(Duration::from_millis(20), &mut close_signal)
            .await
            .is_err(),
        "the server still owns the stream, so close cannot be reported early"
    );

    release_stream
        .send(())
        .expect("server task must still be waiting to drop the stream");
    assert!(
        time::timeout(Duration::from_secs(1), &mut close_signal)
            .await
            .expect("server stream drop must notify")
            .is_ok(),
        "the close signal must fire once the server releases the stream"
    );
    server_task.await.expect("server-drop task must not panic");

    drop(raw_client);
    listener
        .cleanup()
        .expect("cleanup unchanged server-drop fixture socket");
}

#[tokio::test]
async fn closed_incoming_handoff_returns_no_close_handle() {
    let runtime = TestRuntime::new("closefail");
    let listener = bind_listener(&runtime, core_peer());
    let (sender, incoming) = AuthenticatedIncoming::channel(1);
    drop(incoming);

    let handoff = handoff_authenticated_connection_with_close_signal(&listener, sender);
    let raw_client = authenticated_raw_core(&runtime.socket);
    let (handoff, raw_client) = tokio::join!(handoff, raw_client);

    assert!(
        matches!(handoff, Err(PreauthError::Transport)),
        "a closed incoming must fail before creating a close handle"
    );
    drop(raw_client.expect("raw Core authentication succeeds before handoff failure"));
    listener
        .cleanup()
        .expect("cleanup unchanged failed-handoff fixture socket");
}

#[tokio::test]
async fn incoming_drop_after_enqueue_before_delivery_returns_no_close_handle() {
    let runtime = TestRuntime::new("closedrace");
    let listener = bind_listener(&runtime, core_peer());

    let raw_client = {
        let (sender, incoming) = AuthenticatedIncoming::channel(1);
        let handoff = handoff_authenticated_connection_with_close_signal(&listener, sender);
        tokio::pin!(handoff);

        let raw_client = tokio::select! {
            handoff_result = &mut handoff => {
                panic!("handoff must wait for delivery acknowledgement, got {handoff_result:?}");
            }
            raw_client = authenticated_raw_core(&runtime.socket) => {
                raw_client.expect("raw Core authentication succeeds before delivery race")
            }
        };

        assert!(
            time::timeout(Duration::from_millis(50), &mut handoff)
                .await
                .is_err(),
            "an enqueued stream must wait for AuthenticatedIncoming to acknowledge delivery"
        );
        drop(incoming);
        assert!(
            matches!(handoff.await, Err(PreauthError::Transport)),
            "dropping incoming before poll must cancel delivery acknowledgement without a handle"
        );
        raw_client
    };

    drop(raw_client);
    listener
        .cleanup()
        .expect("cleanup unchanged delivery-race fixture socket");
}

#[tokio::test]
async fn fixture_handler_status_is_preserved_after_authenticated_dispatch() {
    let runtime = TestRuntime::new("status");
    let listener = bind_listener(&runtime, core_peer());
    let counters = Arc::new(FixtureCounters::default());
    let fixture = start_fixture(Arc::clone(&counters), ObserveOutcome::StatusError);

    let mut client = connect_fixture_client(&listener, &fixture, &runtime.socket).await;
    let status = client
        .observe_owned_state(wire::ObserveOwnedStateRequest { lookup_key: None })
        .await
        .expect_err("fixture status must reach the Core unchanged");
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(status.message(), FIXTURE_STATUS_MESSAGE);
    assert_eq!(counters.observe_handlers.load(Ordering::SeqCst), 1);
    counters.assert_no_mutation();

    drop(client);
    stop_fixture(listener, fixture).await;
}
