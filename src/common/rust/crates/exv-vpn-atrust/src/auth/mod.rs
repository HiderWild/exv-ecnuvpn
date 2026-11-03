//! 原生票据兑换与资源 HTTP 客户端；浏览器候选会话的完整交接编排见 [`browser_handoff`]，
//! Windows 生产路径由 `win32-engine/src/atrust_auth.rs` 的薄组合消费（经 `atrust_prepare.rs`
//! 进入运行时），Darwin 尚未接线。
//! 接口依据 docs/research/atrust/controlled-browser-debug-session.md §7.6。
//! 宿主注入 TLS 之下的裸 TCP 流；库执行平台证书验证，不提供默认拨号器。
//! 请求路径由宿主显式声明的 [`AuthClientIdentity`] 构造，Common 不提供平台默认值。

pub mod browser_handoff;
pub mod browser_session;
pub mod client_identity;
pub mod origin;

use self::client_identity::AuthClientIdentity;
use self::origin::GatewayOrigin;
use crate::{ConnectPurpose, OutboundTarget, Secret, Underlay};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Request};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use zeroize::{Zeroize, Zeroizing};

const MAX_COOKIE_BYTES: usize = 32 * 1024;

/// 错误仅包含稳定分类，不包含网关正文、Cookie 或票据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthHttpError {
    #[error("invalid gateway endpoint or client options")]
    InvalidConfiguration,
    #[error("invalid gateway cookie")]
    InvalidCookie,
    #[error("authentication request timed out")]
    Timeout,
    #[error("underlay connection failed")]
    Underlay,
    #[error("TLS verification or handshake failed")]
    Tls,
    #[error("HTTP transport failed")]
    Http,
    #[error("gateway returned HTTP status {status}")]
    HttpStatus { status: u16 },
    #[error("response or request exceeds configured limit")]
    SizeLimit,
    #[error("gateway returned malformed protocol response")]
    Protocol,
    #[error("gateway rejected request (code {code})")]
    Rejected { code: i64 },
    #[error("authConfig must provide CSRF token first")]
    MissingCsrf,
}

/// 所有 HTTP 操作共用的有限预算；超时覆盖拨号、TLS、响应头和响应正文。
/// 客户端身份由宿主显式提供，因此没有隐式默认构造；预算取值保持原默认。
#[derive(Debug, Clone)]
pub struct AuthClientOptions {
    pub user_agent: String,
    pub timeout: Duration,
    pub max_response_bytes: usize,
    pub identity: AuthClientIdentity,
    /// 仅 `cfg(test)` 可用的每实例测试信任注入：正式构建不存在该字段，也没有全局开关。
    /// 注入的根集合仍走完整链与主机名校验，只是信任锚来自测试夹具而非宿主系统。
    #[cfg(test)]
    pub(crate) test_trust: Option<Arc<rustls::RootCertStore>>,
}
impl AuthClientOptions {
    /// 以宿主声明的身份构造选项；预算取原默认值：UA `EXV-aTrust/0.1`、30 秒、8 MiB。
    #[must_use]
    pub fn new(identity: AuthClientIdentity) -> Self {
        Self {
            user_agent: "EXV-aTrust/0.1".into(),
            timeout: Duration::from_secs(30),
            max_response_bytes: 8 * 1024 * 1024,
            identity,
            #[cfg(test)]
            test_trust: None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthStatus {
    pub is_login: bool,
}

/// 单一固定网关作用域会话；不跟随重定向，也不向其他源发送 Cookie。
pub struct AuthClient {
    host: String,
    authority: String,
    port: u16,
    underlay: Arc<dyn Underlay>,
    options: AuthClientOptions,
    cookies: BTreeMap<String, Secret>,
    csrf: Option<Secret>,
}

impl AuthClient {
    /// 创建固定网关客户端。
    /// # Errors
    /// 非 HTTPS、带用户信息或路径的端点以及无效预算返回配置错误。
    pub fn new(
        endpoint: &str,
        underlay: Arc<dyn Underlay>,
        options: AuthClientOptions,
    ) -> Result<Self, AuthHttpError> {
        let origin =
            GatewayOrigin::parse(endpoint).map_err(|_| AuthHttpError::InvalidConfiguration)?;
        if options.timeout.is_zero()
            || options.max_response_bytes == 0
            || options.max_response_bytes > 64 * 1024 * 1024
            || options.user_agent.len() > 4096
            || HeaderValue::from_str(&options.user_agent).is_err()
        {
            return Err(AuthHttpError::InvalidConfiguration);
        }
        let host = origin.host().to_owned();
        let port = origin.port();
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        Ok(Self {
            host,
            authority,
            port,
            underlay,
            options,
            cookies: BTreeMap::new(),
            csrf: None,
        })
    }

    /// 仅供受信任宿主导入当前网关的 Cookie 请求头；原子替换，随后需 authConfig 验证会话。
    /// # Errors
    /// 非法 Cookie 字符或超限返回错误，原会话保持不变。
    pub fn import_gateway_cookies(&mut self, cookie_header: Secret) -> Result<(), AuthHttpError> {
        if cookie_header.len() > MAX_COOKIE_BYTES {
            return Err(AuthHttpError::SizeLimit);
        }
        let mut cookies = BTreeMap::new();
        for pair in cookie_header.expose().split(';') {
            let (name, value) = cookie_pair(pair.trim())?;
            cookies.insert(name.into(), Secret::new(value));
        }
        self.cookies = cookies;
        self.csrf = None;
        drop(cookie_header);
        Ok(())
    }

    #[must_use]
    pub fn sid(&self) -> Option<&Secret> {
        self.cookies.get("sid")
    }

    /// 查询会话状态并更新 CSRF。
    /// # Errors
    /// 返回有界传输、HTTP 状态、协议形状或业务错误。
    #[allow(clippy::items_after_statements)]
    pub async fn auth_config(&mut self, need_ticket: bool) -> Result<AuthStatus, AuthHttpError> {
        let path = self.options.identity.auth_config_path(need_ticket);
        let raw = self
            .request(
                Method::GET,
                &path,
                Zeroizing::new(Vec::new()),
                ConnectPurpose::AuthGateway,
                false,
            )
            .await?;
        #[derive(Deserialize)]
        struct Security {
            #[serde(rename = "csrfToken")]
            csrf_token: Option<String>,
        }
        impl Drop for Security {
            fn drop(&mut self) {
                if let Some(value) = &mut self.csrf_token {
                    value.zeroize();
                }
            }
        }
        #[derive(Deserialize)]
        struct Config {
            #[serde(rename = "isLogin")]
            is_login: u8,
            security: Option<Security>,
        }
        let config: Config = decode(&raw)?;
        self.csrf = config
            .security
            .as_ref()
            .and_then(|s| s.csrf_token.as_ref())
            .map(|s| Secret::new(s.as_str()));
        Ok(AuthStatus {
            is_login: config.is_login == 1,
        })
    }

    /// 将一次性票据兑换为当前网关会话。
    /// # Errors
    /// 缺少 CSRF 或网关拒绝、传输失败时返回错误。
    pub async fn exchange_sid_ticket(&mut self, ticket: Secret) -> Result<(), AuthHttpError> {
        #[derive(serde::Serialize)]
        struct Ticket<'a> {
            #[serde(rename = "sidTicket")]
            sid_ticket: &'a str,
        }
        let body = Zeroizing::new(
            serde_json::to_vec(&Ticket {
                sid_ticket: ticket.expose(),
            })
            .map_err(|_| AuthHttpError::Protocol)?,
        );
        let path = self.options.identity.session_exchange_path();
        let raw = self
            .request(
                Method::POST,
                &path,
                body,
                ConnectPurpose::AuthGateway,
                true,
            )
            .await?;
        decode::<serde::de::IgnoredAny>(&raw)?;
        Ok(())
    }

    /// 已登录源会话生成票据，供受信任宿主立即交给另一个网关会话。
    /// # Errors
    /// 缺少 CSRF 或网关拒绝、传输失败时返回错误。
    #[allow(clippy::items_after_statements)]
    pub async fn ticket_exchange(&mut self) -> Result<Secret, AuthHttpError> {
        let path = self.options.identity.ticket_exchange_path();
        let raw = self
            .request(
                Method::POST,
                &path,
                Zeroizing::new(b"{}".to_vec()),
                ConnectPurpose::AuthGateway,
                true,
            )
            .await?;
        #[derive(Deserialize)]
        struct Ticket {
            #[serde(rename = "sidTicket")]
            sid_ticket: String,
        }
        impl Drop for Ticket {
            fn drop(&mut self) {
                self.sid_ticket.zeroize();
            }
        }
        let ticket: Ticket = decode(&raw)?;
        Ok(Secret::new(&ticket.sid_ticket))
    }

    /// 返回网关完整 JSON 正文，供资源解析器处理；调用方负责禁止日志输出。
    /// 请求必须包含 appList；同时校验顶层与 appList 业务码。
    /// # Errors
    /// 传输失败、顶层或 appList 拒绝、响应缺少 appList 时返回错误。
    pub async fn client_resource(
        &mut self,
        request_json: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AuthHttpError> {
        let path = self.options.identity.resource_path();
        let raw = self
            .request(
                Method::POST,
                &path,
                Zeroizing::new(request_json.to_vec()),
                ConnectPurpose::ResourceGateway,
                true,
            )
            .await?;
        validate_resource(&raw)?;
        Ok(raw)
    }

    async fn request(
        &mut self,
        method: Method,
        path: &str,
        body: Zeroizing<Vec<u8>>,
        purpose: ConnectPurpose,
        csrf_required: bool,
    ) -> Result<Zeroizing<Vec<u8>>, AuthHttpError> {
        if body.len() > self.options.max_response_bytes {
            return Err(AuthHttpError::SizeLimit);
        }
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", &self.authority)
            .header("user-agent", &self.options.user_agent)
            .header("accept", "application/json")
            .header("content-type", "application/json");
        if csrf_required {
            let csrf = self.csrf.as_ref().ok_or(AuthHttpError::MissingCsrf)?;
            builder = builder.header("x-csrf-token", sensitive_header(csrf.expose())?);
        }
        let mut cookie_header = Zeroizing::new(String::new());
        for (name, value) in &self.cookies {
            if !cookie_header.is_empty() {
                cookie_header.push_str("; ");
            }
            cookie_header.push_str(name);
            cookie_header.push('=');
            cookie_header.push_str(value.expose());
        }
        if !cookie_header.is_empty() {
            builder = builder.header("cookie", sensitive_header(&cookie_header)?);
        }
        // HTTP/TLS 实现会持有短期传输缓冲区；本层原始敏感材料和完整正文采用零化容器。
        let request = builder
            .body(Full::new(Bytes::copy_from_slice(&body)))
            .map_err(|_| AuthHttpError::Protocol)?;
        let result = tokio::time::timeout(self.options.timeout, async {
            let stream = self
                .underlay
                .connect(&OutboundTarget::new(&self.host, self.port, purpose))
                .await
                .map_err(|_| AuthHttpError::Underlay)?;
            let config = client_config(&self.options)?;
            let server_name = rustls::pki_types::ServerName::try_from(self.host.clone())
                .map_err(|_| AuthHttpError::InvalidConfiguration)?;
            let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(server_name, stream)
                .await
                .map_err(|_| AuthHttpError::Tls)?;
            send_http(tls, request, self.options.max_response_bytes).await
        })
        .await
        .map_err(|_| AuthHttpError::Timeout)??;
        self.update_cookies(&result.0)?;
        Ok(result.1)
    }

    fn update_cookies(&mut self, headers: &HeaderMap) -> Result<(), AuthHttpError> {
        // 本流程使用同网关根路径会话 Cookie。跨域或非根路径 Cookie 不导入。
        // 当前不实现仅 Expires 的过期删除或浏览器完整 CookieJar 语义。
        for header in headers.get_all("set-cookie") {
            let value = header.to_str().map_err(|_| AuthHttpError::InvalidCookie)?;
            if value.len() > MAX_COOKIE_BYTES {
                return Err(AuthHttpError::SizeLimit);
            }
            let mut parts = value.split(';');
            let (name, value) = cookie_pair(parts.next().unwrap_or("").trim())?;
            let mut delete = value.is_empty();
            let mut applicable = true;
            for attribute in parts {
                let (key, val) = attribute
                    .trim()
                    .split_once('=')
                    .unwrap_or((attribute.trim(), ""));
                if key.eq_ignore_ascii_case("domain")
                    && !val.trim_start_matches('.').eq_ignore_ascii_case(&self.host)
                {
                    applicable = false;
                }
                if key.eq_ignore_ascii_case("path") && val != "/" {
                    applicable = false;
                }
                if key.eq_ignore_ascii_case("max-age")
                    && val.parse::<i64>().is_ok_and(|age| age <= 0)
                {
                    delete = true;
                }
            }
            if applicable {
                if delete {
                    self.cookies.remove(name);
                } else {
                    self.cookies.insert(name.into(), Secret::new(value));
                }
            }
            if self
                .cookies
                .iter()
                .map(|(k, v)| k.len() + v.len() + 2)
                .sum::<usize>()
                > MAX_COOKIE_BYTES
            {
                self.cookies.clear();
                return Err(AuthHttpError::SizeLimit);
            }
        }
        Ok(())
    }
}

/// 生产构建唯一使用平台证书验证器；`cfg(test)` 下允许测试按实例注入显式根集合，
/// 用于本地合成 TLS 夹具。注入根仍执行完整链与主机名校验，不提供任何禁用校验的开关，
/// 也不使用全局或线程局部测试配置。
fn client_config(options: &AuthClientOptions) -> Result<rustls::ClientConfig, AuthHttpError> {
    // 正式构建里 `test_trust` 字段不存在；显式忽略该参数，避免“未使用”警告掩盖意图。
    #[cfg(not(test))]
    let _ = options;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    #[cfg(test)]
    if let Some(roots) = &options.test_trust {
        return Ok(rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| AuthHttpError::Tls)?
            .with_root_certificates((**roots).clone())
            .with_no_client_auth());
    }
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
        .map_err(|_| AuthHttpError::Tls)?;
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| AuthHttpError::Tls)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

fn sensitive_header(value: &str) -> Result<HeaderValue, AuthHttpError> {
    let mut header = HeaderValue::from_str(value).map_err(|_| AuthHttpError::Protocol)?;
    header.set_sensitive(true);
    Ok(header)
}
fn cookie_pair(value: &str) -> Result<(&str, &str), AuthHttpError> {
    let (name, value) = value.split_once('=').ok_or(AuthHttpError::InvalidCookie)?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || !value.bytes().all(|b| {
            b == 0x21
                || (0x23..=0x2b).contains(&b)
                || (0x2d..=0x3a).contains(&b)
                || (0x3c..=0x5b).contains(&b)
                || (0x5d..=0x7e).contains(&b)
        })
    {
        return Err(AuthHttpError::InvalidCookie);
    }
    Ok((name, value))
}
fn decode<T: serde::de::DeserializeOwned>(raw: &[u8]) -> Result<T, AuthHttpError> {
    #[derive(Deserialize)]
    struct Status {
        code: i64,
    }
    #[derive(Deserialize)]
    struct Envelope<T> {
        data: T,
    }
    let status: Status = serde_json::from_slice(raw).map_err(|_| AuthHttpError::Protocol)?;
    if status.code != 0 {
        return Err(AuthHttpError::Rejected { code: status.code });
    }
    let result: Envelope<T> = serde_json::from_slice(raw).map_err(|_| AuthHttpError::Protocol)?;
    Ok(result.data)
}

fn validate_resource(raw: &[u8]) -> Result<(), AuthHttpError> {
    #[derive(Deserialize)]
    struct AppList {
        code: i64,
    }
    #[derive(Deserialize)]
    struct Resources {
        #[serde(rename = "appList")]
        app_list: AppList,
    }
    let data: Resources = decode(raw)?;
    if data.app_list.code != 0 {
        return Err(AuthHttpError::Rejected {
            code: data.app_list.code,
        });
    }
    Ok(())
}

async fn send_http<S>(
    stream: S,
    request: Request<Full<Bytes>>,
    limit: usize,
) -> Result<(HeaderMap, Zeroizing<Vec<u8>>), AuthHttpError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(64 * 1024)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| AuthHttpError::Http)?;
    // 不分离后台任务：超时/取消 drop 同时关闭 HTTP 驱动和注入流。
    let receive = async {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| AuthHttpError::Http)?;
        if !response.status().is_success() {
            return Err(AuthHttpError::HttpStatus {
                status: response.status().as_u16(),
            });
        }
        let (parts, mut body) = response.into_parts();
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| AuthHttpError::Http)?;
            if let Some(data) = frame.data_ref() {
                if data.len() > limit.saturating_sub(bytes.len()) {
                    return Err(AuthHttpError::SizeLimit);
                }
                bytes.extend_from_slice(data);
            }
        }
        Ok((parts.headers, bytes))
    };
    tokio::pin!(connection);
    tokio::pin!(receive);
    tokio::select! { biased;
        result = &mut receive => result,
        result = &mut connection => { result.map_err(|_| AuthHttpError::Http)?; receive.await }
    }
}
