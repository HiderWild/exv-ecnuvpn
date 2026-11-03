//! 脱离当前学校页面的固定网关探针，只报告会话事实，不提交连接。
use exv_vpn_atrust::Secret;
use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
use exv_vpn_atrust::auth::{AuthClient, AuthClientOptions, AuthHttpError};
use exv_vpn_atrust::transport::Underlay;
use std::{sync::Arc, time::Duration};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

pub struct SessionProbe {
    pub is_login: bool,
    pub first_auth: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeError {
    pub code: &'static str,
    pub http_status: Option<u16>,
}

impl ProbeError {
    fn from_auth(error: AuthHttpError) -> Self {
        if let Some(diagnostic) = error.transport_diagnostic() {
            log::warn!(target: "exv.atrust_auth", "session_probe.transport {diagnostic}");
        }
        let (code, http_status) = match error {
            AuthHttpError::InvalidConfiguration => ("config", None),
            AuthHttpError::InvalidCookie => ("cookie", None),
            AuthHttpError::Timeout => ("timeout", None),
            AuthHttpError::Underlay(_) => ("underlay", None),
            AuthHttpError::Tls { .. } => ("tls", None),
            AuthHttpError::Http { .. } => ("http", None),
            AuthHttpError::HttpStatus { status } => ("http", Some(status)),
            AuthHttpError::SizeLimit => ("size_limit", None),
            AuthHttpError::Protocol => ("protocol", None),
            AuthHttpError::Rejected { .. } => ("rejected", None),
            AuthHttpError::MissingCsrf => ("csrf", None),
            AuthHttpError::MissingSignKey => ("sign_key", None),
        };
        Self { code, http_status }
    }
}

/// 对冻结网关的一次 GET。Cookie 只作为本次请求材料，返回错误不含正文或秘密。
/// 外层仍须用本次认证剩余期限截断等待；本层不重试。
///
/// # Errors
/// 配置、请求和服务端响应实际失败时返回脱敏分类。
pub async fn probe_auth_config(
    gateway: &str,
    cookie_header: &str,
    user_agent: &str,
    timeout: Duration,
    underlay: Arc<dyn Underlay>,
) -> Result<SessionProbe, ProbeError> {
    let identity = AuthClientIdentity::browser("Mac", "zh-CN").map_err(|_| ProbeError {
        code: "identity",
        http_status: None,
    })?;
    let options = AuthClientOptions {
        user_agent: user_agent.to_owned(),
        timeout: timeout.min(PROBE_TIMEOUT),
        ..AuthClientOptions::new(identity)
    };
    let mut client = AuthClient::new(gateway, underlay, options).map_err(ProbeError::from_auth)?;
    if !cookie_header.trim().is_empty() {
        client
            .import_gateway_cookies(Secret::new(cookie_header.to_owned()))
            .map_err(ProbeError::from_auth)?;
    }
    let status = client
        .auth_config(false)
        .await
        .map_err(ProbeError::from_auth)?;
    Ok(SessionProbe {
        is_login: status.is_login,
        first_auth: status.first_auth.and_then(|paths| paths.into_iter().next()),
    })
}
