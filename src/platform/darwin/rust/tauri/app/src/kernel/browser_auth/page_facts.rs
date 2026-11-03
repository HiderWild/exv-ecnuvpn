//! Darwin 同次页面事实；原生 WKWebView 与外部浏览器共用，不执行分类或认证提交。

/// 仅采集有限类别；每个新文档分别保留首个同步异常与当前未处理拒绝，资源失败仅作备用。
/// 不回传 URL、源码、堆栈或消息原文。
pub(super) const PAGE_STARTUP_SCRIPT: &str = r#"(()=>{
try {
if (window.__exvAuthStartupSample) return;
let scriptError=null,resourceError=null;
const rejections=new Map();
const phase=()=>['loading','interactive','complete'].includes(document.readyState)?document.readyState:'unknown';
const observation=(error,message,fallback)=>{
let kind=fallback,hasMessage=false,source="unknown";
try {
const name=error&&error.name;
if(fallback==='script_error'&&['SyntaxError','ReferenceError','TypeError','RangeError','EvalError','URIError'].includes(name)) kind=name;
hasMessage=!!(message||(error&&error.message)||(typeof error==='string'&&error));
if(window.__exvIsOwnedLocalApiFailure&&window.__exvIsOwnedLocalApiFailure(error)) source='exv_local_api';
} catch(_) {}
return {kind,hasMessage,source,phase:phase()};
};
addEventListener('error',(event)=>{
if(event.target===window) {
if(!scriptError) scriptError=observation(event.error,event.message,'script_error');
} else if(event.target&&event.target.tagName==='SCRIPT'&&!resourceError) {
resourceError=observation(null,event.message,'resource_error');
}
},true);
addEventListener('unhandledrejection',(event)=>{
if(!rejections.has(event.promise)) rejections.set(event.promise,observation(event.reason,null,'promise_rejection'));
});
addEventListener('rejectionhandled',(event)=>rejections.delete(event.promise));
window.__exvAuthStartupSample=()=>{
const first=scriptError||rejections.values().next().value||resourceError;
let loading='unknown',code='none';
try {
const e=document.querySelector('#preContainer, #loading, #loading-mask, .loading, .loading-page, .el-loading-mask');
const style=e&&getComputedStyle(e);
loading=e&&e.getClientRects().length&&style.display!=='none'&&style.visibility!=='hidden'?'1':'0';
} catch(_) {}
try {
const info=JSON.parse(new URLSearchParams(location.search).get('err_info')||'null');
const raw=info&&(info.errorCode??info.code);
if((typeof raw==='number'||typeof raw==='string')&&/^-?[0-9]{1,10}$/.test(String(raw))) {
const n=Number(raw);if(Number.isSafeInteger(n)&&n>=-2147483648&&n<=2147483647) code=String(n);
}
} catch(_) {}
return ['v2',phase(),loading,first?first.kind:'none',first&&first.hasMessage?'1':'0',first?first.phase:'none',code,first?first.source:'none'].join('|');
};
} catch(_) {}
})()"#;

/// 页面可改写回传值，宿主在日志边界再次只接受有限枚举及 i32 数字，不记录自由文本。
pub(super) fn startup_diagnostic(payload: &str) -> Option<String> {
    let fields: Vec<_> = payload.split('|').collect();
    if !((fields.len() == 7 && fields[0] == "v1")
        || (fields.len() == 8 && fields[0] == "v2" && ["none", "unknown", "exv_local_api"].contains(&fields[7])))
        || !["loading", "interactive", "complete", "unknown"].contains(&fields[1])
        || !["0", "1", "unknown"].contains(&fields[2])
        || ![
            "none",
            "SyntaxError",
            "ReferenceError",
            "TypeError",
            "RangeError",
            "EvalError",
            "URIError",
            "script_error",
            "resource_error",
            "promise_rejection",
        ]
        .contains(&fields[3])
        || !["0", "1"].contains(&fields[4])
        || !["none", "loading", "interactive", "complete", "unknown"].contains(&fields[5])
    {
        return None;
    }
    let code = if fields[6] == "none" {
        "none".to_owned()
    } else {
        fields[6].parse::<i32>().ok()?.to_string()
    };
    let mut diagnostic = format!(
        "phase={} loading_visible={} first_error={} has_message={} error_phase={} gateway_code={}",
        fields[1], fields[2], fields[3], fields[4], fields[5], code
    );
    if fields.len() == 8 {
        diagnostic.push_str(&format!(" error_source={}", fields[7]));
    }
    Some(diagnostic)
}

/// 局部事实的可用性；未观察与不可达都不表示失败。
#[derive(Clone, PartialEq, Eq)]
pub enum Observation<T> {
    Observed(T),
    NotObserved,
    Unavailable,
}

/// 同次 DOM 标记与受限文本，不实现明文 Debug。
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
pub struct DomFacts {
    /// 同次学校页面上三个控件可见且 enabled；不包含任何字段值。
    #[serde(default)]
    pub school_form_ready: bool,
    /// 同次页面可见验证码；未交付凭据即可转人工。
    #[serde(default)]
    pub challenge_visible: bool,
    pub client_install_marker: bool,
    pub shortcut_page: bool,
    pub download_page: bool,
    pub body_has_failure_phrase: bool,
    pub body_char_count: usize,
    pub error_container_text: Option<String>,
    pub short_body_text: Option<String>,
}

/// 一次页面采样；URL 可能含票据，故不实现 Debug。
#[derive(Clone, PartialEq, Eq)]
pub struct PageObservation {
    pub sequence: u64,
    pub url: Observation<String>,
    pub origin: Observation<String>,
    pub dom: Observation<DomFacts>,
    pub coherent: bool,
    /// 无诊断回包时为 None；有值时已经核对文档代次。
    pub startup: Option<exv_vpn_atrust::auth::browser_auth_policy::StartupFacts>,
}

/// 网关探针独立事实；只有明确网关响应才能声明已登录或已登出。
#[derive(Clone, PartialEq, Eq)]
pub enum SessionStatus {
    LoggedIn,
    LoggedOut(Option<String>),
    NotObserved,
    Unavailable,
}

/// 同步单次采样表达式；只采集 Windows 已活动的标记，返回 JSON 字符串。
/// 无 Cookie 侧信道、账号密码采集、网络请求或后台采样循环。
pub fn page_sample_script(sequence: u64) -> String {
    const SCRIPT: &str = r#"(()=>{
const sequence=__EXV_SEQUENCE__;
const url=location.href,origin=location.origin;
const b=document.body;
const raw=(b&&typeof b.innerText==='string')?b.innerText:'';
const t=raw.trim();
const e=document.querySelector('#errorData');
const et=(e&&typeof e.innerText==='string')?e.innerText.trim():'';
const schoolOrigin=origin==='https://sso.ecnu.edu.cn';
const school=schoolOrigin&&location.pathname==='/login';
const visible=(element)=>!!element&&element.offsetParent!==null;
const challenge=schoolOrigin&&(
Array.from(document.querySelectorAll('[id*="captcha" i], [class*="captcha" i]')).some((element)=>element.tagName!=='INPUT'&&visible(element))||
Array.from(document.querySelectorAll('iframe')).some((frame)=>visible(frame)&&/captcha/i.test(frame.src||''))
);
const form=school&&(document.querySelector('form#normalLoginForm')||document.querySelector('form[action*="/login"]'));
const user=form&&form.querySelector('input[name="username"]');
const pass=form&&form.querySelector('input[type="password"]');
const button=form&&(form.querySelector('button#submitBtn')||form.querySelector('button[type="submit"]')||form.querySelector('button'));
const ready=visible(user)&&visible(pass)&&visible(button)&&!user.disabled&&!pass.disabled&&!button.disabled;
return JSON.stringify({sequence,url,origin,startup:(()=>{try{return window.__exvAuthStartupSample?window.__exvAuthStartupSample():null}catch(_){return null}})(),dom:{
school_form_ready:ready,
challenge_visible:challenge,
client_install_marker:!!document.querySelector('.down-client-pc__body'),
shortcut_page:!!(document.querySelector('#shortcut-page')||e),
download_page:t.includes('下载客户端'),
body_has_failure_phrase:__EXV_FAILURE_PHRASES__.some((phrase)=>t.includes(phrase)),
body_char_count:Math.min(t.length,99),
error_container_text:et?et.slice(0,60):null,
short_body_text:t.length<=60?t:null
}});
})()"#;
    SCRIPT
        .replace("__EXV_SEQUENCE__", &sequence.to_string())
        .replace(
            "__EXV_FAILURE_PHRASES__",
            &serde_json::to_string(AUTH_FAILED_PHRASES).expect("静态短语表可序列化"),
        )
}

/// 核对启动诊断的序号、位置与加载事件计数；期间发生导航只舍弃诊断。
pub(super) fn parse_startup_sample(
    json: Option<&str>,
    sequence: u64,
    before: Option<&str>,
    after: Option<&str>,
    navigation: (u64, u64),
) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Sample {
        sequence: u64,
        url: String,
        startup: String,
    }
    let sample: Sample = serde_json::from_str(json?).ok()?;
    if navigation.0 != navigation.1
        || sequence != sample.sequence
        || before != after
        || after != Some(sample.url.as_str())
    {
        return None;
    }
    startup_diagnostic(&sample.startup)
}

/// 将同次脚本返回值与原生前后位置核对；不一致只舍弃 DOM，保留当前位置。
/// `json` 是脚本返回的 JSON 字符串本身，CDP 调用方应取 result.value。
pub fn parse_sample(
    json: Option<&str>,
    sequence: u64,
    before: Option<&str>,
    after: Option<&str>,
) -> PageObservation {
    #[derive(serde::Deserialize)]
    struct Sample {
        sequence: u64,
        url: String,
        origin: String,
        dom: DomFacts,
    }
    let location = after.and_then(|s| tauri::Url::parse(s).ok());
    let sample = json
        .and_then(|raw| serde_json::from_str::<Sample>(raw).ok())
        .filter(|sample| {
            sample.sequence == sequence
                && before == after
                && after == Some(sample.url.as_str())
                && location
                    .as_ref()
                    .is_some_and(|url| url.origin().ascii_serialization() == sample.origin)
        });
    let coherent = sample.is_some();
    let dom = sample.map(|mut sample| {
        sample.dom.error_container_text = sample
            .dom
            .error_container_text
            .as_deref()
            .map(sanitize_server_detail);
        sample.dom.short_body_text = sample
            .dom
            .short_body_text
            .as_deref()
            .map(sanitize_server_detail);
        sample.dom
    });
    PageObservation {
        sequence,
        url: location.as_ref().map_or(Observation::NotObserved, |url| {
            Observation::Observed(url.as_str().to_owned())
        }),
        origin: location.as_ref().map_or(Observation::NotObserved, |url| {
            Observation::Observed(url.origin().ascii_serialization())
        }),
        dom: dom.map_or(Observation::NotObserved, Observation::Observed),
        startup: None,
        coherent,
    }
}

/// 同 URL 的原生重新加载也使旧文档事实失效；仍保留本次原生当前位置。
pub(super) fn parse_native_sample(
    json: Option<&str>, sequence: u64, before: Option<&str>, after: Option<&str>, navigation: (u64, u64),
) -> PageObservation {
    parse_sample(json.filter(|_| navigation.0 == navigation.1), sequence, before, after)
}

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
        && AUTH_FAILED_PHRASES
            .iter()
            .any(|phrase| trimmed.contains(phrase))
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

/// 宿主组合呈现文案（单轮失败原因/终局失败原因）的长度兜底：远大于任一标准单轮
/// 文案（约 162 字符（含满额摘录）；两轮链按轮分别净化），只拦通道侧异常长错误串。
pub const PRESENTATION_MESSAGE_MAX_CHARS: usize = 240;

/// 净化宿主组合呈现文案：剥全部控制字符（防日志与 UI 注入）、去首尾空白，并按
/// [`PRESENTATION_MESSAGE_MAX_CHARS`] 兜底。不得复用 [`sanitize_server_detail`] 的
/// 服务端摘录上限——整条文案是宿主固定文案加内嵌已净化摘录（每段 ≤60 字符），
/// 按摘录上限整体截断会把诊断编号与「学校提示」从中间切断（c312181b 引入的回归，
/// 2026-10-26 S8 修复；测试 `browser_policy_error_reopens_once…` 与
/// `final_adjudication_explicit_browser_during_recheck…` 固化该合同）。
#[must_use]
pub fn sanitize_presentation_message(text: &str) -> String {
    let cleaned: String = text.chars().filter(|ch| !ch.is_control()).collect();
    let trimmed = cleaned.trim();
    trimmed.chars().take(PRESENTATION_MESSAGE_MAX_CHARS).collect()
}

/// 服务端消息摘录的净化：剥全部控制字符（含换行/制表，防日志与 UI 注入）、去首尾
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
    let rest = json[start..]
        .find('"')
        .map(|end| &json[start..start + end])?;
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
            if let Some(hint) =
                extract_json_string_field(&candidate, field).filter(|hint| !hint.trim().is_empty())
            {
                return Some(sanitize_server_detail(&hint));
            }
        }
    }
    None
}

