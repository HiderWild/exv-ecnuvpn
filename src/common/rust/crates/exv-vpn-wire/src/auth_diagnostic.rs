//! 认证诊断小契约：双端 App/Core 共用有限字段校验，聚合元数据由 Core 生成。
use crate::generated::LogEvent;

pub const STARTUP: &str = "AUTH_BROWSER_STARTUP";
pub const FAILURE: &str = "AUTH_BROWSER_FAILURE";
const KINDS: &[&str] = &[
    "credentials_rejected",
    "policy_restricted",
    "gateway_throttled",
    "browser_startup_failed",
    "browser_auth_stalled",
    "browser_auth_transport",
    "browser_auth_unknown_page",
];
// Windows AuthErrorCode::machine_code 的既有有限码；原码与粗分类分别保留。
const FAILURE_CODES: &[&str] = &[
    "browser_auth_client_install_required",
    "browser_auth_failed_page",
    "browser_auth_stalled",
    "browser_auth_gateway_throttled",
    "browser_auth_acquisition_timeout",
    "browser_auth_channel_open_failed",
    "browser_auth_channel_transport_failed",
    "browser_auth_observation_malformed",
    "browser_auth_session_probe_failed",
    "browser_auth_session_missing",
    "browser_auth_session_storage_failed",
    "browser_auth_session_invalid",
    "browser_auth_login_entry_invalid",
    "auto_login_no_credentials",
    "browser_auth_injection_failed",
    "browser_auth_unsupported_operation",
    "browser_auth_unclassified",
];
const OPERATIONS: &[&str] = &[
    "authentication",
    "window_open",
    "navigation",
    "page_observation",
    "session_probe",
    "presentation",
    "credential_submission",
    "session_extract",
    "page_policy",
    "cleanup",
];

fn has_auth_material(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "http:",
        "https:",
        "atrust:",
        "://",
        "cookie",
        "ticket",
        "authorization",
        "sessionid",
        "password=",
        "password:",
        "passwd=",
        "token=",
        "token:",
        "secret=",
        "secret:",
        "口令：",
        "密码：",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// 本地操作错误只保存单行摘录；认证材料或URL线索出现时省略整项。
#[must_use]
pub fn local_error_detail(value: &str) -> Option<String> {
    if has_auth_material(value) {
        return None;
    }
    let line: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(240)
        .collect();
    let line = line.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

fn numeric(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}
fn enumeration(value: &str, choices: &[&str]) -> bool {
    choices.contains(&value)
}
/// 学校提示沿既有采集边界传入，认证材料线索整项省略。
#[must_use]
pub fn school_error_hint(value: &str) -> Option<String> {
    if has_auth_material(value) {
        return None;
    }
    let line: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(120)
        .collect();
    (!line.trim().is_empty()).then(|| line.trim().to_owned())
}

/// 仅接收认证专用代码与字段，忽略调用方传入的组件、时钟、级别和消息元数据。
///
/// # Errors
/// 字段超界或类型不符时返回固定原因，调用方按诊断降级收尾。
pub fn normalize(mut event: LogEvent) -> Result<LogEvent, &'static str> {
    if event.fields.len() > 16 {
        return Err("auth_diagnostic/too_many_fields");
    }
    let startup = match event.code.as_str() {
        STARTUP => true,
        FAILURE => false,
        _ => return Err("auth_diagnostic/code"),
    };
    for (key, value) in &event.fields {
        let valid = match key.as_str() {
            "generation" | "round" | "sample" | "navigation" => numeric(value),
            "phase" | "error_phase" if startup => enumeration(
                value,
                &["none", "loading", "interactive", "complete", "unknown"],
            ),
            "loading_visible" if startup => enumeration(value, &["0", "1", "unknown"]),
            "has_message" if startup => enumeration(value, &["0", "1"]),
            "first_error" if startup => enumeration(
                value,
                &[
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
                ],
            ),
            "error_source" if startup => enumeration(value, &["none", "unknown", "exv_local_api"]),
            "gateway_code" => {
                value == "none" || (value.len() <= 11 && value.parse::<i32>().is_ok())
            }
            "result" if startup => value == "unavailable",
            "kind" if !startup => KINDS.contains(&value.as_str()),
            "operation" if !startup => OPERATIONS.contains(&value.as_str()),
            "failure_code" if !startup => FAILURE_CODES.contains(&value.as_str()),
            "native_code" if !startup => value.len() <= 20 && value.parse::<i64>().is_ok(),
            "school_hint" if !startup => {
                school_error_hint(value).as_deref() == Some(value.as_str())
            }
            "detail" if !startup => {
                value.chars().count() <= 240
                    && local_error_detail(value).as_deref() == Some(value.as_str())
            }
            _ => false,
        };
        if !valid {
            return Err("auth_diagnostic/field");
        }
    }
    if !event.fields.contains_key("generation")
        || !event.fields.contains_key("round")
        || (!startup && !event.fields.contains_key("kind"))
    {
        return Err("auth_diagnostic/missing_field");
    }
    event.level = if startup { "info" } else { "warn" }.into();
    event.component = "auth.browser".into();
    event.message = if startup {
        "认证页面启动采样"
    } else {
        "浏览器认证轮次失败"
    }
    .into();
    event.timestamp_ms = 0;
    Ok(event)
}
