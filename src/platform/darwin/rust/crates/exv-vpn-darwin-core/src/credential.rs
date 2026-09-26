//! 本次连接的一次性凭据组装。
//!
//! 密码由 Windows 同构配置单独加密保存。调用方将一次性明文交给
//! [`build_connect_envelope`] 后，得到只能交给 Engine 的版本化 memory payload。
//!
//! W1-C（P7）：连接弹窗的一次性凭据经 `ConnectRequest.secret_payload` 以 JSON v1
//! （win32 `CredentialPackage` 同形）进入 Core——[`UiCredentialPackage`] 负责解析与
//! 零化，`kernel_control_service` 负责「UI 优先 / 磁盘回退」双分支与 persist 落盘。

use std::{fmt, net::Ipv4Addr};

use exv_vpn_darwin_ipc::connect_envelope::{ConnectEnvelopeError, DarwinEngineConnectV1};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{DarwinConfigError, DarwinUiConfig};

/// UI `secret_payload` 凭据包格式版本（当前 1）。与 win32
/// `credential.rs::SECRET_PAYLOAD_VERSION` 同值；未知版本 typed 拒绝。
pub const SECRET_PAYLOAD_VERSION: u32 = 1;

/// 从 Windows 同构配置读取一次性密码并组装 Engine payload 的稳定错误。
#[derive(Debug)]
pub enum SavedConnectEnvelopeError {
    Config(DarwinConfigError),
    Envelope(ConnectEnvelopeError),
}

impl std::fmt::Display for SavedConnectEnvelopeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Envelope(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SavedConnectEnvelopeError {}

/// UI 一次性凭据载荷解析失败的稳定错误（形状不符 / 非法 JSON）。
///
/// 不携带任何明文或底层诊断（serde 错误文本可能回显载荷片段），调用方映射为
/// typed 拒绝码即可。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiCredentialPayloadError;

impl fmt::Display for UiCredentialPayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DARWIN_CORE_UI_CREDENTIAL_PAYLOAD_INVALID")
    }
}

impl std::error::Error for UiCredentialPayloadError {}

/// 连接弹窗的一次性凭据包（`ConnectRequest.secret_payload` 的 JSON 载荷）。
///
/// serde JSON 形状与 win32 host `credential.rs::CredentialPackage` 逐字一致：
/// `{"version":1,"username":"...","password":"..."}`。**零化类型**：`Drop` /
/// 显式 [`UiCredentialPackage::zeroize`] 覆写 username+password 已用字节后清空；
/// `Debug` 对 password 恒 `<redacted>`。
#[derive(Serialize, Deserialize)]
pub struct UiCredentialPackage {
    /// 格式版本（必须等于 [`SECRET_PAYLOAD_VERSION`]）。
    pub version: u32,
    /// 登录用户名。
    pub username: String,
    /// 登录密码（明文；一次性，零化后为空）。
    pub password: String,
}

impl UiCredentialPackage {
    /// 构造一次性凭据包（win32 `CredentialPackage::new` 对应；测试与往返断言用）。
    #[must_use]
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            version: SECRET_PAYLOAD_VERSION,
            username: username.into(),
            password: password.into(),
        }
    }

    /// 序列化为 `secret_payload` 的一次性 JSON 字节。
    ///
    /// # Errors
    /// 序列化失败 → [`UiCredentialPayloadError`]。
    pub fn to_bytes(&self) -> Result<Vec<u8>, UiCredentialPayloadError> {
        serde_json::to_vec(self).map_err(|_| UiCredentialPayloadError)
    }

    /// 确定性零化包内明文（`Drop` 亦调用）。
    pub fn zeroize(&mut self) {
        zeroize_string(&mut self.username);
        zeroize_string(&mut self.password);
    }
}

impl fmt::Debug for UiCredentialPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiCredentialPackage")
            .field("version", &self.version)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Drop for UiCredentialPackage {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// 从 `secret_payload` 一次性字节反序列化 UI 凭据包（不校验版本/空值——那是
/// `kernel_control_service::take_ui_credentials` 的 typed 拒绝边界）。
///
/// # Errors
/// 反序列化失败（形状不符 / 非法 JSON）→ [`UiCredentialPayloadError`]；
/// 错误永不回显载荷内容。
pub fn parse_ui_secret_payload(
    bytes: &[u8],
) -> Result<UiCredentialPackage, UiCredentialPayloadError> {
    serde_json::from_slice(bytes).map_err(|_| UiCredentialPayloadError)
}

/// 覆写 String 的已用字节为 0 后清空（win32 `credential.rs::zeroize_string` 同款）。
fn zeroize_string(s: &mut String) {
    // SAFETY: `as_mut_vec` 返回 String 字节的可变视图；本函数持唯一 `&mut`，无其它
    // 引用借用这些字节。
    unsafe {
        s.as_mut_vec().zeroize();
    }
    s.clear();
}

/// 用当前 Windows 同构配置和本次输入的密码创建 Engine payload。
///
/// 配置缺少连接必填项时由 envelope 返回稳定验证错误；密码 ownership 随返回 envelope 转移。
///
/// # Errors
///
/// 保存的 profile 或本次密码不满足 V1 连接字段约束时返回
/// [`ConnectEnvelopeError`]。
pub fn build_connect_envelope(
    config: &DarwinUiConfig,
    password: String,
) -> Result<DarwinEngineConnectV1, ConnectEnvelopeError> {
    DarwinEngineConnectV1::new(
        config.server().to_owned(),
        config.username().to_owned(),
        password,
        normalized_connect_routes(config.routes()),
        u16::try_from(config.mtu()).ok(),
        config.user_agent().to_owned(),
    )
}

/// 把配置路由规范化为 Engine 信封可投递的规范 IPv4 CIDR（保序、按规范化身份去重）。
///
/// 接受合法 IPv4 CIDR（`59.78.199.0/21`）或裸 IPv4（`219.228.60.69`，等价 `/32`），
/// 并按掩码规范化主机位（`59.78.199.0/21 → 59.78.192.0/21`）。非 IPv4、非法或非十进制
/// 前缀、前缀 0 的文本被跳过。
///
/// 事实依据（修正原先「与 Windows `plan_from_config` 一致」的错误注释）：win32
/// `plan_from_config`/`parse_cidr` 既**不要求**规范形，也**不在解析层掩码**；掩码发生在
/// 资源层 `exv-vpn-win32-resource/src/routes.rs` 的 `RouteRow::new`（内部 `masked_network`）。
/// C++ darwin 参考实现 `native_route_config.cpp::normalize_cidr` 同样做掩码而非丢弃。
/// 因此这里必须掩码而不是把主机位非零的网段整体丢弃；又因为
/// `exv-vpn-darwin-ipc::connect_envelope::validate_routes` 会对非规范串与重复项
/// 返回 `InvalidRoutes` 拒绝整个信封，去重必须与规范化同时发生。
fn normalized_connect_routes(routes: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    routes
        .iter()
        .filter_map(|route| {
            let (network, prefix) = split_normalized_route(route)?;
            seen.insert((network, prefix))
                .then(|| format!("{}/{}", Ipv4Addr::from(network), prefix))
        })
        .collect()
}

/// 解析单条路由文本并返回掩码规范化后的网络地址与前缀长度；非法文本返回 `None`。
fn split_normalized_route(value: &str) -> Option<(u32, u8)> {
    let (address_text, prefix) = match value.split_once('/') {
        Some((address_text, prefix_text)) => {
            if prefix_text.contains('/') || !prefix_text.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let prefix = prefix_text.parse::<u8>().ok()?;
            if !(1..=32).contains(&prefix) || prefix.to_string() != prefix_text {
                return None;
            }
            (address_text, prefix)
        }
        // 裸 IPv4 等价 /32。
        None => (value, 32),
    };
    let address = address_text.parse::<Ipv4Addr>().ok()?;
    if address.to_string() != address_text {
        return None;
    }
    let mask = u32::MAX << (u32::BITS - u32::from(prefix));
    Some((u32::from(address) & mask, prefix))
}

/// 从已保存的 Windows 同构配置读取密码，并只在内存中创建本次 Engine payload。
///
/// # Errors
///
/// 缺少、无法解密保存密码或字段不符合 Engine V1 约束时返回稳定错误。
pub fn build_saved_connect_envelope(
    config: &DarwinUiConfig,
) -> Result<DarwinEngineConnectV1, SavedConnectEnvelopeError> {
    let password = config
        .load_saved_password()
        .map_err(SavedConnectEnvelopeError::Config)?;
    build_connect_envelope(config, password).map_err(SavedConnectEnvelopeError::Envelope)
}
