//! aTrust 学校 SSO 自动填交材料（T4c，2026-09-30 用户明确授权）。
//!
//! 一次 Connect 至多构建一个 [`SchoolAutoLogin`]，只持有并向已授权网关交付材料。
//! 提交预算由 [`crate::browser_auth::AuthAttempt`] 管理，跨通道沿用同一次尝试；
//! 页面注入与人工接管由统一编排器决定。凭据来源是
//! 与 Cisco 完全共享的单对账号密码（显式本次输入优先，其次本机已保存对）。
//! 密码只经 [`zeroize::Zeroizing`] 与注入表达式传递，不落日志、不落普通配置。

use std::sync::Arc;

use zeroize::Zeroizing;

/// 已确认的学校统一认证 origin（T4a 2026-09-30 隔离动态页观察；产品面向本校）。
pub const SCHOOL_LOGIN_ORIGIN: &str = "https://sso.ecnu.edu.cn";
/// 学校登录页路径（运行时普通表单 action 同源同路径）。
pub const SCHOOL_LOGIN_PATH: &str = "/login";
/// 内嵌 WebView 无法直接取回 eval 结果，用一次性 cookie 回传状态（hex 编码）。
pub const RESULT_COOKIE: &str = "__exv_school_auto";

/// 学校自动填交的页面脚本执行结果（与 `school_login_fill.js` 返回值一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillOutcome {
    /// 已点击学校提交按钮一次；结果仍由网关会话复核决定，不等于登录成功。
    Submitted,
    /// 当前页不是已确认的学校登录页（重定向未完成或去向异常）。
    NotSchoolPage,
    /// 页面正确但表单/控件在有界等待内未出现（页面改版或加载过慢）。
    FormNotReady,
    /// `service` 参数与本次所选网关不对应。
    ServiceMismatch,
    /// 页面已出现可见验证码挑战；不填不点，整页交还人工。
    ChallengeVisible,
    /// 脚本执行异常。
    Error,
}

impl FillOutcome {
    /// 脚本状态字面量 → 结果。
    #[must_use]
    pub fn parse(status: &str) -> Self {
        match status {
            "submitted" => Self::Submitted,
            "not_school_page" => Self::NotSchoolPage,
            "form_not_ready" => Self::FormNotReady,
            "service_mismatch" => Self::ServiceMismatch,
            "challenge_visible" => Self::ChallengeVisible,
            _ => Self::Error,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::NotSchoolPage => "not_school_page",
            Self::FormNotReady => "form_not_ready",
            Self::ServiceMismatch => "service_mismatch",
            Self::ChallengeVisible => "challenge_visible",
            Self::Error => "error",
        }
    }
}

/// 归一化网关地址为 HTTPS origin（scheme + host[:port]，默认端口省略、host 小写——
/// 与浏览器 `new URL(...).origin` 的比较规则一致）。不引入 `url` 依赖：输入上游已
/// 经 [`crate::atrust_browser::acquire`] 的 `BrowserSession` 校验过。
fn gateway_origin(gateway: &str) -> Result<String, String> {
    let normalized = if gateway.starts_with("https://") {
        gateway.trim_end_matches('/').to_owned()
    } else if gateway.starts_with("http://") || gateway.contains("://") {
        return Err("aTrust 网关必须使用 HTTPS".to_owned());
    } else {
        format!("https://{}", gateway.trim_matches('/'))
    };
    let rest = normalized
        .strip_prefix("https://")
        .ok_or_else(|| "aTrust 网关必须使用 HTTPS".to_owned())?;
    let mut host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if let Some(without_default_port) = host.strip_suffix(":443") {
        host = without_default_port.to_owned();
    }
    // host[:port]：端口为空或非数字、主机含分隔符时拒绝（防 `http:` 这类残留混入）。
    let (name, port) = host.split_once(':').map_or((host.as_str(), None), |(n, p)| (n, Some(p)));
    if name.is_empty()
        || name.contains(['\\', ' ', '@', ':'])
        || !port.is_none_or(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err("aTrust 网关地址无效".to_owned());
    }
    Ok(format!("https://{host}"))
}

/// 本次连接的学校凭据来源；克隆共享零化密码生命周期，不持有提交阶段。
#[derive(Clone)]
pub struct SchoolAutoLogin {
    username: String,
    password: Arc<Zeroizing<String>>,
    gateway_origin: String,
}

impl SchoolAutoLogin {
    /// 以共享凭据对构建来源。`gateway` 用于页面 `service` 参数一致性校验。
    ///
    /// # Errors
    /// 网关地址无法解析为 HTTPS origin 时返回错误字符串。
    pub fn new(gateway: &str, username: String, password: String) -> Result<Self, String> {
        Ok(Self {
            username,
            password: Arc::new(Zeroizing::new(password)),
            gateway_origin: gateway_origin(gateway)?,
        })
    }

    /// 本次连接所选网关的 HTTPS origin（脚本用于 `service` 校验）。
    #[must_use]
    pub fn gateway_origin(&self) -> &str {
        &self.gateway_origin
    }
}

/// 只向本次已授权网关按值交付零化材料；提交预算由统一编排器独占。
impl crate::browser_auth::CredentialProvider for SchoolAutoLogin {
    fn delivery(&self, gateway: &str) -> Option<crate::browser_auth::CredentialDelivery> {
        if gateway != self.gateway_origin {
            return None;
        }
        Some(crate::browser_auth::CredentialDelivery {
            username: Zeroizing::new(self.username.clone()),
            password: Zeroizing::new(self.password.to_string()),
            gateway_origin: self.gateway_origin.clone(),
        })
    }
}

/// 解码内嵌 WebView 回传的状态 cookie 值（hex → 状态字面量）。
#[must_use]
pub fn decode_result_cookie(value: &str) -> Option<FillOutcome> {
    if value.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect();
    let text = String::from_utf8(bytes?).ok()?;
    Some(FillOutcome::parse(&text))
}

/// 登记填交结果的稳定日志字面量（不含账号、密码或页面内容）。
#[must_use]
pub fn outcome_log_field(outcome: FillOutcome) -> &'static str {
    outcome.as_str()
}
