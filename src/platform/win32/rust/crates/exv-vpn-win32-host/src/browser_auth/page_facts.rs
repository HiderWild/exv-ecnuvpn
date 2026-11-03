//! 活动浏览器认证页面原语；宿主事实采集与统一策略共用单一实现。

/// 服务端消息摘录的最大字符数：页面事实采集（cookie 预算内）与 Rust 侧净化截断
/// 共用同一上限。
pub const SERVER_DETAIL_MAX_CHARS: usize = 60;
/// 失败页短正文判定的最大长度（字符）：正文超过该上限的页面（正常门户/服务页，
/// 研究报告 §5.1/§5.4）即使含「失败」等字样也绝不判为失败页（保留长度护栏）。
pub const AUTH_FAILED_PHRASE_MAX_CHARS: usize = SERVER_DETAIL_MAX_CHARS;
/// 已知服务端失败短语族（研究报告 §一(a)/§5.1 的实测服务端 message 文案：
/// 「失败」「当前认证已超时」「操作异常，请稍后重试」「登录失败，错误码：…」；
/// 「已超时」含「超时」故不单列）。仅与长度护栏联用。
pub const AUTH_FAILED_PHRASES: &[&str] = &["失败", "超时", "操作异常"];

/// URL（含 hash 路由）是否指向门户「下载客户端」页；大小写不敏感。
#[must_use]
pub fn url_marks_client_install(url: &str) -> bool {
    url.to_ascii_lowercase().contains("down_client_pc")
}

/// URL 是否指向网关门户的客户端启动页 `/portal/shortcut.html`；大小写不敏感。
///
/// 该页是网关唤起官方客户端的启动页（`id="shortcut-page"`），自带失败即
/// `window.location.reload()` 的重载逻辑；落在该页等价于「网关要求官方客户端」。
#[must_use]
pub fn url_marks_client_shortcut(url: &str) -> bool {
    url.to_ascii_lowercase().contains("/portal/shortcut.html")
}

/// URL 的路径是否就是 authConfig 探测路径（忽略 query/fragment）。
///
/// 用于失败页判定时排除本应用自己的 authConfig JSON 探测页，避免误判。
#[must_use]
pub fn url_is_auth_config_path(url: &str, auth_path: &str) -> bool {
    let url_path = url.split(['?', '#']).next().unwrap_or(url);
    let config_path = auth_path.split(['?', '#']).next().unwrap_or(auth_path);
    !config_path.is_empty() && url_path.ends_with(config_path)
}

/// 短正文是否构成认证失败页正文：长度不超过 [`AUTH_FAILED_PHRASE_MAX_CHARS`] 且含
/// 任一已知服务端失败短语。长度护栏保留（宁漏勿误，研究报告 §5.1/§5.4）——正常
/// 门户/服务页正文远超上限，含「失败」二字也不判；页面事实采集只在正文足够短时
/// 携带正文摘录，与本判定天然一致。
#[must_use]
pub fn text_is_failed_like(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= AUTH_FAILED_PHRASE_MAX_CHARS
        && AUTH_FAILED_PHRASES.iter().any(|phrase| trimmed.contains(phrase))
}

/// 当前结果页的位置只表示结果类型，不能单独证明限流。
#[must_use]
pub fn page_result_kind(url: &str) -> exv_vpn_atrust::auth::browser_auth_policy::PageResultKind {
    use exv_vpn_atrust::auth::browser_auth_policy::PageResultKind;
    let lowered = url.to_ascii_lowercase();
    if lowered.contains("type=common_error") { PageResultKind::Error }
    else if lowered.contains("#/common_info") || lowered.contains("type=common_info") { PageResultKind::Info }
    else { PageResultKind::Other }
}

/// 兼容已有调用者：只有实际提示被共同规则归为限流时才返回 true。
#[must_use]
pub fn url_marks_gateway_throttled(url: &str) -> bool {
    use exv_vpn_atrust::auth::browser_auth_policy::{classify_page_result, AuthFailureKind, PageResultFacts};
    let detail = extract_err_info_hint(url);
    classify_page_result(&PageResultFacts {
        result: page_result_kind(url), detail: detail.as_deref(), ..Default::default()
    }) == Some(AuthFailureKind::GatewayThrottled)
}

/// 净化服务端消息摘录：剥全部控制字符（含换行/制表，防日志与 UI 注入）、去首尾
/// 空白、按字符数截断到 [`SERVER_DETAIL_MAX_CHARS`]。摘录只进入日志/UI；票据不会
/// 出现在 `#errorData` 文本中（研究报告 §一(a)：正文 = 服务端 message 原文），
/// 截断+剥控制字符作为最后兜底，测试另行断言票据不外泄。
#[must_use]
pub fn sanitize_server_detail(text: &str) -> String {
    let cleaned: String = text.chars().filter(|ch| !ch.is_control()).collect();
    let trimmed = cleaned.trim();
    trimmed.chars().take(SERVER_DETAIL_MAX_CHARS).collect()
}

/// 解码 `encodeURIComponent` 产物：%XX 序列按 UTF-8 字节还原；非法/截断序列以
/// 替换字符容错（页面摘录仅用于展示与判定，不要求字节精确）。
#[must_use]
pub fn percent_decode_component(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let high = (bytes[index + 1] as char).to_digit(16).unwrap_or(0) as u8;
            let low = (bytes[index + 2] as char).to_digit(16).unwrap_or(0) as u8;
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 在（可能被截断的）JSON 文本中提取 `"field":"value"` 的字符串值；不做完整 JSON
/// 解析——`err_info` 是受控服务端构造（实测形态值内无转义引号），前缀扫描足够且
/// 无需额外依赖。
fn extract_json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\":\"");
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].find('"').map(|end| &json[start..start + end])?;
    Some(rest.to_owned())
}

/// 从 URL query 的 `err_info` 参数提取可读提示（`errorMessage` 优先，`errorTitle`
/// 兜底——字段名见网关 `assets/index.50994ae4.js` common_error 分支）。percent 编码
/// 与直书 JSON 两种形态都可解析；提取失败返回 `None`（分类保留实际结果类型，
/// 只是不携带提示）。
#[must_use]
pub fn extract_err_info_hint(url: &str) -> Option<String> {
    let fragmentless = url.split('#').next().unwrap_or(url);
    let query = fragmentless
        .split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("err_info="))?;
    for candidate in [percent_decode_component(query), query.to_owned()] {
        for field in ["errorMessage", "errorTitle"] {
            if let Some(hint) = extract_json_string_field(&candidate, field)
                .filter(|hint| !hint.trim().is_empty())
            {
                return Some(sanitize_server_detail(&hint));
            }
        }
    }
    None
}
