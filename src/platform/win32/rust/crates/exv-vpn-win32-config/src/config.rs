
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
/// aTrust 校园统一认证门户默认地址。
pub const DEFAULT_ATRUST_SERVER: &str = "https://vpn.ecnu.edu.cn";
/// 新配置使用的校园 DNS 搜索域。
pub const DEFAULT_CAMPUS_DNS_DOMAINS: [&str; 1] = ["ecnu.edu.cn"];
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

/// core 退出时 engine 对连接的处置策略（T4，计划 §2.5/§2.6，决策 D6）。
///
/// 该键随连接信封（`ApplyTunnelRequest.on_core_exit`）镜像进 engine 连接痕迹；
/// engine 侧在 T5 前不据此改变行为（信封缺省语义见 proto 头注）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnCoreExit {
    /// 保连接等新 core 采纳（默认，D6；engine EOF + PID 复核后不拆隧道）。
    #[default]
    Keep,
    /// 随 core 退出执行拆除善后（遗留语义）。
    Teardown,
}

impl OnCoreExit {
    /// 稳定配置字面量；也用于不含凭据的诊断日志。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Teardown => "teardown",
        }
    }
}

impl std::fmt::Display for OnCoreExit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for OnCoreExit {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "keep" => Ok(Self::Keep),
            "teardown" => Ok(Self::Teardown),
            _ => Err(()),
        }
    }
}

impl<'de> Deserialize<'de> for OnCoreExit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        // 与 ConnectionMode 同款局部回退：未知取值按默认 keep，不丢整份配置。
        Ok(value.parse().unwrap_or_default())
    }
}

/// 新配置的 `reconnect_notify` 默认值（计划 §2.8：默认开）。
const fn default_reconnect_notify() -> bool {
    true
}

/// 本次连接使用的 VPN 协议。
///
/// 2026-09-21（T02）：协议是**显式**配置项——同一个域名不靠名字猜协议。缺失
/// （旧配置）或未知取值一律解释为 [`Protocol::Cisco`]，保持升级前的行为；用户
/// 选择 `aTrust` 必须由显式写入该字段表达。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// Cisco `AnyConnect` / `CSTP`（现有协议，缺省值）。
    #[default]
    Cisco,
    /// 华师大 `aTrust`（外部浏览器统一认证）。
    Atrust,
}

impl Protocol {
    /// 稳定配置字面量；也用于不含凭据的诊断日志。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cisco => "cisco",
            Self::Atrust => "atrust",
        }
    }

    /// 是否需要在连接前完成外部浏览器统一认证。
    #[must_use]
    pub const fn requires_browser_login(self) -> bool {
        match self {
            Self::Cisco => false,
            Self::Atrust => true,
        }
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        // 与 ConnectionMode 同一条纪律：未知字符串只局部回退为 cisco，
        // 不让整份 ExvConfig 回落默认值而丢掉账户、自定义服务器与密文。
        Ok(value.parse().unwrap_or_default())
    }
}

fn legacy_default_protocol() -> Protocol { Protocol::Cisco }

fn default_campus_dns_domains() -> Vec<String> {
    DEFAULT_CAMPUS_DNS_DOMAINS.iter().map(|domain| (*domain).to_string()).collect()
}

/// `parse_campus_dns_domains` 的拒绝标记。无 payload：被拒输入字符串本身即全部
/// 诊断信息（调用方要么回退默认值、要么向用户回显自己的固定提示语）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCampusDnsDomain;

/// 解析逗号分隔的校园 DNS 域名列表。空串表示关闭；其他空白条目忽略。
///
/// 域名只接受 ASCII DNS labels，不接受 IP、URL、通配符或端口。
///
/// # Errors
/// 任一条目不是合法 ASCII DNS 域名（含 IP/URL/通配符/端口形态）时返回
/// [`InvalidCampusDnsDomain`]；错误不区分具体条目（拒绝即整体拒绝）。
pub fn parse_campus_dns_domains(value: &str) -> Result<Vec<String>, InvalidCampusDnsDomain> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    normalize_campus_dns_domain_values(value.split(',').collect())
}

fn normalize_campus_dns_domain_values(
    values: Vec<&str>,
) -> Result<Vec<String>, InvalidCampusDnsDomain> {
    let mut domains = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in values {
        let domain = value.trim().trim_end_matches('.').to_ascii_lowercase();
        if domain.is_empty() {
            continue;
        }
        if !is_valid_campus_dns_domain(&domain) {
            return Err(InvalidCampusDnsDomain);
        }
        if seen.insert(domain.clone()) {
            domains.push(domain);
        }
    }
    Ok(domains)
}

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
        .and_then(|domains| normalize_campus_dns_domain_values(domains).ok())
        .unwrap_or_else(default_campus_dns_domains))
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
/// 布尔字段数超出 pedantic 阈值（T4 增 `reconnect_on_oneshot_engine_crash` /
/// `reconnect_notify` 后为 5）：它们是随连接信封镜像的**策略平面**（§2.7/§2.8），
/// `与凭据开关（remember_password）语义不同类但同表共存——拆表收益为负（跨文件`
/// 原子写换不来任何不变量），按字段族聚合是刻意的。
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExvConfig {
    /// 当前活动协议的有效服务器地址。Engine 仍从该字段读取当前连接目标。
    pub server: String,
    /// Cisco 专属服务器地址，独立于当前协议保存。
    pub cisco_server: String,
    /// aTrust 专属服务器地址，独立于当前协议保存。
    pub atrust_server: String,
    /// aTrust 认证入口：`browser` 为系统浏览器，`embedded` 为学校网页内嵌认证。
    pub atrust_auth_mode: String,
    /// 校园 DNS 搜索域；每个裸域名同时覆盖该域及其子域。
    #[serde(default = "default_campus_dns_domains", deserialize_with = "deserialize_campus_dns_domains")]
    pub campus_dns_domains: Vec<String>,
    /// 本次连接使用的协议；旧配置缺该键时解释为 Cisco。
    #[serde(default = "legacy_default_protocol")]
    pub protocol: Protocol,
    /// Login username.
    pub username: String,
    /// AES-GCM sealed password (base64), or empty when not remembered.
    pub password: String,
    /// 已封存密码所属的协议。写入密码时随当时的 [`ExvConfig::protocol`] 一起记录，
    /// 使“认证材料按协议区分”成为结构约束而非约定：Cisco 的密码不会被当作
    /// `aTrust` 会话材料消费（旧配置缺该键时为 Cisco，与密码字段的既有语义一致）。
    pub password_protocol: Protocol,
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
    /// core 退出策略（T4，D6 默认 keep）：随连接信封镜像进 engine 连接痕迹；
    /// engine 侧行为接线归 T5。旧配置缺键按 keep（struct 级 `serde(default)` +
    /// `Default` 实现共同给出；未知取值由 `OnCoreExit::deserialize` 局部回退）。
    pub on_core_exit: OnCoreExit,
    /// 「一次性 engine 崩溃导致断连时仍然重连」（T4/§2.8，R2 oneshot 分支门控；
    /// 默认**关**——重连必然重新提权弹 UAC）。仅未安装服务时可见（UI 可见性规则）。
    pub reconnect_on_oneshot_engine_crash: bool,
    /// 「自动重连时发送通知」（T4/§2.8，既有托盘气泡通道；默认**开**，见
    /// [`default_reconnect_notify`]）。
    pub reconnect_notify: bool,
}

impl Default for ExvConfig {
    fn default() -> Self {
        Self {
            server: DEFAULT_ATRUST_SERVER.to_string(),
            cisco_server: DEFAULT_SERVER.to_string(),
            atrust_server: DEFAULT_ATRUST_SERVER.to_string(),
            atrust_auth_mode: "browser".to_string(),
            campus_dns_domains: default_campus_dns_domains(),
            protocol: Protocol::Atrust,
            username: String::new(),
            password: String::new(),
            password_protocol: Protocol::Cisco,
            remember_password: false,
            routes: DEFAULT_ROUTES.iter().map(|r| (*r).to_string()).collect(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            mtu: DEFAULT_MTU,
            connection_mode: ConnectionMode::Standard,
            auto_reconnect: false,
            auto_reconnect_max_attempts: 0,
            auto_reconnect_backoff: false,
            on_core_exit: OnCoreExit::Keep,
            reconnect_on_oneshot_engine_crash: false,
            reconnect_notify: default_reconnect_notify(),
        }
    }
}

/// 「协议切换前的 Cisco 备份」文件名（与 `config.json` 同目录）。
///
/// 2026-09-21（T02）：旧版本 EXV 不认识 `protocol` 字段（serde 未知字段容忍 ⇒ 读到即忽略），
/// 会把 aTrust 门户地址当作 CSTP 网关并把已封存密码发过去。因此离开 Cisco 之前必须留下
/// 一份**旧版本可读**的配置：回退时把它复制回 `config.json` 即可恢复原有行为。
/// 参见计划 T02「降级恢复使用升级前备份或显式导出的 Cisco 配置」。
pub const PROTOCOL_SWITCH_BACKUP_FILE: &str = "config.protocol-switch-backup.json";

/// 把这份配置写成协议切换前的 Cisco 备份，返回落盘路径。
///
/// 原子写入到 [`PROTOCOL_SWITCH_BACKUP_FILE`]：先写同目录临时文件再改名，失败时删除临时
/// 文件并保留原有备份。**不触碰 `config.json`**。
///
/// # Errors
///
/// 序列化或写入失败 → [`ConfigError`]。
pub fn write_protocol_switch_backup(
    dir: &Path,
    config: &ExvConfig,
) -> Result<PathBuf, ConfigError> {
    let mut snapshot = config.clone();
    snapshot.sync_active_server();
    let json = serde_json::to_string_pretty(&snapshot)?;
    let destination = dir.join(PROTOCOL_SWITCH_BACKUP_FILE);
    write_json_atomically_to(dir, &destination, &json)?;
    Ok(destination)
}

/// 读取配置文件里 `protocol` 的**原始字面量**（不做解释）。
///
/// 2026-09-21（T02）：反序列化对未知取值只做局部回退，以免丢掉同一文档里的账户、密文与
/// 自定义地址；但“键缺失”（旧配置，合规地解释为 Cisco）与“键存在且本版本不认识”（可能由
/// 更高版本写入）是两件事。后者若被静默当成 Cisco，就会拿 Cisco 凭据去连一个别的协议的
/// 网关。因此这里把原始字面量交给调用方，由连接入口决定如何拒绝。
///
/// # Errors
///
/// 读取失败（`NotFound` 之外）→ [`ConfigError::Io`]。缺失或无法解析的文件返回 `Ok(None)`，
/// 与 [`ExvConfig::load_from_dir`] 的容错语义一致。
pub fn read_protocol_literal(dir: &Path) -> Result<Option<String>, ConfigError> {
    let path = config_path(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(None);
    };
    Ok(value
        .get("protocol")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
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
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Ok(Self::default());
        };
        let Some(object) = value.as_object_mut() else {
            return Ok(Self::default());
        };
        // 无 protocol 键表示升级前的旧配置，固定迁移为 Cisco；缺文件的新安装仍直接
        // 返回 Default（aTrust），两种来源在这里明确区分。
        let protocol = object
            .get("protocol")
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| raw.parse::<Protocol>().ok())
            .unwrap_or(Protocol::Cisco);
        if !object.contains_key("protocol") {
            object.insert("protocol".into(), serde_json::Value::String(Protocol::Cisco.as_str().into()));
        }
        if !object.contains_key("server") {
            let default = match protocol { Protocol::Cisco => DEFAULT_SERVER, Protocol::Atrust => DEFAULT_ATRUST_SERVER };
            object.insert("server".into(), serde_json::Value::String(default.into()));
        }
        let legacy_server = object.get("server").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
        if !object.contains_key("cisco_server") {
            let value = if protocol == Protocol::Cisco { legacy_server.as_str() } else { DEFAULT_SERVER };
            object.insert("cisco_server".into(), serde_json::Value::String(value.into()));
        }
        if !object.contains_key("atrust_server") {
            let value = if protocol == Protocol::Atrust { legacy_server.as_str() } else { DEFAULT_ATRUST_SERVER };
            object.insert("atrust_server".into(), serde_json::Value::String(value.into()));
        }
        if !object.contains_key("atrust_auth_mode") {
            object.insert("atrust_auth_mode".into(), serde_json::Value::String("browser".into()));
        }
        let Ok(mut cfg) = serde_json::from_value::<Self>(value) else {
            return Ok(Self::default());
        };
        // 显式独立服务器字段是权威值；活动 server 始终投影到 Engine 使用的旧键。
        cfg.server = match cfg.protocol {
            Protocol::Cisco => cfg.cisco_server.clone(),
            Protocol::Atrust => cfg.atrust_server.clone(),
        };
        Ok(cfg)
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
        let mut snapshot = self.clone();
        snapshot.sync_active_server();
        let json = serde_json::to_string_pretty(&snapshot)?;
        write_config_json_atomically(dir, &json)
    }

    /// Keep the legacy Engine-facing `server` and the selected protocol's stored endpoint aligned.
    pub fn sync_active_server(&mut self) {
        match self.protocol {
            Protocol::Cisco => self.cisco_server.clone_from(&self.server),
            Protocol::Atrust => self.atrust_server.clone_from(&self.server),
        }
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
    /// 2026-09-21（T02）：同时把 [`ExvConfig::password_protocol`] 记为写入时的
    /// [`ExvConfig::protocol`]。配置提交流程在所有键应用完毕后才封存密码，因此
    /// “同一批同时改协议与密码”也会得到匹配的协议标记。
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
        self.password_protocol = self.protocol;
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
/// 原子写入任意目标文件（同目录临时文件 + 改名）。失败时删除临时文件，目标保持原状。
pub(crate) fn write_json_atomically_to(
    dir: &Path,
    destination: &Path,
    json: &str,
) -> Result<(), ConfigError> {
    std::fs::create_dir_all(dir)?;
    let (temporary, mut file) = create_temporary_config_file(dir)?;
    let write_result = (|| -> Result<(), ConfigError> {
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}

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

