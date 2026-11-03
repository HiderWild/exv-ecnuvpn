//! 两端浏览器认证共享的纯业务规则；窗口、时钟、取消与会话材料由宿主持有。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailureKind {
    CredentialsRejected,
    PolicyRestricted,
    GatewayThrottled,
    StartupFailed,
    LocalTimeout,
    TransportFailed,
    UnknownFailure,
}

/// 已通过宿主采样序号、位置及导航代次校验的有限启动事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupFacts {
    pub loading_visible: Option<bool>,
    pub script_failed: bool,
    pub promise_rejected: bool,
    pub gateway_code: Option<i32>,
}

impl StartupFacts {
    /// 兼容 v1/v2 有限采样协议；调用前宿主核对序号、位置与加载代次。
    #[must_use]
    pub fn parse(payload: &str) -> Option<Self> {
        let fields: Vec<_> = payload.split('|').collect();
        if !((fields.len() == 7 && fields[0] == "v1")
            || (fields.len() == 8
                && fields[0] == "v2"
                && ["none", "unknown", "exv_local_api"].contains(&fields[7])))
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
        Some(Self {
            loading_visible: match fields[2] {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            },
            // 资源、当前 Promise 拒绝与同步异常分别保留事实；脚本异常仅供诊断。
            script_failed: !matches!(fields[3], "none" | "resource_error" | "promise_rejection"),
            promise_rejected: fields[3] == "promise_rejection",
            gateway_code: if fields[6] == "none" {
                None
            } else {
                Some(fields[6].parse().ok()?)
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageResultKind {
    #[default]
    Other,
    Info,
    Error,
}

/// 结果路由来自当前位置；DOM 失败只在 coherent 时使用。
/// detail 仅允许宿主已核对的 URL 提示或同次 DOM 摘录；不存储秘密。
#[derive(Default)]
pub struct PageResultFacts<'a> {
    pub coherent: bool,
    pub result: PageResultKind,
    pub detail: Option<&'a str>,
    pub dom_failure: bool,
    pub startup: Option<StartupFacts>,
}

#[must_use]
pub fn classify_page_result(facts: &PageResultFacts<'_>) -> Option<AuthFailureKind> {
    let actual_page_failure =
        facts.result == PageResultKind::Error || (facts.coherent && facts.dom_failure);
    // 路由本身不携带失败原因；只解释已观察的错误提示。
    if facts.result != PageResultKind::Other || actual_page_failure {
        let detail = facts.detail.unwrap_or_default().to_ascii_lowercase();
        for (phrases, kind) in [
            (
                &[
                    "concurrency login",
                    "too many requests",
                    "rate limit",
                    "登录频繁",
                    "操作频繁",
                    "并发登录",
                    "并发数",
                    "登录冲突",
                ][..],
                AuthFailureKind::GatewayThrottled,
            ),
            (
                &[
                    "用户名或密码错误",
                    "用户名或密码不正确",
                    "密码错误",
                    "账号或密码错误",
                    "invalid credentials",
                    "incorrect password",
                ][..],
                AuthFailureKind::CredentialsRejected,
            ),
            (
                &[
                    "策略限制",
                    "禁止登录",
                    "无权登录",
                    "policy denied",
                    "access denied",
                    "not permitted",
                ][..],
                AuthFailureKind::PolicyRestricted,
            ),
        ] {
            if phrases.iter().any(|phrase| detail.contains(phrase)) {
                return Some(kind);
            }
        }
    }
    // 任意脚本拒绝与加载遮罩可同时出现在正常认证跳转中，继续等待业务结果。
    if actual_page_failure
        || facts
            .startup
            .and_then(|startup| startup.gateway_code)
            .is_some_and(|code| code != 0)
    {
        return Some(AuthFailureKind::UnknownFailure);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExtraction {
    NotAttempted,
    ValidSameOrigin,
    Failed,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFacts {
    pub gateway_logged_in: bool,
    pub extraction: SessionExtraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionDecision {
    Extract,
    Complete,
    HandleExtractionFailure,
    EvaluatePage,
}

#[must_use]
pub fn decide_session_precedence(facts: &SessionFacts) -> SessionDecision {
    match facts.extraction {
        SessionExtraction::ValidSameOrigin => SessionDecision::Complete,
        SessionExtraction::Failed | SessionExtraction::Missing => {
            SessionDecision::HandleExtractionFailure
        }
        SessionExtraction::NotAttempted if facts.gateway_logged_in => SessionDecision::Extract,
        SessionExtraction::NotAttempted => SessionDecision::EvaluatePage,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRound {
    First,
    Second,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubmissionState {
    #[default]
    NotStarted,
    InFlight,
    Submitted,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionOutcome {
    NotReady,
    Submitted,
    Finished,
}

/// 一个认证轮次共享一个预算；切换渠道不创建新的预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RoundBudget {
    pub state: SubmissionState,
}

impl RoundBudget {
    pub fn begin_submission(&mut self) -> bool {
        if self.state != SubmissionState::NotStarted {
            return false;
        }
        self.state = SubmissionState::InFlight;
        true
    }
    pub fn finish_submission(&mut self, outcome: SubmissionOutcome) {
        if self.state == SubmissionState::InFlight {
            self.state = match outcome {
                SubmissionOutcome::NotReady => SubmissionState::NotStarted,
                SubmissionOutcome::Submitted => SubmissionState::Submitted,
                SubmissionOutcome::Finished => SubmissionState::Finished,
            };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRecheck {
    Pending,
    NoValidSession,
    ValidSameOrigin,
}

pub struct RecoveryFacts {
    pub round: AuthRound,
    pub failure: AuthFailureKind,
    pub cancelled: bool,
    pub time_remaining: bool,
    pub session_recheck: SessionRecheck,
    pub challenge_visible: bool,
    pub submission: SubmissionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDecision {
    Complete,
    RecheckSession,
    RestartRound,
    AwaitChallenge,
    FinishFailure,
    Cancelled,
}

#[must_use]
pub fn decide_recovery(facts: &RecoveryFacts) -> RecoveryDecision {
    if facts.cancelled {
        return RecoveryDecision::Cancelled;
    }
    if !facts.time_remaining {
        return RecoveryDecision::FinishFailure;
    }
    if facts.session_recheck == SessionRecheck::ValidSameOrigin {
        return RecoveryDecision::Complete;
    }
    if facts.challenge_visible {
        return RecoveryDecision::AwaitChallenge;
    }
    if facts.session_recheck == SessionRecheck::Pending {
        return RecoveryDecision::RecheckSession;
    }
    if facts.round == AuthRound::Second
        || matches!(
            facts.failure,
            AuthFailureKind::CredentialsRejected
                | AuthFailureKind::PolicyRestricted
                | AuthFailureKind::GatewayThrottled
        )
    {
        return RecoveryDecision::FinishFailure;
    }
    RecoveryDecision::RestartRound
}

/// 旧阶段名称由宿主原 wire 映射保持兼容；Recovering 仅新增轮次语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProgress {
    Opening,
    SchoolLogin,
    Submitting,
    GatewayConfirming,
    AwaitingManual,
    Recovering { round: AuthRound },
}
