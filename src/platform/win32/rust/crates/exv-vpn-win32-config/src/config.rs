
//! The MVP `ExvConfig` structure plus load/save and credential helpers.
//!
//! Field names and default values are aligned with the C++ config
//! (`src/core/config/config.hpp` at v3.3.7 and `distribution/ecnu.json`).
//! The MVP subset covers: target server, routes, server control-plane bypass
//! destinations, and credentials (username + AES-sealed password). The password
//! is stored as ciphertext and decrypted one-shot with the independent
//! `key.bin` key.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Deserializer, Serialize};

use crate::crypto::{KEY_LEN, decrypt_password, encrypt_password};
use crate::error::ConfigError;
use crate::paths::{config_path, key_path};

/// Default gateway hostname (`distribution/ecnu.json` `default_vpn_server`).
pub const DEFAULT_SERVER: &str = "vpn-cn.ecnu.edu.cn";
/// 新建配置的八条默认校园路由；历史用户配置不随默认值迁移。
///
/// 末条 `219.228.144.0/22` 是 2026-09-20 补入的唯一新增网段：同批给出的
/// `219.228.144.107/22` 与 `219.228.144.105/22` 规范化后是同一条，而
/// `219.228.60.69/22` 已被既有的 `219.228.63.0/21`（规范形 `219.228.56.0/21`）覆盖。
pub const DEFAULT_ROUTES: [&str; 8] = [
    "49.52.4.0/25",
    "59.78.176.0/20",
    "59.78.199.0/21",
    "58.198.176.128/25",
    "59.78.189.128/25",
    "219.228.63.0/21",
    "202.120.80.0/20",
    "219.228.144.0/22",
];
/// Default Windows user-agent (`distribution/ecnu.json` `default_user_agents.windows`).
pub const DEFAULT_USER_AGENT: &str = "AnyConnect Win_x86_64 4.10.05095";
/// Default MTU, matching the C++ `Config` default (1290).
pub const DEFAULT_MTU: u32 = 1290;

/// Windows 连接使用的物理出口选择策略。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    /// 优先使用既有物理直连路径。
    #[default]
    Standard,
    /// 使用 Windows 当前选择的实际网络出口。
    Compatibility,
}

impl ConnectionMode {
    /// 稳定配置字面量；也用于不含凭据的诊断日志。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Compatibility => "compatibility",
        }
    }
}

impl std::fmt::Display for ConnectionMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ConnectionMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "standard" => Ok(Self::Standard),
            "compatibility" => Ok(Self::Compatibility),
            _ => Err(()),
        }
    }
}

impl<'de> Deserialize<'de> for ConnectionMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        // 配置文件可能来自更高版本。未知字符串只局部回退，不让整个 ExvConfig
        // 回落默认值而丢失同一文档里的账户、密文和其他设置。
        Ok(value.parse().unwrap_or_default())
    }
}

/// User VPN configuration (MVP subset).
///
/// Fields mirror the C++ `Config` JSON keys. `password` holds the AES-256-GCM
/// ciphertext (`base64( nonce || tag || ct )`), never the plaintext.
///
/// 2026-09-08 计划（config 基线）：**不含任何 VPN 网关 IP 字段**——网关地址由
/// VGDC 双线解析（DoH）每次连接即时获得，手填 IP 已退役。历史 `server_bypass_ips`
/// 字段自本计划起删除：serde 未知字段容忍 ⇒ 旧配置读到即丢弃（忽略不读），字段
/// 不存在 ⇒ 任何保存路径都写不出它（结构性禁止写入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExvConfig {
    /// Gateway hostname, e.g. `vpn-cn.ecnu.edu.cn` (never a gateway IP — the
    /// real gateway address is VGDC-resolved per connection).
    pub server: String,
    /// Login username.
    pub username: String,
    /// AES-GCM sealed password (base64), or empty when not remembered.
    pub password: String,
    /// Whether `password` should be remembered across sessions.
    pub remember_password: bool,
    /// Campus routes to push into the tunnel.
    pub routes: Vec<String>,
    /// Client user-agent presented to the gateway.
    #[serde(rename = "useragent")]
    pub user_agent: String,
    /// Tunnel MTU.
    pub mtu: u32,
    /// Windows 物理出口选择策略；旧配置缺省为标准模式。
    pub connection_mode: ConnectionMode,
    /// Whether to automatically reconnect after an unexpected disconnect.
    pub auto_reconnect: bool,
    /// Maximum auto-reconnect attempts (0 = unlimited).
    pub auto_reconnect_max_attempts: u32,
    /// Whether automatic reconnects back off exponentially (base 2s, cap 30s);
    /// off keeps the legacy immediate-reconnect behavior.
    pub auto_reconnect_backoff: bool,
}

impl Default for ExvConfig {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.to_string(),
            username: String::new(),
            password: String::new(),
            remember_password: false,
            routes: DEFAULT_ROUTES.iter().map(|r| (*r).to_string()).collect(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            mtu: DEFAULT_MTU,
            connection_mode: ConnectionMode::Standard,
            auto_reconnect: false,
            auto_reconnect_max_attempts: 0,
            auto_reconnect_backoff: false,
        }
    }
}

impl ExvConfig {
    /// Load the user config from the resolved config directory.
    ///
    /// Mirrors C++ `config_initialization`: a missing file (or a file that is
    /// not valid JSON / not an object) yields a fresh default config, so the
    /// caller can `save()` to persist it.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] only on filesystem read failures, never on a
    /// missing/malformed file (those produce the default).
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from_dir(&crate::paths::config_dir())
    }

    /// Load the user config from an explicit directory (testable / overridable).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] on a filesystem read failure; a missing or
    /// malformed `config.json` yields the default config, never an error.
    pub fn load_from_dir(dir: &Path) -> Result<Self, ConfigError> {
        let path = config_path(dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(cfg) => Ok(cfg),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Persist this config to the resolved config directory (creating it if
    /// needed).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on any filesystem or serialization failure.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to_dir(&crate::paths::config_dir())
    }

    /// Persist this config to an explicit directory.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on any filesystem or serialization failure.
    pub fn save_to_dir(&self, dir: &Path) -> Result<(), ConfigError> {
        let json = serde_json::to_string_pretty(self)?;
        write_config_json_atomically(dir, &json)
    }

    /// Ensure a 32-byte key exists in `dir`, generating and persisting a new
    /// one if the file is missing or malformed.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the key file cannot be read/written.
    pub fn ensure_key(dir: &Path) -> Result<[u8; KEY_LEN], ConfigError> {
        if let Ok(Some(key)) = Self::load_key(dir) {
            Ok(key)
        } else {
            let key = crate::crypto::generate_key()?;
            Self::save_key(dir, &key)?;
            Ok(key)
        }
    }

    /// Read the 32-byte key from `<dir>/key.bin`.
    ///
    /// Returns `Ok(None)` when the key file does not exist, and `Err` on
    /// unreadable or wrong-length files.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] on an unreadable key file, or
    /// [`ConfigError::Malformed`] when the file is not exactly 32 bytes.
    pub fn load_key(dir: &Path) -> Result<Option<[u8; KEY_LEN]>, ConfigError> {
        let path = key_path(dir);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if bytes.len() != KEY_LEN {
            return Err(ConfigError::Malformed(
                "key.bin must be exactly 32 bytes (AES-256)",
            ));
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&bytes);
        Ok(Some(key))
    }

    /// Write a 32-byte key to `<dir>/key.bin` (creating `dir` if needed).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if the directory or key file cannot be
    /// written.
    pub fn save_key(dir: &Path, key: &[u8; KEY_LEN]) -> Result<(), ConfigError> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(key_path(dir), key)?;
        Ok(())
    }

    /// Decrypt the stored password with the supplied key.
    ///
    /// Returns an empty string when `password` is empty (not remembered). Any
    /// decryption failure (wrong key, tampered ciphertext) is surfaced as a
    /// typed [`ConfigError`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Malformed`] for an empty or badly-shaped blob,
    /// [`ConfigError::Base64`] for bad base64, or [`ConfigError::Crypto`] when
    /// authentication fails (wrong key or tampered ciphertext).
    pub fn decrypt_password(&self, key: &[u8; KEY_LEN]) -> Result<String, ConfigError> {
        if self.password.is_empty() {
            return Ok(String::new());
        }
        decrypt_password(&self.password, key)
    }

    /// Encrypt `plaintext` with `key` and store the ciphertext, marking the
    /// password as remembered.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if the OS RNG fails or
    /// [`ConfigError::Crypto`] if sealing fails.
    pub fn set_password_encrypted(
        &mut self,
        plaintext: &str,
        key: &[u8; KEY_LEN],
    ) -> Result<(), ConfigError> {
        self.password = encrypt_password(plaintext, key)?;
        self.remember_password = !plaintext.is_empty();
        Ok(())
    }
}

/// Atomically replace `config.json` with already-serialized JSON.
///
/// The temporary file is created next to the destination so the final rename
/// stays on the same filesystem. `std::fs::rename` uses replace-existing
/// semantics on Windows; if replacement fails, this function never removes
/// the previous destination.
pub(crate) fn write_config_json_atomically(dir: &Path, json: &str) -> Result<(), ConfigError> {
    write_config_json_atomically_with(dir, json, |temporary, destination| {
        std::fs::rename(temporary, destination)
    })
}

pub(crate) fn write_config_json_atomically_with<F>(
    dir: &Path,
    json: &str,
    replace: F,
) -> Result<(), ConfigError>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    std::fs::create_dir_all(dir)?;
    let destination = config_path(dir);
    let (temporary, mut file) = create_temporary_config_file(dir)?;

    let write_result = (|| -> Result<(), ConfigError> {
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, &destination)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}

fn create_temporary_config_file(dir: &Path) -> Result<(PathBuf, File), ConfigError> {
    static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    for _ in 0..32 {
        let id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let temporary = dir.join(format!(".config.json.{}.{}.tmp", std::process::id(), id));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique config temporary file",
    )
    .into())
}

