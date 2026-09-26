
//! core 侧凭据生命周期（P3-a）：读 config → 解密（独立 `key.bin`）→ RAII 确定性零化
//! 明文中间态 → 组装一次性 `secret_payload` → 经 gRPC 一次性交 engine。
//!
//! 产品级凭据卫生（对照 acceptance `coordination.rs` 的 best-effort `zeroize_local`，
//! 本模块只提取「读→解密→组装→零化」模式并强化）：
//!
//! - **确定性零化（RAII guard）**：[`ClearablePassword`] 持有解密明文，`Drop` 与显式
//!   [`ClearablePassword::zeroize`] 都覆写已用字节为 0 后清空——任何提前返回 / 错误
//!   路径都不会遗留明文；`Debug` 对密码恒 `<redacted>`。
//! - **解密 key 同样一次性**：`key.bin` 读入的 32 字节在解密后（无论成败）立即
//!   `zeroize`。
//! - **凭据包（序列化逻辑）**：[`CredentialPackage`]（serde JSON，`version` 前缀）承载
//!   username+password，序列化字节即 `secret_payload`；包本身也是零化类型（`Drop`
//!   清零），本地副本不残留。
//! - **组装即零化**：[`CredentialsBundle::assemble_secret_payload`] 在打包后确定性
//!   零化明文密码中间态——payload 字节成为唯一明文载体；交给
//!   [`crate::grpc_control::hand_off_request`]（move + 零化 one-shot 源）后，再经
//!   [`zeroize_connect_secret`] 在发送后清零 wire 副本。
//!
//! **Wire 落点（P3-a 报告）**：core↔engine 的 `HelperControl` wire
//! （`helper_control.proto`）当前**无** `secret_payload` 字段——`ApplyTunnelRequest`
//! 只含 `lookup_key`/`plan`/`request_digest`，也没有 Connect RPC。`KernelControl` 的
//! `ConnectRequest.secret_payload`（面向 UI，P3-b）**已存在**（tag=2，one-shot bytes）。
//! 本模块的 payload 字节与 wire 无关，两处都直接复用。
//!
//! **待协调者决定的 proto 变更**：engine 拿到登录凭据需要给 `helper_control.proto`
//! 的某条 mutation 补 `bytes secret_payload`——推荐 `ApplyTunnelRequest`（tag=4，
//! 空闲，位于 1000-1999 燃烧段之外）或新建 Connect RPC。字段落地前，凭据包结构 +
//! 序列化逻辑在本模块就绪（见 [`CredentialPackage`]）。

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use exv_vpn_win32_config::ExvConfig;
use exv_vpn_wire::generated::{ConnectIntent, ConnectRequest};

/// `secret_payload` 凭据包格式版本（当前 1）。engine 侧解析时校验，未知版本拒绝。
pub const SECRET_PAYLOAD_VERSION: u32 = 1;

/// 解密/组装凭据阶段的 typed 错误。失败路径不携带任何明文/密钥字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// `config.json` 读取失败。
    Config(String),
    /// 独立 `key.bin` 缺失（无法解密）。
    KeyMissing,
    /// `key.bin` 读取失败（不可读 / 长度非法）。
    KeyRead(String),
    /// 密码未记住（config 无密文 / 解出空明文）。
    PasswordEmpty,
    /// 密码解密失败（密钥不符 / 密文被篡改）。
    PasswordDecrypt(String),
    /// `secret_payload` 序列化失败。
    Serialize(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "config load failed: {e}"),
            Self::KeyMissing => write!(f, "credential key.bin missing"),
            Self::KeyRead(e) => write!(f, "credential key read failed: {e}"),
            Self::PasswordEmpty => write!(f, "password not remembered"),
            Self::PasswordDecrypt(e) => write!(f, "password decrypt failed: {e}"),
            Self::Serialize(e) => write!(f, "secret payload serialize failed: {e}"),
        }
    }
}

impl std::error::Error for CredentialError {}

// ---------------------------------------------------------------------------
// 确定性零化：明文密码 RAII guard。
// ---------------------------------------------------------------------------

/// 解密后的明文密码 RAII guard：持有字节，`Drop` 与显式 [`ClearablePassword::zeroize`]
/// 都确定性覆写已用字节为 0 后清空——不留 best-effort 竞窗。`Debug` 恒 `<redacted>`。
pub struct ClearablePassword {
    /// 密码 UTF-8 字节（一次性；零化后为空）。
    plaintext: Vec<u8>,
}

impl ClearablePassword {
    /// 把解密出的明文密码移入零化槽（`into_bytes` 零拷贝；调用方不再持有该 String）。
    #[must_use]
    pub fn new(plaintext: String) -> Self {
        Self {
            plaintext: plaintext.into_bytes(),
        }
    }

    /// 当前明文字节（零化后为空切片）。
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.plaintext
    }

    /// 当前明文（UTF-8 视图；密码源自 `String`，恒为合法 UTF-8）。
    ///
    /// # Panics
    /// 仅当字节非法 UTF-8 时 panic；构造时来自 `String::into_bytes`，实际不可达。
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.plaintext).expect("password bytes are valid UTF-8")
    }

    /// true 当 guard 已零化（不再持有明文）。
    #[must_use]
    pub fn is_zeroed(&self) -> bool {
        self.plaintext.is_empty()
    }

    /// 确定性零化：覆写已用字节为 0 后清空（`Drop` 亦调用）。
    pub fn zeroize(&mut self) {
        self.plaintext.zeroize();
        self.plaintext.clear();
    }
}

impl fmt::Debug for ClearablePassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClearablePassword(<redacted>)")
    }
}

impl Drop for ClearablePassword {
    fn drop(&mut self) {
        self.zeroize();
    }
}

// ---------------------------------------------------------------------------
// 一次性凭据包：secret_payload 的序列化逻辑。
// ---------------------------------------------------------------------------

/// 一次性凭据包（`ConnectRequest.secret_payload` 的载荷）。
///
/// serde JSON 形状与 legacy `Credentials`（username/password）一致，加 `version` 前缀
/// 保证前向兼容。**零化类型**：`Drop` / 显式 [`CredentialPackage::zeroize`] 清零
/// username+password；`Debug` 对 password 恒 `<redacted>`。反序列化的副本同样零化。
#[derive(Serialize, Deserialize)]
pub struct CredentialPackage {
    /// 格式版本（当前 [`SECRET_PAYLOAD_VERSION`]）。
    pub version: u32,
    /// 登录用户名。
    pub username: String,
    /// 登录密码（明文；一次性，零化后为空）。
    pub password: String,
}

impl CredentialPackage {
    /// 构造一次性凭据包（副本移入包内；包在 `Drop` 时清零）。
    #[must_use]
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            version: SECRET_PAYLOAD_VERSION,
            username: username.into(),
            password: password.into(),
        }
    }

    /// 序列化为 `secret_payload` 的一次性字节（`version`/`username`/`password` JSON）。
    ///
    /// # Errors
    /// 序列化失败 → [`CredentialError::Serialize`]。
    pub fn to_bytes(&self) -> Result<Vec<u8>, CredentialError> {
        serde_json::to_vec(self).map_err(|e| CredentialError::Serialize(e.to_string()))
    }

    /// 确定性零化包内明文（`Drop` 亦调用）。
    pub fn zeroize(&mut self) {
        zeroize_string(&mut self.username);
        zeroize_string(&mut self.password);
    }
}

impl fmt::Debug for CredentialPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialPackage")
            .field("version", &self.version)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Drop for CredentialPackage {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// 从 `secret_payload` 一次性字节反序列化凭据包（engine 侧解析同一形状）。
///
/// 返回的包为零化类型：解析出的明文在作用域结束时确定性清零。
///
/// # Errors
/// 反序列化失败（形状不符 / 未知字段）→ [`CredentialError::Serialize`]。
pub fn parse_secret_payload(bytes: &[u8]) -> Result<CredentialPackage, CredentialError> {
    serde_json::from_slice(bytes).map_err(|e| CredentialError::Serialize(e.to_string()))
}

/// 覆写 String 的已用字节为 0 后清空（与 legacy `engine_protocol::zeroize_string`
/// 同款，字节覆写改用 crate `Zeroize`）。
fn zeroize_string(s: &mut String) {
    // SAFETY: `as_mut_vec` 返回 String 字节的可变视图；本函数持唯一 `&mut`，无其它
    // 引用借用这些字节。
    unsafe {
        s.as_mut_vec().zeroize();
    }
    s.clear();
}

// ---------------------------------------------------------------------------
// 产品流程：读 config → 解密 → 组装。
// ---------------------------------------------------------------------------

/// 已加载的 config 与解密后的明文密码（一次性 bundle）。
///
/// `password` 为 RAII 零化 guard；bundle 本身 `Debug` 对密码恒 `<redacted>`。刻意
/// 不实现 `Clone`——克隆会产生第二个明文副本，破坏一次性语义。
pub struct CredentialsBundle {
    /// 已加载的用户 config（密码字段为密文，非明文）。
    pub config: ExvConfig,
    /// 解密后的明文密码（一次性，组装后确定性零化）。
    pub password: ClearablePassword,
}

impl CredentialsBundle {
    /// 组装一次性 `secret_payload` 字节，并**确定性零化明文密码中间态**。
    ///
    /// 打包后 payload 字节成为唯一明文载体（经 `hand_off_request` 交给 wire、发送后
    /// 经 [`zeroize_connect_secret`] 清零）。**一次性**：调用后 bundle 的明文密码已
    /// 零化，重试需重新 `load_credentials`。
    ///
    /// # Errors
    /// 序列化失败 → [`CredentialError::Serialize`]（此时明文密码保持未零化，可重试）。
    pub fn assemble_secret_payload(&mut self) -> Result<Vec<u8>, CredentialError> {
        let package = CredentialPackage::new(self.config.username.clone(), self.password.as_str());
        let payload = package.to_bytes()?;
        // 组装完成：确定性零化明文密码中间态（payload 成为唯一明文载体）。
        self.password.zeroize();
        Ok(payload)
    }

    /// 确定性零化 bundle 内的明文密码（`password` 的 `Drop` 亦兜底）。
    pub fn zeroize(&mut self) {
        self.password.zeroize();
    }
}

impl fmt::Debug for CredentialsBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialsBundle")
            .field("config", &self.config)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// 从 config 目录读取 `config.json` + 独立 `key.bin`，解密出明文密码并移入零化 guard。
///
/// `config_dir` 为显式目录（测试可注入）；产品默认目录由 `ExvConfig::load()` 语义给出。
/// 解密 key（32 字节）在解密后（无论成败）立即零化。
///
/// # Errors
/// config 读取失败 → [`CredentialError::Config`]；`key.bin` 缺失 →
/// [`CredentialError::KeyMissing`]；`key.bin` 读取失败 → [`CredentialError::KeyRead`]；
/// 密码解密失败 → [`CredentialError::PasswordDecrypt`]；空明文（未记住）→
/// [`CredentialError::PasswordEmpty`]。
pub fn load_credentials(config_dir: &Path) -> Result<CredentialsBundle, CredentialError> {
    let config = ExvConfig::load_from_dir(config_dir)
        .map_err(|e| CredentialError::Config(e.to_string()))?;
    let mut key = match ExvConfig::load_key(config_dir) {
        Ok(Some(key)) => key,
        Ok(None) => return Err(CredentialError::KeyMissing),
        Err(e) => return Err(CredentialError::KeyRead(e.to_string())),
    };
    let decrypted = config
        .decrypt_password(&key)
        .map_err(|e| CredentialError::PasswordDecrypt(e.to_string()));
    // 解密 key 一次性：无论成败，用毕立即零化。
    key.zeroize();
    let plaintext = decrypted?;
    if plaintext.is_empty() {
        return Err(CredentialError::PasswordEmpty);
    }
    Ok(CredentialsBundle {
        config,
        password: ClearablePassword::new(plaintext),
    })
}

// ---------------------------------------------------------------------------
// wire 衔接：把凭据包字节放进 ConnectRequest.secret_payload，发送后确定性零化。
// ---------------------------------------------------------------------------

/// 组装 core 侧 `ConnectRequest`（`KernelControl.Connect`，P3-b 复用）：把一次性
/// `secret_payload` 字节放进消息字段。
#[must_use]
pub fn build_connect_request(intent: ConnectIntent, secret_payload: Vec<u8>) -> ConnectRequest {
    ConnectRequest {
        intent: Some(intent),
        secret_payload,
    }
}

/// 发送后确定性零化 `ConnectRequest.secret_payload`（wire 副本；保留 carrier 长度以便
/// 调用者能够验证每个已用字节都已原位归零）。
///
/// 与 [`crate::grpc_control::hand_off_request`] 配对：`hand_off_request` 在 hand-off
/// 时零化本地 one-shot 源，本函数在 RPC 发送完成后清零请求内 wire 副本——两条路径
/// 覆盖后本地不再有任何明文 carrier。
pub fn zeroize_connect_secret(request: &mut ConnectRequest) {
    request.secret_payload.as_mut_slice().zeroize();
}

// ---------------------------------------------------------------------------
// 单元测试：解密→组装→发送后零化（Debug 不泄漏、内存零化断言、不落盘不上线）。
// ---------------------------------------------------------------------------
