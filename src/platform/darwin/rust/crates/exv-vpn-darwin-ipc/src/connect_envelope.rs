//! Core 与 Engine 间单次连接意图的版本化内存 envelope。
//!
//! 该类型只编码到既有 `HelperControl` 的 `secret_payload`；它不创建 transport、socket、
//! 网络连接或任何平台资源。Core 构造后将字节交给已认证 Engine，Engine 必须再用
//! [`DarwinEngineConnectV1::decode`] 验证并消费一次。

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
        push_string(&mut bytes, &self.server);
        push_string(&mut bytes, &self.username);
        push_string(&mut bytes, &self.password);
        push_u16(
            &mut bytes,
            u16::try_from(self.campus_routes.len()).expect("validated route count fits u16"),
        );
        for route in &self.campus_routes {
            push_string(&mut bytes, route);
        }
        push_u16(&mut bytes, self.mtu_override.unwrap_or_default());
        push_string(&mut bytes, &self.user_agent);
        bytes.extend_from_slice(&self.profile_digest);
        Zeroizing::new(bytes)
    }

    #[must_use]
    pub fn profile_digest(&self) -> &[u8; DIGEST_LEN] {
        &self.profile_digest
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
