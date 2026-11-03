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

use serde::{Deserialize, Deserializer, Serialize};
use tonic::Code;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    config_crypto::{KEY_LEN, decrypt_password, encrypt_password, generate_key},
    config_paths::{atomic_write, config_dir, config_path, key_path},
};

const CONFIG_KEYS: [&str; 15] = [
    "server",
    "username",
    "password",
    "remember_password",
    "routes",
    "user_agent",
    "mtu",
    "protocol",
    // M1（darwin aTrust 适配）：aTrust 三键（设计 §8）。
    "atrust_server",
    "atrust_auth_mode",
    "atrust_auto_login",
    "campus_dns_domains",
    "auto_reconnect",
    "auto_reconnect_max_attempts",
    "auto_reconnect_backoff",
];

const DEFAULT_SERVER: &str = "vpn-cn.ecnu.edu.cn";
/// M1：校园 DNS 搜索域默认值（与 win32 `DEFAULT_CAMPUS_DNS_DOMAINS` 同值共享语义）。
const DEFAULT_CAMPUS_DNS_DOMAINS: [&str; 1] = ["ecnu.edu.cn"];
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

/// 本次连接使用的 VPN 协议（与 win32 `Protocol` 同构）。
///
/// 2026-09-21（T02）：协议是**显式**配置项——同一个域名不靠名字猜协议。缺失
/// （旧配置）或未知取值一律解释为 [`Protocol::Cisco`]，保持升级前的行为；用户选择
/// aTrust 必须由显式写入该字段表达。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// Cisco AnyConnect / CSTP（现有协议，缺省值）。
    #[default]
    Cisco,
    /// 华师大 aTrust（外部浏览器统一认证）。
    Atrust,
}

impl Protocol {
    /// 稳定配置字面量；也用于 `ConfigGet` 字段表与不含凭据的诊断日志。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cisco => "cisco",
            Self::Atrust => "atrust",
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for Protocol {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "cisco" => Ok(Self::Cisco),
            "atrust" => Ok(Self::Atrust),
            _ => Err(()),
        }
    }
}

impl<'de> Deserialize<'de> for Protocol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        // 配置文件可能来自更高版本。与 win32 同一条纪律：未知字符串只局部回退为
        // cisco，不让整份配置回落默认值而丢掉同一文档里的账户、服务器与密文。
        Ok(value.parse().unwrap_or_default())
    }
}

/// aTrust 认证入口模式（M1 新键 `atrust_auth_mode` 的取值域）。
///
/// 2026-10-22 双端对齐裁定：默认 [`AtrustAuthMode::Headless`]（与 win32 新装默认
/// 一致；此前默认 embedded）。`browser` 为系统浏览器/受控 CDP 回退，按 M5 完成度
/// 如实开关；已显式保存的旧值不受默认变化影响（缺键/未知值才回落默认）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AtrustAuthMode {
    /// 内嵌认证窗（M5 产物；M1 仅冻结契约）。
    Embedded,
    /// 系统浏览器交接 / 受控 CDP 回退。
    #[default]
    Browser,
    /// HTTP/CAS 复用优先，必要时回退隐藏内嵌窗。
    Headless,
}

impl AtrustAuthMode {
    /// 稳定配置字面量（ConfigGet 字段表/诊断日志用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::Browser => "browser",
            Self::Headless => "headless",
        }
    }
}

impl fmt::Display for AtrustAuthMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for AtrustAuthMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "embedded" => Ok(Self::Embedded),
            "browser" => Ok(Self::Browser),
            "headless" => Ok(Self::Headless),
            _ => Err(()),
        }
    }
}

impl<'de> Deserialize<'de> for AtrustAuthMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        // 与 Protocol 同一条纪律：未知字符串只局部回退默认（headless），不落整份默认。
        Ok(value.parse().unwrap_or_default())
    }
}

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DarwinConfigError {
    UnknownKey,
    DuplicateKey,
    InvalidServer,
    InvalidUsername,
    InvalidRememberPassword,
    InvalidRoutes,
    InvalidMtu,
    InvalidUserAgent,
    InvalidProtocol,
    InvalidAtrustServer,
    InvalidAtrustAuthMode,
    InvalidAtrustAutoLogin,
    InvalidCampusDnsDomains,
    InvalidAutoReconnect,
    InvalidAutoReconnectMaxAttempts,
    InvalidAutoReconnectBackoff,
    Storage,
    ReadFailed {
        path: PathBuf,
        raw_os_error: Option<i32>,
    },
    PasswordCrypto,
}

impl DarwinConfigError {
    #[must_use]
    pub const fn grpc_code(&self) -> Code {
        match self {
            Self::UnknownKey
            | Self::DuplicateKey
            | Self::InvalidServer
            | Self::InvalidUsername
            | Self::InvalidRememberPassword
            | Self::InvalidRoutes
            | Self::InvalidMtu
            | Self::InvalidUserAgent
            | Self::InvalidProtocol
            | Self::InvalidAtrustServer
            | Self::InvalidAtrustAuthMode
            | Self::InvalidAtrustAutoLogin
            | Self::InvalidCampusDnsDomains
            | Self::InvalidAutoReconnect
            | Self::InvalidAutoReconnectMaxAttempts
            | Self::InvalidAutoReconnectBackoff => Code::InvalidArgument,
            Self::Storage | Self::ReadFailed { .. } | Self::PasswordCrypto => Code::Internal,
        }
    }

    #[must_use]
    pub const fn message(&self) -> &'static str {
        match self {
            Self::UnknownKey => "DARWIN_CONFIG_UNKNOWN_KEY",
            Self::DuplicateKey => "DARWIN_CONFIG_DUPLICATE_KEY",
            Self::InvalidServer => "DARWIN_CONFIG_INVALID_SERVER",
            Self::InvalidUsername => "DARWIN_CONFIG_INVALID_USERNAME",
            Self::InvalidRememberPassword => "DARWIN_CONFIG_INVALID_REMEMBER_PASSWORD",
            Self::InvalidRoutes => "DARWIN_CONFIG_INVALID_ROUTES",
            Self::InvalidMtu => "DARWIN_CONFIG_INVALID_MTU",
            Self::InvalidUserAgent => "DARWIN_CONFIG_INVALID_USER_AGENT",
            Self::InvalidProtocol => "DARWIN_CONFIG_INVALID_PROTOCOL",
            Self::InvalidAtrustServer => "DARWIN_CONFIG_INVALID_ATRUST_SERVER",
            Self::InvalidAtrustAuthMode => "DARWIN_CONFIG_INVALID_ATRUST_AUTH_MODE",
            Self::InvalidAtrustAutoLogin => "DARWIN_CONFIG_INVALID_ATRUST_AUTO_LOGIN",
            Self::InvalidCampusDnsDomains => "DARWIN_CONFIG_INVALID_CAMPUS_DNS_DOMAINS",
            Self::InvalidAutoReconnect => "DARWIN_CONFIG_INVALID_AUTO_RECONNECT",
            Self::InvalidAutoReconnectMaxAttempts => {
                "DARWIN_CONFIG_INVALID_AUTO_RECONNECT_MAX_ATTEMPTS"
            }
            Self::InvalidAutoReconnectBackoff => "DARWIN_CONFIG_INVALID_AUTO_RECONNECT_BACKOFF",
            Self::Storage => "DARWIN_CONFIG_STORAGE_FAILED",
            Self::ReadFailed { .. } => "DARWIN_CONFIG_READ_FAILED",
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
    /// S5a（2026-10-26 owner 标记）：已封存密文归属的账户名。写入新密码时随写为
    /// 当时的 `username`；仅改用户名（设置模态 keep 分支只发 `username` 键）时密文
    /// 与本字段原样保留——凭据对的有效性由 `username == password_owner_username`
    /// 判定（`DarwinUiConfig::has_saved_credential_pair`），改回原用户名即恢复资格。
    /// 物理清空只发生在显式 `remember_password=false` / 清除密码路径（连同本字段
    /// 一并清空）。旧配置缺该键时空串，读取入口按 owner=username 迁移（历史写路径
    /// 在任何用户名变更时都清空密文，存量密文必然属于当前用户名，迁移保持存量行为）。
    password_owner_username: String,
    remember_password: bool,
    routes: Vec<String>,
    #[serde(rename = "useragent")]
    user_agent: String,
    mtu: u32,
    /// 本次连接使用的协议。
    ///
    /// 三层默认刻意分层（2026-09-30 用户裁定，翻转设计 §8 的「默认不翻转」）：
    /// - **反序列化缺键**（旧配置无 protocol 键，本字段级默认）保持 Cisco——协议键
    ///   面世（T02）前的安装基数全是 Cisco 用户，升级不得隐式改协议；
    /// - **[`Default`]**（兼容加载/测试基线）保持 Cisco——除新装 bootstrap 外的一切
    ///   缺省构造不翻转；
    /// - **新装 bootstrap**（config.json 缺失，[`Self::bootstrap_default`]）为
    ///   aTrust——darwin 新装默认激活 aTrust（与前端 `RECOMMENDED_PROTOCOL` 对齐）。
    #[serde(default = "default_protocol_for_missing_key")]
    protocol: Protocol,
    /// aTrust 网关（裸主机名，与 `server` 同款约定）。M1 新键：**空 = 未配置**——
    /// darwin 不给默认网关（已有 Cisco 安装基数；不学 win32 新装默认 aTrust）。旧
    /// 配置缺该键时为空串，语义即「未配置」，与读路径回退一致。
    atrust_server: String,
    /// aTrust 认证入口模式（M1 新键；默认 headless，见 [`AtrustAuthMode`]）。
    atrust_auth_mode: AtrustAuthMode,
    atrust_auth_mode_version: u32,
    /// 用户发起连接后允许一次学校表单填交；新装与旧配置缺键均默认关闭。
    atrust_auto_login: bool,
    /// 校园 DNS 搜索域（M1 新键）。**键形=列表**（win32 共享语义/前向兼容——多域
    /// 属前向能力，首发 UI 仅单域编辑）；旧配置缺键或值非法时回默认 `ecnu.edu.cn`。
    #[serde(default = "default_campus_dns_domains", deserialize_with = "deserialize_campus_dns_domains")]
    campus_dns_domains: Vec<String>,
    auto_reconnect: bool,
    auto_reconnect_max_attempts: u32,
    auto_reconnect_backoff: bool,
}

pub(crate) fn normalize_atrust_auth_fields(object: &mut serde_json::Map<String, serde_json::Value>) {
    let mode = object.get("atrust_auth_mode").and_then(serde_json::Value::as_str).unwrap_or("browser");
    let legacy = object.get("atrust_auth_mode_version").and_then(serde_json::Value::as_u64) != Some(1);
    let automatic = object.get("atrust_auto_login").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let mode = if legacy && automatic { "headless" } else if legacy && mode == "headless" { "embedded" } else { mode }.to_owned();
    object.insert("atrust_auto_login".into(), serde_json::Value::Bool(mode == "headless"));
    object.insert("atrust_auth_mode".into(), serde_json::Value::String(mode));
    object.insert("atrust_auth_mode_version".into(), serde_json::Value::from(1));
}

impl Default for DarwinStoredConfig {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.to_owned(),
            username: String::new(),
            password: String::new(),
            password_owner_username: String::new(),
            remember_password: false,
            routes: DEFAULT_ROUTES
                .iter()
                .map(|route| (*route).to_owned())
                .collect(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            mtu: DEFAULT_MTU,
            protocol: default_protocol_for_missing_key(),
            // aTrust 三键默认——未配置网关 / browser / ecnu.edu.cn。
            atrust_server: String::new(),
            atrust_auth_mode: AtrustAuthMode::Browser,
            atrust_auth_mode_version: 1,
            atrust_auto_login: false,
            campus_dns_domains: default_campus_dns_domains(),
            auto_reconnect: false,
            auto_reconnect_max_attempts: 0,
            auto_reconnect_backoff: false,
        }
    }
}

impl DarwinStoredConfig {
    /// **新装 bootstrap 专用缺省**（2026-09-30 用户裁定）：protocol = aTrust，其余
    /// 同 [`Self::default()`]。唯一调用方是 `config_hygiene::bootstrap_default`
    /// （config.json 缺失/不可用 → 落盘 + `requires_quick_start=true` 的产码点）；
    /// 其它一切缺省构造（兼容加载、测试基线、反序列化缺键）仍 Cisco，安装基数
    /// 不被隐式翻转。
    pub(crate) fn bootstrap_default() -> Self {
        Self {
            protocol: Protocol::Atrust,
            ..Self::default()
        }
    }

    fn load_from_dir(dir: &Path) -> Result<Self, DarwinConfigError> {
        let text = match std::fs::read_to_string(config_path(dir)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(_) => return Err(DarwinConfigError::Storage),
        };
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else { return Ok(Self::default()); };
        let Some(object) = value.as_object_mut() else { return Ok(Self::default()); };
        normalize_atrust_auth_fields(object);
        let mut stored = serde_json::from_value::<Self>(value).unwrap_or_default();
        stored.reconcile_password_owner();
        Ok(stored)
    }

    /// S5a 迁移：旧 `config.json` 无 `password_owner_username` 键（反序列化为空串）
    /// 时视为 owner=当前用户名。历史写路径在任何用户名变更时都清空密文，故存量
    /// 密文必然属于当前用户名——该迁移保证既有安装的凭据对继续有效，行为不变。
    /// 新代码写出的 owner 非空（keep 分支保留原值），不会误触发迁移。
    pub(crate) fn reconcile_password_owner(&mut self) {
        if self.password_owner_username.is_empty() && !self.password.is_empty() {
            self.password_owner_username.clone_from(&self.username);
        }
    }

    pub(crate) fn save_to_dir_atomically(&self, dir: &Path) -> Result<(), DarwinConfigError> {
        let json = serde_json::to_string_pretty(self).map_err(|_| DarwinConfigError::Storage)?;
        atomic_write(&config_path(dir), json.as_bytes()).map_err(|_| DarwinConfigError::Storage)
    }

    /// 读取或按需生成 `key.bin`（~/.exv 文件加密体系的唯一密钥源；密码与会话缓存
    /// 共用）。M5 起 `atrust_session_cache` 经本入口取键，保证两处加密同一实现。
    pub(crate) fn ensure_key(dir: &Path) -> Result<[u8; KEY_LEN], DarwinConfigError> {
        if let Ok(Some(key)) = Self::load_key(dir) {
            Ok(key)
        } else {
            let key = generate_key()?;
            Self::save_key(dir, &key)?;
            Ok(key)
        }
    }

    pub(crate) fn load_key(dir: &Path) -> Result<Option<[u8; KEY_LEN]>, DarwinConfigError> {
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
        // S5a：replace 语义——owner 随写入更新为当前用户名（调用方保证此时尚未
        // 应用 keep 分支的"仅改用户名"效果：本方法只在用户名已确定为新值后调用）。
        self.password_owner_username.clone_from(&self.username);
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
            // 键序对齐 win32 `config_get` 字段表（mtu → protocol → 自动重连三项）；
            // M1：aTrust 三键插在 protocol 之后（win32 把 aTrust 设置组同样排在协议位
            // 附近），自动重连三项与 has_stored_password 顺延。
            DarwinConfigItem::new("protocol", self.inner.protocol.as_str()),
            DarwinConfigItem::new("atrust_server", self.inner.atrust_server.clone()),
            DarwinConfigItem::new("atrust_auth_mode", self.inner.atrust_auth_mode.as_str()),
            DarwinConfigItem::new(
                "campus_dns_domains",
                self.inner.campus_dns_domains.join(","),
            ),
            DarwinConfigItem::new("auto_reconnect", self.inner.auto_reconnect.to_string()),
            DarwinConfigItem::new(
                "auto_reconnect_max_attempts",
                self.inner.auto_reconnect_max_attempts.to_string(),
            ),
            DarwinConfigItem::new(
                "auto_reconnect_backoff",
                self.inner.auto_reconnect_backoff.to_string(),
            ),
            DarwinConfigItem::new("atrust_auto_login", self.inner.atrust_auto_login.to_string()),
            DarwinConfigItem::new("has_saved_credential_pair", self.has_saved_credential_pair().to_string()),
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

    /// T02：本次连接使用的协议。连接入口据此判定协议可服务性（aTrust 尚未接入
    /// 本版本时必须显式拒绝，绝不静默按 Cisco 连接）。
    #[must_use]
    pub const fn protocol(&self) -> Protocol {
        self.inner.protocol
    }

    /// M1：aTrust 网关（裸主机名；空 = 未配置）。
    #[must_use]
    pub fn atrust_server(&self) -> &str {
        &self.inner.atrust_server
    }

    /// aTrust 认证入口模式（默认 browser）。
    #[must_use]
    pub const fn atrust_auth_mode(&self) -> AtrustAuthMode {
        self.inner.atrust_auth_mode
    }

    /// 是否允许本次连接中的一次学校表单自动填交；不表示已登录。
    #[must_use]
    pub const fn atrust_auto_login(&self) -> bool {
        self.inner.atrust_auto_login
    }

    /// 校园 DNS 域列表；完整传入当前连接的 Cisco V3 或 aTrust V2 载荷。
    #[must_use]
    pub fn campus_dns_domains(&self) -> &[String] {
        &self.inner.campus_dns_domains
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
    ///
    /// S5a（2026-10-26 owner 标记）：未提供新密码（`None` 或空串）而切换账户时
    /// **保留旧密文与 owner**——凭据对按 owner 匹配门失效（`has_saved_credential_
    /// pair` 为假），改回原用户名即恢复资格；密文本身只有显式清除路径才会物理
    /// 删除。空串不是密码：`Some("")` 与 `None` 同义（不写空密文、不生成密钥、
    /// 不动记住开关）。
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
        candidate.username = username;
        if let Some(password) = password.filter(|password| !password.is_empty()) {
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

    /// 恢复派发使用冻结协议的内存副本，配置目录及其他保存字段不变，不写磁盘。
    pub(crate) fn with_temporary_protocol(&self, protocol: Protocol) -> Self {
        let mut temporary = self.clone();
        temporary.inner.protocol = protocol;
        temporary
    }

    /// 恢复沿用受理时已规范化的 aTrust 网关，不写回用户保存设置。
    pub(crate) fn with_temporary_atrust_server(&self, server: &str) -> Self {
        let mut temporary = self.clone();
        temporary.inner.atrust_server = server.to_owned();
        temporary
    }

    /// 只有真实解密成功且非空的记住密码才算可用；只读，不创建密钥。
    #[must_use]
    pub fn has_stored_password(&self) -> bool {
        self.load_saved_password().map(zeroize::Zeroizing::new)
            .is_ok_and(|password| !password.is_empty())
    }

    /// 两协议共用原保存对，前台认证模式不参与后台资格。
    ///
    /// S5a owner 匹配门：凭据对有效还要求当前用户名与密文 owner（写入密码时的
    /// 账户名）一致——keep 分支仅改用户名后此判定为假（后台重连资格暂停），改回
    /// 原用户名立即恢复为真。重连资格（`background_reconnect_allowed`）与受理身份
    /// （`SessionReconnectPolicy::from_config`）共用本门。
    pub fn has_saved_credential_pair(&self) -> bool {
        !self.username().trim().is_empty()
            && self.username() == self.inner.password_owner_username
            && self.has_stored_password()
    }

    /// S5a：已封存密文的 owner（写入密码时的账户名）；空串表示无已封存密文
    /// （读取入口对旧数据按 owner=username 迁移，见 [`DarwinStoredConfig`]
    /// 字段注释）。
    #[must_use]
    pub fn password_owner_username(&self) -> &str {
        &self.inner.password_owner_username
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
    protocol: Option<Protocol>,
    atrust_server: Option<String>,
    atrust_auth_mode: Option<AtrustAuthMode>,
    atrust_auto_login: Option<bool>,
    campus_dns_domains: Option<Vec<String>>,
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
                "protocol" => update.protocol = Some(parse_protocol(&value)?),
                // 与 `server` 同款规范化（裸主机名），但错误归入 aTrust 专属码——
                // 设置页据此把可行动原因指向 aTrust 服务器字段而非 Cisco 服务器字段。
                "atrust_server" => {
                    update.atrust_server =
                        Some(normalize_server(&value).map_err(|_| DarwinConfigError::InvalidAtrustServer)?);
                }
                "atrust_auth_mode" => {
                    update.atrust_auth_mode = Some(parse_atrust_auth_mode(&value)?);
                }
                "atrust_auto_login" => {
                    update.atrust_auto_login = Some(
                        parse_strict_bool(&value).ok_or(DarwinConfigError::InvalidAtrustAutoLogin)?,
                    );
                }
                "campus_dns_domains" => {
                    update.campus_dns_domains = Some(parse_campus_dns_domains(&value)?);
                }
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
            // S5a：仅改用户名（keep 分支——设置模态只发 username 键）不再清空密文。
            // 密文与 owner 原样保留：凭据对按 owner 匹配门失效（本函数末尾的
            // has_pair 复核据此关闭自动重连），改回原用户名即恢复资格。物理清空
            // 只由下方显式 remember_password=false 路径触发。
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
        if let Some(protocol) = self.protocol {
            candidate.protocol = protocol;
        }
        if let Some(atrust_server) = self.atrust_server {
            candidate.atrust_server = atrust_server;
        }
        if let Some(atrust_auth_mode) = self.atrust_auth_mode {
            candidate.atrust_auth_mode = atrust_auth_mode;
        }
        candidate.atrust_auth_mode_version = 1;
        candidate.atrust_auto_login = candidate.atrust_auth_mode == AtrustAuthMode::Headless;
        if let Some(campus_dns_domains) = self.campus_dns_domains {
            candidate.campus_dns_domains = campus_dns_domains;
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
            // 显式清除路径：密文与 owner 一并物理清空（既有语义保留）。
            candidate.password.zeroize();
            candidate.password.clear();
            candidate.password_owner_username.clear();
            if self.auto_reconnect == Some(true) && self.remember_password != Some(false) {
                return Err(DarwinConfigError::InvalidAutoReconnect);
            }
            candidate.auto_reconnect = false;
            return Ok(());
        }

        if let Some(password) = self.password.filter(|password| !password.is_empty()) {
            let mut key = DarwinStoredConfig::ensure_key(config_dir)?;
            let result = candidate.set_password_encrypted(password.as_str(), &key);
            key.zeroize();
            result?;
        }
        let has_pair = DarwinUiConfig::from_stored(candidate.clone(), config_dir).has_saved_credential_pair();
        if !has_pair {
            if self.auto_reconnect == Some(true) {
                return Err(DarwinConfigError::InvalidAutoReconnect);
            }
            candidate.auto_reconnect = false;
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

/// T02：写路径必须严格——非法协议报错，绝不静默落回 cisco（读路径对未知取值的
/// 局部回退只用于容忍更高版本写下的配置文件）。取值大小写敏感，与 win32
/// `Protocol::from_str` 同源。
fn parse_protocol(value: &str) -> Result<Protocol, DarwinConfigError> {
    value
        .parse::<Protocol>()
        .map_err(|()| DarwinConfigError::InvalidProtocol)
}

/// 反序列化缺键与 `Default` 的协议默认：**Cisco**（安装基数保护；新装 bootstrap 的
/// aTrust 见 [`DarwinStoredConfig::bootstrap_default`]）。
fn default_protocol_for_missing_key() -> Protocol {
    Protocol::Cisco
}

/// M1：`atrust_auth_mode` 写路径严格（win32 `config_set` 对该键的 must-be 检查同款），
/// 仅接受 `embedded`/`browser`/`headless`（大小写敏感）；读路径的局部回退见 [`AtrustAuthMode`]。
fn parse_atrust_auth_mode(value: &str) -> Result<AtrustAuthMode, DarwinConfigError> {
    value
        .parse::<AtrustAuthMode>()
        .map_err(|()| DarwinConfigError::InvalidAtrustAuthMode)
}

/// 校园 DNS 搜索域默认值（`#[serde(default)]`/读路径回退共用）。
fn default_campus_dns_domains() -> Vec<String> {
    DEFAULT_CAMPUS_DNS_DOMAINS
        .iter()
        .map(|domain| (*domain).to_owned())
        .collect()
}

/// M1：解析逗号分隔的校园 DNS 域名列表（win32 `parse_campus_dns_domains` 同款语义）。
///
/// 空串/全空白表示关闭（清空列表）；其余条目去空白、去尾点、小写、保序去重。域名只
/// 接受 ASCII DNS labels，不接受 IP、URL、通配符或端口；任一条目非法则整批拒绝。
///
/// # Errors
/// 任一条目不是合法 ASCII DNS 域名 → [`DarwinConfigError::InvalidCampusDnsDomains`]。
fn parse_campus_dns_domains(value: &str) -> Result<Vec<String>, DarwinConfigError> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    normalize_campus_dns_domain_values(value.split(',').collect())
        .ok_or(DarwinConfigError::InvalidCampusDnsDomains)
}

/// 条目级规范化：小写、去空白、去尾点、保序去重；非法条目使整批失败（返回 `None`）。
fn normalize_campus_dns_domain_values(values: Vec<&str>) -> Option<Vec<String>> {
    let mut domains = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in values {
        let domain = value.trim().trim_end_matches('.').to_ascii_lowercase();
        if domain.is_empty() {
            continue;
        }
        if !is_valid_campus_dns_domain(&domain) {
            return None;
        }
        if seen.insert(domain.clone()) {
            domains.push(domain);
        }
    }
    Some(domains)
}

/// 合法校园域：ASCII、非 IP/非纯数字、总长 ≤253、逐 label（1..=63、首尾字母数字、
/// 内部允许连字符）。win32 `is_valid_campus_dns_domain` 同款。
fn is_valid_campus_dns_domain(domain: &str) -> bool {
    if domain.len() > 253
        || !domain.is_ascii()
        || domain.parse::<std::net::IpAddr>().is_ok()
        || domain.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return false;
    }
    domain.split('.').all(|label| {
        (1..=63).contains(&label.len())
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

/// 读路径容忍派生（win32 `deserialize_campus_dns_domains` 同款）：磁盘上的列表来自
/// 本产品写路径时必然合法；更高版本写下的非法取值只把该键回默认，不落整份默认
/// （同文档里的账户、自定义服务器与密文必须原样保留）。
fn deserialize_campus_dns_domains<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let values = value.as_array().and_then(|items| {
        items
            .iter()
            .map(serde_json::Value::as_str)
            .collect::<Option<Vec<_>>>()
    });
    Ok(values
        .and_then(|domains| normalize_campus_dns_domain_values(domains))
        .unwrap_or_else(default_campus_dns_domains))
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
