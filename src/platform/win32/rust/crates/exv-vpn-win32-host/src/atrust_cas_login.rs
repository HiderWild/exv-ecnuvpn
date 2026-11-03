
use crate::sso_session_jar::SsoSessionJar;
use exv_vpn_atrust::auth::client_identity::{AuthClientIdentity, PLAIN_CHROME_USER_AGENT};
use exv_vpn_atrust::auth::origin::GatewayOrigin;
use exv_vpn_atrust::auth::{AuthHttpError, SessionExchange};
use exv_vpn_atrust::transport::{ConnectPurpose, OutboundTarget, Underlay};
use base64::Engine as _;
use exv_vpn_atrust::Secret;
use http::{HeaderMap, HeaderValue, Method};
use http_body_util::BodyExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// 单请求超时：覆盖拨号、TLS、响应头与正文；与 `atrust_session_probe::PROBE_TIMEOUT` 同值。
pub const CAS_REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// headless 段总预算（计划 §二 ⑤：全程 ~10s；窗口段沿用旧线 300s 由上层掌握）。
pub const CAS_TOTAL_BUDGET: Duration = Duration::from_secs(10);
/// 重定向链最大跳数（门禁实验链 4 跳，保守放宽到 6；超限视为异常）。
pub const MAX_REDIRECT_HOPS: usize = 6;
/// SSO 登录表单页标记：`id="login-page-flowkey"`（数据链取证 §一.3，服务端渲染、无 JS）。
pub const SSO_FORM_MARKER: &str = "login-page-flowkey";
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
        self.http_status.map_or_else(
            || self.code.to_owned(),
            |status| format!("http:{status}"),
        )
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

/// 无头 CAS 链结果。**分类先于核销**：只有 [`CasOutcome::Ticket`] 意味着票据已被
/// 消费链使用；其余两类都零核销调用，上层应静默转窗口线（§二 v3 路由 ②→③）。
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
    /// 网络/协议/预算失败（4xx、未知重定向 origin、跳数超限、超时等）。零核销；
    /// 按 §二点五.4 fail-safe，与 `FormRequired` 同样静默转窗口线。
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
// URL / 表单 / 票据交接解析（纯函数，宿主 crate 级唯一实现；
// `tauri/app` 内嵌线的同名解析后续由其归属阶段改为委托本实现，不另存副本）
// ---------------------------------------------------------------------------

/// 绝对 https URL 的最小解析产物（只取链路跟随所需字段；不引入 `url` 依赖，
/// 与 `school_auto_login::gateway_origin` 的既有做法一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct AbsoluteUrl {
    host: String,
    port: u16,
    path_and_query: String,
}

/// 解析绝对 `https://host[:port]/path[?query]` URL；非 https、无 host 拒绝。
fn parse_absolute_url(url: &str) -> Option<AbsoluteUrl> {
    let rest = url.strip_prefix("https://")?;
    let (authority, path_and_query) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        // IPv6 字面量不在本链路域内（网关与 SSO 都是域名）；authority 含多冒号拒绝。
        Some((host, port)) if !host.contains(':') => (host, port.parse::<u16>().ok()?),
        _ if authority.contains(':') => return None,
        _ => (authority, 443),
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some(AbsoluteUrl {
        host: host.to_ascii_lowercase(),
        port,
        path_and_query: path_and_query.to_owned(),
    })
}

/// query 值的百分号解码（`%XX` 与 `+`→空格；非法 `%` 序列保持原样），语义与
/// `url` crate 的 `query_pairs()` 一致。
fn decode_query_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let hex_digit = |byte: u8| match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    };
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%'
                if index + 2 < bytes.len()
                    && bytes[index + 1].is_ascii_hexdigit()
                    && bytes[index + 2].is_ascii_hexdigit() =>
            {
                let high = hex_digit(bytes[index + 1]);
                let low = hex_digit(bytes[index + 2]);
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// 解析 query（`?` 后、`#` 前）为解码后的键值对。
fn query_pairs_of(url: &str) -> Vec<(String, String)> {
    let query = url.split_once('?').map_or("", |(_, rest)| rest);
    let query = query.split('#').next().unwrap_or(query);
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (decode_query_value(key), decode_query_value(value)),
            None => (decode_query_value(pair), String::new()),
        })
        .collect()
}

/// CAS 票据交接页解析产物（`/portal/shortcut.html?source=auth_cas` 的 `data` 与
/// `nextService`）。`ticket` 是单次敏感材料：仅内存持有，`Debug` 只显示存在性。
pub struct CasHandover {
    ticket: Secret,
    timing: Option<String>,
    env_need: bool,
    next_service: Option<String>,
}

impl CasHandover {
    /// 预登录票据（`<unitid>_<guid>` 形态）；调用点保证不落日志。
    #[must_use]
    pub fn ticket(&self) -> &Secret {
        &self.ticket
    }

    /// `data.env.timing`（官方样本 `pre-login`）；缺省为 `None`。
    #[must_use]
    pub fn timing(&self) -> Option<&str> {
        self.timing.as_deref()
    }

    /// `data.env.need`（缺省 `false`）。
    #[must_use]
    pub fn env_need(&self) -> bool {
        self.env_need
    }

    /// `nextService`（官方样本 `auth/authCheck`）；缺省为 `None`。
    #[must_use]
    pub fn next_service(&self) -> Option<&str> {
        self.next_service.as_deref()
    }
}

// 票据是敏感材料：Debug 只显示存在性与非敏感字段。
impl std::fmt::Debug for CasHandover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CasHandover")
            .field("ticket_present", &!self.ticket.is_empty())
            .field("timing", &self.timing)
            .field("env_need", &self.env_need)
            .field("next_service", &self.next_service)
            .finish()
    }
}

/// 解析 CAS 票据交接页 URL（逻辑自 `tauri/app` 内嵌线的同名纯函数上移为宿主 crate
/// 级唯一实现；证据形态见 `docs/research/atrust/2026-10-05-cas-ticket-must-be-consumed-by-client.md`
/// §2.2 与 2026-10-07 门禁实验）：网关把预登录票据交回浏览器
/// `/portal/shortcut.html?source=auth_cas&data={"ticket":…,"env":{"need":…,"timing":…}}&nextService=auth/authCheck`。
///
/// 只认 `source=auth_cas` 且 `data` JSON 内含非空 `ticket` 的形态（`source=client` 是
/// 官方客户端入口，不在此列）；`data` 的百分号编码形态与直书 JSON 形态均可解析。
/// 只做 URL 级解析，不发任何请求。
#[must_use]
pub fn extract_cas_handover(url: &str) -> Option<CasHandover> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if !path.to_ascii_lowercase().contains("/portal/shortcut.html") {
        return None;
    }
    let mut source: Option<String> = None;
    let mut data: Option<String> = None;
    let mut next_service: Option<String> = None;
    for (key, value) in query_pairs_of(url) {
        match key.as_str() {
            "source" => source = Some(value),
            "data" => data = Some(value),
            "nextService" => next_service = Some(value),
            _ => {}
        }
    }
    if source.as_deref() != Some("auth_cas") {
        return None;
    }
    let payload: serde_json::Value = serde_json::from_str(&data?).ok()?;
    let ticket = payload
        .get("ticket")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|ticket| !ticket.is_empty())
        .map(str::to_owned)?;
    let env = payload.get("env");
    let env_need = env
        .and_then(|env| env.get("need"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let timing = env
        .and_then(|env| env.get("timing"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some(CasHandover {
        ticket: Secret::new(ticket),
        timing,
        env_need,
        next_service,
    })
}

/// SSO 200 响应是否为登录表单页（服务端渲染的 `id="login-page-flowkey"` 标记；
/// 匹配裸标记以容忍引号/空白形态）。这是 [`CasOutcome::FormRequired`] 的唯一判据。
#[must_use]
pub fn is_sso_login_form(body: &[u8]) -> bool {
    // 页面是 UTF-8 HTML；标记是纯 ASCII，字节级子串匹配即可，避免整页解码。
    body.windows(SSO_FORM_MARKER.len())
        .any(|window| window == SSO_FORM_MARKER.as_bytes())
}

/// 解析 `Location` 头并相对当前 URL 得到下一跳：支持绝对 `https://` URL 与
/// 绝对路径（`/passport/...`，门禁实验链的实际形态）；协议相对（`//host/..`）、
/// 相对路径或非 https 一律拒绝（fail-safe）。
fn resolve_location(location: &str, current: &AbsoluteUrl) -> Option<AbsoluteUrl> {
    if location.starts_with("https://") {
        parse_absolute_url(location)
    } else if location.starts_with('/') && !location.starts_with("//") {
        Some(AbsoluteUrl {
            host: current.host.clone(),
            port: current.port,
            path_and_query: location.to_owned(),
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// cookie 域规则（网关 jar 与 SSO jar 共用；与 AuthClient::update_cookies 同语义）
// ---------------------------------------------------------------------------

/// 校验并插入一枚 cookie 名值对；非法字符返回 `Err`（不修改 jar）。
pub(crate) fn apply_set_cookie_pair(
    jar: &mut BTreeMap<String, Secret>,
    name: &str,
    value: &str,
) -> Result<(), ()> {
    let name_valid = |byte: u8| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
    let value_valid = |byte: u8| {
        byte == 0x21
            || (0x23..=0x2b).contains(&byte)
            || (0x2d..=0x3a).contains(&byte)
            || (0x3c..=0x5b).contains(&byte)
            || (0x5d..=0x7e).contains(&byte)
    };
    if name.is_empty()
        || !name.bytes().all(name_valid)
        || !value.bytes().all(value_valid)
        || name.len() + value.len() + 2 > 32 * 1024
    {
        return Err(());
    }
    jar.insert(name.to_owned(), Secret::new(value));
    Ok(())
}

/// 吸收一条 `Set-Cookie` 头到指定 host 的 jar：跨域（`Domain` 非本域）、非根路径
/// 忽略；空值或 `Max-Age<=0` 删除；非法字符静默忽略（cookie 是附带状态，忽略不劣于
/// 没有，链路结果由上层分类兜底）。
pub(crate) fn absorb_set_cookie(
    jar: &mut BTreeMap<String, Secret>,
    header_value: &str,
    host: &str,
) {
    let mut parts = header_value.split(';');
    let Some(pair) = parts.next() else {
        return;
    };
    let Some((name, value)) = pair.trim().split_once('=') else {
        return;
    };
    let mut delete = value.is_empty();
    let mut applicable = true;
    for attribute in parts {
        let (key, val) = attribute
            .trim()
            .split_once('=')
            .unwrap_or((attribute.trim(), ""));
        if key.eq_ignore_ascii_case("domain")
            && !val.trim_start_matches('.').eq_ignore_ascii_case(host)
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
    if !applicable {
        return;
    }
    if delete {
        jar.remove(name);
    } else if apply_set_cookie_pair(jar, name, value).is_err() {
        // 非法字符：忽略该枚 cookie，不影响链路。
    }
}

/// 渲染 Cookie 请求头（BTreeMap 序）；零化容器持有，空 jar 返回 `None`。
fn cookie_header(jar: &BTreeMap<String, Secret>) -> Option<Zeroizing<String>> {
    if jar.is_empty() {
        return None;
    }
    let mut header = Zeroizing::new(String::new());
    for (name, value) in jar {
        if !header.is_empty() {
            header.push_str("; ");
        }
        header.push_str(name);
        header.push('=');
        header.push_str(value.expose());
    }
    Some(header)
}

fn absorb_all_set_cookies(jar: &mut BTreeMap<String, Secret>, headers: &HeaderMap, host: &str) {
    for header in headers.get_all("set-cookie") {
        if let Ok(value) = header.to_str() {
            absorb_set_cookie(jar, value, host);
        }
    }
}

// ---------------------------------------------------------------------------
// 客户端
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
    fn location(&self) -> Option<String> {
        self.headers
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
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
    /// （`SDPBrowserClient`/`Windows`/`zh-CN`，与 `atrust_browser::client_identity`
    /// 同源），UA 由 `options.user_agent` 提供（默认普通 Chrome）。
    /// # Errors
    /// 网关端点或选项非法返回 [`CasNetError`]（`config`）。
    pub fn new(
        gateway: &str,
        underlay: Arc<dyn Underlay>,
        options: CasLoginOptions,
    ) -> Result<Self, CasNetError> {
        let origin = GatewayOrigin::parse(gateway).map_err(|_| CasNetError::new("config"))?;
        if options.timeout.is_zero()
            || options.budget < options.timeout
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
            identity: crate::atrust_browser::client_identity(),
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
                Err(error) => return CasOutcome::NetError(error),
            },
            Err(error) => return CasOutcome::NetError(error),
        };
        let csrf = config.csrf;
        let first_auth = match config.first_auth {
            Some(entry) if !entry.is_empty() => entry,
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
        if let Err(error) = self
            .gateway_json(RawRequest::post(&report_path).csrf(&csrf).body(body), deadline)
            .await
        {
            return CasOutcome::NetError(error);
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
            Err(error) => return CasOutcome::NetError(error),
        };

        // 步骤 5：sessionIdExchange → 会话集（复用既有 SessionExchange 解析）。
        let Ok(exchange_bytes) = serde_json::to_vec(&SidTicketBody {
            sid_ticket: sid_ticket.expose(),
        }) else {
            return CasOutcome::NetError(CasNetError::new("protocol"));
        };
        let exchange_body = Zeroizing::new(exchange_bytes);
        let exchange_path = format!(
            "{SESSION_ID_EXCHANGE_PATH}?{}",
            self.identity.protocol_query()
        );
        let exchange = match self
            .gateway_json(
                RawRequest::post(&exchange_path).csrf(&csrf).body(exchange_body),
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
            let url =
                parse_absolute_url(first_auth).ok_or_else(|| CasNetError::new("protocol"))?;
            if url.host != self.host || url.port != self.port {
                return Err(CasNetError::new("redirect_origin"));
            }
            url
        };
        for _ in 0..MAX_REDIRECT_HOPS {
            let response = self.chain_get(&current, sso_profile, deadline).await?;
            if let Some(location) = response.location() {
                if let Some(handover) = extract_cas_handover(&location) {
                    return Ok(Some(handover));
                }
                current = resolve_location(&location, &current)
                    .ok_or_else(|| CasNetError::new("redirect_location"))?;
                if !self.origin_allowed(&current) {
                    return Err(CasNetError::new("redirect_origin"));
                }
                continue;
            }
            match response.status {
                200 => {
                    if is_sso_login_form(&response.body) {
                        return Ok(None);
                    }
                    // 兜底：响应本体 URL 即交接页（正常链在 Location 已停）。
                    let url = format!("https://{}{}", current.host, current.path_and_query);
                    if let Some(handover) = extract_cas_handover(&url) {
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
            let mut header = HeaderValue::from_str(csrf.expose())
                .map_err(|_| CasNetError::new("protocol"))?;
            header.set_sensitive(true);
            builder = builder.header("x-csrf-token", header);
        }
        if let Some(referer) = request.referer {
            builder = builder.header("referer", referer);
        }
        if let Some(cookie) = request.cookie {
            builder = builder.header("cookie", cookie);
        }
        let http_request = builder
            .body(FullBody::new(bytes::Bytes::copy_from_slice(&request.body)))
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

/// authConfig 响应的最小承载（只取无头链需要的字段；`Secret` 自身脱敏 Debug）。
#[derive(Debug)]
struct AuthConfigEssentials {
    csrf: Secret,
    first_auth: Option<String>,
}

fn parse_auth_config(raw: &[u8]) -> Result<AuthConfigEssentials, CasNetError> {
    #[derive(serde::Deserialize)]
    struct Security {
        #[serde(rename = "csrfToken")]
        csrf_token: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Config {
        #[serde(rename = "isLogin")]
        is_login: u8,
        #[serde(rename = "firstAuth")]
        first_auth: Option<Vec<String>>,
        security: Option<Security>,
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        data: Option<Config>,
    }
    // 业务码先行：被拒响应可能不带完整 data 形状，拒绝分类先于形状校验
    // （与协议库 `decode()` 同一语义）。
    #[derive(serde::Deserialize)]
    struct Status {
        code: i64,
    }
    let status: Status = serde_json::from_slice(raw).map_err(|_| CasNetError::new("protocol"))?;
    if status.code != 0 {
        return Err(CasNetError::new("rejected"));
    }
    let envelope: Envelope =
        serde_json::from_slice(raw).map_err(|_| CasNetError::new("protocol"))?;
    let config = envelope.data.ok_or_else(|| CasNetError::new("protocol"))?;
    if config.is_login == 1 {
        // headless 线只应在未登录时进入；已登录说明上层快路径漏判，fail-safe 上抛。
        return Err(CasNetError::new("already_login"));
    }
    let csrf = config
        .security
        .and_then(|security| security.csrf_token)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| CasNetError::new("csrf"))?;
    Ok(AuthConfigEssentials {
        csrf: Secret::new(csrf),
        first_auth: config.first_auth.and_then(|paths| paths.into_iter().next()),
    })
}

/// authCheck 响应解析：`code=0` 且 `data.sidTicket`（§二点八.3；code 非 0 → rejected，
/// 形状不符 → protocol）。票据即刻进入 [`Secret`]。
fn parse_auth_check_sid_ticket(raw: &[u8]) -> Result<Secret, CasNetError> {
    #[derive(serde::Deserialize)]
    struct Check {
        #[serde(rename = "sidTicket")]
        sid_ticket: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        code: i64,
        data: Option<Check>,
    }
    let envelope: Envelope =
        serde_json::from_slice(raw).map_err(|_| CasNetError::new("protocol"))?;
    if envelope.code != 0 {
        return Err(CasNetError::new("rejected"));
    }
    let ticket = envelope
        .data
        .and_then(|data| data.sid_ticket)
        .filter(|ticket| !ticket.is_empty())
        .ok_or_else(|| CasNetError::new("protocol"))?;
    Ok(Secret::new(ticket))
}

#[derive(serde::Serialize)]
struct SidTicketBody<'a> {
    #[serde(rename = "sidTicket")]
    sid_ticket: &'a str,
}

/// reportEnv 请求体（§二点八.2 字节配方）：
/// `{"ticket":…,"deviceId":…,"env":{"endpoint":{"device_id":…,"device":{"type":"browser"}}}}`。
/// `deviceId` 与 `env.endpoint.device_id` 同值（门禁实验证明该形态被接受）；
/// 序列化结果由零化容器持有。
fn build_report_env_body(
    ticket: &Secret,
    device_id: &Zeroizing<String>,
) -> Result<Zeroizing<Vec<u8>>, ()> {
    #[derive(serde::Serialize)]
    struct Device {
        r#type: &'static str,
    }
    #[derive(serde::Serialize)]
    struct Endpoint<'a> {
        device_id: &'a str,
        device: Device,
    }
    #[derive(serde::Serialize)]
    struct Env<'a> {
        endpoint: Endpoint<'a>,
    }
    #[derive(serde::Serialize)]
    struct ReportEnv<'a> {
        ticket: &'a str,
        #[serde(rename = "deviceId")]
        device_id: &'a str,
        env: Env<'a>,
    }
    serde_json::to_vec(&ReportEnv {
        ticket: ticket.expose(),
        device_id,
        env: Env {
            endpoint: Endpoint {
                device_id,
                device: Device { r#type: "browser" },
            },
        },
    })
    .map(Zeroizing::new)
    .map_err(|_| ())
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
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).is_err() {
        // 随机源不可用：回退纳秒时间戳（仅请求关联 id，非凭据）。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
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
