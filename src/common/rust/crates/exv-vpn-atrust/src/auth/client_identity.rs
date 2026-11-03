//! 认证请求的显式客户端身份声明与请求路径构造。
//!
//! `clientType`/`platform`/`lang` 三个协议字段由宿主显式给出：Common 不内置平台默认值，
//! 也不按 `target_os` 猜测平台名。Windows 产品声明 `SDPClient`/`Windows`/`zh-CN`，其他宿主
//! 必须按各自的协议证据提供合法值（Darwin 的合法取值未经确认，本模块不编造）。
//!
//! 本模块是四个认证请求路径（`authConfig`、`ticketExchange`、`sessionIdExchange`、
//! `clientResource`）的唯一构造点：值按 RFC 3986 非保留字符集百分号编码（大写十六进制），
//! 值中的 `&`、`=`、`#` 等字节不能增加或改写其他 query 键。旧实现在 `auth/mod.rs` 内以
//! `QUERY` 常量和各方法字面量硬编码 Windows 身份，迁到本模块后字节不变（三个 Windows
//! 取值都在非保留字符集内）。

use super::AuthHttpError;

/// 显式客户端身份声明；不含平台默认值，也不实现 `Default`。
#[derive(Debug, Clone)]
pub struct AuthClientIdentity {
    client_type: String,
    platform: String,
    language: String,
}

impl AuthClientIdentity {
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
