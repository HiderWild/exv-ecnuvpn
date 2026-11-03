//! 浏览器候选会话 → 票据兑换结果与资源目录的宿主无关编排。
//!
//! 本模块沿用现有交接次序：同一网关上先复核浏览器源会话，再建立独立接收会话，
//! 用一次性票据取得接收会话，获取资源所需材料后请求资源目录，最后才构造
//! [`AuthenticatedAtrustSession`]。全部出站连接都经注入的 [`Underlay`]；本模块不拥有适配器、
//! 路由、DNS 或任何服务，也不提供默认拨号器。资源请求体（`resource_request`）是宿主产品选择
//! （例如 Windows 的 appList/favoriteAppList/featureCenter/uemSpace 组合），不是通用协议默认值。
//!
//! 本编排是可选协议操作：底层 [`AuthClient`] 的公开请求保持可独立使用，未来宿主不必采用
//! 浏览器交接路径。

use super::AuthHttpError;
use super::{AuthClient, AuthClientOptions};
use crate::auth::browser_session::BrowserSession;
use crate::auth::origin::GatewayOrigin;
use crate::{ResourceCatalog, ResourceError, ResourceParseOptions, Secret, Underlay};
use std::sync::Arc;

/// 交接失败的阶段标记。阶段名稳定，供宿主日志与 UI 映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStage {
    Gateway,
    Source,
    Receiver,
    Ticket,
    Exchange,
    /// 保留宿主阶段匹配的兼容面；当前交接不再产生兑换后复验阶段。
    Verified,
    Resources,
}

/// 交接错误：只携带稳定阶段与协议库已脱敏的原因，不携带 Cookie、票据或 SID。
#[derive(Debug)]
pub struct AtrustAuthError {
    pub stage: AuthStage,
    cause: AuthFailure,
}

#[derive(Debug)]
enum AuthFailure {
    Http(AuthHttpError),
    ResourceMaterials(AuthHttpError),
    Resource(ResourceError),
    Message(&'static str),
}

impl From<AuthHttpError> for AuthFailure {
    fn from(error: AuthHttpError) -> Self {
        Self::Http(error)
    }
}
impl From<ResourceError> for AuthFailure {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}
impl From<&'static str> for AuthFailure {
    fn from(message: &'static str) -> Self {
        Self::Message(message)
    }
}

impl std::fmt::Display for AuthFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(error) => std::fmt::Display::fmt(error, f),
            Self::ResourceMaterials(error) => write!(f, "resource material request: {error}"),
            Self::Resource(error) => std::fmt::Display::fmt(error, f),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl AtrustAuthError {
    fn at(stage: AuthStage, cause: impl Into<AuthFailure>) -> Self {
        Self {
            stage,
            cause: cause.into(),
        }
    }
}
impl std::fmt::Display for AtrustAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "aTrust {:?}: {}", self.stage, self.cause)
    }
}
impl std::error::Error for AtrustAuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            AuthFailure::Http(error) | AuthFailure::ResourceMaterials(error) => Some(error),
            AuthFailure::Resource(error) => Some(error),
            AuthFailure::Message(_) => None,
        }
    }
}

/// 原票据兑换与资源请求、解析完成后的接收会话。它不表示 L3 或宿主隧道已连接。
/// SID 不经过日志或子进程。
pub struct AuthenticatedAtrustSession {
    client: AuthClient,
    catalog: ResourceCatalog,
    resource_bytes: usize,
}
impl AuthenticatedAtrustSession {
    #[must_use]
    pub fn client(&mut self) -> &mut AuthClient {
        &mut self.client
    }
    #[must_use]
    pub fn catalog(&self) -> &ResourceCatalog {
        &self.catalog
    }
    #[must_use]
    pub fn sid(&self) -> Option<&Secret> {
        self.client.sid()
    }
    #[must_use]
    pub fn resource_bytes(&self) -> usize {
        self.resource_bytes
    }
}
/// 只打印非敏感元数据：接收客户端持有 SID 与 Cookie，目录与正文都不进入日志。
impl std::fmt::Debug for AuthenticatedAtrustSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticatedAtrustSession")
            .field("resource_bytes", &self.resource_bytes)
            .finish_non_exhaustive()
    }
}

/// 端点解释失败时保持原有阶段与文案。`atrust_prepare` 等宿主入口复用同一构造，
/// 保证原无效端点错误文本不变。
#[must_use]
pub fn invalid_https_origin() -> AtrustAuthError {
    AtrustAuthError::at(AuthStage::Gateway, "invalid HTTPS origin")
}

/// 消费浏览器候选会话并建立独立认证会话。出口由宿主连接 owner 注入。
/// `expected_gateway` 是本次连接已经选定的网关，不能由回传 Cookie 临时改写。
/// 请求沿用 [`AuthClientOptions`] 的逐请求期限；取消本 future 即释放当前 HTTP/TLS 连接，
/// 不表示服务端已注销已兑换的会话，也不做远端补偿。
///
/// 浏览器候选会话的 UA 优先于 `options.user_agent`；identity、timeout 与
/// `max_response_bytes` 由传入的 `options` 提供，不回退任何默认值。
///
/// # Errors
/// 返回失败的精确阶段及协议库已脱敏原因；任何一步失败都不返回已认证会话。
pub async fn authenticate_browser_session(
    browser: BrowserSession,
    expected_gateway: &str,
    underlay: Arc<dyn Underlay>,
    options: AuthClientOptions,
    resource_request: &[u8],
) -> Result<AuthenticatedAtrustSession, AtrustAuthError> {
    let origin = GatewayOrigin::parse(browser.gateway()).map_err(|_| invalid_https_origin())?;
    let expected = GatewayOrigin::parse(expected_gateway).map_err(|_| invalid_https_origin())?;
    if !origin.same_origin(&expected) {
        return Err(AtrustAuthError::at(
            AuthStage::Gateway,
            "browser gateway differs from selected gateway",
        ));
    }
    // 浏览器 UA 在候选会话被 drop 前取出；两次 AuthClient 构造都用它。
    let user_agent = browser.user_agent().to_owned();
    let client_options = || AuthClientOptions {
        user_agent: user_agent.clone(),
        ..options.clone()
    };
    let mut source = AuthClient::new(browser.gateway(), underlay.clone(), client_options())
        .map_err(|e| AtrustAuthError::at(AuthStage::Source, e))?;
    source
        .import_gateway_cookies(Secret::new(browser.cookie_header().to_owned()))
        .map_err(|e| AtrustAuthError::at(AuthStage::Source, e))?;
    if !source
        .auth_config(false)
        .await
        .map_err(|e| AtrustAuthError::at(AuthStage::Source, e))?
        .is_login
    {
        return Err(AtrustAuthError::at(
            AuthStage::Source,
            "browser session is not authenticated",
        ));
    }
    let mut receiver = AuthClient::new(browser.gateway(), underlay, client_options())
        .map_err(|e| AtrustAuthError::at(AuthStage::Receiver, e))?;
    // 浏览器候选会话不再被后续步骤使用：不把其 Cookie 材料留到交接后半程。
    drop(browser);
    if receiver
        .auth_config(true)
        .await
        .map_err(|e| AtrustAuthError::at(AuthStage::Receiver, e))?
        .is_login
    {
        return Err(AtrustAuthError::at(
            AuthStage::Receiver,
            "fresh session unexpectedly authenticated",
        ));
    }
    let ticket = source
        .ticket_exchange()
        .await
        .map_err(|e| AtrustAuthError::at(AuthStage::Ticket, e))?;
    drop(source);
    let exchange = receiver
        .exchange_sid_ticket(ticket)
        .await
        .map_err(|e| AtrustAuthError::at(AuthStage::Exchange, e))?;
    receiver
        .admit_session_exchange(exchange)
        .map_err(|e| AtrustAuthError::at(AuthStage::Exchange, e))?;
    receiver
        .prepare_resource_materials()
        .await
        .map_err(|error| {
            AtrustAuthError::at(AuthStage::Resources, AuthFailure::ResourceMaterials(error))
        })?;
    let resources = receiver
        .client_resource(resource_request)
        .await
        .map_err(|e| AtrustAuthError::at(AuthStage::Resources, e))?;
    let catalog = ResourceCatalog::from_json(
        &resources,
        &ResourceParseOptions::with_sdpc_host(origin.host()),
    )
    .map_err(|e| AtrustAuthError::at(AuthStage::Resources, e))?;
    Ok(AuthenticatedAtrustSession {
        client: receiver,
        catalog,
        resource_bytes: resources.len(),
    })
}
