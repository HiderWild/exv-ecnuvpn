//! Core 与 Engine 间单次连接意图的版本化内存 envelope。
//!
//! 该类型只编码到既有 `HelperControl` 的 `secret_payload`；它不创建 transport、socket、
//! 网络连接或任何平台资源。Core 构造后将字节交给已认证 Engine，Engine 必须再用
//! [`DarwinEngineConnectV1::decode`] / [`DarwinEngineConnectV2::decode`]（按 wire 头
//! MAGIC+VERSION 分派，M2 接线）验证并消费一次。
//!
//! M1（darwin aTrust 适配）：新增 [`DarwinEngineConnectV2`]（VERSION=2，
//! protocol 判别 + Cisco/aTrust 两载荷）；**Cisco 路径的 V1 wire 字节不变**
//! （旧字节锁定测试继续守卫）；当前 Cisco 分域 DNS 使用独立 V3 封装旧 V1。

use std::{fmt, net::Ipv4Addr};

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: [u8; 4] = *b"EXVC";
const VERSION: u16 = 1;
const DIGEST_LEN: usize = 32;
const MAX_SERVER_LEN: usize = 253;
const MAX_USERNAME_LEN: usize = 256;
const MAX_PASSWORD_LEN: usize = 4_096;
const MAX_USER_AGENT_LEN: usize = 256;
const MAX_ROUTE_COUNT: usize = 256;
const HEADER_LEN: usize = MAGIC.len() + 2;

/// `DarwinEngineConnectV1` 的稳定错误分类；永不包含提交的用户名或密码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectEnvelopeError {
    InvalidEncoding,
    UnsupportedVersion,
    InvalidServer,
    InvalidUsername,
    InvalidPassword,
    InvalidRoutes,
    InvalidMtu,
    InvalidUserAgent,
    DigestMismatch,
    /// M1（V2）：wire 协议判别位不是已知的 Cisco/aTrust 取值。
    InvalidProtocol,
    /// M1（V2 aTrust）：网关不是可被 Common `GatewayOrigin::parse` 规范化的 HTTPS 端点。
    InvalidGateway,
    /// M1（V2 `aTrust）：browser_session` wire 不是合法 Common `BrowserSession`、超上限
    /// 或其网关与载荷网关不同源。
    InvalidBrowserSession,
    /// M1（V2 aTrust）：platform 字段为空、非 ASCII、含控制字符或超上限。
    InvalidPlatform,
    /// M1（V2 aTrust）：language 字段为空、非 ASCII、含控制字符或超上限。
    InvalidLanguage,
    /// M1（V2 `aTrust）：resource_request` 为空或超上限。
    InvalidResourceRequest,
    /// 校园 DNS 域清单不满足规范域名或 wire 长度边界。
    InvalidResolverDomains,
}

impl ConnectEnvelopeError {
    #[must_use]
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::InvalidEncoding => "DARWIN_ENGINE_CONNECT_INVALID_ENCODING",
            Self::UnsupportedVersion => "DARWIN_ENGINE_CONNECT_UNSUPPORTED_VERSION",
            Self::InvalidServer => "DARWIN_ENGINE_CONNECT_INVALID_SERVER",
            Self::InvalidUsername => "DARWIN_ENGINE_CONNECT_INVALID_USERNAME",
            Self::InvalidPassword => "DARWIN_ENGINE_CONNECT_INVALID_PASSWORD",
            Self::InvalidRoutes => "DARWIN_ENGINE_CONNECT_INVALID_ROUTES",
            Self::InvalidMtu => "DARWIN_ENGINE_CONNECT_INVALID_MTU",
            Self::InvalidUserAgent => "DARWIN_ENGINE_CONNECT_INVALID_USER_AGENT",
            Self::DigestMismatch => "DARWIN_ENGINE_CONNECT_DIGEST_MISMATCH",
            Self::InvalidProtocol => "DARWIN_ENGINE_CONNECT_INVALID_PROTOCOL",
            Self::InvalidGateway => "DARWIN_ENGINE_CONNECT_INVALID_GATEWAY",
            Self::InvalidBrowserSession => "DARWIN_ENGINE_CONNECT_INVALID_BROWSER_SESSION",
            Self::InvalidPlatform => "DARWIN_ENGINE_CONNECT_INVALID_PLATFORM",
            Self::InvalidLanguage => "DARWIN_ENGINE_CONNECT_INVALID_LANGUAGE",
            Self::InvalidResourceRequest => "DARWIN_ENGINE_CONNECT_INVALID_RESOURCE_REQUEST",
            Self::InvalidResolverDomains => "DARWIN_ENGINE_CONNECT_INVALID_RESOLVER_DOMAINS",
        }
    }
}

impl fmt::Display for ConnectEnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.stable_code())
    }
}

impl std::error::Error for ConnectEnvelopeError {}

/// Core→Engine 的一次性连接意图。
///
/// username/password 只在此 owner 存活期间保存；`Drop` 会清零它们。`profile_digest`
/// 只由非密码的规范化 profile 产生，用作 request 关联信息，不是授权材料。
pub struct DarwinEngineConnectV1 {
    server: String,
    username: String,
    password: String,
    campus_routes: Vec<String>,
    mtu_override: Option<u16>,
    user_agent: String,
    profile_digest: [u8; DIGEST_LEN],
}

impl DarwinEngineConnectV1 {
    /// 验证并创建一次性连接意图。
    ///
    /// # Errors
    ///
    /// 任一必填字段、路由或 MTU 不符合 V1 边界时返回对应的 [`ConnectEnvelopeError`]。
    pub fn new(
        server: String,
        username: String,
        password: String,
        campus_routes: Vec<String>,
        mtu_override: Option<u16>,
        user_agent: String,
    ) -> Result<Self, ConnectEnvelopeError> {
        validate_server(&server)?;
        validate_username(&username)?;
        validate_password(&password)?;
        validate_routes(&campus_routes)?;
        validate_mtu(mtu_override)?;
        validate_user_agent(&user_agent)?;
        let profile_digest = digest_profile(
            &server,
            &username,
            &campus_routes,
            mtu_override,
            &user_agent,
        );
        Ok(Self {
            server,
            username,
            password,
            campus_routes,
            mtu_override,
            user_agent,
            profile_digest,
        })
    }

    /// 解析一个完整且无尾随字节的 wire payload。输入 buffer 在所有返回路径清零。
    ///
    /// # Errors
    ///
    /// header、长度、UTF-8、字段边界或 canonical digest 不符合 V1 时返回
    /// [`ConnectEnvelopeError`]。
    pub fn decode(mut payload: Vec<u8>) -> Result<Self, ConnectEnvelopeError> {
        let result = Self::decode_inner(&payload);
        payload.zeroize();
        result
    }

    fn decode_inner(payload: &[u8]) -> Result<Self, ConnectEnvelopeError> {
        let mut cursor = Cursor::new(payload);
        if cursor.take_exact(MAGIC.len())? != MAGIC {
            return Err(ConnectEnvelopeError::InvalidEncoding);
        }
        if cursor.read_u16()? != VERSION {
            return Err(ConnectEnvelopeError::UnsupportedVersion);
        }
        Self::decode_body(&mut cursor)
    }

    /// V1 wire 的 body 段解析（[`Self::encode_body`] 的对偶；含 digest 复核）。
    fn decode_body(cursor: &mut Cursor<'_>) -> Result<Self, ConnectEnvelopeError> {
        let server = cursor.read_string()?;
        let username = cursor.read_string()?;
        let password = cursor.read_string()?;
        let route_count = usize::from(cursor.read_u16()?);
        if route_count > MAX_ROUTE_COUNT {
            return Err(ConnectEnvelopeError::InvalidRoutes);
        }
        let mut campus_routes = Vec::with_capacity(route_count);
        for _ in 0..route_count {
            campus_routes.push(cursor.read_string()?);
        }
        let mtu_override = match cursor.read_u16()? {
            0 => None,
            mtu => Some(mtu),
        };
        let user_agent = cursor.read_string()?;
        let digest = cursor.take_exact(DIGEST_LEN)?;
        if !cursor.is_finished() {
            return Err(ConnectEnvelopeError::InvalidEncoding);
        }
        let mut profile_digest = [0_u8; DIGEST_LEN];
        profile_digest.copy_from_slice(digest);

        let value = Self::new(
            server,
            username,
            password,
            campus_routes,
            mtu_override,
            user_agent,
        )?;
        if value.profile_digest != profile_digest {
            return Err(ConnectEnvelopeError::DigestMismatch);
        }
        Ok(value)
    }

    /// 消费并编码为 `secret_payload`。返回值的 buffer 在 drop 时清零。
    ///
    /// # Panics
    ///
    /// 仅当本模块内部绕过已验证构造路径，令某个字段长度或 route count 超出 `u16` 时 panic；
    /// 公共构造器和 decoder 都会先拒绝这种输入。
    #[must_use]
    pub fn encode(self) -> Zeroizing<Vec<u8>> {
        let mut bytes = Vec::with_capacity(
            HEADER_LEN
                + self.server.len()
                + self.username.len()
                + self.password.len()
                + self.user_agent.len()
                + self.campus_routes.iter().map(String::len).sum::<usize>()
                + (self.campus_routes.len() + 4) * 2
                + DIGEST_LEN,
        );
        bytes.extend_from_slice(&MAGIC);
        push_u16(&mut bytes, VERSION);
        self.encode_body(&mut bytes);
        Zeroizing::new(bytes)
    }

    /// V1 wire 的 body 段（header 之后的全部字段+digest）。M1 拆出：V2 信封的 Cisco
    /// 载荷**内嵌本段字节**（见 [`DarwinEngineConnectV2`]），保证 Cisco 字段语义与
    /// V1 逐字一致是构造性的，不靠平行字段重写。字节布局不得改变（有字节锁定测试）。
    pub(crate) fn encode_body(&self, bytes: &mut Vec<u8>) {
        push_string(bytes, &self.server);
        push_string(bytes, &self.username);
        push_string(bytes, &self.password);
        push_u16(
            bytes,
            u16::try_from(self.campus_routes.len()).expect("validated route count fits u16"),
        );
        for route in &self.campus_routes {
            push_string(bytes, route);
        }
        push_u16(bytes, self.mtu_override.unwrap_or_default());
        push_string(bytes, &self.user_agent);
        bytes.extend_from_slice(&self.profile_digest);
    }

    #[must_use]
    pub fn profile_digest(&self) -> &[u8; DIGEST_LEN] {
        &self.profile_digest
    }

    #[must_use]
    /// T2 采纳判定：只读用户名（不消耗 envelope、不触发清零——采纳匹配是非
    /// 秘密比较，密码永不参与）。
    pub fn username(&self) -> &str {
        &self.username
    }

    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    #[must_use]
    pub fn routes(&self) -> &[String] {
        &self.campus_routes
    }

    #[must_use]
    pub const fn mtu_override(&self) -> Option<u16> {
        self.mtu_override
    }

    #[must_use]
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// 消费 envelope，取出一次性凭据 `(username, password)`。
    ///
    /// 内部副本被 `std::mem::take` 取空，`Drop` 时照常清零；调用方在登录
    /// 返回后必须对返回值清零，不得写入日志、错误或磁盘。
    #[must_use]
    pub fn take_credentials(mut self) -> (String, String) {
        (
            std::mem::take(&mut self.username),
            std::mem::take(&mut self.password),
        )
    }
}

impl Drop for DarwinEngineConnectV1 {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

impl fmt::Debug for DarwinEngineConnectV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DarwinEngineConnectV1")
            .field("server", &self.server)
            .field("username", &"[REDACTED]")
            .field("password", &"[REDACTED]")
            .field("campus_routes", &self.campus_routes)
            .field("mtu_override", &self.mtu_override)
            .field("user_agent", &self.user_agent)
            .field("profile_digest", &"[REDACTED]")
            .finish()
    }
}

/// Cisco 分域 DNS 的宿主 V3；旧 V1 作为完整嵌套 wire 保持原字节和解码边界。
/// 域列表与非密码 profile 共同入摘要，不创建新的持久化记录。
pub struct DarwinEngineConnectV3 {
    cisco: DarwinEngineConnectV1,
    resolver_domains: Vec<String>,
    profile_digest: [u8; DIGEST_LEN],
}

impl DarwinEngineConnectV3 {
    /// 冻结当前配置的域列表；空列表明确关闭分域 DNS。
    ///
    /// # Errors
    /// 域列表不规范、重复或超出 wire 长度时返回稳定错误。
    pub fn new(cisco: DarwinEngineConnectV1, domains: Vec<String>) -> Result<Self, ConnectEnvelopeError> {
        validate_resolver_domains(&domains)?;
        let mut digest = Sha256::new();
        digest.update(b"EXV-DARWIN-CISCO-PROFILE-V3\0");
        digest.update(cisco.profile_digest());
        digest.update(u16::try_from(domains.len()).expect("validated domain count").to_be_bytes());
        for domain in &domains { update_digest_string(&mut digest, domain); }
        Ok(Self { cisco, resolver_domains: domains, profile_digest: digest.finalize().into() })
    }

    /// 旧客户端不写 resolver；不能给旧 V1/V2-Cisco 隐式补默认域。
    #[must_use]
    pub fn legacy(cisco: DarwinEngineConnectV1) -> Self {
        Self::new(cisco, Vec::new()).expect("empty domains are valid")
    }

    /// 解析 V3 并消费输入；嵌套 V1 继续使用原 decoder。
    ///
    /// # Errors
    /// 拒绝版本、编码、域列表或摘要错误，不返回输入正文。
    pub fn decode(mut payload: Vec<u8>) -> Result<Self, ConnectEnvelopeError> {
        let result = (|| {
            let mut cursor = Cursor::new(&payload);
            if cursor.take_exact(MAGIC.len())? != MAGIC { return Err(ConnectEnvelopeError::InvalidEncoding); }
            if cursor.read_u16()? != 3 { return Err(ConnectEnvelopeError::UnsupportedVersion); }
            let cisco = DarwinEngineConnectV1::decode(cursor.read_vec()?)?;
            let count = usize::from(cursor.read_u16()?);
            let mut domains = Vec::with_capacity(count);
            for _ in 0..count { domains.push(cursor.read_string()?); }
            let digest = cursor.take_exact(DIGEST_LEN)?;
            if !cursor.is_finished() { return Err(ConnectEnvelopeError::InvalidEncoding); }
            let value = Self::new(cisco, domains)?;
            if value.profile_digest.as_slice() != digest { return Err(ConnectEnvelopeError::DigestMismatch); }
            Ok(value)
        })();
        payload.zeroize();
        result
    }

    /// 消费为 V3 wire；外层和嵌套 V1 临时字节均在 drop 时清零。
    #[must_use]
    pub fn encode(self) -> Zeroizing<Vec<u8>> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        push_u16(&mut bytes, 3);
        let cisco = self.cisco.encode();
        push_u32(&mut bytes, u32::try_from(cisco.len()).expect("bounded V1 wire"));
        bytes.extend_from_slice(&cisco);
        push_u16(&mut bytes, u16::try_from(self.resolver_domains.len()).expect("validated domain count"));
        for domain in &self.resolver_domains { push_string(&mut bytes, domain); }
        bytes.extend_from_slice(&self.profile_digest);
        Zeroizing::new(bytes)
    }

    #[must_use]
    pub fn resolver_domains(&self) -> &[String] { &self.resolver_domains }
    #[must_use]
    pub fn profile_digest(&self) -> &[u8; DIGEST_LEN] { &self.profile_digest }
    #[must_use]
    pub fn server(&self) -> &str { self.cisco.server() }
    #[must_use]
    pub fn username(&self) -> &str { self.cisco.username() }
    #[must_use]
    pub fn routes(&self) -> &[String] { self.cisco.routes() }
    #[must_use]
    pub fn user_agent(&self) -> &str { self.cisco.user_agent() }
    #[must_use]
    pub fn mtu_override(&self) -> Option<u16> { self.cisco.mtu_override() }
    #[must_use]
    pub fn take_credentials(self) -> (String, String) { self.cisco.take_credentials() }
}

impl fmt::Debug for DarwinEngineConnectV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("DarwinEngineConnectV3")
            .field("cisco", &self.cisco)
            .field("resolver_domains", &self.resolver_domains)
            .finish_non_exhaustive()
    }
}

fn validate_resolver_domains(domains: &[String]) -> Result<(), ConnectEnvelopeError> {
    if domains.len() > usize::from(u16::MAX) || domains.iter().any(|domain| {
        domain != &domain.to_ascii_lowercase() || validate_server(domain).is_err()
            || domain.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
    }) { return Err(ConnectEnvelopeError::InvalidResolverDomains); }
    let mut seen = std::collections::HashSet::new();
    if domains.iter().any(|domain| !seen.insert(domain)) { return Err(ConnectEnvelopeError::InvalidResolverDomains); }
    Ok(())
}

// ---------------------------------------------------------------------------
// M1（darwin aTrust 适配）：信封 V2——protocol 判别 + Cisco/aTrust 两载荷。
//
// **wire 裁决（设计 §4.3 + M1 任务裁定）**：
// - V2 沿用 V1 的 MAGIC `EXVC`，VERSION=2——老引擎（只会 `DarwinEngineConnectV1::
//   decode`，例如 dev core × 已装 4.0.0 引擎的混合部署）对 V2 wire 得到 typed
//   `UnsupportedVersion` 拒绝而非误解析（镜像 win32 `secret_payload` v1/v2 纪律；
//   锁定测试 `v1_decode_typed_rejects_a_v2_version_header`）。
// - **Cisco 路径字节不变**：生产分派（darwin-core `build_connect_envelope`）对
//   Cisco 当时产出**纯 V1 wire**；当前分域 DNS 使用独立 V3。V2::Cisco 载荷为结构完备
//   而设（设计 §4.3 的判别枚举要求两载荷都在；后续 Cisco 载荷 V2 化共享
//   `fresh_authentication` 位是已登记的 B6 后续项），其 body **内嵌 V1 编码字节**
//   （`DarwinEngineConnectV1::encode_body`），Cisco 字段语义与 V1 逐字一致是
//   构造性的，不靠平行字段重写。
// - 引擎侧解码入口按 MAGIC+VERSION 分派两版（M2 接线）；本 crate 不提供统一
//   入口函数，避免 M1 引入无消费者的死代码。
// ---------------------------------------------------------------------------

const VERSION_V2: u16 = 2;
// aTrust DNS 开关的显式连接信封；旧 V2 解码为关闭，Cisco V1/V3 保持原字节。
const VERSION_ATRUST_DNS: u16 = 4;
/// V2 wire 的协议判别位取值（u8）。
const PROTOCOL_TAG_CISCO: u8 = 1;
const PROTOCOL_TAG_ATRUST: u8 = 2;
/// 网关上限：canonical 端点 `https://{host≤253}:{port}` 的安全上界。
const MAX_GATEWAY_LEN: usize = 320;
/// platform 上限：M0 证据值 `Mac`（3 字节）与候选回流集（Linux/Windows ≤7）的上界。
const MAX_PLATFORM_LEN: usize = 32;
/// language 上限：`zh-CN` 一类 BCP-47 短标签的上界。
const MAX_LANGUAGE_LEN: usize = 16;
/// `resource_request` 上限：产品定版字节（M0：127 字节）的产品面安全上界。
const MAX_RESOURCE_REQUEST_LEN: usize = 4_096;
/// `browser_session` wire 上限——**Common `BrowserSession` `MAX_PAYLOAD（64KB）同值`
/// 重声明**（设计 §4.3：V1 的 `MAX_PASSWORD_LEN=4096` 不适用；边界测试钉死）。
/// 65536 恰为 64*1024，放不进 u16，故本字段用 u32 长度前缀。
const MAX_BROWSER_SESSION_LEN: usize = 64 * 1_024;

/// V2 的 aTrust 载荷（设计 §4.3/§7）。
pub struct DarwinAtrustConnect {
    /// Common `GatewayOrigin::parse` 规范化后的端点文本（`https://host:port`）。
    gateway: String,
    /// Common `BrowserSession` v2 wire（零化容器持有；仅供引擎消费，不进日志/Debug）。
    browser_session: Zeroizing<Vec<u8>>,
    /// 平台身份值——**显式注入、无默认**（M0 证据暂定值 `Mac` 的唯一消费点之一；
    /// 终局确认在 M3 每流授权）。
    platform: String,
    /// 认证语言（`zh-CN`）。
    language: String,
    /// 产品资源请求字节（非秘密；宿主产品选择，M0 定版 127 字节）。
    resource_request: Vec<u8>,
    /// 新认证语境位（设计 §7 keep/采纳：core 置位——新浏览器登录=true、缓存复用=
    /// false；非身份非秘密，驱动引擎 `DARWIN_ENGINE_ADOPT_NEW_AUTH` 采纳分叉）。
    fresh_authentication: bool,
    /// Core 当前配置投影；空列表表示不安装 resolver。
    resolver_domains: Vec<String>,
    atrust_dns_enabled: bool,
    profile_digest: [u8; DIGEST_LEN],
}

impl DarwinAtrustConnect {
    #[must_use]
    pub fn gateway(&self) -> &str {
        &self.gateway
    }

    /// Common `BrowserSession` v2 wire 字节（秘密——绝不入日志/trace/Debug）。
    #[must_use]
    pub fn browser_session_wire(&self) -> &[u8] {
        &self.browser_session
    }

    #[must_use]
    pub fn platform(&self) -> &str {
        &self.platform
    }

    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }

    #[must_use]
    pub fn resource_request(&self) -> &[u8] {
        &self.resource_request
    }

    #[must_use]
    pub const fn fresh_authentication(&self) -> bool {
        self.fresh_authentication
    }

    #[must_use]
    pub fn resolver_domains(&self) -> &[String] {
        &self.resolver_domains
    }

    #[must_use]
    pub const fn atrust_dns_enabled(&self) -> bool { self.atrust_dns_enabled }

    /// 非秘密 profile `摘要（gateway/platform/language/resource_request/fresh/resolver_domains` 的
    /// SHA-256；不含 `browser_session），仅作请求关联`。
    #[must_use]
    pub const fn profile_digest(&self) -> &[u8; DIGEST_LEN] {
        &self.profile_digest
    }
}

impl fmt::Debug for DarwinAtrustConnect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DarwinAtrustConnect")
            .field("gateway", &self.gateway)
            .field("browser_session", &"[REDACTED]")
            .field("platform", &self.platform)
            .field("language", &self.language)
            .field("resource_request_len", &self.resource_request.len())
            .field("fresh_authentication", &self.fresh_authentication)
            .field("profile_digest", &"[REDACTED]")
            .finish()
    }
}

/// V2 信封的协议判别载荷。
pub enum EngineConnectPayload {
    /// Cisco——**内嵌完整 V1 实例**：字段语义/校验/零化/digest 与 V1 逐字一致
    /// （构造性保证，见上文 wire 裁决）。无 `fresh_authentication` 位（Cisco 采纳
    /// 语义维持 T2 现状；B6 登记的「Cisco 载荷 V2 化共享 fresh 位」是后续项）。
    Cisco(DarwinEngineConnectV1),
    /// aTrust（[`DarwinAtrustConnect`]）。
    Atrust(DarwinAtrustConnect),
}

/// Core→Engine 的一次性连接意图（wire VERSION=2；aTrust 路径专用）。
///
/// 秘密纪律与 V1 同源：`Drop` 经内嵌类型清零（Cisco 的 username/password 由
/// [`DarwinEngineConnectV1`] 的 Drop 清零；aTrust 的 `browser_session` 由 `Zeroizing`
/// 清零）；decode 输入 buffer 全路径清零；`Debug` 对一切秘密恒 `[REDACTED]`。
pub struct DarwinEngineConnectV2 {
    payload: EngineConnectPayload,
}

impl DarwinEngineConnectV2 {
    /// 以既有 V1 实例构造 `V2::Cisco（结构完备/边界测试用；生产` Cisco 路径产出纯
    /// V1 wire，不经本类型）。
    #[must_use]
    pub fn new_cisco(envelope: DarwinEngineConnectV1) -> Self {
        Self {
            payload: EngineConnectPayload::Cisco(envelope),
        }
    }

    /// 验证并创建 V2 aTrust 载荷。`gateway` 须为可被 Common `GatewayOrigin::parse`
    /// 规范化的 HTTPS 端点（宿主负责把裸主机名补全为端点——补 `https://`、去尾
    /// 斜杠，见 Common `origin.rs` 模块注释）；存储值为规范化端点文本。
    /// `browser_session` 须为合法 Common `BrowserSession` v2 wire 且其网关与
    /// `gateway` 同源（同一规范化 host:port）——为别的网关采集的会话不能在本网关
    /// 认证，构造期即拒绝。任一校验失败时传入的 `browser_session` 字节先清零再返回。
    /// 兼容构造使用 ecnu.edu.cn；生产 Core 随后用 [`Self::with_resolver_domains`]
    /// 显式注入当前列表（包括用户清空后的空列表）。
    ///
    /// # Errors
    ///
    /// 任一字段不符 V2 aTrust 边界（含 `browser_session` 非法/超 64KB/网关不同源）
    /// 时返回对应的 [`ConnectEnvelopeError`]；错误不携带任何输入材料。
    pub fn new_atrust(
        gateway: String,
        mut browser_session: Vec<u8>,
        platform: String,
        language: String,
        resource_request: Vec<u8>,
        fresh_authentication: bool,
    ) -> Result<Self, ConnectEnvelopeError> {
        match Self::validate_atrust(
            &gateway,
            &browser_session,
            platform,
            language,
            resource_request,
            fresh_authentication,
        ) {
            Ok((gateway, platform, language, resource_request, digest)) => Ok(Self {
                payload: EngineConnectPayload::Atrust(DarwinAtrustConnect {
                    gateway,
                    browser_session: Zeroizing::new(browser_session),
                    platform,
                    language,
                    resource_request,
                    fresh_authentication,
                    resolver_domains: vec!["ecnu.edu.cn".to_owned()],
                    atrust_dns_enabled: false,
                    profile_digest: digest_atrust_dns_profile(digest, false),
                }),
            }),
            Err(error) => {
                browser_session.zeroize();
                Err(error)
            }
        }
    }

    /// 显式携带 Core 已解析的校园域列表，不在 Engine 重新解析配置路径。
    /// 空列表保持为空；域列表参与 profile 摘要，避免配置变化后静默采纳旧 resolver。
    ///
    /// # Errors
    /// 域名非规范小写 ASCII DNS 名称、重复或超出 wire 长度时返回稳定错误。
    pub fn with_resolver_domains(mut self, domains: Vec<String>) -> Result<Self, ConnectEnvelopeError> {
        validate_resolver_domains(&domains)?;
        let EngineConnectPayload::Atrust(atrust) = &mut self.payload else {
            return Err(ConnectEnvelopeError::InvalidProtocol);
        };
        atrust.profile_digest = digest_atrust_dns_profile(digest_atrust_profile(
            &atrust.gateway,
            &atrust.platform,
            &atrust.language,
            &atrust.resource_request,
            atrust.fresh_authentication,
            &domains,
        ), atrust.atrust_dns_enabled);
        atrust.resolver_domains = domains;
        Ok(self)
    }

    /// 显式传递本次连接的 aTrust DNS 开关，域列表仍完整保存给代理消费者。
    /// # Errors
    /// Cisco 载荷返回协议不匹配；Cisco DNS 沿既有行为。
    pub fn with_atrust_dns_enabled(mut self, enabled: bool) -> Result<Self, ConnectEnvelopeError> {
        let EngineConnectPayload::Atrust(atrust) = &mut self.payload else {
            return Err(ConnectEnvelopeError::InvalidProtocol);
        };
        atrust.atrust_dns_enabled = enabled;
        atrust.profile_digest = digest_atrust_dns_profile(digest_atrust_profile(
            &atrust.gateway, &atrust.platform, &atrust.language, &atrust.resource_request,
            atrust.fresh_authentication, &atrust.resolver_domains,
        ), enabled);
        Ok(self)
    }

    #[allow(clippy::type_complexity)]
    fn validate_atrust(
        gateway: &str,
        browser_session: &[u8],
        platform: String,
        language: String,
        resource_request: Vec<u8>,
        fresh_authentication: bool,
    ) -> Result<(String, String, String, Vec<u8>, [u8; DIGEST_LEN]), ConnectEnvelopeError>
    {
        use exv_vpn_atrust::auth::origin::GatewayOrigin;

        let origin =
            GatewayOrigin::parse(gateway).map_err(|_| ConnectEnvelopeError::InvalidGateway)?;
        let gateway = origin.endpoint().to_owned();
        if gateway.len() > MAX_GATEWAY_LEN {
            return Err(ConnectEnvelopeError::InvalidGateway);
        }
        if browser_session.is_empty() || browser_session.len() > MAX_BROWSER_SESSION_LEN {
            return Err(ConnectEnvelopeError::InvalidBrowserSession);
        }
        let session = crate::browser_session::BrowserSession::decode(browser_session)
            .map_err(|_| ConnectEnvelopeError::InvalidBrowserSession)?;
        let session_origin = GatewayOrigin::parse(session.gateway())
            .map_err(|_| ConnectEnvelopeError::InvalidBrowserSession)?;
        if !session_origin.same_origin(&origin) {
            return Err(ConnectEnvelopeError::InvalidBrowserSession);
        }
        validate_ascii_token(&platform, MAX_PLATFORM_LEN, ConnectEnvelopeError::InvalidPlatform)?;
        validate_ascii_token(&language, MAX_LANGUAGE_LEN, ConnectEnvelopeError::InvalidLanguage)?;
        if resource_request.is_empty() || resource_request.len() > MAX_RESOURCE_REQUEST_LEN {
            return Err(ConnectEnvelopeError::InvalidResourceRequest);
        }
        let digest = digest_atrust_profile(
            &gateway,
            &platform,
            &language,
            &resource_request,
            fresh_authentication,
            &["ecnu.edu.cn".to_owned()],
        );
        Ok((gateway, platform, language, resource_request, digest))
    }

    /// 解析一个完整且无尾随字节的 V2 wire payload。输入 buffer 在所有返回路径清零。
    ///
    /// # Errors
    ///
    /// header、判别位、长度、UTF-8、字段边界或 canonical digest 不符 V2 时返回
    /// [`ConnectEnvelopeError`]；V1 wire（VERSION=1）得到 typed
    /// [`ConnectEnvelopeError::UnsupportedVersion`]。
    pub fn decode(mut payload: Vec<u8>) -> Result<Self, ConnectEnvelopeError> {
        let result = Self::decode_inner(&payload);
        payload.zeroize();
        result
    }

    fn decode_inner(payload: &[u8]) -> Result<Self, ConnectEnvelopeError> {
        let mut cursor = Cursor::new(payload);
        if cursor.take_exact(MAGIC.len())? != MAGIC {
            return Err(ConnectEnvelopeError::InvalidEncoding);
        }
        let version = cursor.read_u16()?;
        if !matches!(version, VERSION_V2 | VERSION_ATRUST_DNS) {
            return Err(ConnectEnvelopeError::UnsupportedVersion);
        }
        match cursor.read_u8()? {
            PROTOCOL_TAG_CISCO => {
                let envelope = DarwinEngineConnectV1::decode_body(&mut cursor)?;
                Ok(Self::new_cisco(envelope))
            }
            PROTOCOL_TAG_ATRUST => Self::decode_atrust_body(&mut cursor, version),
            _ => Err(ConnectEnvelopeError::InvalidProtocol),
        }
    }

    fn decode_atrust_body(cursor: &mut Cursor<'_>, version: u16) -> Result<Self, ConnectEnvelopeError> {
        let gateway = cursor.read_string()?;
        let browser_session = cursor.read_vec()?;
        let platform = cursor.read_string()?;
        let language = cursor.read_string()?;
        let resource_request = cursor.read_vec()?;
        let fresh_authentication = match cursor.read_u8()? {
            0 => false,
            1 => true,
            _ => return Err(ConnectEnvelopeError::InvalidEncoding),
        };
        let domain_count = usize::from(cursor.read_u16()?);
        let mut domains = Vec::with_capacity(domain_count);
        for _ in 0..domain_count {
            domains.push(cursor.read_string()?);
        }
        let enabled = if version == VERSION_ATRUST_DNS {
            match cursor.read_u8()? { 0 => false, 1 => true, _ => return Err(ConnectEnvelopeError::InvalidEncoding) }
        } else { false };
        let digest = cursor.take_exact(DIGEST_LEN)?;
        if !cursor.is_finished() {
            return Err(ConnectEnvelopeError::InvalidEncoding);
        }
        let mut profile_digest = [0_u8; DIGEST_LEN];
        profile_digest.copy_from_slice(digest);

        let value = Self::new_atrust(
            gateway,
            browser_session,
            platform,
            language,
            resource_request,
            fresh_authentication,
        )?.with_resolver_domains(domains)?.with_atrust_dns_enabled(enabled)?;
        let EngineConnectPayload::Atrust(atrust) = &value.payload else {
            return Err(ConnectEnvelopeError::InvalidEncoding);
        };
        let expected = if version == VERSION_V2 {
            digest_atrust_profile(&atrust.gateway, &atrust.platform, &atrust.language,
                &atrust.resource_request, atrust.fresh_authentication, &atrust.resolver_domains)
        } else { atrust.profile_digest };
        if expected != profile_digest {
            return Err(ConnectEnvelopeError::DigestMismatch);
        }
        Ok(value)
    }

    /// 消费并编码为 `secret_payload`。返回值的 buffer 在 drop 时清零。
    ///
    /// # Panics
    ///
    /// 仅当本模块内部绕过已验证构造路径，令某个字段长度超出其长度前缀宽度时 panic；
    /// 公共构造器和 decoder 都会先拒绝这种输入。
    #[must_use]
    pub fn encode(self) -> Zeroizing<Vec<u8>> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        push_u16(&mut bytes, match &self.payload { EngineConnectPayload::Cisco(_) => VERSION_V2, EngineConnectPayload::Atrust(_) => VERSION_ATRUST_DNS });
        match self.payload {
            EngineConnectPayload::Cisco(envelope) => {
                bytes.push(PROTOCOL_TAG_CISCO);
                envelope.encode_body(&mut bytes);
            }
            EngineConnectPayload::Atrust(atrust) => {
                bytes.push(PROTOCOL_TAG_ATRUST);
                push_string(&mut bytes, &atrust.gateway);
                push_u32(
                    &mut bytes,
                    u32::try_from(atrust.browser_session.len())
                        .expect("validated browser session length fits u32"),
                );
                bytes.extend_from_slice(&atrust.browser_session);
                push_string(&mut bytes, &atrust.platform);
                push_string(&mut bytes, &atrust.language);
                push_u32(
                    &mut bytes,
                    u32::try_from(atrust.resource_request.len())
                        .expect("validated resource request length fits u32"),
                );
                bytes.extend_from_slice(&atrust.resource_request);
                bytes.push(u8::from(atrust.fresh_authentication));
                push_u16(&mut bytes, u16::try_from(atrust.resolver_domains.len()).expect("validated domain count"));
                for domain in &atrust.resolver_domains {
                    push_string(&mut bytes, domain);
                }
                bytes.push(u8::from(atrust.atrust_dns_enabled));
                bytes.extend_from_slice(&atrust.profile_digest);
            }
        }
        Zeroizing::new(bytes)
    }

    #[must_use]
    pub const fn payload(&self) -> &EngineConnectPayload {
        &self.payload
    }

    /// 消费信封取回协议判别载荷（引擎侧 MAGIC+VERSION 分派后的消费口，M2 接线）。
    #[must_use]
    pub fn into_payload(self) -> EngineConnectPayload {
        self.payload
    }
}

impl fmt::Debug for DarwinEngineConnectV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.payload {
            EngineConnectPayload::Cisco(envelope) => envelope.fmt(formatter),
            EngineConnectPayload::Atrust(atrust) => atrust.fmt(formatter),
        }
    }
}

/// 非秘密短 token 校验：非空、ASCII、无控制字符、长度不超上限。
fn validate_ascii_token(
    value: &str,
    max_len: usize,
    error: ConnectEnvelopeError,
) -> Result<(), ConnectEnvelopeError> {
    if value.is_empty() || value.len() > max_len || !value.is_ascii() || contains_control(value) {
        return Err(error);
    }
    Ok(())
}

fn digest_atrust_dns_profile(legacy: [u8; DIGEST_LEN], enabled: bool) -> [u8; DIGEST_LEN] {
    let mut digest = Sha256::new();
    digest.update(b"EXV-DARWIN-CONNECT-PROFILE-V4-ATRUST-DNS\0");
    digest.update(legacy);
    digest.update([u8::from(enabled)]);
    digest.finalize().into()
}

/// aTrust 非秘密 profile `摘要（browser_session` 绝不参与）。
fn digest_atrust_profile(
    gateway: &str,
    platform: &str,
    language: &str,
    resource_request: &[u8],
    fresh_authentication: bool,
    resolver_domains: &[String],
) -> [u8; DIGEST_LEN] {
    let mut digest = Sha256::new();
    digest.update(b"EXV-DARWIN-CONNECT-PROFILE-V2-ATRUST\0");
    digest.update([PROTOCOL_TAG_ATRUST]);
    update_digest_string(&mut digest, gateway);
    update_digest_string(&mut digest, platform);
    update_digest_string(&mut digest, language);
    digest.update(
        u32::try_from(resource_request.len())
            .expect("validated resource request length fits u32")
            .to_be_bytes(),
    );
    digest.update(resource_request);
    digest.update([u8::from(fresh_authentication)]);
    digest.update(u16::try_from(resolver_domains.len()).expect("validated domain count").to_be_bytes());
    for domain in resolver_domains {
        update_digest_string(&mut digest, domain);
    }
    digest.finalize().into()
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take_exact(&mut self, length: usize) -> Result<&'a [u8], ConnectEnvelopeError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(ConnectEnvelopeError::InvalidEncoding)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(ConnectEnvelopeError::InvalidEncoding)?;
        self.position = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, ConnectEnvelopeError> {
        let bytes: [u8; 2] = self
            .take_exact(2)?
            .try_into()
            .map_err(|_| ConnectEnvelopeError::InvalidEncoding)?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32, ConnectEnvelopeError> {
        let bytes: [u8; 4] = self
            .take_exact(4)?
            .try_into()
            .map_err(|_| ConnectEnvelopeError::InvalidEncoding)?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn read_u8(&mut self) -> Result<u8, ConnectEnvelopeError> {
        Ok(self.take_exact(1)?[0])
    }

    fn read_vec(&mut self) -> Result<Vec<u8>, ConnectEnvelopeError> {
        let length = self.read_u32()? as usize;
        let bytes = self.take_exact(length)?;
        Ok(bytes.to_vec())
    }

    fn read_string(&mut self) -> Result<String, ConnectEnvelopeError> {
        let length = usize::from(self.read_u16()?);
        let bytes = self.take_exact(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ConnectEnvelopeError::InvalidEncoding)
    }

    const fn is_finished(&self) -> bool {
        self.position == self.bytes.len()
    }
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_string(bytes: &mut Vec<u8>, value: &str) {
    push_u16(
        bytes,
        u16::try_from(value.len()).expect("validated field length fits u16"),
    );
    bytes.extend_from_slice(value.as_bytes());
}

fn validate_server(value: &str) -> Result<(), ConnectEnvelopeError> {
    if value.is_empty()
        || value.len() > MAX_SERVER_LEN
        || !value.is_ascii()
        || is_decimal_ipv4_literal(value)
        || !value.split('.').all(valid_dns_label)
    {
        return Err(ConnectEnvelopeError::InvalidServer);
    }
    Ok(())
}

fn valid_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

fn is_decimal_ipv4_literal(value: &str) -> bool {
    let labels: Vec<_> = value.split('.').collect();
    labels.len() == 4
        && labels
            .iter()
            .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()))
}

fn validate_username(value: &str) -> Result<(), ConnectEnvelopeError> {
    if value.is_empty() || value.len() > MAX_USERNAME_LEN || contains_control(value) {
        return Err(ConnectEnvelopeError::InvalidUsername);
    }
    Ok(())
}

fn validate_password(value: &str) -> Result<(), ConnectEnvelopeError> {
    if value.is_empty() || value.len() > MAX_PASSWORD_LEN || contains_control(value) {
        return Err(ConnectEnvelopeError::InvalidPassword);
    }
    Ok(())
}

fn validate_routes(routes: &[String]) -> Result<(), ConnectEnvelopeError> {
    if routes.len() > MAX_ROUTE_COUNT || routes.iter().any(|route| !is_canonical_route(route)) {
        return Err(ConnectEnvelopeError::InvalidRoutes);
    }
    let mut deduplicated = routes.to_vec();
    deduplicated.sort_unstable();
    if deduplicated.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ConnectEnvelopeError::InvalidRoutes);
    }
    Ok(())
}

fn is_canonical_route(value: &str) -> bool {
    let Some((address_text, prefix_text)) = value.split_once('/') else {
        return false;
    };
    if prefix_text.contains('/') || !prefix_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Ok(prefix) = prefix_text.parse::<u8>() else {
        return false;
    };
    if !(1..=32).contains(&prefix) || prefix.to_string() != prefix_text {
        return false;
    }
    let Ok(address) = address_text.parse::<Ipv4Addr>() else {
        return false;
    };
    if address.to_string() != address_text {
        return false;
    }
    let mask = u32::MAX << (u32::BITS - u32::from(prefix));
    u32::from(address) & mask == u32::from(address)
}

fn validate_mtu(value: Option<u16>) -> Result<(), ConnectEnvelopeError> {
    if value.is_some_and(|mtu| !(576..=1_500).contains(&mtu)) {
        return Err(ConnectEnvelopeError::InvalidMtu);
    }
    Ok(())
}

fn validate_user_agent(value: &str) -> Result<(), ConnectEnvelopeError> {
    if value.is_empty() || value.len() > MAX_USER_AGENT_LEN || contains_control(value) {
        return Err(ConnectEnvelopeError::InvalidUserAgent);
    }
    Ok(())
}

fn contains_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn digest_profile(
    server: &str,
    username: &str,
    routes: &[String],
    mtu_override: Option<u16>,
    user_agent: &str,
) -> [u8; DIGEST_LEN] {
    let mut digest = Sha256::new();
    digest.update(b"EXV-DARWIN-CONNECT-PROFILE-V1\0");
    update_digest_string(&mut digest, server);
    update_digest_string(&mut digest, username);
    digest.update(
        u16::try_from(routes.len())
            .expect("validated route count fits u16")
            .to_be_bytes(),
    );
    for route in routes {
        update_digest_string(&mut digest, route);
    }
    digest.update(mtu_override.unwrap_or_default().to_be_bytes());
    update_digest_string(&mut digest, user_agent);
    digest.finalize().into()
}

fn update_digest_string(digest: &mut Sha256, value: &str) {
    digest.update(
        u16::try_from(value.len())
            .expect("validated field length fits u16")
            .to_be_bytes(),
    );
    digest.update(value.as_bytes());
}

