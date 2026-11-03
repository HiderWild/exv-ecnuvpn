//! 浏览器候选会话的 EXV v2 交接消息与 Cookie 纯构造。
//!
//! 本模块是 `BrowserSession`/`BrowserSessionError` 的唯一实现：Windows IPC 入口只做 re-export，
//! 依赖方向固定为 Common ← 平台，Common 不反向依赖任何宿主 crate。
//!
//! v2、`kind`、字段顺序与名称是 EXV 宿主间交接契约（不是 aTrust 服务端 wire 标准），
//! 移动模块不改变编码结果。候选会话只表示“浏览器向指定网关提供了会话材料”，不表示服务端
//! 已授予内网权限；Cookie 随实例释放清零，编码结果由零化容器持有。
//!
//! 浏览器差异（外部 CDP 的有序数组、内嵌 `WebView` 的合并/去重）属于宿主采集适配；两处适配
//! 把已取得的有序 `(name, value)` 交给 [`session_from_cookie_pairs`]，共享函数只做允许集合
//! 过滤、`sid` 存在性与控制字符校验，不采集、不合并、不排序。

use super::origin::GatewayOrigin;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const VERSION: u32 = 2;
const KIND: &str = "atrust-browser-session";
const MAX_PAYLOAD: usize = 64 * 1024;

/// 两个浏览器入口共用的网关 Cookie 名称允许集合；名称匹配区分大小写且必须完全一致。
pub const GATEWAY_COOKIE_NAMES: &[&str] = &[
    "sid",
    "sid.sig",
    "sid-legacy",
    "sid-legacy.sig",
    "lang",
    "lang.sig",
];

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(transparent)]
struct CookieHeader(String);

impl std::fmt::Debug for CookieHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// 浏览器向指定网关提供的候选会话，不是已经授予的内网权限。
/// 不含学校密码；Cookie 随实例释放清零，编码结果同样由零化容器持有。
#[derive(Serialize, Deserialize)]
pub struct BrowserSession {
    version: u32,
    kind: String,
    gateway: String,
    cookie_header: CookieHeader,
    user_agent: String,
}

impl std::fmt::Debug for BrowserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserSession")
            .field("version", &self.version)
            .field("cookie_header", &self.cookie_header)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BrowserSessionError {
    Payload,
    Version,
    Kind,
    Gateway,
    Headers,
    TooLarge,
}

impl std::fmt::Display for BrowserSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Payload => "browser session: invalid payload",
            Self::Version => "browser session: unsupported version",
            Self::Kind => "browser session: unsupported kind",
            Self::Gateway => "browser session: invalid HTTPS gateway",
            Self::Headers => "browser session: invalid headers",
            Self::TooLarge => "browser session: payload too large",
        })
    }
}
impl std::error::Error for BrowserSessionError {}

/// 由宿主已取得的有序 `(name, value)` 构造候选会话；不做采集、合并或排序。
#[derive(Debug, PartialEq, Eq)]
pub enum CookieSessionError {
    /// 允许集合内没有非空 `sid`：尚未取得网关会话。
    MissingSession,
    /// 允许集合内的值含 `\r`、`\n` 或 `;`：字段本身不合法。
    InvalidField,
    /// 交给 [`BrowserSession::new`] 时的消息级失败（网关/头部/大小）。
    Session(BrowserSessionError),
}

impl std::fmt::Display for CookieSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSession => f.write_str("browser cookies: no gateway session"),
            Self::InvalidField => f.write_str("browser cookies: invalid field"),
            Self::Session(error) => write!(f, "browser cookies: {error}"),
        }
    }
}

impl std::error::Error for CookieSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Session(error) => Some(error),
            Self::MissingSession | Self::InvalidField => None,
        }
    }
}

impl BrowserSession {
    /// 创建待认证候选会话。网关为 HTTPS origin；错误不包含输入材料。
    /// # Errors
    /// 网关、头部或消息大小不合法时返回固定分类。
    pub fn new(
        gateway: String,
        cookie_header: String,
        user_agent: String,
    ) -> Result<Self, BrowserSessionError> {
        let session = Self {
            version: VERSION,
            kind: KIND.into(),
            gateway,
            cookie_header: CookieHeader(cookie_header),
            user_agent,
        };
        session.validate()?;
        Ok(session)
    }

    /// # Errors
    /// 拒绝非本消息种类、未知版本及无效字段，不将 JSON 错误文本带入日志。
    pub fn decode(bytes: &[u8]) -> Result<Self, BrowserSessionError> {
        if bytes.len() > MAX_PAYLOAD {
            return Err(BrowserSessionError::TooLarge);
        }
        let session: Self =
            serde_json::from_slice(bytes).map_err(|_| BrowserSessionError::Payload)?;
        session.validate()?;
        Ok(session)
    }

    /// # Errors
    /// 编码失败或超过 IPC 材料上限时返回错误。结果不得进入日志或命令行。
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, BrowserSessionError> {
        self.validate()?;
        let bytes =
            Zeroizing::new(serde_json::to_vec(self).map_err(|_| BrowserSessionError::Payload)?);
        if bytes.len() > MAX_PAYLOAD {
            return Err(BrowserSessionError::TooLarge);
        }
        Ok(bytes)
    }

    fn validate(&self) -> Result<(), BrowserSessionError> {
        if self.version != VERSION {
            return Err(BrowserSessionError::Version);
        }
        if self.kind != KIND {
            return Err(BrowserSessionError::Kind);
        }
        GatewayOrigin::parse(&self.gateway).map_err(|_| BrowserSessionError::Gateway)?;
        if self.cookie_header.0.is_empty()
            || self.user_agent.is_empty()
            || http::HeaderValue::from_str(&self.cookie_header.0).is_err()
            || http::HeaderValue::from_str(&self.user_agent).is_err()
        {
            return Err(BrowserSessionError::Headers);
        }
        if self
            .gateway
            .len()
            .saturating_add(self.cookie_header.0.len())
            .saturating_add(self.user_agent.len())
            > MAX_PAYLOAD
        {
            return Err(BrowserSessionError::TooLarge);
        }
        Ok(())
    }

    #[must_use]
    pub fn gateway(&self) -> &str {
        &self.gateway
    }
    /// 仅供同一进程认证客户端构造请求，不可记录。
    #[must_use]
    pub fn cookie_header(&self) -> &str {
        &self.cookie_header.0
    }
    #[must_use]
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
}

/// 共享的 Cookie 纯构造：按 [`GATEWAY_COOKIE_NAMES`] 过滤，要求非空 `sid`，
/// 拒绝允许集合值中的 `\r`、`\n`、`;`，其余错误由 [`BrowserSession::new`] 给出。
/// 输入顺序与重复项原样保留，返回头部用 `"; "` 连接；错误不包含 Cookie 值。
///
/// # Errors
/// 缺少非空 `sid` 返回 [`CookieSessionError::MissingSession`]；允许集合内的值含控制字符
/// 返回 [`CookieSessionError::InvalidField`]；网关/头部/大小不合法返回
/// [`CookieSessionError::Session`]。
pub fn session_from_cookie_pairs(
    gateway: &str,
    user_agent: &str,
    cookies: Vec<(String, String)>,
) -> Result<BrowserSession, CookieSessionError> {
    let mut fields = Vec::new();
    let mut has_sid = false;
    for (name, value) in cookies {
        if !GATEWAY_COOKIE_NAMES.contains(&name.as_str()) {
            continue;
        }
        if value.contains(['\r', '\n', ';']) {
            return Err(CookieSessionError::InvalidField);
        }
        has_sid |= name == "sid" && !value.is_empty();
        fields.push(format!("{name}={value}"));
    }
    if !has_sid {
        return Err(CookieSessionError::MissingSession);
    }
    BrowserSession::new(gateway.to_owned(), fields.join("; "), user_agent.to_owned())
        .map_err(CookieSessionError::Session)
}
