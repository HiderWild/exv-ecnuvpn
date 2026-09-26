//! Darwin 的 Windows 兼容单配置存储。
//!
//! 这里在 Darwin Core 内实现 Windows `ExvConfig` 的字段、默认值、`config.json`
//! / `key.bin` 路径和 AES-256-GCM 密文格式。它不依赖 Win32 crate，也不触发
//! Engine、网络或特权操作。

use std::{
    fmt,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tonic::Code;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    config_crypto::{KEY_LEN, decrypt_password, encrypt_password, generate_key},
    config_paths::{atomic_write, config_dir, config_path, key_path},
};

const CONFIG_KEYS: [&str; 10] = [
    "server",
    "username",
    "password",
    "remember_password",
    "routes",
    "user_agent",
    "mtu",
    "auto_reconnect",
    "auto_reconnect_max_attempts",
    "auto_reconnect_backoff",
];

const DEFAULT_SERVER: &str = "vpn-cn.ecnu.edu.cn";
const DEFAULT_ROUTES: [&str; 8] = [
    "49.52.4.0/25",
    "59.78.176.0/20",
    "59.78.199.0/21",
    "58.198.176.128/25",
    "59.78.189.128/25",
    "219.228.63.0/21",
    "202.120.80.0/20",
    "219.228.144.0/22",
];
const DEFAULT_USER_AGENT: &str = "AnyConnect Win_x86_64 4.10.05095";
const DEFAULT_MTU: u32 = 1290;

/// Core 配置边界使用的一个键值项。
#[derive(Clone, PartialEq, Eq)]
pub struct DarwinConfigItem {
    key: String,
    value: String,
}

impl DarwinConfigItem {
    #[must_use]
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for DarwinConfigItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DarwinConfigItem")
            .field("key", &"[REDACTED]")
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// 配置读取、保存或字段解析的稳定本地错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DarwinConfigError {
    UnknownKey,
    DuplicateKey,
    InvalidServer,
    InvalidUsername,
    InvalidRememberPassword,
    InvalidRoutes,
    InvalidMtu,
    InvalidUserAgent,
    InvalidAutoReconnect,
    InvalidAutoReconnectMaxAttempts,
    InvalidAutoReconnectBackoff,
    Storage,
    PasswordCrypto,
}

impl DarwinConfigError {
    #[must_use]
    pub const fn grpc_code(self) -> Code {
        match self {
            Self::UnknownKey
            | Self::DuplicateKey
            | Self::InvalidServer
            | Self::InvalidUsername
            | Self::InvalidRememberPassword
            | Self::InvalidRoutes
            | Self::InvalidMtu
            | Self::InvalidUserAgent
            | Self::InvalidAutoReconnect
            | Self::InvalidAutoReconnectMaxAttempts
            | Self::InvalidAutoReconnectBackoff => Code::InvalidArgument,
            Self::Storage | Self::PasswordCrypto => Code::Internal,
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnknownKey => "DARWIN_CONFIG_UNKNOWN_KEY",
            Self::DuplicateKey => "DARWIN_CONFIG_DUPLICATE_KEY",
            Self::InvalidServer => "DARWIN_CONFIG_INVALID_SERVER",
            Self::InvalidUsername => "DARWIN_CONFIG_INVALID_USERNAME",
            Self::InvalidRememberPassword => "DARWIN_CONFIG_INVALID_REMEMBER_PASSWORD",
            Self::InvalidRoutes => "DARWIN_CONFIG_INVALID_ROUTES",
            Self::InvalidMtu => "DARWIN_CONFIG_INVALID_MTU",
            Self::InvalidUserAgent => "DARWIN_CONFIG_INVALID_USER_AGENT",
            Self::InvalidAutoReconnect => "DARWIN_CONFIG_INVALID_AUTO_RECONNECT",
            Self::InvalidAutoReconnectMaxAttempts => {
                "DARWIN_CONFIG_INVALID_AUTO_RECONNECT_MAX_ATTEMPTS"
            }
            Self::InvalidAutoReconnectBackoff => "DARWIN_CONFIG_INVALID_AUTO_RECONNECT_BACKOFF",
            Self::Storage => "DARWIN_CONFIG_STORAGE_FAILED",
            Self::PasswordCrypto => "DARWIN_CONFIG_PASSWORD_CRYPTO_FAILED",
        }
    }
}

impl fmt::Display for DarwinConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for DarwinConfigError {}

/// 与 Windows `config.json` 同形的本地记录。
///
/// `password` 只保存 `base64(nonce || tag || ciphertext)`，从不保存明文。
/// W3-3/P2b：对 crate 内卫生层（`config_hygiene`）可见，供启动期分类/修复直接
/// 复用同一反序列化 schema。
///
/// 2026-09-12：历史 `server_bypass_ips`（手填网关 IP）字段已退役——结构体无该字段
/// 且无 `deny_unknown_fields`，旧 `config.json` 携带该键时解析不报错、值不进任何状态。
///
/// 落盘清洗的**准确边界**（勿据"任何保存路径都写不出它"这类旧措辞推断）：
/// - `apply_and_save` 等整结构体序列化的写路径**写不出**该键（结构体无此字段）；
/// - 但 `config_hygiene::load_for_startup` 为兼容未来版本字段会**原样保留未知键**并
///   回写磁盘，因此仅"打开应用/读一次配置"**不会**把旧键从磁盘抹掉，需一次显式
///   `ConfigSet`（设置页保存 / 快速入门）才清洗。该行为与 win32 卫生层一致。
///
/// 网关地址由每次连接即时解析获得，不需要配置项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DarwinStoredConfig {
    server: String,
    username: String,
    password: String,
    remember_password: bool,
    routes: Vec<String>,
    #[serde(rename = "useragent")]
    user_agent: String,
    mtu: u32,
    auto_reconnect: bool,
    auto_reconnect_max_attempts: u32,
    auto_reconnect_backoff: bool,
}

impl Default for DarwinStoredConfig {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.to_owned(),
            username: String::new(),
            password: String::new(),
            remember_password: false,
            routes: DEFAULT_ROUTES
                .iter()
                .map(|route| (*route).to_owned())
                .collect(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            mtu: DEFAULT_MTU,
            auto_reconnect: false,
            auto_reconnect_max_attempts: 0,
            auto_reconnect_backoff: false,
        }
    }
}

impl DarwinStoredConfig {
    fn load_from_dir(dir: &Path) -> Result<Self, DarwinConfigError> {
        let text = match std::fs::read_to_string(config_path(dir)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(_) => return Err(DarwinConfigError::Storage),
        };
        Ok(serde_json::from_str(&text).unwrap_or_default())
    }

    pub(crate) fn save_to_dir_atomically(&self, dir: &Path) -> Result<(), DarwinConfigError> {
        let json = serde_json::to_string_pretty(self).map_err(|_| DarwinConfigError::Storage)?;
        atomic_write(&config_path(dir), json.as_bytes()).map_err(|_| DarwinConfigError::Storage)
    }

    fn ensure_key(dir: &Path) -> Result<[u8; KEY_LEN], DarwinConfigError> {
        if let Ok(Some(key)) = Self::load_key(dir) {
            Ok(key)
        } else {
            let key = generate_key()?;
            Self::save_key(dir, &key)?;
            Ok(key)
        }
    }

    fn load_key(dir: &Path) -> Result<Option<[u8; KEY_LEN]>, DarwinConfigError> {
        let bytes = match std::fs::read(key_path(dir)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(DarwinConfigError::Storage),
        };
        if bytes.len() != KEY_LEN {
            return Err(DarwinConfigError::Storage);
        }
        let mut key = [0_u8; KEY_LEN];
        key.copy_from_slice(&bytes);
        Ok(Some(key))
    }

    fn save_key(dir: &Path, key: &[u8; KEY_LEN]) -> Result<(), DarwinConfigError> {
        atomic_write(&key_path(dir), key).map_err(|_| DarwinConfigError::Storage)
    }

    fn decrypt_password(&self, key: &[u8; KEY_LEN]) -> Result<String, DarwinConfigError> {
        if self.password.is_empty() {
            return Ok(String::new());
        }
        decrypt_password(&self.password, key)
    }

    fn set_password_encrypted(
        &mut self,
        plaintext: &str,
        key: &[u8; KEY_LEN],
    ) -> Result<(), DarwinConfigError> {
        self.password = encrypt_password(plaintext, key)?;
        Ok(())
    }
}

/// Windows 兼容的用户配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DarwinUiConfig {
    inner: DarwinStoredConfig,
    config_dir: PathBuf,
}

impl Default for DarwinUiConfig {
    fn default() -> Self {
        Self {
            inner: DarwinStoredConfig::default(),
            config_dir: config_dir(),
        }
    }
}

impl DarwinUiConfig {
    /// 从当前用户的 Windows 同构配置目录读取；缺失或错误 JSON 使用默认值。
    ///
    /// # Errors
    ///
    /// 不可读取的配置文件返回 [`DarwinConfigError::Storage`]。
    pub fn load() -> Result<Self, DarwinConfigError> {
        Self::load_from_dir(config_dir())
    }

    /// 从指定目录读取 Windows 同构配置。此入口用于 Core 测试隔离存储。
    ///
    /// # Errors
    ///
    /// 不可读取的配置文件返回 [`DarwinConfigError::Storage`]。
    pub fn load_from_dir(config_dir: impl Into<PathBuf>) -> Result<Self, DarwinConfigError> {
        let config_dir = config_dir.into();
        let inner = DarwinStoredConfig::load_from_dir(&config_dir)?;
        Ok(Self { inner, config_dir })
    }

    /// `ConfigGet` 的配置字段及可用密码标志。密码键固定为空，绝不回显密文或明文。
    #[must_use]
    pub fn items(&self) -> Vec<DarwinConfigItem> {
        vec![
            DarwinConfigItem::new("server", self.inner.server.clone()),
            DarwinConfigItem::new("username", self.inner.username.clone()),
            DarwinConfigItem::new("password", String::new()),
            DarwinConfigItem::new(
                "remember_password",
                self.inner.remember_password.to_string(),
            ),
            DarwinConfigItem::new("routes", self.inner.routes.join(",")),
            DarwinConfigItem::new("user_agent", self.inner.user_agent.clone()),
            DarwinConfigItem::new("mtu", self.inner.mtu.to_string()),
            DarwinConfigItem::new("auto_reconnect", self.inner.auto_reconnect.to_string()),
            DarwinConfigItem::new(
                "auto_reconnect_max_attempts",
                self.inner.auto_reconnect_max_attempts.to_string(),
            ),
            DarwinConfigItem::new(
                "auto_reconnect_backoff",
                self.inner.auto_reconnect_backoff.to_string(),
            ),
            DarwinConfigItem::new("has_stored_password", self.has_stored_password().to_string()),
        ]
    }

    #[must_use]
    pub fn server(&self) -> &str {
        &self.inner.server
    }

    #[must_use]
    pub fn username(&self) -> &str {
        &self.inner.username
    }

    #[must_use]
    pub fn routes(&self) -> &[String] {
        &self.inner.routes
    }

    #[must_use]
    pub fn user_agent(&self) -> &str {
        &self.inner.user_agent
    }

    #[must_use]
    pub const fn mtu(&self) -> u32 {
        self.inner.mtu
    }

    /// W3-3/P2b：启动卫生层（`config_hygiene`）构造接缝——从**已验证**的存储记录
    /// 组装绑定指定目录的 UI 配置；调用方负责该记录确实来自合法 JSON 对象。
    pub(crate) fn from_stored(inner: DarwinStoredConfig, config_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            config_dir: config_dir.into(),
        }
    }
    /// W3-2/P3：自动重连开关（内存配置单一事实源；重连判定与 `ReconnectStatus`
    /// 上报共用同一读点）。
    #[must_use]
    pub const fn auto_reconnect(&self) -> bool {
        self.inner.auto_reconnect
    }

    /// W3-2/P3：自动重连最大尝试数（0 = 无限；`config_set` 校验后原子入内存）。
    #[must_use]
    pub const fn auto_reconnect_max_attempts(&self) -> u32 {
        self.inner.auto_reconnect_max_attempts
    }

    /// W3-2/P3：自动重连指数退避开关。
    #[must_use]
    pub const fn auto_reconnect_backoff(&self) -> bool {
        self.inner.auto_reconnect_backoff
    }

    /// 完整验证 request 后，原子替换内存和 `config.json`。
    ///
    /// 显式用户提交是卫生边界（win32 `config_hygiene::save_after_user_submission`
    /// 的 darwin 天然对应，W3-3/P2b）：落盘走 typed 序列化，只写当前已知 schema
    /// 字段——启动期 hydrate 保留的未知字段在提交成功后即被剥除。
    ///
    /// # Errors
    ///
    /// 未知键、重复键和字段格式错误不会修改现有配置；存储或密码加密失败也不会替换内存值。
    pub fn apply_and_save(
        &mut self,
        items: Vec<DarwinConfigItem>,
    ) -> Result<(), DarwinConfigError> {
        let update = ConfigUpdate::validate(items)?;
        let mut candidate = self.inner.clone();
        update.apply(&mut candidate, &self.config_dir)?;
        candidate.save_to_dir_atomically(&self.config_dir)?;
        self.inner = candidate;
        Ok(())
    }

    /// 显式保存连接凭据：用户名与新密码密文同批写入。
    /// 未提供新密码而切换账户时清除旧密文，避免新账户继承另一账户密码。
    /// 临时连接不调用此落盘入口，而是使用 `with_temporary_username` 内存副本。
    ///
    /// # Errors
    ///
    /// 用户名不合字段约束、密钥生成/加密失败或存储失败时返回 [`DarwinConfigError`]；
    /// 任一失败都不替换内存值。
    pub fn apply_ui_credentials_and_save(
        &mut self,
        username: &str,
        password: Option<&str>,
    ) -> Result<(), DarwinConfigError> {
        let username = validate_username(username)?;
        let mut candidate = self.inner.clone();
        if candidate.username != username {
            candidate.password.zeroize();
            candidate.password.clear();
        }
        candidate.username = username;
        if let Some(password) = password {
            let mut key = DarwinStoredConfig::ensure_key(&self.config_dir)?;
            let result = candidate.set_password_encrypted(password, &key);
            key.zeroize();
            result?;
            candidate.remember_password = true;
        }
        candidate.save_to_dir_atomically(&self.config_dir)?;
        self.inner = candidate;
        Ok(())
    }

    /// 一次性连接使用临时用户名副本，不修改已保存配置或密码身份。
    pub(crate) fn with_temporary_username(&self, username: &str) -> Result<Self, DarwinConfigError> {
        let mut temporary = self.clone();
        temporary.inner.username = validate_username(username)?;
        Ok(temporary)
    }

    /// 只有真实解密成功且非空的记住密码才算可用；只读，不创建密钥。
    #[must_use]
    pub fn has_stored_password(&self) -> bool {
        self.load_saved_password().map(zeroize::Zeroizing::new)
            .is_ok_and(|password| !password.is_empty())
    }

    /// 读取已保存密码；无已保存密码时返回空串。
    ///
    /// # Errors
    ///
    /// 密文存在但 key 缺失、不可读或无法解密时返回 [`DarwinConfigError`]。
    pub fn load_saved_password(&self) -> Result<String, DarwinConfigError> {
        if !self.inner.remember_password || self.inner.password.is_empty() {
            return Ok(String::new());
        }
        let mut key =
            DarwinStoredConfig::load_key(&self.config_dir)?.ok_or(DarwinConfigError::Storage)?;
        let password = self.inner.decrypt_password(&key);
        key.zeroize();
        password
    }

    /// 返回当前配置的存储目录。
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }
}

#[derive(Default)]
struct ConfigUpdate {
    server: Option<String>,
    username: Option<String>,
    password: Option<Zeroizing<String>>,
    remember_password: Option<bool>,
    routes: Option<Vec<String>>,
    user_agent: Option<String>,
    mtu: Option<u32>,
    auto_reconnect: Option<bool>,
    auto_reconnect_max_attempts: Option<u32>,
    auto_reconnect_backoff: Option<bool>,
}

impl ConfigUpdate {
    fn validate(items: Vec<DarwinConfigItem>) -> Result<Self, DarwinConfigError> {
        let mut items = items
            .into_iter()
            .map(|mut item| {
                (
                    std::mem::take(&mut item.key),
                    Zeroizing::new(std::mem::take(&mut item.value)),
                )
            })
            .collect::<Vec<_>>();

        validate_keys(&items)?;

        let mut update = Self::default();
        for (mut key, value) in items.drain(..) {
            match key.as_str() {
                "server" => update.server = Some(normalize_server(&value)?),
                "username" => update.username = Some(validate_username(&value)?),
                "password" => update.password = Some(value),
                "remember_password" => {
                    update.remember_password = Some(parse_remember_password(&value)?);
                }
                "routes" => update.routes = Some(parse_routes(&value)?),
                "user_agent" => update.user_agent = Some(validate_user_agent(&value)?),
                "mtu" => update.mtu = Some(parse_mtu(&value)?),
                "auto_reconnect" => {
                    update.auto_reconnect = Some(parse_auto_reconnect(&value)?);
                }
                "auto_reconnect_max_attempts" => {
                    update.auto_reconnect_max_attempts =
                        Some(parse_auto_reconnect_max_attempts(&value)?);
                }
                "auto_reconnect_backoff" => {
                    update.auto_reconnect_backoff = Some(parse_auto_reconnect_backoff(&value)?);
                }
                _ => return Err(DarwinConfigError::UnknownKey),
            }
            key.zeroize();
        }
        Ok(update)
    }

    fn apply(
        self,
        candidate: &mut DarwinStoredConfig,
        config_dir: &Path,
    ) -> Result<(), DarwinConfigError> {
        if let Some(server) = self.server {
            candidate.server = server;
        }
        if let Some(username) = self.username {
            if candidate.username != username {
                candidate.password.zeroize();
                candidate.password.clear();
            }
            candidate.username = username;
        }
        if let Some(routes) = self.routes {
            candidate.routes = routes;
        }
        if let Some(user_agent) = self.user_agent {
            candidate.user_agent = user_agent;
        }
        if let Some(mtu) = self.mtu {
            candidate.mtu = mtu;
        }
        if let Some(auto_reconnect) = self.auto_reconnect {
            candidate.auto_reconnect = auto_reconnect;
        }
        if let Some(auto_reconnect_max_attempts) = self.auto_reconnect_max_attempts {
            candidate.auto_reconnect_max_attempts = auto_reconnect_max_attempts;
        }
        if let Some(auto_reconnect_backoff) = self.auto_reconnect_backoff {
            candidate.auto_reconnect_backoff = auto_reconnect_backoff;
        }

        candidate.remember_password = self
            .remember_password
            .unwrap_or(candidate.remember_password);
        if !candidate.remember_password {
            candidate.password.zeroize();
            candidate.password.clear();
            return Ok(());
        }

        if let Some(password) = self.password.filter(|password| !password.is_empty()) {
            let mut key = DarwinStoredConfig::ensure_key(config_dir)?;
            let result = candidate.set_password_encrypted(password.as_str(), &key);
            key.zeroize();
            result?;
        }
        Ok(())
    }
}

fn validate_keys(items: &[(String, Zeroizing<String>)]) -> Result<(), DarwinConfigError> {
    for (index, (key, _)) in items.iter().enumerate() {
        if !CONFIG_KEYS.contains(&key.as_str()) {
            return Err(DarwinConfigError::UnknownKey);
        }
        if items[..index].iter().any(|(previous, _)| previous == key) {
            return Err(DarwinConfigError::DuplicateKey);
        }
    }
    Ok(())
}

fn normalize_server(value: &str) -> Result<String, DarwinConfigError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if !value.is_ascii() || !(1..=253).contains(&value.len()) || value.parse::<Ipv4Addr>().is_ok() {
        return Err(DarwinConfigError::InvalidServer);
    }
    let valid = value.split('.').all(|label| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    });
    valid
        .then(|| value.to_ascii_lowercase())
        .ok_or(DarwinConfigError::InvalidServer)
}

fn validate_username(value: &str) -> Result<String, DarwinConfigError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if !(1..=256).contains(&value.len()) || value.chars().any(char::is_control) {
        return Err(DarwinConfigError::InvalidUsername);
    }
    Ok(value.to_owned())
}

fn parse_remember_password(value: &str) -> Result<bool, DarwinConfigError> {
    parse_strict_bool(value).ok_or(DarwinConfigError::InvalidRememberPassword)
}

/// 与 win32 `bool::from_str` 相同的严格串：仅接受 `"true"` / `"false"`。
fn parse_strict_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_auto_reconnect(value: &str) -> Result<bool, DarwinConfigError> {
    parse_strict_bool(value).ok_or(DarwinConfigError::InvalidAutoReconnect)
}

/// win32 同款：`u32` 解析即可，`0` = 不限次数；无上限校验（前端已挡）。
fn parse_auto_reconnect_max_attempts(value: &str) -> Result<u32, DarwinConfigError> {
    value
        .parse::<u32>()
        .ok()
        .ok_or(DarwinConfigError::InvalidAutoReconnectMaxAttempts)
}

fn parse_auto_reconnect_backoff(value: &str) -> Result<bool, DarwinConfigError> {
    parse_strict_bool(value).ok_or(DarwinConfigError::InvalidAutoReconnectBackoff)
}

fn parse_routes(value: &str) -> Result<Vec<String>, DarwinConfigError> {
    split_csv(value)
        .into_iter()
        .all(|value| is_windows_compatible_route(&value))
        .then(|| split_csv(value))
        .ok_or(DarwinConfigError::InvalidRoutes)
}

fn parse_mtu(value: &str) -> Result<u32, DarwinConfigError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|mtu| *mtu != 0)
        .ok_or(DarwinConfigError::InvalidMtu)
}

fn validate_user_agent(value: &str) -> Result<String, DarwinConfigError> {
    if !(1..=256).contains(&value.len()) || value.chars().any(char::is_control) {
        return Err(DarwinConfigError::InvalidUserAgent);
    }
    Ok(value.to_owned())
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_windows_compatible_route(value: &str) -> bool {
    if parse_windows_ipv4(value).is_some() {
        return true;
    }
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    parse_windows_ipv4(address).is_some() && prefix.parse::<u32>().is_ok_and(|prefix| prefix <= 32)
}

fn parse_windows_ipv4(value: &str) -> Option<[u8; 4]> {
    let octets = value
        .split('.')
        .map(|octet| octet.parse::<u8>().ok())
        .collect::<Option<Vec<_>>>()?;
    octets.try_into().ok()
}
