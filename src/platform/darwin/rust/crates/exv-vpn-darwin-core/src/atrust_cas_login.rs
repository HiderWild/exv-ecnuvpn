//! Darwin 连接前 HTTP/CAS 会话复用；只使用已有学校 Cookie，不接收账密或提交学校表单。
//! 出站仅经调用方注入的 Underlay；本模块不创建窗口、不管理连接提交、不自动重试。

pub use crate::atrust_cas_facts::SSO_FORM_MARKER;
use crate::atrust_cas_facts::{
    AbsoluteUrl, CasFactsError, CasHandover, absorb_set_cookie, build_report_env_body,
    build_sid_ticket_body, cookie_header, extract_cas_handover, is_sso_login_form,
    parse_absolute_url, parse_auth_check_sid_ticket, parse_auth_config, parse_gateway_status,
    resolve_location,
};
use crate::sso_session_jar::SsoSessionJar;
use base64::Engine as _;
use exv_vpn_atrust::Secret;
use exv_vpn_atrust::auth::{AuthHttpError, SessionExchange};
use exv_vpn_atrust::auth::client_identity::{AuthClientIdentity, PLAIN_CHROME_USER_AGENT};
use exv_vpn_atrust::auth::origin::GatewayOrigin;
use exv_vpn_atrust::transport::{ConnectPurpose, OutboundTarget, Underlay};
use http::{HeaderMap, HeaderValue, Method};
use http_body_util::BodyExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// 单请求超时：覆盖拨号、TLS、响应头与正文。
pub const CAS_REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// headless 段总预算（计划 §二 ⑤：全程 ~10s；窗口段沿用旧线 300s 由上层掌握）。
pub const CAS_TOTAL_BUDGET: Duration = Duration::from_secs(10);
/// 重定向链最大跳数（门禁实验链 4 跳，保守放宽到 6；超限视为异常）。
pub const MAX_REDIRECT_HOPS: usize = 6;
/// 网关环境上报端点（浏览器身份链：query 含 `lang`，见 [`AuthClientIdentity::protocol_query`]）。
pub const REPORT_ENV_PATH: &str = "/controller/v1/public/reportEnv";
/// 网关会话兑换端点（响应经 [`SessionExchange::parse`] 解析）。
pub const SESSION_ID_EXCHANGE_PATH: &str = "/passport/v1/public/sessionIdExchange";
/// 响应正文上限（SSO 登录页实测 ~13KB；8 MiB 与 `AuthClientOptions` 默认同值）。
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 结果与错误分类（全部脱敏：不含票据、cookie、deviceId、正文）
// ---------------------------------------------------------------------------

/// 无头链失败分类：稳定码 + 可选 HTTP 状态；与 `ProbeError` 同一脱敏约定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CasNetError {
    /// 稳定分类码（日志白名单词汇）。
    pub code: &'static str,
    /// HTTP 非 2xx/3xx 时的状态码；其它失败为 `None`。
    pub http_status: Option<u16>,
}

impl CasNetError {
    /// 供日志使用的稳定标签：HTTP 状态优先，否则分类码。
    #[must_use]
    pub fn label(self) -> String {
        self.http_status
            .map_or_else(|| self.code.to_owned(), |status| format!("http:{status}"))
    }

    const fn new(code: &'static str) -> Self {
        Self {
            code,
            http_status: None,
        }
    }

    fn tls_io(operation: &'static str, error: &std::io::Error) -> Self {
        Self::tls_failure(AuthHttpError::tls_io(operation, error))
    }

    fn tls_config(operation: &'static str, error: &rustls::Error) -> Self {
        Self::tls_failure(AuthHttpError::tls_config(operation, error))
    }

    fn tls_failure(error: AuthHttpError) -> Self {
        if let Some(diagnostic) = error.transport_diagnostic() {
            log::warn!(target: "exv.atrust_auth", "cas.transport {diagnostic}");
        }
        Self::new("tls")
    }

    const fn status(code: &'static str, status: u16) -> Self {
        Self {
            code,
            http_status: Some(status),
        }
    }
}

/// 无头 CAS 链结果。表单分类发生在核销之前；网络错误也可发生在核销期间。
/// 上层使用分类决定交互回退，不能将错误解释成票据尚未被使用。
pub enum CasOutcome {
    /// 快路径成功：预登录票据（已被本链核销，单次有效）+ authConfig 的 CSRF +
    /// `authCheck → sessionIdExchange` 的会话产物（`data.sid` 等；见
    /// [`SessionExchange`]）。字段值均为敏感材料：不落日志，`Debug` 只显示存在性。
    Ticket {
        /// shortcut.html 交接 URL 抽出的预登录票据（`<unitid>_<guid>` 形态）。
        ticket: Secret,
        /// authConfig 下发的 `security.csrfToken`（后续网关会话操作需要）。
        csrf: Secret,
        /// `authCheck code=0` 后经 `sessionIdExchange` 取得的会话集。
        sid_via_auth_check: Option<SessionExchange>,
    },
    /// SSO 会话过期：SSO 返回 200 登录表单页（含 [`SSO_FORM_MARKER`]）。
    /// 零核销、零凭据 POST；上层静默转内嵌窗口线自动填账密。
    FormRequired,
    /// 网络、协议或预算错误；可能发生在票据核销中，不据此重试同一票据。
    NetError(CasNetError),
}

impl CasOutcome {
    /// 是否应静默转交互窗口线：`FormRequired` 与**全部** [`CasOutcome::NetError`]
    /// 都是 fail-safe 交互类（真失败分类与处置由窗口线承担）。
    #[must_use]
    pub fn requires_interactive(&self) -> bool {
        !matches!(self, Self::Ticket { .. })
    }

    /// 供日志使用的稳定分类标签（不含任何值）。
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ticket { .. } => "ticket",
            Self::FormRequired => "form_required",
            Self::NetError(_) => "net_error",
        }
    }
}

// Debug 只显示存在性/分类，不显示票据与 CSRF 值（SessionExchange 自身已是脱敏 Debug）。
impl std::fmt::Debug for CasOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ticket {
                ticket,
                csrf,
                sid_via_auth_check,
            } => f
                .debug_struct("CasOutcome::Ticket")
                .field("ticket_present", &!ticket.is_empty())
                .field("csrf_len", &csrf.len())
                .field("sid_via_auth_check", &sid_via_auth_check)
                .finish(),
            Self::FormRequired => f.write_str("CasOutcome::FormRequired"),
            Self::NetError(error) => f.debug_tuple("CasOutcome::NetError").field(error).finish(),
        }
    }
}

// ---------------------------------------------------------------------------
/// 无头 CAS 链预算与身份选项。UA 默认 [`PLAIN_CHROME_USER_AGENT`]（门禁实验配方）；
/// 超时默认 [`CAS_REQUEST_TIMEOUT`]，总预算默认 [`CAS_TOTAL_BUDGET`]。
#[derive(Debug, Clone)]
pub struct CasLoginOptions {
    pub user_agent: String,
    /// 单请求超时（拨号 + TLS + 响应头/正文）。
    pub timeout: Duration,
    /// 整链总预算（超预算即 fail-safe 终止，不再发起后续请求）。
    pub budget: Duration,
    /// 响应正文上限。
    pub max_response_bytes: usize,
    /// 仅本 crate 单测（`cfg(test)`）可用的按实例测试信任注入：注入根仍执行完整
    /// 证书链与主机名校验，不提供禁用校验的开关。
    #[cfg(test)]
    pub(crate) test_trust: Option<Arc<rustls::RootCertStore>>,
}

impl Default for CasLoginOptions {
    fn default() -> Self {
        Self {
            user_agent: PLAIN_CHROME_USER_AGENT.to_owned(),
            timeout: CAS_REQUEST_TIMEOUT,
            budget: CAS_TOTAL_BUDGET,
            max_response_bytes: MAX_RESPONSE_BYTES,
            #[cfg(test)]
            test_trust: None,
        }
    }
}

/// 一条响应的最小承载：状态、头部（供 cookie 吸收与 Location 读取）与正文。
struct CasHttpResponse {
    status: u16,
    headers: HeaderMap,
    body: Zeroizing<Vec<u8>>,
}

impl CasHttpResponse {
    fn location(&self) -> Option<Zeroizing<String>> {
        self.headers
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(|value| Zeroizing::new(value.to_owned()))
    }
}

/// 无头 CAS 快路径客户端：固定网关 + 学校 SSO 双 origin，定向跟随重定向并完成
/// 一次核销。每次 [`CasLoginClient::login`] 是完整一轮（authConfig → 链 → 消费），
/// 内部零重试；票据单次有效，失败后重跑必须重新调用（§二点五.1/2）。
pub struct CasLoginClient {
    host: String,
    authority: String,
    port: u16,
    /// `x-sdp-rid = base64(网关主机名)`，与官方客户端一致，构造期固定。
    sdp_rid: String,
    identity: AuthClientIdentity,
    underlay: Arc<dyn Underlay>,
    options: CasLoginOptions,
    /// 网关 origin 的根路径 cookie（链路中由 Set-Cookie 累积；SSO jar 由调用方持有）。
    gateway_cookies: BTreeMap<String, Secret>,
}

impl CasLoginClient {
    /// 以选定的网关与注入出口构造。身份固定为浏览器路径
    /// （`SDPBrowserClient`/`Mac`/`zh-CN`），UA 由 `options.user_agent` 提供（默认普通 Chrome）。
    /// # Errors
    /// 网关端点或选项非法返回 [`CasNetError`]（`config`）。
    pub fn new(
        gateway: &str,
        underlay: Arc<dyn Underlay>,
        options: CasLoginOptions,
    ) -> Result<Self, CasNetError> {
        let origin = GatewayOrigin::parse(gateway).map_err(|_| CasNetError::new("config"))?;
        if options.timeout.is_zero()
            || options.budget.is_zero()
            || options.timeout > CAS_REQUEST_TIMEOUT
            || options.budget > CAS_TOTAL_BUDGET
            || options.max_response_bytes == 0
            || options.user_agent.len() > 4096
            || HeaderValue::from_str(&options.user_agent).is_err()
        {
            return Err(CasNetError::new("config"));
        }
        let host = origin.host().to_owned();
        let port = origin.port();
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let sdp_rid = base64::engine::general_purpose::STANDARD.encode(host.as_bytes());
        Ok(Self {
            host,
            authority,
            port,
            sdp_rid,
            identity: AuthClientIdentity::browser("Mac", "zh-CN")
                .map_err(|_| CasNetError::new("config"))?,
            underlay,
            options,
            gateway_cookies: BTreeMap::new(),
        })
    }

    /// 执行一轮完整无头链。`sso_profile` 提供快路径所需的 SSO cookie 与 `deviceId`，
    /// 链路中 SSO origin 的 Set-Cookie 会**直接并入**该档案（调用方随后自行
    /// `sso_session_jar::save` 落盘）。返回值见 [`CasOutcome`]；本方法零重试，
    /// 分类先于核销。
    pub async fn login(&mut self, sso_profile: &mut SsoSessionJar) -> CasOutcome {
        let started = Instant::now();
        let deadline = started + self.options.budget;
        // 新一轮只采用本轮网关响应，不能让上次兑换的 Cookie 补足本次空响应。
        self.gateway_cookies.clear();
        let outcome = self.login_inner(sso_profile, deadline).await;
        // 日志只记分类与耗时（§二 ⑥）：不含票据、cookie、deviceId 或正文值。
        let detail = match &outcome {
            CasOutcome::NetError(error) => error.label(),
            _ => outcome.label().to_owned(),
        };
        log::info!(
            target: "exv.atrust_cas",
            "cas.login outcome={} detail={} elapsed_ms={}",
            outcome.label(),
            detail,
            started.elapsed().as_millis()
        );
        outcome
    }

    async fn login_inner(
        &mut self,
        sso_profile: &mut SsoSessionJar,
        deadline: Instant,
    ) -> CasOutcome {
        // 步骤 1：authConfig（浏览器身份 + needTicket=1）取 CSRF 与 firstAuth 入口。
        let config_path = self.identity.auth_config_path(true);
        let config = match self
            .gateway_json(RawRequest::get(&config_path), deadline)
            .await
        {
            Ok(body) => match parse_auth_config(&body) {
                Ok(config) => config,
                Err(error) => return CasOutcome::NetError(error.into()),
            },
            Err(error) => return CasOutcome::NetError(error),
        };
        let csrf = config.csrf;
        let first_auth = match config.first_auth {
            Some(entry) if !entry.is_empty() => Zeroizing::new(entry),
            _ => return CasOutcome::NetError(CasNetError::new("protocol")),
        };

        // 步骤 2：firstAuth → 定向跟随重定向链，先分类（票据/表单/异常），后消费。
        let handover = match self.follow_chain(&first_auth, sso_profile, deadline).await {
            Ok(Some(handover)) => handover,
            Ok(None) => return CasOutcome::FormRequired,
            Err(error) => return CasOutcome::NetError(error),
        };
        // 分类已定：此刻起票据进入消费链（单次，不重试）。

        // 步骤 3：reportEnv（无签名；浏览器身份仅带 csrf + trace + rid，§二点八配方）。
        let device_id = Zeroizing::new(sso_profile.device_id().to_owned());
        let Ok(body) = build_report_env_body(handover.ticket(), &device_id) else {
            return CasOutcome::NetError(CasNetError::new("protocol"));
        };
        let report_path = format!("{REPORT_ENV_PATH}?{}", self.identity.protocol_query());
        let report = match self
            .gateway_json(
                RawRequest::post(&report_path).csrf(&csrf).body(body),
                deadline,
            )
            .await
        {
            Ok(report) => report,
            Err(error) => return CasOutcome::NetError(error),
        };
        if let Err(error) = parse_gateway_status(&report) {
            return CasOutcome::NetError(error.into());
        }

        // 步骤 4：authCheck → 期待 code=0 且 data.sidTicket（可省 ticketExchange，§二点八.3）。
        let check_path = self.identity.auth_check_path();
        let check = match self
            .gateway_json(RawRequest::get(&check_path), deadline)
            .await
        {
            Ok(body) => body,
            Err(error) => return CasOutcome::NetError(error),
        };
        let sid_ticket = match parse_auth_check_sid_ticket(&check) {
            Ok(ticket) => ticket,
            Err(error) => return CasOutcome::NetError(error.into()),
        };

        // 步骤 5：sessionIdExchange → 会话集（复用既有 SessionExchange 解析）。
        let Ok(exchange_body) = build_sid_ticket_body(&sid_ticket) else {
            return CasOutcome::NetError(CasNetError::new("protocol"));
        };
        let exchange_path = format!(
            "{SESSION_ID_EXCHANGE_PATH}?{}",
            self.identity.protocol_query()
        );
        let exchange = match self
            .gateway_json(
                RawRequest::post(&exchange_path)
                    .csrf(&csrf)
                    .body(exchange_body),
                deadline,
            )
            .await
        {
            Ok(body) => body,
            Err(error) => return CasOutcome::NetError(error),
        };
        // 浏览器配方的会话落地是双通道（2026-10-07 真机实证）：官方客户端上下文把
        // sid 集放在响应体；浏览器上下文响应体只有 onlineInfo，sid/sid.sig 等经
        // Set-Cookie 下发（gateway_json 已吸收进 gateway_cookies）。体解析为空时
        // 从网关 jar 构造，两通道都空则保持空集由上层 fail-safe 回退。
        if let Err(error) = parse_gateway_status(&exchange) {
            return CasOutcome::NetError(error.into());
        }
        let mut session = match SessionExchange::parse(&exchange) {
            Ok(parsed) => parsed,
            Err(_) => SessionExchange::default(),
        };
        if session.is_empty() {
            session = self.session_from_gateway_cookies();
        }
        CasOutcome::Ticket {
            ticket: Secret::new(handover.ticket().expose()),
            csrf: Secret::new(csrf.expose()),
            sid_via_auth_check: Some(session),
        }
    }

    /// 从网关 cookie jar 构造会话集（浏览器上下文的 Set-Cookie 通道；只挑会话键，
    /// 其余网关 cookie 不入会话）。
    fn session_from_gateway_cookies(&self) -> SessionExchange {
        fn take(jar: &BTreeMap<String, Secret>, name: &str) -> Option<Secret> {
            jar.get(name)
                .filter(|value| !value.expose().is_empty())
                .map(|value| Secret::new(value.expose()))
        }
        SessionExchange {
            sid: take(&self.gateway_cookies, "sid"),
            sid_sig: take(&self.gateway_cookies, "sid.sig"),
            sid_legacy: take(&self.gateway_cookies, "sid-legacy"),
            sid_legacy_sig: take(&self.gateway_cookies, "sid-legacy.sig"),
            straceid: take(&self.gateway_cookies, "straceid"),
            straceid_sig: take(&self.gateway_cookies, "straceid.sig"),
        }
    }

    /// 定向跟随重定向链（≤ [`MAX_REDIRECT_HOPS`] 跳，origin 白名单=网关+SSO）。
    /// `Ok(Some(handover))` = 拿到票据；`Ok(None)` = SSO 表单页（FormRequired）；
    /// `Err` = fail-safe 网络分类。链上每一跳：发送对应 origin 的 cookie、吸收
    /// `Set-Cookie`（SSO 并入档案）、先查 `Location` 是否已是交接 URL（拿到即停，
    /// 不再请求 shortcut.html 页面本体——票据已在 URL 内，省一次往返）。
    async fn follow_chain(
        &mut self,
        first_auth: &str,
        sso_profile: &mut SsoSessionJar,
        deadline: Instant,
    ) -> Result<Option<CasHandover>, CasNetError> {
        let mut current = if first_auth.starts_with('/') && !first_auth.starts_with("//") {
            AbsoluteUrl {
                host: self.host.clone(),
                port: self.port,
                path_and_query: first_auth.to_owned(),
            }
        } else {
            let url = parse_absolute_url(first_auth).ok_or_else(|| CasNetError::new("protocol"))?;
            if url.host != self.host || url.port != self.port {
                return Err(CasNetError::new("redirect_origin"));
            }
            url
        };
        for _ in 0..MAX_REDIRECT_HOPS {
            let response = self.chain_get(&current, sso_profile, deadline).await?;
            if (300..400).contains(&response.status) {
                let location = response
                    .location()
                    .ok_or_else(|| CasNetError::new("redirect_location"))?;
                let next = resolve_location(&location, &current)
                    .ok_or_else(|| CasNetError::new("redirect_location"))?;
                if !self.origin_allowed(&next) {
                    return Err(CasNetError::new("redirect_origin"));
                }
                if next.host == self.host
                    && next.port == self.port
                    && let Some(handover) = extract_cas_handover(&next.path_and_query)
                {
                    return Ok(Some(handover));
                }
                current = next;
                continue;
            }
            match response.status {
                200 => {
                    if is_sso_login_form(&response.body) {
                        return Ok(None);
                    }
                    // 兜底：响应本体 URL 即交接页（正常链在 Location 已停）。
                    let url = Zeroizing::new(format!(
                        "https://{}{}",
                        current.host, current.path_and_query
                    ));
                    if is_gateway_origin(&current, &self.host, self.port)
                        && let Some(handover) = extract_cas_handover(&url)
                    {
                        return Ok(Some(handover));
                    }
                    return Err(CasNetError::new("unexpected_page"));
                }
                status => return Err(CasNetError::status("http", status)),
            }
        }
        Err(CasNetError::new("redirect_limit"))
    }

    /// 链上一跳 GET：按 origin 选 cookie 源并吸收 Set-Cookie（网关入内部 jar，
    /// SSO 直接并入档案）。SSO origin 是普通浏览器导航：不带网关协议头（`x-sdp-*`）。
    async fn chain_get(
        &mut self,
        url: &AbsoluteUrl,
        sso_profile: &mut SsoSessionJar,
        deadline: Instant,
    ) -> Result<CasHttpResponse, CasNetError> {
        let is_gateway = url.host == self.host && url.port == self.port;
        let cookie = if is_gateway {
            cookie_header(&self.gateway_cookies)
        } else {
            let header = sso_profile.cookie_header();
            (!header.is_empty()).then_some(header)
        };
        let referer = is_gateway.then(|| format!("https://{}/portal/", self.url_host()));
        let response = self
            .raw_request(
                RawRequest::get(&url.path_and_query)
                    .host(&url.host)
                    .port(url.port)
                    .cookie(cookie.as_ref().map(|value| value.as_str()))
                    .referer(referer.as_deref())
                    .gateway_protocol_headers(is_gateway),
                deadline,
            )
            .await?;
        if is_gateway {
            absorb_all_set_cookies(&mut self.gateway_cookies, &response.headers, &self.host);
        } else {
            for header in response.headers.get_all("set-cookie") {
                if let Ok(value) = header.to_str() {
                    sso_profile.merge_set_cookie(value);
                }
            }
        }
        Ok(response)
    }

    /// 网关 JSON 端点（authConfig/reportEnv/authCheck/sessionIdExchange）：浏览器
    /// 身份 + 网关协议头（`x-sdp-rid`/`x-sdp-traceid`）；请求带 CSRF 时附
    /// `x-csrf-token`（reportEnv 与 sessionIdExchange 的配方要求）。非 200 或
    /// 正文超限归为网络分类；业务码解析由调用方负责。
    async fn gateway_json(
        &mut self,
        request: RawRequest<'_>,
        deadline: Instant,
    ) -> Result<Zeroizing<Vec<u8>>, CasNetError> {
        let cookie = cookie_header(&self.gateway_cookies);
        let referer = format!("https://{}/portal/", self.url_host());
        let request = request
            .host(&self.host)
            .port(self.port)
            .cookie(cookie.as_ref().map(|value| value.as_str()))
            .referer(Some(&referer))
            .gateway_protocol_headers(true);
        let response = self.raw_request(request, deadline).await?;
        absorb_all_set_cookies(&mut self.gateway_cookies, &response.headers, &self.host);
        if response.status != 200 {
            return Err(CasNetError::status("http", response.status));
        }
        Ok(response.body)
    }

    /// origin 白名单：网关自身或学校 SSO（固定 443）。
    fn origin_allowed(&self, url: &AbsoluteUrl) -> bool {
        (url.host == self.host && url.port == self.port)
            || (url.host == crate::sso_session_jar::SSO_HOST && url.port == 443)
    }

    /// Referer 用的 `host[:port]`（默认端口省略，与 `AuthClient` 的门户 Referer 同形态）。
    fn url_host(&self) -> String {
        if self.port == 443 {
            self.authority
                .strip_suffix(":443")
                .unwrap_or(&self.authority)
                .to_owned()
        } else {
            self.authority.clone()
        }
    }

    /// 底层 HTTPS 请求：一条请求一条连接（逐请求拨号，与 `AuthClient` 同形态），
    /// 不跟随重定向、不校验业务码；状态/头/正文原样返回，语义由调用方分类。
    async fn raw_request(
        &self,
        request: RawRequest<'_>,
        deadline: Instant,
    ) -> Result<CasHttpResponse, CasNetError> {
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .map(|remaining| remaining.min(self.options.timeout))
            .ok_or_else(|| CasNetError::new("budget"))?;
        let authority = if request.host.contains(':') {
            format!("[{}]:{}", request.host, request.port)
        } else {
            format!("{}:{}", request.host, request.port)
        };
        let mut builder = http::Request::builder()
            .method(request.method)
            .uri(request.path_and_query)
            .header("host", &authority)
            .header("user-agent", &self.options.user_agent)
            // 普通浏览器/Node fetch 的接受形态；不为链路发明专用头。
            .header("accept", "*/*");
        if !request.body.is_empty() {
            builder = builder.header("content-type", "application/json");
        }
        if request.gateway_protocol_headers {
            // 网关 L1 协议身份（门禁实验配方：rid + 每请求随机 traceid）。
            builder = builder
                .header("x-sdp-rid", &self.sdp_rid)
                .header("x-sdp-traceid", random_trace_id());
        }
        if let Some(csrf) = request.csrf {
            let mut header =
                HeaderValue::from_str(csrf.expose()).map_err(|_| CasNetError::new("protocol"))?;
            header.set_sensitive(true);
            builder = builder.header("x-csrf-token", header);
        }
        if let Some(referer) = request.referer {
            builder = builder.header("referer", referer);
        }
        if let Some(cookie) = request.cookie {
            let mut header =
                HeaderValue::from_str(cookie).map_err(|_| CasNetError::new("protocol"))?;
            header.set_sensitive(true);
            builder = builder.header("cookie", header);
        }
        let http_request = builder
            .body(FullBody::new(bytes::Bytes::from_owner(SensitiveBody(
                request.body,
            ))))
            .map_err(|_| CasNetError::new("protocol"))?;
        tokio::time::timeout(timeout, async {
            let stream = self
                .underlay
                .connect(&OutboundTarget::new(
                    request.host,
                    request.port,
                    ConnectPurpose::AuthGateway,
                ))
                .await
                .map_err(|_| CasNetError::new("underlay"))?;
            let config = client_config(&self.options)?;
            let server_name = rustls::pki_types::ServerName::try_from(request.host.to_owned())
                .map_err(|_| CasNetError::new("config"))?;
            let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(server_name, stream)
                .await
                .map_err(|error| CasNetError::tls_io("tls.connect", &error))?;
            send_receive(tls, http_request, self.options.max_response_bytes).await
        })
        .await
        .map_err(|_| CasNetError::new("timeout"))?
    }
}

/// 内部请求描述（把参数收拢成结构，避免长参数列）。
struct RawRequest<'a> {
    method: Method,
    host: &'a str,
    port: u16,
    path_and_query: &'a str,
    cookie: Option<&'a str>,
    referer: Option<&'a str>,
    csrf: Option<&'a Secret>,
    gateway_protocol_headers: bool,
    body: Zeroizing<Vec<u8>>,
}

impl<'a> RawRequest<'a> {
    fn get(path: &'a str) -> Self {
        Self {
            method: Method::GET,
            host: "",
            port: 0,
            path_and_query: path,
            cookie: None,
            referer: None,
            csrf: None,
            gateway_protocol_headers: false,
            body: Zeroizing::new(Vec::new()),
        }
    }

    fn post(path: &'a str) -> Self {
        Self {
            method: Method::POST,
            ..Self::get(path)
        }
    }

    fn host(mut self, host: &'a str) -> Self {
        self.host = host;
        self
    }

    fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    fn cookie(mut self, cookie: Option<&'a str>) -> Self {
        self.cookie = cookie;
        self
    }

    fn referer(mut self, referer: Option<&'a str>) -> Self {
        self.referer = referer;
        self
    }

    fn csrf(mut self, csrf: &'a Secret) -> Self {
        self.csrf = Some(csrf);
        self
    }

    fn gateway_protocol_headers(mut self, enabled: bool) -> Self {
        self.gateway_protocol_headers = enabled;
        self
    }

    fn body(mut self, body: Zeroizing<Vec<u8>>) -> Self {
        self.body = body;
        self
    }
}
/// 生产构建唯一使用平台证书验证器；本 crate 单测允许按实例注入显式根集合（回环
/// 夹具自建 CA）。注入根仍执行完整链与主机名校验，无禁用校验的开关。
fn client_config(options: &CasLoginOptions) -> Result<rustls::ClientConfig, CasNetError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    #[cfg(test)]
    if let Some(roots) = &options.test_trust {
        return Ok(rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| CasNetError::tls_config("tls.config", &error))?
            .with_root_certificates((**roots).clone())
            .with_no_client_auth());
    }
    #[cfg(not(test))]
    let _ = &options;
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
        .map_err(|error| CasNetError::tls_config("tls.verifier", &error))?;
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| CasNetError::tls_config("tls.config", &error))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// 官方形态的 `x-sdp-traceid`：8 字节随机数的 16 位小写十六进制（每请求新值）。
fn random_trace_id() -> String {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        // 随机源不可用：回退纳秒时间戳（仅请求关联 id，非凭据）。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
            });
        bytes = nanos.to_le_bytes();
    }
    let mut trace_id = String::with_capacity(16);
    for byte in bytes {
        trace_id.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        trace_id.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    trace_id
}

// hyper 正文承载：与 exv-vpn-atrust 的 send_http 同形态。
type FullBody = http_body_util::Full<bytes::Bytes>;

/// 发送一条 HTTP/1.1 请求并读完响应（不判成功状态、不跟随重定向；状态/头/正文
/// 由调用方分类）。取消本 future 即同时释放 HTTP 驱动与注入流。
async fn send_receive<S>(
    stream: S,
    request: http::Request<FullBody>,
    limit: usize,
) -> Result<CasHttpResponse, CasNetError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(64 * 1024)
        .handshake(hyper_util::rt::TokioIo::new(stream))
        .await
        .map_err(|_| CasNetError::new("http"))?;
    let receive = async {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| CasNetError::new("http"))?;
        let (parts, mut body) = response.into_parts();
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| CasNetError::new("http"))?;
            if let Some(data) = frame.data_ref() {
                if data.len() > limit.saturating_sub(bytes.len()) {
                    return Err(CasNetError::new("size_limit"));
                }
                bytes.extend_from_slice(data);
            }
        }
        Ok::<CasHttpResponse, CasNetError>(CasHttpResponse {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body: bytes,
        })
    };
    tokio::pin!(connection);
    tokio::pin!(receive);
    tokio::select! { biased;
        result = &mut receive => result,
        result = &mut connection => { result.map_err(|_| CasNetError::new("http"))?; receive.await }
    }
}

/// 请求体交给 hyper 时继续持有零化缓冲，避免复制到普通 Bytes 所有者。
struct SensitiveBody(Zeroizing<Vec<u8>>);
impl AsRef<[u8]> for SensitiveBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

fn is_gateway_origin(url: &AbsoluteUrl, host: &str, port: u16) -> bool {
    url.host == host && url.port == port
}

fn absorb_all_set_cookies(jar: &mut BTreeMap<String, Secret>, headers: &HeaderMap, host: &str) {
    for header in headers.get_all("set-cookie") {
        if let Ok(value) = header.to_str() {
            absorb_set_cookie(jar, value, host);
        }
    }
}

impl From<CasFactsError> for CasNetError {
    fn from(error: CasFactsError) -> Self {
        Self::new(match error {
            CasFactsError::Protocol => "protocol",
            CasFactsError::Rejected => "rejected",
            CasFactsError::AlreadyLoggedIn => "already_login",
            CasFactsError::MissingCsrf => "csrf",
        })
    }
}
