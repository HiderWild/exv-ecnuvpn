//! 认证请求的显式客户端身份声明与请求路径构造。
//!
//! `clientType`/`platform`/`lang` 三个协议字段由宿主显式给出：Common 不内置平台默认值，
//! 也不按 `target_os` 猜测平台名。`clientType` 由 [`AuthClientIdentity::browser`] 与
//! [`AuthClientIdentity::desktop`] 两个入口区分：浏览器路径 `SDPBrowserClient`，桌面/隧道
//! 路径 `SDPClient`；`platform`/`lang` 仍由宿主按协议证据提供（Windows 声明 `Windows`/`zh-CN`，
//! Darwin 的合法取值未经确认，本模块不编造）。
//!
//! 本模块是四个认证请求路径（`authConfig`、`ticketExchange`、`sessionIdExchange`、
//! `clientResource`）的唯一构造点：值按 RFC 3986 非保留字符集百分号编码（大写十六进制），
//! 值中的 `&`、`=`、`#` 等字节不能增加或改写其他 query 键。旧实现在 `auth/mod.rs` 内以
//! `QUERY` 常量和各方法字面量硬编码 Windows 身份，迁到本模块后字节不变（三个 Windows
//! 取值都在非保留字符集内）。

use super::AuthHttpError;

/// 官方 Windows aTrust 客户端 UA 字面量（逐字还原）。
///
/// 来源：`docs/research/atrust/2026-10-04-client-identity-and-portal-detection.md`
/// §一·补（本机官方客户端 `C:\Program Files (x86)\Sangfor\aTrust\aTrustTray\` 只读取证）：
/// 应用名与版本取自 `resources/app/package.json`（`aTrustTray` / `2.5.16.31`），
/// `Chrome/87.0.4280.141` 与 `Electron/11.5.0` 取自 `aTrustTray.exe` 内字符串，
/// 后缀 `aTrustTray-Windows-Plat SPCClientType` 取自
/// `resources/app/src/service/common.js` 的 `Common.userAgentAddPlatformConfig`。
///
/// 浏览器路径（系统浏览器、内嵌 `WebView`）与原生认证默认 UA 共用本常量，使浏览器、
/// 会话缓存与引擎协议请求三者 UA 同源。
pub const ATRUST_OFFICIAL_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) aTrustTray/2.5.16.31 Chrome/87.0.4280.141 Electron/11.5.0 Safari/537.36 aTrustTray-Windows-Plat SPCClientType";

pub const PLAIN_CHROME_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 桌面/隧道路径的 `clientType` 取值。
pub const CLIENT_TYPE_DESKTOP: &str = "SDPClient";
/// 浏览器路径的 `clientType` 取值。
pub const CLIENT_TYPE_BROWSER: &str = "SDPBrowserClient";

/// 环境检查策略下发端点（环境上报族，携带 `x-sdp-signature`/`x-sdp-random`，规格 2.3/2.4）。
pub const ENDPOINT_STRATEGY_PATH: &str = "/controller/v1/public/endpointStrategy";
/// 环境检查结果上报端点（环境上报族；无签名被网关以 10000008 拒绝，R1 §5.2）。
pub const REPORT_ENV_PATH: &str = "/controller/v1/public/reportEnv";
/// 认证状态检查端点（`nextService=auth/authCheck` 对应端点，官方默认 GET，R1 §5.1）。
pub const AUTH_CHECK_PATH: &str = "/passport/v1/auth/authCheck";

/// 显式客户端身份声明；不含平台默认值，也不实现 `Default`。
#[derive(Debug, Clone)]
pub struct AuthClientIdentity {
    client_type: String,
    platform: String,
    language: String,
}

impl AuthClientIdentity {
    /// 浏览器路径身份：`clientType=SDPBrowserClient`（与门户/浏览器历史成功样本一致）。
    /// # Errors
    /// 同 [`Self::new`]：平台或语言为空、含控制字符时返回
    /// [`AuthHttpError::InvalidConfiguration`]。
    pub fn browser(platform: &str, language: &str) -> Result<Self, AuthHttpError> {
        Self::new(CLIENT_TYPE_BROWSER, platform, language)
    }

    /// 桌面/隧道路径身份：`clientType=SDPClient`。
    /// # Errors
    /// 同 [`Self::new`]：平台或语言为空、含控制字符时返回
    /// [`AuthHttpError::InvalidConfiguration`]。
    pub fn desktop(platform: &str, language: &str) -> Result<Self, AuthHttpError> {
        Self::new(CLIENT_TYPE_DESKTOP, platform, language)
    }

    /// 以宿主声明构造身份。平台名是 aTrust 协议字段，由宿主按产品声明提供，绝不等同于编译目标。
    /// # Errors
    /// 任一值为空或含控制字符（`< 0x20` 或 `0x7F`）时返回
    /// [`AuthHttpError::InvalidConfiguration`]；错误不包含输入文本。
    pub fn new(client_type: &str, platform: &str, language: &str) -> Result<Self, AuthHttpError> {
        if [client_type, platform, language]
            .into_iter()
            .any(|value| {
                value.is_empty() || value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
            })
        {
            return Err(AuthHttpError::InvalidConfiguration);
        }
        Ok(Self {
            client_type: client_type.to_owned(),
            platform: platform.to_owned(),
            language: language.to_owned(),
        })
    }

    /// 公开 `authConfig` 路径；`need_ticket` 保持既有语义：`true` 只带 `needTicket=1`，
    /// `false` 带 `mod=1&needTicket=0`。
    #[must_use]
    pub fn auth_config_path(&self, need_ticket: bool) -> String {
        let query = self.query();
        if need_ticket {
            format!("/passport/v1/public/authConfig?{query}&needTicket=1")
        } else {
            format!("/passport/v1/public/authConfig?{query}&mod=1&needTicket=0")
        }
    }

    /// 已登录源会话生成一次性票据的路径。
    #[must_use]
    pub fn ticket_exchange_path(&self) -> String {
        format!("/passport/v1/public/ticketExchange?{}", self.query())
    }

    /// 接收会话兑换票据的路径。
    #[must_use]
    pub fn session_exchange_path(&self) -> String {
        format!("/passport/v1/public/sessionIdExchange?{}", self.query())
    }

    /// 客户端资源目录请求路径。
    #[must_use]
    pub fn resource_path(&self) -> String {
        format!("/controller/v1/user/clientResource?{}", self.query())
    }

    /// 环境检查策略下发路径。参数四元组 `timing`/`ticket`/`platform`/`clientType` 缺
    /// `platform` 或 `clientType` 时网关直接 404（路由不匹配，规格 2.4）；顺序按官方
    /// agent 样本（L1:1070），票据按 RFC 3986 编码后上送并参与签名原文。
    #[must_use]
    pub fn endpoint_strategy_path(&self, timing: &str, ticket: &str) -> String {
        format!(
            "{ENDPOINT_STRATEGY_PATH}?timing={}&ticket={}&platform={}&clientType={}",
            encode_value(timing),
            encode_value(ticket),
            encode_value(&self.platform),
            encode_value(&self.client_type)
        )
    }

    /// 环境检查结果上报路径。官方 agent 样本只带 `platform`/`clientType` 两参数
    /// （规格 §一.5），不带 `lang`。
    #[must_use]
    pub fn report_env_path(&self) -> String {
        format!(
            "{REPORT_ENV_PATH}?platform={}&clientType={}",
            encode_value(&self.platform),
            encode_value(&self.client_type)
        )
    }

    /// 认证状态检查路径（GET，空正文）。
    #[must_use]
    pub fn auth_check_path(&self) -> String {
        format!("{AUTH_CHECK_PATH}?{}", self.query())
    }

    /// 三个身份字段按固定顺序组成的 query（`clientType=…&platform=…&lang=…`，各值单独
    /// 编码）。S7 无头 CAS 链（宿主 `atrust_cas_login`）用它拼门禁实验钉死的
    /// `reportEnv` 等端点（该链的 query 含 `lang`，与官方桌面 agent 样本的两参数形态
    /// 不同）；既有路径构造方法不变。
    #[must_use]
    pub fn protocol_query(&self) -> String {
        self.query()
    }

    /// 三个身份字段按固定顺序组成 query，各值单独编码。
    fn query(&self) -> String {
        format!(
            "clientType={}&platform={}&lang={}",
            encode_value(&self.client_type),
            encode_value(&self.platform),
            encode_value(&self.language)
        )
    }
}

/// RFC 3986 非保留字符集 `A-Za-z0-9-._~` 之外的每个字节编码为 `%` + 大写十六进制。
fn encode_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}
