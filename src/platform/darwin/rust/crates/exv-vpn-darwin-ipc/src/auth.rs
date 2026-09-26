//! `DarwinAuthV1` 的认证前固定长度握手。
//!
//! 本模块只建立本地 UDS 的认证绑定。认证成功后的首个应用层字节必须由后续的
//! tonic bridge 写入；这里绝不定义第二套业务 wire。

use std::{fmt, sync::Mutex, time::Duration};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time,
};
use zeroize::Zeroize;

use crate::peer::{ExpectedPeer, PeerLookup, VerifiedLocalPeer, verify_peer};

/// `DarwinAuthV1` 的八字节固定 magic。
pub const AUTH_MAGIC: [u8; 8] = *b"EXVDIPC\0";
/// 当前唯一允许的认证版本。
pub const AUTH_VERSION: u16 = 1;
/// 每个 client/server nonce 的固定长度。
pub const NONCE_LEN: usize = 32;
/// listener 单次认证 key 的固定长度。
pub const AUTH_KEY_LEN: usize = 32;
/// `ClientHello` 的固定长度。
pub const CLIENT_HELLO_LEN: usize = 8 + 2 + NONCE_LEN;
/// `ServerProof` 的固定长度。
pub const SERVER_PROOF_LEN: usize = 8 + 2 + NONCE_LEN + MAC_LEN;
/// `ClientProof` 的固定长度。
pub const CLIENT_PROOF_LEN: usize = MAC_LEN;
/// 单条连接的认证截止时间。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

const MAC_LEN: usize = 32;
const SERVER_DOMAIN: &[u8] = b"server";
const CLIENT_DOMAIN: &[u8] = b"client";

/// 认证前和 endpoint 生命周期中的稳定 transport 错误类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreauthError {
    /// runtime path 不满足本地 socket 的安全约束。
    PathInvalid,
    /// 目标 endpoint 已存在，不能先删除再 bind。
    EndpointExists,
    /// endpoint 或运行目录的 uid/gid 不匹配。
    EndpointOwnership,
    /// OS 提供的 uid/pid 与预期 peer 不匹配。
    PeerMismatch,
    /// 固定认证帧使用了未知版本。
    AuthVersion,
    /// 固定认证帧截断、magic 错误或结构非法。
    AuthFrame,
    /// HMAC 校验失败。
    AuthFailed,
    /// 已成功消费的 listener key 被再次使用。
    AuthReplay,
    /// 等待连接或认证帧超时。
    AuthTimeout,
    /// 无法读取或写入本地 transport。
    Transport,
    /// endpoint 已变化，拒绝清理。
    CleanupRefused,
}

impl fmt::Display for PreauthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::PathInvalid => "Darwin IPC path is invalid",
            Self::EndpointExists => "Darwin IPC endpoint already exists",
            Self::EndpointOwnership => "Darwin IPC endpoint ownership is invalid",
            Self::PeerMismatch => "Darwin IPC peer does not match the expected uid/pid",
            Self::AuthVersion => "Darwin IPC authentication version is unsupported",
            Self::AuthFrame => "Darwin IPC authentication frame is malformed",
            Self::AuthFailed => "Darwin IPC authentication proof is invalid",
            Self::AuthReplay => "Darwin IPC authentication key was already consumed",
            Self::AuthTimeout => "Darwin IPC authentication timed out",
            Self::Transport => "Darwin IPC transport failed",
            Self::CleanupRefused => "Darwin IPC endpoint cleanup was refused",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PreauthError {}

/// 只存在于内存中的单 listener 认证 key。
///
/// `Debug` 不会泄露 key 字节；key 在 drop 和成功消费时都会清零。
pub struct AuthKey([u8; AUTH_KEY_LEN]);

impl AuthKey {
    /// 生成一个独立 listener 的随机 32 字节 key。
    ///
    /// # Errors
    ///
    /// 当系统随机源不可用时返回 [`PreauthError::Transport`]。
    pub fn random() -> Result<Self, PreauthError> {
        let mut bytes = [0_u8; AUTH_KEY_LEN];
        getrandom::fill(&mut bytes).map_err(|_| PreauthError::Transport)?;
        Ok(Self(bytes))
    }

    /// 从调用方已安全提供的字节构造 key。
    ///
    /// 该构造器同时是测试注入 seam；生产调用方不得复用同一数组给两个 listener。
    #[must_use]
    pub const fn from_bytes(bytes: [u8; AUTH_KEY_LEN]) -> Self {
        Self(bytes)
    }

    fn bytes(&self) -> &[u8; AUTH_KEY_LEN] {
        &self.0
    }
}

impl AuthKey {
    fn duplicate_for_transcript(&self) -> Self {
        Self(self.0)
    }
}

impl fmt::Debug for AuthKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthKey([REDACTED])")
    }
}

impl Drop for AuthKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

enum KeyState {
    Available(AuthKey),
    Consumed,
}

/// listener 作用域的单次 key 状态机。
///
/// 多条并发连接可读取同一个可用 key 进行 proof 校验，但只有第一个完成
/// `ClientProof` constant-time 校验的连接能够原子地消费它。
pub struct OneTimeAuthenticator {
    state: Mutex<KeyState>,
}

impl OneTimeAuthenticator {
    /// 创建一个拥有独立 key 的 listener 认证器。
    #[must_use]
    pub fn new(key: AuthKey) -> Self {
        Self {
            state: Mutex::new(KeyState::Available(key)),
        }
    }

    fn available_key(&self) -> Result<AuthKey, PreauthError> {
        let state = self.state.lock().map_err(|_| PreauthError::Transport)?;
        match &*state {
            KeyState::Available(key) => Ok(key.duplicate_for_transcript()),
            KeyState::Consumed => Err(PreauthError::AuthReplay),
        }
    }

    fn consume_after_valid_client_proof(&self) -> Result<(), PreauthError> {
        let mut state = self.state.lock().map_err(|_| PreauthError::Transport)?;
        if matches!(&*state, KeyState::Consumed) {
            return Err(PreauthError::AuthReplay);
        }

        // Replacing the state while holding the mutex makes consumption atomic. The former
        // `AuthKey` drops at scope exit and wipes its bytes before the mutex is released.
        let consumed = std::mem::replace(&mut *state, KeyState::Consumed);
        drop(consumed);
        Ok(())
    }

    /// 返回 key 是否已经被一个完整 `ClientProof` 成功消费。
    ///
    /// # Errors
    ///
    /// 当内部状态锁不可用时返回 [`PreauthError::Transport`]。
    pub fn is_consumed(&self) -> Result<bool, PreauthError> {
        let state = self.state.lock().map_err(|_| PreauthError::Transport)?;
        Ok(matches!(&*state, KeyState::Consumed))
    }
}

/// 已认证握手产生的不可伪造绑定材料。
///
/// `remote_peer` 始终是调用方通过 OS credential 实际验证的对端：Core client
/// 返回 Engine peer，Engine server 返回 Core peer。它绝不能由 HMAC transcript 或
/// 任何 wire 字段补造。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticatedBinding {
    /// 通过 OS credential 验证得到的远端 uid/gid/pid。
    pub remote_peer: VerifiedLocalPeer,
    /// 与 [`Self::remote_peer`] 相同的兼容字段。
    ///
    /// 旧 fixture 曾把此字段理解为 Core peer；现在它绝不构造 gid=0 的假 peer，且
    /// 对 client 同样保存实际验证到的 Engine peer。新代码应读取 `remote_peer`。
    pub core_peer: VerifiedLocalPeer,
    /// Core 创建的 nonce。
    pub client_nonce: [u8; NONCE_LEN],
    /// Engine 创建的 nonce。
    pub server_nonce: [u8; NONCE_LEN],
}

/// 作为 Core 的 UDS client 完成固定 `DarwinAuthV1` transcript。
///
/// 调用方传入的 `expected_server` 先由 OS credential 校验；校验失败时不会发送
/// `ClientHello`。`core_identity` 必须是当前 Core 进程的 uid/pid，而不是 wire 中的
/// 自报字段。
///
/// # Errors
///
/// 当 OS peer、认证帧、HMAC、I/O 或握手 deadline 校验失败时返回相应的
/// [`PreauthError`]，且不会把失败连接认证为可用。
pub async fn authenticate_client(
    stream: &mut UnixStream,
    expected_server: ExpectedPeer,
    core_identity: ExpectedPeer,
    peer_lookup: &dyn PeerLookup,
    key: &AuthKey,
) -> Result<AuthenticatedBinding, PreauthError> {
    time::timeout(
        HANDSHAKE_TIMEOUT,
        authenticate_client_inner(stream, expected_server, core_identity, peer_lookup, key),
    )
    .await
    .map_err(|_| PreauthError::AuthTimeout)?
}

async fn authenticate_client_inner(
    stream: &mut UnixStream,
    expected_server: ExpectedPeer,
    core_identity: ExpectedPeer,
    peer_lookup: &dyn PeerLookup,
    key: &AuthKey,
) -> Result<AuthenticatedBinding, PreauthError> {
    let remote_peer = verify_peer(peer_lookup, stream, expected_server)?;

    let client_nonce = random_nonce()?;
    let mut hello = [0_u8; CLIENT_HELLO_LEN];
    hello[..AUTH_MAGIC.len()].copy_from_slice(&AUTH_MAGIC);
    hello[AUTH_MAGIC.len()..AUTH_MAGIC.len() + 2].copy_from_slice(&AUTH_VERSION.to_be_bytes());
    hello[AUTH_MAGIC.len() + 2..].copy_from_slice(&client_nonce);
    write_frame(stream, &hello).await?;

    let server_proof = read_frame::<SERVER_PROOF_LEN>(stream).await?;
    let server_nonce = parse_server_proof(&server_proof, key, client_nonce, core_identity)?;

    let client_proof = calculate_mac(
        key,
        CLIENT_DOMAIN,
        client_nonce,
        server_nonce,
        core_identity,
    )?;
    write_frame(stream, &client_proof).await?;

    Ok(AuthenticatedBinding {
        remote_peer,
        core_peer: remote_peer,
        client_nonce,
        server_nonce,
    })
}

/// 作为 Engine 的 UDS server 完成固定 `DarwinAuthV1` transcript。
///
/// 只有本函数已经对 `ClientProof` 成功完成 constant-time 校验后，才消费
/// `authenticator` 的 key。此前任意 peer/frame/MAC/EOF/timeout 失败都保持 key 可用。
///
/// # Errors
///
/// 当 OS peer、认证帧、HMAC、I/O 或握手 deadline 校验失败时返回相应的
/// [`PreauthError`]；失败不会消费 key。
pub async fn authenticate_server(
    stream: &mut UnixStream,
    expected_core: ExpectedPeer,
    peer_lookup: &dyn PeerLookup,
    authenticator: &OneTimeAuthenticator,
) -> Result<AuthenticatedBinding, PreauthError> {
    time::timeout(
        HANDSHAKE_TIMEOUT,
        authenticate_server_inner(stream, expected_core, peer_lookup, authenticator),
    )
    .await
    .map_err(|_| PreauthError::AuthTimeout)?
}

async fn authenticate_server_inner(
    stream: &mut UnixStream,
    expected_core: ExpectedPeer,
    peer_lookup: &dyn PeerLookup,
    authenticator: &OneTimeAuthenticator,
) -> Result<AuthenticatedBinding, PreauthError> {
    let core_peer = verify_peer(peer_lookup, stream, expected_core)?;
    let key = authenticator.available_key()?;
    let hello = read_frame::<CLIENT_HELLO_LEN>(stream).await?;
    let client_nonce = parse_client_hello(&hello)?;
    let server_nonce = random_nonce()?;

    let server_mac = calculate_mac(
        &key,
        SERVER_DOMAIN,
        client_nonce,
        server_nonce,
        core_peer.identity(),
    )?;
    let mut server_proof = [0_u8; SERVER_PROOF_LEN];
    server_proof[..AUTH_MAGIC.len()].copy_from_slice(&AUTH_MAGIC);
    server_proof[AUTH_MAGIC.len()..AUTH_MAGIC.len() + 2]
        .copy_from_slice(&AUTH_VERSION.to_be_bytes());
    server_proof[AUTH_MAGIC.len() + 2..AUTH_MAGIC.len() + 2 + NONCE_LEN]
        .copy_from_slice(&server_nonce);
    server_proof[AUTH_MAGIC.len() + 2 + NONCE_LEN..].copy_from_slice(&server_mac);
    write_frame(stream, &server_proof).await?;

    let client_proof = read_frame::<CLIENT_PROOF_LEN>(stream).await?;
    if !verify_mac(
        &key,
        CLIENT_DOMAIN,
        client_nonce,
        server_nonce,
        core_peer.identity(),
        &client_proof,
    )? {
        return Err(PreauthError::AuthFailed);
    }

    authenticator.consume_after_valid_client_proof()?;
    Ok(AuthenticatedBinding {
        remote_peer: core_peer,
        core_peer,
        client_nonce,
        server_nonce,
    })
}

fn random_nonce() -> Result<[u8; NONCE_LEN], PreauthError> {
    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|_| PreauthError::Transport)?;
    Ok(nonce)
}

fn parse_client_hello(frame: &[u8; CLIENT_HELLO_LEN]) -> Result<[u8; NONCE_LEN], PreauthError> {
    validate_header(&frame[..AUTH_MAGIC.len() + 2])?;
    let mut nonce = [0_u8; NONCE_LEN];
    nonce.copy_from_slice(&frame[AUTH_MAGIC.len() + 2..]);
    Ok(nonce)
}

fn parse_server_proof(
    frame: &[u8; SERVER_PROOF_LEN],
    key: &AuthKey,
    client_nonce: [u8; NONCE_LEN],
    core_identity: ExpectedPeer,
) -> Result<[u8; NONCE_LEN], PreauthError> {
    validate_header(&frame[..AUTH_MAGIC.len() + 2])?;
    let mut server_nonce = [0_u8; NONCE_LEN];
    server_nonce.copy_from_slice(&frame[AUTH_MAGIC.len() + 2..AUTH_MAGIC.len() + 2 + NONCE_LEN]);
    let mut supplied_mac = [0_u8; MAC_LEN];
    supplied_mac.copy_from_slice(&frame[AUTH_MAGIC.len() + 2 + NONCE_LEN..]);
    if !verify_mac(
        key,
        SERVER_DOMAIN,
        client_nonce,
        server_nonce,
        core_identity,
        &supplied_mac,
    )? {
        return Err(PreauthError::AuthFailed);
    }
    Ok(server_nonce)
}

fn validate_header(frame: &[u8]) -> Result<(), PreauthError> {
    if frame.len() != AUTH_MAGIC.len() + 2 || frame[..AUTH_MAGIC.len()] != AUTH_MAGIC {
        return Err(PreauthError::AuthFrame);
    }

    let version = u16::from_be_bytes([frame[AUTH_MAGIC.len()], frame[AUTH_MAGIC.len() + 1]]);
    if version != AUTH_VERSION {
        return Err(PreauthError::AuthVersion);
    }
    Ok(())
}

fn calculate_mac(
    key: &AuthKey,
    domain: &[u8],
    client_nonce: [u8; NONCE_LEN],
    server_nonce: [u8; NONCE_LEN],
    core_identity: ExpectedPeer,
) -> Result<[u8; MAC_LEN], PreauthError> {
    let mut mac = new_mac(key)?;
    update_transcript(&mut mac, domain, client_nonce, server_nonce, core_identity);
    let bytes = mac.finalize().into_bytes();
    let mut result = [0_u8; MAC_LEN];
    result.copy_from_slice(&bytes);
    Ok(result)
}

fn verify_mac(
    key: &AuthKey,
    domain: &[u8],
    client_nonce: [u8; NONCE_LEN],
    server_nonce: [u8; NONCE_LEN],
    core_identity: ExpectedPeer,
    supplied: &[u8; MAC_LEN],
) -> Result<bool, PreauthError> {
    let mut mac = new_mac(key)?;
    update_transcript(&mut mac, domain, client_nonce, server_nonce, core_identity);
    // `Mac::verify_slice` delegates to the HMAC crate's constant-time verification path.
    Ok(mac.verify_slice(supplied).is_ok())
}

fn new_mac(key: &AuthKey) -> Result<Hmac<Sha256>, PreauthError> {
    Hmac::<Sha256>::new_from_slice(key.bytes()).map_err(|_| PreauthError::Transport)
}

fn update_transcript(
    mac: &mut Hmac<Sha256>,
    domain: &[u8],
    client_nonce: [u8; NONCE_LEN],
    server_nonce: [u8; NONCE_LEN],
    core_identity: ExpectedPeer,
) {
    mac.update(domain);
    mac.update(&AUTH_VERSION.to_be_bytes());
    mac.update(&client_nonce);
    mac.update(&server_nonce);
    mac.update(&core_identity.uid().to_be_bytes());
    mac.update(&core_identity.pid().to_be_bytes());
}

async fn read_frame<const N: usize>(stream: &mut UnixStream) -> Result<[u8; N], PreauthError> {
    let mut frame = [0_u8; N];
    stream
        .read_exact(&mut frame)
        .await
        .map_err(|_| PreauthError::AuthFrame)?;
    Ok(frame)
}

async fn write_frame(stream: &mut UnixStream, frame: &[u8]) -> Result<(), PreauthError> {
    stream
        .write_all(frame)
        .await
        .map_err(|_| PreauthError::Transport)
}
