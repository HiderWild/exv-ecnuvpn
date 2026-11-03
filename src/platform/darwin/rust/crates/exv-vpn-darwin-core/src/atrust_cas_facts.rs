//! Darwin 认证链的纯事实解析；页面策略与 HTTP 客户端共用，不执行网络或凭据提交。

use exv_vpn_atrust::Secret;
use std::collections::BTreeMap;
use zeroize::{Zeroize, Zeroizing};

/// 学校登录页的服务端表单标记。
pub const SSO_FORM_MARKER: &str = "login-page-flowkey";

/// 已观察到的响应错误，不把未知页面解释为失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasFactsError {
    Protocol,
    Rejected,
    AlreadyLoggedIn,
    MissingCsrf,
}

/// 绝对 https URL 的最小解析产物（只取链路跟随所需字段；不引入 `url` 依赖，
/// 与 `school_auto_login::gateway_origin` 的既有做法一致）。
#[derive(Clone, PartialEq, Eq)]
pub struct AbsoluteUrl {
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

impl std::fmt::Debug for AbsoluteUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbsoluteUrl")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Drop for AbsoluteUrl {
    fn drop(&mut self) {
        self.path_and_query.zeroize();
    }
}

/// 解析绝对 `https://host[:port]/path[?query]` URL；非 https、无 host 拒绝。
pub fn parse_absolute_url(url: &str) -> Option<AbsoluteUrl> {
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
pub fn decode_query_value(value: &str) -> Zeroizing<String> {
    let bytes = value.as_bytes();
    let mut decoded = Zeroizing::new(Vec::with_capacity(bytes.len()));
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
            b'%' if index + 2 < bytes.len()
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
    Zeroizing::new(String::from_utf8_lossy(&decoded).into_owned())
}

/// 解析 query（`?` 后、`#` 前）为解码后的键值对。
pub fn query_pairs_of(url: &str) -> Vec<(Zeroizing<String>, Zeroizing<String>)> {
    let query = url.split_once('?').map_or("", |(_, rest)| rest);
    let query = query.split('#').next().unwrap_or(query);
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (decode_query_value(key), decode_query_value(value)),
            None => (decode_query_value(pair), Zeroizing::new(String::new())),
        })
        .collect()
}

fn deserialize_secret<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Secret, D::Error> {
    use serde::Deserialize;
    String::deserialize(deserializer).map(Secret::new)
}

fn deserialize_optional_secret<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Secret>, D::Error> {
    use serde::Deserialize;
    Option::<String>::deserialize(deserializer).map(|value| value.map(Secret::new))
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

/// 解析 CAS 票据交接页 URL；网关把预登录票据交回浏览器
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
    let mut source = None;
    let mut data = None;
    let mut next_service = None;
    for (key, value) in query_pairs_of(url) {
        match key.as_str() {
            "source" => source = Some(value),
            "data" => data = Some(value),
            "nextService" => next_service = Some(value.as_str().to_owned()),
            _ => {}
        }
    }
    if source.as_ref().map(|s| s.as_str()) != Some("auth_cas") {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Env {
        #[serde(default)]
        need: bool,
        timing: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Payload {
        #[serde(deserialize_with = "deserialize_secret")]
        ticket: Secret,
        env: Option<Env>,
    }
    let payload: Payload = serde_json::from_str(data.as_ref()?.as_str()).ok()?;
    let ticket = Secret::new(payload.ticket.expose().trim());
    if ticket.is_empty() {
        return None;
    }
    let (env_need, timing) = payload.env.map(|e| (e.need, e.timing)).unwrap_or_default();
    Some(CasHandover {
        ticket,
        timing,
        env_need,
        next_service,
    })
}

/// SSO 200 响应是否为登录表单页（服务端渲染的 `id="login-page-flowkey"` 标记；
/// 匹配裸标记以容忍引号/空白形态）。只报告该标记是否实际出现。
#[must_use]
pub fn is_sso_login_form(body: &[u8]) -> bool {
    // 页面是 UTF-8 HTML；标记是纯 ASCII，字节级子串匹配即可，避免整页解码。
    body.windows(SSO_FORM_MARKER.len())
        .any(|window| window == SSO_FORM_MARKER.as_bytes())
}

/// 解析 `Location` 头并相对当前 URL 得到下一跳：支持绝对 `https://` URL 与
/// 绝对路径（`/passport/...`，门禁实验链的实际形态）；协议相对（`//host/..`）、
/// 相对路径或非 https 一律拒绝（fail-safe）。
pub fn resolve_location(location: &str, current: &AbsoluteUrl) -> Option<AbsoluteUrl> {
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
pub fn apply_set_cookie_pair(
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
pub fn absorb_set_cookie(jar: &mut BTreeMap<String, Secret>, header_value: &str, host: &str) {
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
        if key.eq_ignore_ascii_case("max-age") && val.parse::<i64>().is_ok_and(|age| age <= 0) {
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
pub fn cookie_header(jar: &BTreeMap<String, Secret>) -> Option<Zeroizing<String>> {
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

/// authConfig 中后续 CAS 链所需的最小材料。
pub struct AuthConfigEssentials {
    pub csrf: Secret,
    pub first_auth: Option<String>,
}

/// 只解析网关业务码，供 reportEnv 等无载荷应答共用。
pub fn parse_gateway_status(raw: &[u8]) -> Result<(), CasFactsError> {
    #[derive(serde::Deserialize)]
    struct Status {
        code: i64,
    }
    let status: Status = serde_json::from_slice(raw).map_err(|_| CasFactsError::Protocol)?;
    if status.code == 0 {
        Ok(())
    } else {
        Err(CasFactsError::Rejected)
    }
}

/// 提取未登录 authConfig 的 CSRF 与首条认证地址。
pub fn parse_auth_config(raw: &[u8]) -> Result<AuthConfigEssentials, CasFactsError> {
    #[derive(serde::Deserialize)]
    struct Security {
        #[serde(
            rename = "csrfToken",
            default,
            deserialize_with = "deserialize_optional_secret"
        )]
        csrf_token: Option<Secret>,
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
    parse_gateway_status(raw)?;
    let envelope: Envelope = serde_json::from_slice(raw).map_err(|_| CasFactsError::Protocol)?;
    let config = envelope.data.ok_or_else(|| CasFactsError::Protocol)?;
    if config.is_login == 1 {
        // headless 线只应在未登录时进入；已登录说明上层快路径漏判，fail-safe 上抛。
        return Err(CasFactsError::AlreadyLoggedIn);
    }
    let csrf = config
        .security
        .and_then(|security| security.csrf_token)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| CasFactsError::MissingCsrf)?;
    Ok(AuthConfigEssentials {
        csrf,
        first_auth: config.first_auth.and_then(|paths| paths.into_iter().next()),
    })
}

/// authCheck 响应解析：`code=0` 且 `data.sidTicket`（§二点八.3；code 非 0 → rejected，
/// 形状不符 → protocol）。票据即刻进入 [`Secret`]。
pub fn parse_auth_check_sid_ticket(raw: &[u8]) -> Result<Secret, CasFactsError> {
    #[derive(serde::Deserialize)]
    struct Check {
        #[serde(
            rename = "sidTicket",
            default,
            deserialize_with = "deserialize_optional_secret"
        )]
        sid_ticket: Option<Secret>,
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        data: Option<Check>,
    }
    parse_gateway_status(raw)?;
    let envelope: Envelope = serde_json::from_slice(raw).map_err(|_| CasFactsError::Protocol)?;
    let ticket = envelope
        .data
        .and_then(|data| data.sid_ticket)
        .filter(|ticket| !ticket.is_empty())
        .ok_or_else(|| CasFactsError::Protocol)?;
    Ok(ticket)
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
pub fn build_report_env_body(
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
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(
        &mut *bytes,
        &ReportEnv {
            ticket: ticket.expose(),
            device_id,
            env: Env {
                endpoint: Endpoint {
                    device_id,
                    device: Device { r#type: "browser" },
                },
            },
        },
    )
    .map_err(|_| ())?;
    Ok(bytes)
}

/// 构造 authCheck 与 sessionIdExchange 的票据体；明文缓冲随作用域零化。
pub fn build_sid_ticket_body(ticket: &Secret) -> Result<Zeroizing<Vec<u8>>, ()> {
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(
        &mut *bytes,
        &SidTicketBody {
            sid_ticket: ticket.expose(),
        },
    )
    .map_err(|_| ())?;
    Ok(bytes)
}
