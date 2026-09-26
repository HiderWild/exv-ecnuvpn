//! `DarwinAuthV1` 的真实 UDS 认证前契约测试。

use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

use exv_vpn_darwin_ipc::{
    auth::{
        self, AUTH_MAGIC, AUTH_VERSION, AuthKey, PreauthError, SERVER_PROOF_LEN,
        authenticate_client,
    },
    listener::{ListenerConfig, PreauthListener, PublishBarrierHook},
    path::{RuntimeOwner, SocketPath, validate_runtime_dir},
    peer::{ExpectedPeer, PeerLookup, SystemPeerLookup, VerifiedLocalPeer},
};

const CORE: ExpectedPeer = ExpectedPeer::new(501, 4_101);
const ENGINE: ExpectedPeer = ExpectedPeer::new(0, 1);
const KEY_A: [u8; 32] = [0xA1; 32];
const KEY_B: [u8; 32] = [0xB2; 32];

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

struct SequencedPeerLookup {
    peers: Mutex<VecDeque<VerifiedLocalPeer>>,
}

impl SequencedPeerLookup {
    fn new(peers: impl IntoIterator<Item = VerifiedLocalPeer>) -> Self {
        Self {
            peers: Mutex::new(peers.into_iter().collect()),
        }
    }
}

impl PeerLookup for SequencedPeerLookup {
    fn inspect(&self, _stream: &UnixStream) -> Result<VerifiedLocalPeer, PreauthError> {
        self.peers
            .lock()
            .map_err(|_| PreauthError::Transport)?
            .pop_front()
            .ok_or(PreauthError::Transport)
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
        fs::create_dir(&directory).expect("create private runtime directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("set private runtime directory mode");
        let socket = SocketPath::new(directory.join("control.sock"))
            .expect("test socket path stays within sun_path");
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
    VerifiedLocalPeer::new(ENGINE.uid(), 88, ENGINE.pid())
}

struct FailPublishBarrier;

impl PublishBarrierHook for FailPublishBarrier {
    fn before_publish(&self, _socket_path: &SocketPath) -> Result<(), PreauthError> {
        Err(PreauthError::Transport)
    }
}

struct ReplaceBoundEndpoint;

impl PublishBarrierHook for ReplaceBoundEndpoint {
    fn before_publish(&self, socket_path: &SocketPath) -> Result<(), PreauthError> {
        fs::remove_file(socket_path.as_path()).map_err(|_| PreauthError::Transport)?;
        fs::write(socket_path.as_path(), b"foreign endpoint").map_err(|_| PreauthError::Transport)
    }
}

fn server_lookup() -> Arc<dyn PeerLookup> {
    Arc::new(FixedPeerLookup::new(core_peer()))
}

fn client_lookup() -> FixedPeerLookup {
    FixedPeerLookup::new(engine_peer())
}

fn bind(runtime: &TestRuntime, key: [u8; 32]) -> PreauthListener {
    PreauthListener::bind(
        ListenerConfig::new(
            runtime.socket.clone(),
            RuntimeOwner::current(),
            CORE,
            AuthKey::from_bytes(key),
        )
        .with_peer_lookup(server_lookup())
        .with_global_deadline(Duration::from_secs(30)),
    )
    .expect("bind test listener")
}

async fn authenticate_valid_client(
    socket: &SocketPath,
    key: [u8; 32],
) -> Result<auth::AuthenticatedBinding, PreauthError> {
    let mut stream = UnixStream::connect(socket.as_path())
        .await
        .map_err(|_| PreauthError::Transport)?;
    let key = AuthKey::from_bytes(key);
    authenticate_client(&mut stream, ENGINE, CORE, &client_lookup(), &key).await
}

fn assert_error<T>(result: &Result<T, PreauthError>, expected: PreauthError) {
    match result {
        Err(actual) => assert_eq!(*actual, expected),
        Ok(_) => panic!("expected {expected:?}, got a successful preauth result"),
    }
}

async fn admit_valid(listener: &PreauthListener, socket: &SocketPath, key: [u8; 32]) {
    let server = listener.accept_preface();
    let client = authenticate_valid_client(socket, key);
    let (server, client) = tokio::join!(server, client);
    let server = server.expect("server accepts valid preface");
    let client = client.expect("client accepts valid preface");
    assert_eq!(server.binding().remote_peer, core_peer());
    assert_eq!(client.remote_peer, engine_peer());
    drop(server.into_stream());
}

async fn send_hello(socket: &SocketPath, version: u16) {
    let mut stream = UnixStream::connect(socket.as_path())
        .await
        .expect("connect raw client");
    let mut hello = [0_u8; 42];
    hello[..8].copy_from_slice(&AUTH_MAGIC);
    hello[8..10].copy_from_slice(&version.to_be_bytes());
    hello[10..].copy_from_slice(&[0x11; 32]);
    stream.write_all(&hello).await.expect("write fixed hello");
}

async fn connect_then_eof(socket: &SocketPath) {
    drop(
        UnixStream::connect(socket.as_path())
            .await
            .expect("connect raw client"),
    );
}

async fn send_client_proof(socket: &SocketPath, key: [u8; 32], mutate_proof_byte: Option<usize>) {
    let mut stream = UnixStream::connect(socket.as_path())
        .await
        .expect("connect raw client");
    let client_nonce = [0x33; 32];
    let mut hello = [0_u8; 42];
    hello[..8].copy_from_slice(&AUTH_MAGIC);
    hello[8..10].copy_from_slice(&AUTH_VERSION.to_be_bytes());
    hello[10..].copy_from_slice(&client_nonce);
    stream.write_all(&hello).await.expect("write hello");

    let mut server_proof = [0_u8; SERVER_PROOF_LEN];
    stream
        .read_exact(&mut server_proof)
        .await
        .expect("read server proof");
    let mut server_nonce = [0_u8; 32];
    server_nonce.copy_from_slice(&server_proof[10..42]);
    let mut proof = mac(&key, b"client", client_nonce, server_nonce, CORE);
    if let Some(index) = mutate_proof_byte {
        proof[index] ^= 0x80;
    }
    stream.write_all(&proof).await.expect("write client proof");
}

fn mac(
    key: &[u8; 32],
    domain: &[u8],
    client_nonce: [u8; 32],
    server_nonce: [u8; 32],
    core: ExpectedPeer,
) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("fixed HMAC key length");
    mac.update(domain);
    mac.update(&AUTH_VERSION.to_be_bytes());
    mac.update(&client_nonce);
    mac.update(&server_nonce);
    mac.update(&core.uid().to_be_bytes());
    mac.update(&core.pid().to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let mut result = [0_u8; 32];
    result.copy_from_slice(&digest);
    result
}

#[tokio::test]
async fn valid_real_uds_preface_consumes_only_its_listener_key() {
    let runtime = TestRuntime::new("valid");
    let listener = bind(&runtime, KEY_A);
    let generated_key = AuthKey::random().expect("generate independent listener key");
    assert_eq!(format!("{generated_key:?}"), "AuthKey([REDACTED])");

    admit_valid(&listener, &runtime.socket, KEY_A).await;
    assert!(listener.key_is_consumed().expect("read key state"));

    let server = listener.accept_preface();
    let client = authenticate_valid_client(&runtime.socket, KEY_A);
    let (server, _client) = tokio::join!(server, client);
    assert_error(&server, PreauthError::AuthReplay);

    listener.cleanup().expect("cleanup unchanged endpoint");
}

#[tokio::test]
async fn bad_mac_every_byte_does_not_consume_key_and_allows_retry() {
    let runtime = TestRuntime::new("mac");
    let listener = bind(&runtime, KEY_A);

    for byte in 0..32 {
        let server = listener.accept_preface();
        let client = send_client_proof(&runtime.socket, KEY_A, Some(byte));
        let (server, ()) = tokio::join!(server, client);
        match server {
            Err(error) => assert_eq!(error, PreauthError::AuthFailed, "byte {byte}"),
            Ok(_) => panic!("byte {byte} unexpectedly authenticated"),
        }
        assert!(!listener.key_is_consumed().expect("read key state"));
    }

    admit_valid(&listener, &runtime.socket, KEY_A).await;
    listener.cleanup().expect("cleanup unchanged endpoint");
}

#[tokio::test]
async fn version_eof_and_peer_mismatch_are_fail_closed_but_do_not_consume_key() {
    let runtime = TestRuntime::new("retry");
    let peer_lookup: Arc<dyn PeerLookup> = Arc::new(SequencedPeerLookup::new([
        VerifiedLocalPeer::new(CORE.uid() + 1, 20, CORE.pid()),
        core_peer(),
        core_peer(),
        core_peer(),
        core_peer(),
    ]));
    let listener = PreauthListener::bind(
        ListenerConfig::new(
            runtime.socket.clone(),
            RuntimeOwner::current(),
            CORE,
            AuthKey::from_bytes(KEY_A),
        )
        .with_peer_lookup(peer_lookup),
    )
    .expect("bind test listener");

    let server = listener.accept_preface();
    let client = connect_then_eof(&runtime.socket);
    let (server, ()) = tokio::join!(server, client);
    assert_error(&server, PreauthError::PeerMismatch);
    assert!(!listener.key_is_consumed().expect("read key state"));

    for version in [0, 2] {
        let server = listener.accept_preface();
        let client = send_hello(&runtime.socket, version);
        let (server, ()) = tokio::join!(server, client);
        assert_error(&server, PreauthError::AuthVersion);
        assert!(!listener.key_is_consumed().expect("read key state"));
    }

    let server = listener.accept_preface();
    let client = connect_then_eof(&runtime.socket);
    let (server, ()) = tokio::join!(server, client);
    assert_error(&server, PreauthError::AuthFrame);
    assert!(!listener.key_is_consumed().expect("read key state"));

    admit_valid(&listener, &runtime.socket, KEY_A).await;
    listener.cleanup().expect("cleanup unchanged endpoint");
}

#[tokio::test]
async fn listener_keys_are_isolated_and_cross_use_fails() {
    let first_runtime = TestRuntime::new("first");
    let second_runtime = TestRuntime::new("second");
    let first = bind(&first_runtime, KEY_A);
    let second = bind(&second_runtime, KEY_B);

    let server = second.accept_preface();
    let client = send_client_proof(&second_runtime.socket, KEY_A, None);
    let (server, ()) = tokio::join!(server, client);
    assert_error(&server, PreauthError::AuthFailed);
    assert!(!first.key_is_consumed().expect("read first key state"));
    assert!(!second.key_is_consumed().expect("read second key state"));

    admit_valid(&second, &second_runtime.socket, KEY_B).await;
    first.cleanup().expect("cleanup first endpoint");
    second.cleanup().expect("cleanup second endpoint");
}

#[tokio::test]
async fn production_accept_loop_continues_after_a_bad_preface() {
    let runtime = TestRuntime::new("loop");
    let listener = bind(&runtime, KEY_A);

    let server = listener.accept_until_authenticated();
    let clients = async {
        send_hello(&runtime.socket, 0).await;
        authenticate_valid_client(&runtime.socket, KEY_A).await
    };
    let (server, client) = tokio::join!(server, clients);
    let server = server.expect("listener continues to a valid preface");
    client.expect("valid retry succeeds");
    drop(server.into_stream());
    assert!(listener.key_is_consumed().expect("read key state"));
    listener.cleanup().expect("cleanup unchanged endpoint");
}

#[tokio::test]
async fn client_binding_returns_the_real_os_verified_engine_peer() {
    let runtime = TestRuntime::new("real-peer");
    let process_identity = ExpectedPeer::current_process();
    let listener = PreauthListener::bind(ListenerConfig::new(
        runtime.socket.clone(),
        RuntimeOwner::current(),
        process_identity,
        AuthKey::from_bytes(KEY_A),
    ))
    .expect("bind real credential listener");

    let server = listener.accept_preface();
    let client = async {
        let mut stream = UnixStream::connect(runtime.socket.as_path())
            .await
            .expect("connect real credential client");
        let key = AuthKey::from_bytes(KEY_A);
        authenticate_client(
            &mut stream,
            process_identity,
            process_identity,
            &SystemPeerLookup,
            &key,
        )
        .await
    };
    let (server, client) = tokio::join!(server, client);
    let expected = VerifiedLocalPeer::new(
        RuntimeOwner::current().uid(),
        RuntimeOwner::current().gid(),
        std::process::id(),
    );
    let server = server.expect("server accepts real credential client");
    let client = client.expect("client verifies real engine credential");
    assert_eq!(client.remote_peer, expected);
    assert_eq!(client.core_peer, expected);
    assert_eq!(server.binding().remote_peer, expected);
    drop(server.into_stream());
    listener.cleanup().expect("cleanup unchanged endpoint");
}

#[tokio::test]
async fn per_connection_timeout_does_not_consume_key_before_retry() {
    let runtime = TestRuntime::new("timeout");
    let listener = PreauthListener::bind(
        ListenerConfig::new(
            runtime.socket.clone(),
            RuntimeOwner::current(),
            CORE,
            AuthKey::from_bytes(KEY_A),
        )
        .with_peer_lookup(server_lookup())
        .with_global_deadline(Duration::from_secs(12)),
    )
    .expect("bind timeout listener");

    let server = listener.accept_preface();
    let idle_client = async {
        let _stream = UnixStream::connect(runtime.socket.as_path())
            .await
            .expect("connect idle client");
        tokio::time::sleep(Duration::from_secs(6)).await;
    };
    let (server, ()) = tokio::join!(server, idle_client);
    assert_error(&server, PreauthError::AuthTimeout);
    assert!(!listener.key_is_consumed().expect("read key state"));

    admit_valid(&listener, &runtime.socket, KEY_A).await;
    listener.cleanup().expect("cleanup unchanged endpoint");
}

#[tokio::test]
async fn client_rejects_a_tampered_server_nonce_before_sending_client_proof() {
    let runtime = TestRuntime::new("nonce");
    let raw_listener = UnixListener::bind(runtime.socket.as_path()).expect("bind fake server");
    let server = async move {
        let (mut stream, _) = raw_listener.accept().await.expect("accept client");
        let mut hello = [0_u8; 42];
        stream.read_exact(&mut hello).await.expect("read hello");
        let mut client_nonce = [0_u8; 32];
        client_nonce.copy_from_slice(&hello[10..]);
        let server_nonce = [0x44; 32];
        let server_mac = mac(&KEY_A, b"server", client_nonce, server_nonce, CORE);
        let mut proof = [0_u8; SERVER_PROOF_LEN];
        proof[..8].copy_from_slice(&AUTH_MAGIC);
        proof[8..10].copy_from_slice(&AUTH_VERSION.to_be_bytes());
        proof[10..42].copy_from_slice(&server_nonce);
        proof[42..].copy_from_slice(&server_mac);
        proof[10] ^= 0x01;
        stream
            .write_all(&proof)
            .await
            .expect("write tampered proof");
    };
    let client = authenticate_valid_client(&runtime.socket, KEY_A);
    let ((), client) = tokio::join!(server, client);
    assert_eq!(client, Err(PreauthError::AuthFailed));
}

#[test]
fn socket_path_and_runtime_directory_validation_fail_closed() {
    assert_eq!(
        SocketPath::new("relative.sock"),
        Err(PreauthError::PathInvalid)
    );
    let too_long = PathBuf::from(format!("/{}", "a".repeat(104)));
    assert_eq!(SocketPath::new(too_long), Err(PreauthError::PathInvalid));

    let runtime = TestRuntime::new("path");
    let owner = RuntimeOwner::current();
    let metadata = fs::metadata(&runtime.directory).expect("read runtime directory metadata");
    assert_eq!(metadata.uid(), owner.uid());
    assert_eq!(metadata.gid(), owner.gid());
    validate_runtime_dir(&runtime.directory, owner).expect("0700 owner directory is valid");
    fs::set_permissions(&runtime.directory, fs::Permissions::from_mode(0o755))
        .expect("relax mode for negative test");
    assert_eq!(
        validate_runtime_dir(&runtime.directory, owner),
        Err(PreauthError::PathInvalid)
    );
}

#[tokio::test]
async fn occupied_endpoint_and_replaced_inode_are_never_unlinked_blindly() {
    let runtime = TestRuntime::new("endpoint");
    fs::write(runtime.socket.as_path(), b"not a socket").expect("occupy endpoint");
    let occupied = PreauthListener::bind(
        ListenerConfig::new(
            runtime.socket.clone(),
            RuntimeOwner::current(),
            CORE,
            AuthKey::from_bytes(KEY_A),
        )
        .with_peer_lookup(server_lookup()),
    );
    assert_eq!(occupied.err(), Some(PreauthError::EndpointExists));
    fs::remove_file(runtime.socket.as_path()).expect("remove test ordinary file");

    let listener = bind(&runtime, KEY_A);
    fs::remove_file(runtime.socket.as_path()).expect("replace bound endpoint");
    fs::write(runtime.socket.as_path(), b"replacement").expect("write replacement endpoint");
    assert_eq!(listener.cleanup(), Err(PreauthError::CleanupRefused));
}

#[tokio::test]
async fn publish_failure_cleanup_is_guarded_and_never_deletes_a_replacement() {
    let cleanup_runtime = TestRuntime::new("publish-cleanup");
    let failed_publish = PreauthListener::bind(
        ListenerConfig::new(
            cleanup_runtime.socket.clone(),
            RuntimeOwner::current(),
            CORE,
            AuthKey::from_bytes(KEY_A),
        )
        .with_peer_lookup(server_lookup())
        .with_publish_barrier_hook(Arc::new(FailPublishBarrier)),
    );
    assert_error(&failed_publish, PreauthError::Transport);
    assert!(
        !cleanup_runtime.socket.as_path().exists(),
        "guarded cleanup removes only the bound endpoint"
    );

    let replacement_runtime = TestRuntime::new("publish-replace");
    let replaced_publish = PreauthListener::bind(
        ListenerConfig::new(
            replacement_runtime.socket.clone(),
            RuntimeOwner::current(),
            CORE,
            AuthKey::from_bytes(KEY_A),
        )
        .with_peer_lookup(server_lookup())
        .with_publish_barrier_hook(Arc::new(ReplaceBoundEndpoint)),
    );
    assert_error(&replaced_publish, PreauthError::PathInvalid);
    assert_eq!(
        fs::read(replacement_runtime.socket.as_path()).expect("replacement remains present"),
        b"foreign endpoint"
    );
}

#[tokio::test]
async fn system_peer_lookup_reads_macos_uid_and_pid_from_a_real_uds() {
    let runtime = TestRuntime::new("peer");
    let listener = UnixListener::bind(runtime.socket.as_path()).expect("bind raw listener");
    let server = async move {
        let (stream, _) = listener.accept().await.expect("accept raw client");
        SystemPeerLookup.inspect(&stream)
    };
    let client = UnixStream::connect(runtime.socket.as_path());
    let (peer, client) = tokio::join!(server, client);
    drop(client.expect("connect raw client"));
    let peer = peer.expect("read Darwin peer credentials");
    assert_eq!(peer.uid(), RuntimeOwner::current().uid());
    assert_eq!(peer.gid(), RuntimeOwner::current().gid());
    assert_eq!(peer.pid(), std::process::id());
    assert_eq!(
        ExpectedPeer::current_process().uid(),
        RuntimeOwner::current().uid()
    );
    assert_eq!(ExpectedPeer::current_process().pid(), std::process::id());
}
