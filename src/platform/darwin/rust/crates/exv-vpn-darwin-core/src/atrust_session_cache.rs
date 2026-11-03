//! aTrust 会话缓存（M5，设计 §4.2）：`~/.exv` 文件加密体系复用。
//!
//! 规则照抄 Windows 踩坑 10：
//! - **按网关 SHA256 命名独立缓存文件**（`atrust-session-<sha8>.bin`，键取 Common
//!   `GatewayOrigin` 规范化端点的 SHA256 前 8 个十六进制字符）——不同网关互不串台；
//! - **恢复 Cookie 仅作候选**——本层不判有效期、不发任何网络探针；复核落在 engine
//!   侧 Common `browser_handoff` 编排内部（`auth_config(need_ticket=false)` 的
//!   `isLogin` 源会话判定），core 不另造 HTTP 探针（设计 §4.2 裁决）；
//! - **写入时序（M1 冻结规格）**：core 收到 UI v2 会话后**先写缓存再发连接**，连接
//!   失败**不回滚**缓存——候选复核兜底（调用方见
//!   `kernel_control_service::connect_payload_and_plan` 的 aTrust 分支）。
//!
//! 加密与已保存密码同一实现：同一 `key.bin`（`DarwinStoredConfig::ensure_key`）、
//! 同一 AES-256-GCM cipher 语义（`config_crypto::encrypt_bytes`——文件形态为原始
//! `nonce || tag || ciphertext`，不进 JSON 故无 base64 层）。缓存文件本体是密文；
//! 明文 wire 只存在于 `Zeroizing` 容器内。
//!
//! 生产失效回执仅遗忘本次候选的密文版本；调用方以配置锁串行写入与条件删除。
//! 非认证失败保留候选。损坏文件仍在 `load_candidate` 内遗忘。

use std::path::{Path, PathBuf};

use exv_vpn_darwin_ipc::browser_session::{BrowserSession, GatewayOrigin};
use sha2::{Digest, Sha256};

use crate::config::{DarwinConfigError, DarwinStoredConfig};
use crate::config_crypto::{decrypt_bytes, encrypt_bytes};
use crate::config_paths::atomic_write;

const CACHE_FILE_PREFIX: &str = "atrust-session-";
const CACHE_FILE_SUFFIX: &str = ".bin";

/// 前 4 字节 → 8 个十六进制字符（查表编码，不经 format! 迭代收集）。
const HEX: &[u8; 16] = b"0123456789abcdef";

/// 规范化端点 → 缓存文件名（SHA256 前 8 个十六进制字符；win32 `session-<sha>.dpapi`
/// 同款键控法的 darwin 形态——本宿主文件名带 `atrust-` 前缀避免与未来其它缓存混名）。
#[must_use]
pub(crate) fn cache_path(dir: &Path, canonical_gateway: &str) -> PathBuf {
    let digest = Sha256::digest(canonical_gateway.as_bytes());
    let sha8: String = digest
        .iter()
        .take(4)
        .flat_map(|byte| {
            [
                HEX[usize::from(byte >> 4)] as char,
                HEX[usize::from(byte & 0x0f)] as char,
            ]
        })
        .collect();
    dir.join(format!("{CACHE_FILE_PREFIX}{sha8}{CACHE_FILE_SUFFIX}"))
}

/// Common `GatewayOrigin` 规范化端点（`https://host:port`）——缓存键与
/// [`BrowserSession::gateway`] 校验共用同一权威，不做第二份解析。
fn canonical_endpoint(gateway: &str) -> Result<String, DarwinConfigError> {
    GatewayOrigin::parse(gateway)
        .map(|origin| origin.endpoint().to_owned())
        .map_err(|_| DarwinConfigError::InvalidAtrustServer)
}

#[derive(serde::Serialize)]
struct BoundSession<'a> {
    #[serde(flatten)]
    session: &'a BrowserSession,
    #[serde(skip_serializing_if = "str::is_empty")]
    authenticated_username: &'a str,
}
#[derive(serde::Deserialize, Default)]
struct UsernameMetadata {
    #[serde(default)]
    authenticated_username: String,
}

/// 写入候选会话（先写缓存再发连接；失败不回滚由调用方保证——本函数只做一次原子写）。
///
/// # Errors
///
/// 网关不可规范化 → [`DarwinConfigError::InvalidAtrustServer`]；密钥缺失/生成失败、
/// 会话编码或加密失败、落盘失败 → 其余 [`DarwinConfigError`]。错误不包含会话内容。
pub(crate) fn save(dir: &Path, session: &BrowserSession) -> Result<(), DarwinConfigError> {
    let username = username_for(dir, session).unwrap_or_default();
    save_with_username(dir, session, &username)
}

/// 非秘密账户与当前候选一起沿原加密文件保存。
pub(crate) fn save_with_username(dir: &Path, session: &BrowserSession, username: &str) -> Result<(), DarwinConfigError> {
    let preserved = if username.trim().is_empty() { username_for(dir, session) } else { None };
    let username = preserved.as_deref().unwrap_or(username);
    let canonical = canonical_endpoint(session.gateway())?;
    let key = DarwinStoredConfig::ensure_key(dir)?;
    session.encode().map_err(|_| DarwinConfigError::PasswordCrypto)?;
    let encoded = zeroize::Zeroizing::new(serde_json::to_vec(&BoundSession { session, authenticated_username: username.trim() }).map_err(|_| DarwinConfigError::PasswordCrypto)?);
    let sealed = encrypt_bytes(&encoded, &key)?;
    atomic_write(&cache_path(dir, &canonical), &sealed).map_err(|_| DarwinConfigError::Storage)
}

/// 旧缓存缺身份时为空；只接受完整匹配的当前会话绑定。
pub(crate) fn username_for(dir: &Path, session: &BrowserSession) -> Option<String> {
    let canonical = canonical_endpoint(session.gateway()).ok()?;
    let sealed = std::fs::read(cache_path(dir, &canonical)).ok()?;
    let key = DarwinStoredConfig::load_key(dir).ok()??;
    let bytes = decrypt_bytes(&sealed, &key).ok()?;
    let restored = BrowserSession::decode(&bytes).ok()?;
    if restored.encode().ok()?.as_slice() != session.encode().ok()?.as_slice() { return None; }
    let metadata: UsernameMetadata = serde_json::from_slice(&bytes).ok()?;
    let username = metadata.authenticated_username.trim();
    (!username.is_empty()).then(|| username.to_owned())
}

/// 恢复指定网关的**候选**会话；调用方必须让 engine 向该网关复核后才可用于业务。
///
/// 文件缺失 → `None`；密文损坏/篡改（认证标签失配）或解码失败 → 就地遗忘文件并
/// 返回 `None`（自愈：下一次 UI 采集重写缓存）；会话与请求网关不同源 → 同样遗忘
/// 并返回 `None`（防手工搬运文件跨网关串台）。任何失败都不阻断连接受理。
#[must_use]
pub(crate) fn load_candidate(dir: &Path, gateway: &str) -> Option<BrowserSession> {
    let canonical = canonical_endpoint(gateway).ok()?;
    let path = cache_path(dir, &canonical);
    let sealed = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return None,
    };
    let restore = || -> Option<BrowserSession> {
        let key = DarwinStoredConfig::ensure_key(dir).ok()?;
        let wire = decrypt_bytes(&sealed, &key).ok()?;
        let session = BrowserSession::decode(wire.as_slice()).ok()?;
        // 键与内容同源断言：文件名键自规范化端点，内容网关必须规范化到同一端点。
        canonical_endpoint(session.gateway())
            .ok()
            .filter(|endpoint| endpoint == &canonical)?;
        Some(session)
    };
    let restored = restore();
    if restored.is_none() {
        // 损坏/篡改/串台：遗忘坏文件（best effort），按无候选处理。
        let _ = std::fs::remove_file(&path);
    }
    restored
}

/// 本次受理候选的密文版本；不保存或输出 Cookie/明文会话摘要。
pub(crate) struct CandidateReceipt {
    path: PathBuf,
    sealed_digest: Vec<u8>,
}

/// 必须与 save/load 同处调用方的配置锁下；检查文件确为此次提交的候选。
pub(crate) fn receipt(dir: &Path, session: &BrowserSession) -> Option<CandidateReceipt> {
    let canonical = canonical_endpoint(session.gateway()).ok()?;
    let path = cache_path(dir, &canonical);
    let sealed = std::fs::read(&path).ok()?;
    let key = DarwinStoredConfig::ensure_key(dir).ok()?;
    let restored = decrypt_bytes(&sealed, &key).ok()?;
    if BrowserSession::decode(&restored).ok()?.encode().ok()?.as_slice() != session.encode().ok()?.as_slice() {
        return None;
    }
    Some(CandidateReceipt {
        path,
        sealed_digest: Sha256::digest(&sealed).to_vec(),
    })
}

impl CandidateReceipt {
    /// 只比较当前密文版本，不解密、不修改缓存；调用方继续持有原配置锁。
    pub(crate) fn is_current(&self) -> std::io::Result<bool> {
        let sealed = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        Ok(Sha256::digest(&sealed).as_slice() == self.sealed_digest)
    }

    /// 调用方持配置锁，和 save 串行；迟到回执不删除后来写入的新版本。
    pub(crate) fn forget_if_current(&self) -> std::io::Result<bool> {
        if !self.is_current()? {
            return Ok(false);
        }
        std::fs::remove_file(&self.path)?;
        Ok(true)
    }
}

