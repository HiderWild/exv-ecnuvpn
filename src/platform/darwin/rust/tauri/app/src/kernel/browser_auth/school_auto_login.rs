//! 学校登录页的单次同步填交原语。提交预算与重试均由统一认证编排器持有。

pub const SCHOOL_LOGIN_ORIGIN: &str = "https://sso.ecnu.edu.cn";

use zeroize::Zeroizing;

/// 按值消费的一次性学校凭据；不得记录或序列化到普通配置。
pub struct CredentialDelivery {
    pub username: Zeroizing<String>,
    pub password: Zeroizing<String>,
    pub gateway_origin: String,
}

/// 脚本仅报告本次观察与点击事实，Submitted 不代表学校或网关登录成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionOutcome {
    NotReady,
    Submitted,
    ChallengeVisible,
    ServiceMismatch,
    Failed,
}

impl InjectionOutcome {
    /// 接受脚本原始状态或 WK 回调桥接的 JSON 字符串；未知/损坏结果是注入故障。
    pub fn parse(result: &str) -> Self {
        let decoded = serde_json::from_str::<String>(result).ok();
        match decoded.as_deref().unwrap_or(result) {
            "not_ready" => Self::NotReady,
            "submitted" => Self::Submitted,
            "challenge_visible" => Self::ChallengeVisible,
            "service_mismatch" => Self::ServiceMismatch,
            _ => Self::Failed,
        }
    }
}

/// 以 JSON 编码隔离页面脚本与凭据；中间载荷和返回脚本均在释放时零化。
/// 调用方须在 WK 执行返回后释放脚本，不将其写入日志或 Cookie。
pub fn build_fill_script(delivery: CredentialDelivery) -> Result<Zeroizing<String>, String> {
    let mut payload = Zeroizing::new(Vec::new());
    serde_json::to_writer(
        &mut *payload,
        &(
            delivery.username.as_str(),
            delivery.password.as_str(),
            delivery.gateway_origin.as_str(),
        ),
    )
    .map_err(|_| "学校登录材料编码失败".to_owned())?;
    let encoded =
        std::str::from_utf8(payload.as_slice()).map_err(|_| "学校登录材料编码失败".to_owned())?;
    let mut script = Zeroizing::new(String::new());
    script.push('(');
    script.push_str(include_str!("school_login_fill.js"));
    script.push_str(")(...");
    script.push_str(encoded);
    script.push(')');
    Ok(script)
}
