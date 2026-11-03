
//! Typed error for config load/save and password crypto.

use std::fmt;

use std::path::PathBuf;

/// S4（2026-10）：`config.json` 读取在「占用有界重试 + Owner=self 自修 DACL」
/// 两档自愈后仍失败时的结构化报告。携带 Win32 原始错误码与文件路径，供 host
/// 观测码与前端明确报错使用；**该路径绝不重建/覆盖文件**（先备份纪律：读不了
/// 就无法备份，覆盖等于丢配置）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReadFailure {
    /// 最终失败的 `raw_os_error()`；`None` 表示非 OS 错误（合成错误）。
    pub raw_os_error: Option<i32>,
    /// 读取失败的 `config.json` 全路径。
    pub path: PathBuf,
}

impl ConfigReadFailure {
    /// wire/日志用的 Win32 码（无原始码时为 0 = 未知）。
    #[must_use]
    pub fn win32_code(&self) -> i32 {
        self.raw_os_error.unwrap_or(0)
    }
}

impl fmt::Display for ConfigReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "win32 error {} at {}",
            self.win32_code(),
            self.path.display()
        )
    }
}

/// Errors surfaced by [`crate::ExvConfig`] loading/saving and by the
/// AES-256-GCM password helpers.
#[derive(Debug)]
pub enum ConfigError {
    /// Filesystem error while reading/writing `config.json` or `key.bin`.
    Io(std::io::Error),
    /// S4：`config.json` 读失败（三档自愈后仍不可读）的结构化报告——见
    /// [`ConfigReadFailure`]。区别于 [`Self::Io`]：这是**启动读路径专用**的
    /// 终态错误，调用方（host config_get）据此透出 payload 错误字段而非笼统
    /// internal。
    ReadFailed(ConfigReadFailure),
    /// `config.json` is not valid JSON or does not match the schema.
    Json(serde_json::Error),
    /// AES-GCM seal/open failure (bad key, tampered ciphertext).
    Crypto(aes_gcm::Error),
    /// Base64 decoding of the stored password failed.
    Base64(base64::DecodeError),
    /// The ciphertext blob is malformed (not `nonce || tag || ciphertext`).
    Malformed(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "config io: {e}"),
            Self::ReadFailed(failure) => {
                write!(f, "config read failed: {failure} (file left untouched)")
            }
            Self::Json(e) => write!(f, "config json: {e}"),
            Self::Crypto(e) => write!(f, "config crypto: {e}"),
            Self::Base64(e) => write!(f, "config base64: {e}"),
            Self::Malformed(why) => write!(f, "config malformed: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::Base64(e) => Some(e),
            // ReadFailed 携带结构化字段（原始码已抽取为 i32），无 source 可挂。
            // aes_gcm::Error is a unit struct and does not implement
            // std::error::Error; the message is carried in Display only.
            Self::ReadFailed(_) | Self::Crypto(_) | Self::Malformed(_) => None,
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<aes_gcm::Error> for ConfigError {
    fn from(e: aes_gcm::Error) -> Self {
        Self::Crypto(e)
    }
}

impl From<base64::DecodeError> for ConfigError {
    fn from(e: base64::DecodeError) -> Self {
        Self::Base64(e)
    }
}

