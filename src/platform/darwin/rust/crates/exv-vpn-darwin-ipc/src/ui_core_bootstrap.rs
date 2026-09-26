//! Tauri→普通用户 Core 的唯一启动 record。
//!
//! 本模块拥有 `UiCoreBootstrapV1` 的全部固定 wire 细节：长度、magic、big-endian
//! 字段、精确 EOF 和 secret 清零。它不处理 argv、pathname、socket、进程、FFI 或 spawn。
//! Tauri 只通过 [`UiCoreBootstrapV1::encode`] 得到写入 child stdin 的 record；Core 只通过
//! [`UiCoreBootstrapV1::decode`] 消费恰好一条 record。

use std::{fmt, io::Read};

use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::auth::{AUTH_KEY_LEN, AuthKey};

/// `UiCoreBootstrapV1` 的固定 ASCII magic。
pub const UI_CORE_BOOTSTRAP_V1_MAGIC: [u8; 4] = *b"EXVU";
/// `UiCoreBootstrapV1` 的唯一允许版本。
pub const UI_CORE_BOOTSTRAP_V1_VERSION: u16 = 1;
/// `magic(4) || version(u16) || reserved(u16) || auth_key(32)` 的固定长度。
pub const UI_CORE_BOOTSTRAP_V1_LEN: usize = 40;

const MAGIC_OFFSET: usize = 0;
const VERSION_OFFSET: usize = MAGIC_OFFSET + UI_CORE_BOOTSTRAP_V1_MAGIC.len();
const RESERVED_OFFSET: usize = VERSION_OFFSET + std::mem::size_of::<u16>();
const AUTH_KEY_OFFSET: usize = RESERVED_OFFSET + std::mem::size_of::<u16>();

/// UI→Core 启动 record 的稳定错误类别。
///
/// 错误不携带原始字节、认证 key 或底层 I/O 文本，避免它们进入日志或上层错误消息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCoreBootstrapError {
    /// 系统随机源不可用，未能生成新的 UI/Core 一次性 key。
    Random,
    /// 固定 record 的读取发生了非 EOF I/O 错误。
    Io,
    /// EOF 或短读发生在完整 40-byte record 之前。
    Truncated,
    /// 完整 record 后仍有任意额外字节，因而不是唯一启动 record。
    Extra,
    /// magic 不是 [`UI_CORE_BOOTSTRAP_V1_MAGIC`]。
    Magic,
    /// version 不是 [`UI_CORE_BOOTSTRAP_V1_VERSION`]。
    Version,
    /// reserved 字段不是零。
    Reserved,
}

impl fmt::Display for UiCoreBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Random => "Darwin UI/Core bootstrap key generation failed",
            Self::Io => "Darwin UI/Core bootstrap input failed",
            Self::Truncated | Self::Extra | Self::Magic | Self::Version | Self::Reserved => {
                "Darwin UI/Core bootstrap record is invalid"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for UiCoreBootstrapError {}

/// 已编码但尚未写入 child stdin 的 40-byte V1 record。
///
/// 这是 [`Zeroizing`] 的窄封装：它在 drop 时清零，同时拒绝默认 `Debug` 暴露 32-byte
/// key。Tauri 应仅通过 [`AsRef::<[u8]>::as_ref`] 把它交给一次 `write_all`，不能将其
/// 转成日志、事件、argv 或配置值。
pub struct UiCoreBootstrapEncodedV1 {
    bytes: Zeroizing<[u8; UI_CORE_BOOTSTRAP_V1_LEN]>,
}

impl UiCoreBootstrapEncodedV1 {
    /// 返回用于 child stdin 的固定 record 字节。
    ///
    /// 此借用不转移所有权；调用方必须让本值在 stdin 写入结束前保持在作用域内。
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; UI_CORE_BOOTSTRAP_V1_LEN] {
        &self.bytes
    }

    fn new(bytes: Zeroizing<[u8; UI_CORE_BOOTSTRAP_V1_LEN]>) -> Self {
        Self { bytes }
    }
}

impl AsRef<[u8]> for UiCoreBootstrapEncodedV1 {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl fmt::Debug for UiCoreBootstrapEncodedV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UiCoreBootstrapEncodedV1([REDACTED])")
    }
}

impl Zeroize for UiCoreBootstrapEncodedV1 {
    fn zeroize(&mut self) {
        self.bytes.zeroize();
    }
}

impl ZeroizeOnDrop for UiCoreBootstrapEncodedV1 {}

impl Drop for UiCoreBootstrapEncodedV1 {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// 只在内存中存在的 40-byte UI→Core 启动 record。
///
/// 认证 key 没有 getter、`Debug` redaction，且 record 本身在 `Drop` 时清零。编码缓冲由
/// [`Zeroizing`] 承载；解码缓冲在 [`Self::decode`] 的所有成功和错误路径上都在离开函数前清零。
pub struct UiCoreBootstrapV1 {
    auth_key: [u8; AUTH_KEY_LEN],
}

impl UiCoreBootstrapV1 {
    /// 生成一对只用于本次 Tauri→Core 启动的 record 与 Tauri client key。
    ///
    /// 返回的 record 只能交给 [`Self::encode`]；返回的 [`AuthKey`] 只用于随后同一条
    /// `DarwinAuthV1` client proof。二者都拥有独立的内存副本并在 drop 时清零。
    ///
    /// # Errors
    ///
    /// 系统随机源不可用时返回 [`UiCoreBootstrapError::Random`]；临时 key 缓冲仍会清零。
    pub fn random_pair() -> Result<(Self, AuthKey), UiCoreBootstrapError> {
        let mut auth_key = Zeroizing::new([0_u8; AUTH_KEY_LEN]);
        getrandom::fill(&mut *auth_key).map_err(|_| UiCoreBootstrapError::Random)?;

        let record = Self::from_auth_key_bytes(*auth_key);
        let client_key = AuthKey::from_bytes(*auth_key);
        auth_key.zeroize();
        Ok((record, client_key))
    }

    /// 以固定 big-endian V1 layout 编码该 record。
    ///
    /// 消费 record，避免 Tauri 在取得 [`UiCoreBootstrapEncodedV1`] 后额外保留 record key
    /// 副本。调用方必须使返回值存活到 child stdin 写入结束，并让它在任何成功/失败/drop 路径离开
    /// 作用域。
    #[must_use]
    pub fn encode(self) -> UiCoreBootstrapEncodedV1 {
        let mut bytes = Zeroizing::new([0_u8; UI_CORE_BOOTSTRAP_V1_LEN]);
        bytes[MAGIC_OFFSET..VERSION_OFFSET].copy_from_slice(&UI_CORE_BOOTSTRAP_V1_MAGIC);
        bytes[VERSION_OFFSET..RESERVED_OFFSET]
            .copy_from_slice(&UI_CORE_BOOTSTRAP_V1_VERSION.to_be_bytes());
        bytes[AUTH_KEY_OFFSET..].copy_from_slice(&self.auth_key);
        UiCoreBootstrapEncodedV1::new(bytes)
    }

    /// 从 reader 消费恰好一条 40-byte V1 record，且下一次读取必须立即 EOF。
    ///
    /// 该函数是 Core 的唯一 decoder：短读、额外字节、magic/version/reserved 非法均返回固定
    /// [`UiCoreBootstrapError`]，而且不会返回部分 key。内部原始 record 及 trailing-byte
    /// 缓冲都由 [`Zeroizing`] 持有。
    ///
    /// # Errors
    ///
    /// stdin 在完整 record 前 EOF 时返回 [`UiCoreBootstrapError::Truncated`]；完整 record 后
    /// 有额外字节时返回 [`UiCoreBootstrapError::Extra`]；其他 I/O 与字段错误保留其固定类别。
    pub fn decode<R: Read>(reader: &mut R) -> Result<Self, UiCoreBootstrapError> {
        let mut bytes = Zeroizing::new([0_u8; UI_CORE_BOOTSTRAP_V1_LEN]);
        match reader.read_exact(&mut *bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(UiCoreBootstrapError::Truncated);
            }
            Err(_) => return Err(UiCoreBootstrapError::Io),
        }

        let mut trailing = Zeroizing::new([0_u8; 1]);
        match reader.read(&mut *trailing) {
            Ok(0) => Self::decode_record(&bytes),
            Ok(_) => Err(UiCoreBootstrapError::Extra),
            Err(_) => Err(UiCoreBootstrapError::Io),
        }
    }

    /// 将已经验证且被消费的 record key 移交给既有 `DarwinAuthV1` listener。
    ///
    /// record 的字段在移交前被置零；返回的 [`AuthKey`] 继续负责其自身 drop-time 清零。
    #[must_use]
    pub fn into_auth_key(mut self) -> AuthKey {
        AuthKey::from_bytes(std::mem::take(&mut self.auth_key))
    }

    fn from_auth_key_bytes(auth_key: [u8; AUTH_KEY_LEN]) -> Self {
        Self { auth_key }
    }

    fn decode_record(bytes: &[u8; UI_CORE_BOOTSTRAP_V1_LEN]) -> Result<Self, UiCoreBootstrapError> {
        if bytes[MAGIC_OFFSET..VERSION_OFFSET] != UI_CORE_BOOTSTRAP_V1_MAGIC {
            return Err(UiCoreBootstrapError::Magic);
        }
        if u16::from_be_bytes(
            bytes[VERSION_OFFSET..RESERVED_OFFSET]
                .try_into()
                .expect("fixed V1 version span"),
        ) != UI_CORE_BOOTSTRAP_V1_VERSION
        {
            return Err(UiCoreBootstrapError::Version);
        }
        if bytes[RESERVED_OFFSET..AUTH_KEY_OFFSET] != [0, 0] {
            return Err(UiCoreBootstrapError::Reserved);
        }

        let mut auth_key = Zeroizing::new([0_u8; AUTH_KEY_LEN]);
        auth_key.copy_from_slice(&bytes[AUTH_KEY_OFFSET..]);
        Ok(Self::from_auth_key_bytes(std::mem::take(&mut *auth_key)))
    }
}

impl fmt::Debug for UiCoreBootstrapV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiCoreBootstrapV1")
            .field("auth_key", &"[REDACTED]")
            .finish()
    }
}

impl Zeroize for UiCoreBootstrapV1 {
    fn zeroize(&mut self) {
        self.auth_key.zeroize();
    }
}

impl ZeroizeOnDrop for UiCoreBootstrapV1 {}

impl Drop for UiCoreBootstrapV1 {
    fn drop(&mut self) {
        self.zeroize();
    }
}
