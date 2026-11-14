//! 本次连接的一次性凭据组装。
//!
//! 密码由 Windows 同构配置单独加密保存。调用方将一次性明文交给
//! [`build_connect_envelope`] 后，得到只能交给 Engine 的版本化 memory payload。
//!
//! W1-C（P7）：连接弹窗的一次性凭据经 `ConnectRequest.secret_payload` 以 JSON v1
//! （win32 `CredentialPackage` 同形）进入 Core——[`UiCredentialPackage`] 负责解析与
//! 零化，`kernel_control_service` 负责「UI 优先 / 磁盘回退」双分支与 persist 落盘。
//!
//! M1（darwin aTrust 适配）：`secret_payload` 升级为**版本分派**（镜像 win32
//! secret_payload v1/v2 纪律）——v1=Cisco 凭据包（形状逐字不变）；v2=aTrust 载荷
//! （[`UiAtrustCredentialPackage`]：Common `BrowserSession` + `fresh_authentication`
//! 语境位）。[`build_connect_envelope`] 按 `config.protocol` 分派：Cisco 路径产出
//! V3 wire（内嵌原 V1 并携带当前域列表），aTrust 路径保持信封 V2。

use std::{fmt, net::Ipv4Addr};

use exv_vpn_darwin_ipc::browser_session::BrowserSession;
use exv_vpn_darwin_ipc::connect_envelope::{
    ConnectEnvelopeError, DarwinEngineConnectV1, DarwinEngineConnectV2, DarwinEngineConnectV3,
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::{DarwinConfigError, DarwinUiConfig, Protocol};

/// UI `secret_payload` 凭据包格式版本（当前 1）。与 win32
/// `credential.rs::SECRET_PAYLOAD_VERSION` 同值；未知版本 typed 拒绝。
pub const SECRET_PAYLOAD_VERSION: u32 = 1;

/// UI `secret_payload` aTrust 载荷格式版本（M1 冻结；v2）。与 win32 engine 侧对
/// `secret_payload` v2 的版本纪律同构（win32 v2=BrowserSession 本体，darwin v2 在
/// 其上多出 `protocol` 判别与 `fresh_authentication` 位——设计 §4.2b）。
pub const ATRUST_SECRET_PAYLOAD_VERSION: u32 = 2;

/// 信封 V2 aTrust 载荷的 platform 值——**M0 证据暂定值**（darwin-platform-
/// identity-evidence.md §3/§6：core 显式注入、无默认回填）。终局确认在 M3 每流
/// 授权；M3 出 `130`/`10000001` 类授权拒绝时按证据文档回流序（Mac→Linux）只改
/// 本常量。
pub const ATRUST_PLATFORM_IDENTITY: &str = "Mac";

/// 信封 V2 aTrust 载荷的认证语言（设计 §4.3：`zh-CN`）。
pub const ATRUST_AUTH_LANGUAGE: &str = "zh-CN";

/// 产品资源请求字节（**宿主产品选择，不是协议默认值**；M0 证据 §4 定版：127 字节，
/// sha256 `baaa8256…ca81ed4`，与 win32 `RESOURCE_REQUEST` 恒等——指纹锁定测试见
/// 本模块 tests）。
pub const ATRUST_RESOURCE_REQUEST: &[u8] = b"{\"resourceType\":{\"sdpPolicy\":{},\"appList\":{},\"favoriteAppList\":{},\"featureCenter\":{},\"uemSpace\":{\"params\":{\"action\":\"login\"}}}}";

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
/// M1：本入口升级为**版本分派**（win32 engine `parse_connection_credentials` 同款
/// 纪律——先窥 `version` 再按版本解析载荷）：v1 → Cisco 凭据包；v2 → aTrust 载荷
/// （并在此完成 Common `BrowserSession` 的全量校验——`encode()` 内含 validate：
/// 版本/kind/网关/头部/64KB 上限）。v1 分支对既有形状逐字保持。
///
/// # Errors
/// 反序列化失败（形状不符/非法 JSON）、未知版本、v2 判别字段不符或会话非法 →
/// [`UiCredentialPayloadError`]；错误永不回显载荷内容。
pub fn parse_ui_secret_payload(bytes: &[u8]) -> Result<UiSecretPayload, UiCredentialPayloadError> {
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let header: Header =
        serde_json::from_slice(bytes).map_err(|_| UiCredentialPayloadError)?;
    match header.version {
        SECRET_PAYLOAD_VERSION => {
            let package: UiCredentialPackage =
                serde_json::from_slice(bytes).map_err(|_| UiCredentialPayloadError)?;
            Ok(UiSecretPayload::Cisco(package))
        }
        ATRUST_SECRET_PAYLOAD_VERSION => {
            let package: UiAtrustCredentialPackage =
                serde_json::from_slice(bytes).map_err(|_| UiCredentialPayloadError)?;
            if package.version != ATRUST_SECRET_PAYLOAD_VERSION
                || package.protocol != Protocol::Atrust.as_str()
            {
                return Err(UiCredentialPayloadError);
            }
            // Common 全量校验（同时预演引擎信封所需的 wire 编码路径）。
            package
                .browser_session
                .encode()
                .map_err(|_| UiCredentialPayloadError)?;
            Ok(UiSecretPayload::Atrust(package))
        }
        _ => Err(UiCredentialPayloadError),
    }
}

/// UI→core 一次性凭据载荷的版本分派结果（`take_ui_credentials` 消费）。
///
/// `Debug` 委托内层类型的脱敏实现（秘密恒 redacted）。
pub enum UiSecretPayload {
    /// v1：Cisco 用户名/密码包（形状与 win32 host `CredentialPackage` 逐字一致）。
    Cisco(UiCredentialPackage),
    /// v2：aTrust 浏览器会话 + 新认证语境位（M1 冻结契约）。
    Atrust(UiAtrustCredentialPackage),
}

impl fmt::Debug for UiSecretPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cisco(package) => package.fmt(f),
            Self::Atrust(package) => package.fmt(f),
        }
    }
}

/// UI `secret_payload` 的 aTrust 凭据包（v2 载荷；M1 冻结形状，M5 内嵌认证窗为
/// 生产方）。
///
/// JSON 形状：`{"version":2,"protocol":"atrust","browser_session":<Common
/// BrowserSession 对象>,"fresh_authentication":bool}`。`browser_session` 为 Common
/// 类型（经 darwin-ipc re-export；其 `cookie_header` 是 `ZeroizeOnDrop` 秘密——
/// 包 drop 即清零 Cookie，本包无用户名/密码字段）。
///
/// **与缓存写入时序的规格（M5 实现，本注释冻结契约）**：core 采到会话后**先写
/// 会话缓存再发连接**，连接失败**不回滚**缓存——恢复候选必须经网关复核（设计
/// §4.2/§4.2b，Windows 踩坑 10 同款）；`fresh_authentication` 由采集路径置位（新
/// 浏览器登录=true、缓存复用=false），非身份非秘密。
#[derive(Serialize, Deserialize)]
pub struct UiAtrustCredentialPackage {
    /// 格式版本（必须等于 [`ATRUST_SECRET_PAYLOAD_VERSION`]）。
    pub version: u32,
    /// 协议判别（必须为 `"atrust"`——显式判别防跨协议误投，镜像信封 V2 的判别位）。
    pub protocol: String,
    /// Common `BrowserSession`（M5 采集产物；校验在 [`parse_ui_secret_payload`]）。
    pub browser_session: BrowserSession,
    /// 新认证语境位（设计 §7 keep/采纳：驱动引擎 `DARWIN_ENGINE_ADOPT_NEW_AUTH`
    /// 分叉；core 编排 Stop→重建）。
    pub fresh_authentication: bool,
    /// 实际本次认证产生的账户；旧候选没有该事实时保持空。
    #[serde(default)]
    pub authenticated_username: String,
}

impl UiAtrustCredentialPackage {
    /// 构造一次性 aTrust 凭据包（测试与往返断言用；生产方是 M5 采集路径）。
    #[must_use]
    pub fn new(browser_session: BrowserSession, fresh_authentication: bool) -> Self {
        Self {
            version: ATRUST_SECRET_PAYLOAD_VERSION,
            protocol: Protocol::Atrust.as_str().to_owned(),
            browser_session,
            fresh_authentication,
            authenticated_username: String::new(),
        }
    }

    #[must_use]
    pub fn with_authenticated_username(mut self, username: impl Into<String>) -> Self {
        self.authenticated_username = username.into();
        self
    }

    /// 序列化为 `secret_payload` 的一次性 JSON 字节（B5 收口，M1 移交兑现：明文
    /// 副本由 [`Zeroizing`] 持有，drop 即清零——与信封编码的容器纪律对齐；会话
    /// 本体的 Cookie 由 `BrowserSession` 的 `ZeroizeOnDrop` 自行清零）。
    ///
    /// # Errors
    /// 序列化失败 → [`UiCredentialPayloadError`]。
    pub fn to_bytes(&self) -> Result<Zeroizing<Vec<u8>>, UiCredentialPayloadError> {
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| UiCredentialPayloadError)
    }
}

impl fmt::Debug for UiAtrustCredentialPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiAtrustCredentialPackage")
            .field("version", &self.version)
            .field("protocol", &self.protocol)
            .field("browser_session", &"<redacted>")
            .field("fresh_authentication", &self.fresh_authentication)
            .finish()
    }
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

/// 本次连接的一次性材料（`take_ui_credentials` 之后、`build_connect_envelope`
/// 之前的形态）。协议必须与配置的 `protocol` 一致——不一致是 typed 拒绝（跨协议
/// 误投绝不是静默回退）。
pub enum UiConnectMaterials {
    /// Cisco 一次性密码（用户名已在服务层消费——persist/临时副本）。
    Cisco { password: String },
    /// aTrust 浏览器会话（Common 类型）+ 新认证语境位。
    Atrust {
        browser_session: BrowserSession,
        fresh_authentication: bool,
    },
}

/// [`build_connect_envelope`] 的协议分派产物。
///
/// Cisco 使用 V3 携带当前域列表，内嵌旧 V1 字节；aTrust 保持原 V2。
pub enum DarwinConnectEnvelope {
    /// Cisco 路径（V3 wire，原 V1 内容保持）。
    Cisco(DarwinEngineConnectV3),
    /// aTrust 路径（V2 wire）。
    Atrust(DarwinEngineConnectV2),
}

impl DarwinConnectEnvelope {
    /// 消费并编码为 `ApplyTunnelRequest.secret_payload` 字节（Zeroizing；Cisco 分支
    /// 内嵌既有 V1 编码）。
    #[must_use]
    pub fn encode(self) -> Zeroizing<Vec<u8>> {
        match self {
            Self::Cisco(envelope) => envelope.encode(),
            Self::Atrust(envelope) => envelope.encode(),
        }
    }
}

impl fmt::Debug for DarwinConnectEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cisco(envelope) => envelope.fmt(f),
            Self::Atrust(envelope) => envelope.fmt(f),
        }
    }
}

/// 用当前 Windows 同构配置和本次输入的一次性材料创建 Engine payload（M1 起按
/// `config.protocol` 分派，见 [`DarwinConnectEnvelope`] 裁决）。
///
/// 配置缺少连接必填项时由 envelope 返回稳定验证错误；秘密 ownership 随返回 envelope
/// 转移。aTrust 分支的注入值：网关=配置 `atrust_server`（宿主补全 `https://` 后由
/// Common `GatewayOrigin::parse` 规范化）、platform=`ATRUST_PLATFORM_IDENTITY`（M0
/// 暂定值）、language=`ATRUST_AUTH_LANGUAGE`、资源字节=`ATRUST_RESOURCE_REQUEST`。
///
/// # Errors
///
/// 协议与材料不匹配 → [`ConnectEnvelopeError::InvalidProtocol`]；保存的 profile 或
/// 本次材料不满足对应信封的字段约束时返回其余 [`ConnectEnvelopeError`]。
pub fn build_connect_envelope(
    config: &DarwinUiConfig,
    materials: UiConnectMaterials,
) -> Result<DarwinConnectEnvelope, ConnectEnvelopeError> {
    match (config.protocol(), materials) {
        (Protocol::Cisco, UiConnectMaterials::Cisco { password }) => {
            DarwinEngineConnectV1::new(
                config.server().to_owned(),
                config.username().to_owned(),
                password,
                normalized_connect_routes(config.routes()),
                u16::try_from(config.mtu()).ok(),
                config.user_agent().to_owned(),
            )
            .and_then(|cisco| DarwinEngineConnectV3::new(cisco, config.campus_dns_domains().to_vec()))
            .map(DarwinConnectEnvelope::Cisco)
        }
        (
            Protocol::Atrust,
            UiConnectMaterials::Atrust {
                browser_session,
                fresh_authentication,
            },
        ) => {
            // 宿主补全端点（Common origin.rs 模块注释：补 https:// 是宿主职责），
            // 旧空值由配置访问器采用校园预设；非空网关的规范化与校验
            // 由信封经 GatewayOrigin 完成。
            let endpoint = format!("https://{}", config.atrust_server());
            let session_wire = browser_session
                .encode()
                .map_err(|_| ConnectEnvelopeError::InvalidBrowserSession)?;
            DarwinEngineConnectV2::new_atrust(
                endpoint,
                session_wire.to_vec(),
                ATRUST_PLATFORM_IDENTITY.to_owned(),
                ATRUST_AUTH_LANGUAGE.to_owned(),
                ATRUST_RESOURCE_REQUEST.to_vec(),
                fresh_authentication,
            )
            .and_then(|envelope| envelope.with_resolver_domains(config.campus_dns_domains().to_vec()))
            .and_then(|envelope| envelope.with_atrust_dns_enabled(config.atrust_dns_enabled()))
            .map(DarwinConnectEnvelope::Atrust)
        }
        (Protocol::Cisco, UiConnectMaterials::Atrust { .. })
        | (Protocol::Atrust, UiConnectMaterials::Cisco { .. }) => {
            Err(ConnectEnvelopeError::InvalidProtocol)
        }
    }
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
pub(crate) fn normalized_connect_routes(routes: &[String]) -> Vec<String> {
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
/// M1：本入口保持 **Cisco 专用**语义（磁盘回退分支只在 Cisco 协议被服务层调用；
/// aTrust 配置在此得到 `InvalidProtocol` typed 拒绝——aTrust 的缓存会话回退是 M5
/// 交付，M1 由服务层以凭据缺失拒绝）。
///
/// # Errors
///
/// 缺少、无法解密保存密码或字段不符合 Engine 信封约束时返回稳定错误。
pub fn build_saved_connect_envelope(
    config: &DarwinUiConfig,
) -> Result<DarwinEngineConnectV3, SavedConnectEnvelopeError> {
    let password = config
        .load_saved_password()
        .map_err(SavedConnectEnvelopeError::Config)?;
    match build_connect_envelope(config, UiConnectMaterials::Cisco { password }) {
        Ok(DarwinConnectEnvelope::Cisco(envelope)) => Ok(envelope),
        // 分派结构保证不产生 aTrust 信封（协议×材料不匹配先被拒）。
        Ok(DarwinConnectEnvelope::Atrust(_)) => Err(SavedConnectEnvelopeError::Envelope(
            ConnectEnvelopeError::InvalidProtocol,
        )),
        Err(error) => Err(SavedConnectEnvelopeError::Envelope(error)),
    }
}
