//! 统一认证编排器；X3a 内嵌活动线及后续迁移边界见父模块。
use super::*;
use exv_vpn_atrust::auth::browser_auth_policy::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;
use tokio::time::Instant;

/// 同一次连接的预算，调用方跨通道回退继续借用它；新连接创建新实例。
#[derive(Default)]
pub struct AuthAttempt {
    round: Mutex<AttemptRound>,
    form_counted: AtomicBool,
    cancelled: AtomicBool,
    deadline: Mutex<Option<Instant>>,
    first_failure: Mutex<Option<String>>,
}

struct AttemptRound {
    round: AuthRound,
    budget: RoundBudget,
}
impl Default for AttemptRound {
    fn default() -> Self {
        Self {
            round: AuthRound::First,
            budget: RoundBudget::default(),
        }
    }
}

impl AuthAttempt {
    /// 兼容既有绝对期限入口；完整回退链直接继续借用同一 attempt。
    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            deadline: Mutex::new(Some(deadline)),
            ..Self::default()
        }
    }
    /// 过渡到尚未迁移的通道也继续使用本次绝对截止时间；下一连接才建新实例。
    pub fn remaining_budget(&self, total: Duration) -> Duration {
        self.deadline(total)
            .saturating_duration_since(Instant::now())
    }
    fn begin_submission(&self) -> bool {
        self.round
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .budget
            .begin_submission()
    }
    fn finish_submission(&self, result: InjectionOutcome) {
        self.round
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .budget
            .finish_submission(match result {
                InjectionOutcome::NotReady => SubmissionOutcome::NotReady,
                InjectionOutcome::Submitted => SubmissionOutcome::Submitted,
                _ => SubmissionOutcome::Finished,
            });
    }
    pub fn round(&self) -> AuthRound {
        self.round.lock().unwrap_or_else(|e| e.into_inner()).round
    }
    fn recovery_facts(
        &self,
        error: &AuthError,
        deadline: Instant,
        recheck: SessionRecheck,
    ) -> RecoveryFacts {
        let round = self.round.lock().unwrap_or_else(|e| e.into_inner());
        RecoveryFacts {
            round: round.round,
            failure: error.failure_kind.unwrap_or(match error.code {
                AuthErrorCode::GatewayThrottled => AuthFailureKind::GatewayThrottled,
                AuthErrorCode::Stalled | AuthErrorCode::AcquisitionTimeout => {
                    AuthFailureKind::LocalTimeout
                }
                AuthErrorCode::ChannelOpenFailed | AuthErrorCode::InjectionFailed => {
                    AuthFailureKind::StartupFailed
                }
                AuthErrorCode::ChannelTransportFailed | AuthErrorCode::SessionProbeFailed => {
                    AuthFailureKind::TransportFailed
                }
                _ => AuthFailureKind::UnknownFailure,
            }),
            cancelled: self.cancelled.load(Ordering::Acquire),
            time_remaining: Instant::now() < deadline,
            session_recheck: recheck,
            challenge_visible: false,
            submission: round.budget.state,
        }
    }
    fn restart_round(&self, first_failure: String) {
        let mut state = self.round.lock().unwrap_or_else(|e| e.into_inner());
        state.round = AuthRound::Second;
        state.budget = RoundBudget::default();
        *self.first_failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(first_failure);
    }
    fn count_form(&self, events: &dyn AuthEventSink) {
        if !self.form_counted.swap(true, Ordering::AcqRel) {
            events.form_login_reached();
        }
    }
    /// 只读返回首次确定的绝对期限；后续通道不得因交接重新计时。
    pub fn deadline(&self, budget: Duration) -> Instant {
        let mut deadline = self.deadline.lock().unwrap_or_else(|e| e.into_inner());
        *deadline.get_or_insert_with(|| Instant::now() + budget)
    }
}

fn recovery_summary(error: &AuthError) -> String {
    // 两轮各留 24 字，加标记共至多 59 字，经过既有 60 字边界仍保留末轮原因。
    // 过长的上游摘录使用已有有限类别，短的具体原因保持原样。
    error.clean_detail().filter(|detail| detail.chars().count() <= 24).unwrap_or_else(|| {
        match error.failure_kind {
            Some(AuthFailureKind::CredentialsRejected) => "学校账号或密码未通过验证",
            Some(AuthFailureKind::PolicyRestricted) => "网关策略限制了本次认证",
            Some(AuthFailureKind::GatewayThrottled) => "网关限制了本次登录频率或并发数",
            Some(AuthFailureKind::StartupFailed) => "认证页面未能正常启动",
            _ => match error.code {
                AuthErrorCode::InjectionFailed => "学校登录表单填交执行失败",
                AuthErrorCode::Stalled | AuthErrorCode::AcquisitionTimeout => "等待认证结果超时",
                AuthErrorCode::ChannelOpenFailed => "认证窗口未能启动",
                AuthErrorCode::ChannelTransportFailed | AuthErrorCode::SessionProbeFailed => {
                    "认证通道或会话探测失败"
                }
                AuthErrorCode::SessionMissing | AuthErrorCode::SessionInvalid => {
                    "未取得有效网关会话"
                }
                AuthErrorCode::LoginEntryInvalid => "学校认证入口与当前网关不一致或不可用",
                _ => "认证未能完成，请重试或查看日志",
            },
        }
        .to_owned()
    })
}

/// 仅引用本次显式输入或当前已授权的共享来源；不允许读取新的持久秘密。
pub trait CredentialProvider: Send + Sync {
    fn delivery(&self, gateway_origin: &str) -> Option<CredentialDelivery>;
}

/// 精确删除当前网关会话记录；不能删除 profile、deviceId 或学校凭据。
pub trait SessionCacheInvalidator: Send + Sync {
    fn forget<'a>(&'a self, gateway_origin: &'a str) -> ChannelFuture<'a, ()>;
}

pub struct AuthConfig {
    pub gateway_origin: String,
    pub user_agent: String,
    pub auth_path: String,
    pub initial_presentation: PresentationIntent,
    pub allow_interaction: bool,
    pub total_budget: Duration,
    pub stall_budget: Duration,
    pub submitted_manual_budget: Duration,
    pub tick: Duration,
    pub cleanup_budget: Duration,
    /// 内嵌安装页先尝试脱离；系统页保留即时安装分类，不做重导航。
    /// 停滞计时独立依据已开始认证且实际停留网关的页面事实。
    pub gateway_page_recovery: bool,
    /// 宿主的实际页面事实足以计时；系统 P3 使用此事实，独立于安装恢复。
    pub gateway_stall_from_page: bool,
}

impl AuthConfig {
    pub fn new(gateway_origin: String, user_agent: String, auth_path: String) -> Self {
        Self {
            gateway_origin,
            user_agent,
            auth_path,
            initial_presentation: PresentationIntent::Hidden,
            allow_interaction: true,
            total_budget: Duration::from_secs(300),
            stall_budget: Duration::from_secs(30),
            submitted_manual_budget: Duration::from_secs(15),
            tick: Duration::from_secs(1),
            cleanup_budget: Duration::from_secs(2),
            gateway_page_recovery: false,
            gateway_stall_from_page: false,
        }
    }
}

pub enum AcquisitionOutcome {
    Ready(SessionArtifact),
    Control(AuthControl),
    /// HTTP 已观察交互需求，但本次调用没有提供后续窗口通道。
    Interactive(InteractiveReason),
}

pub struct AuthOrchestrator<'a> {
    pub config: AuthConfig,
    pub events: &'a dyn AuthEventSink,
    pub snapshots: &'a dyn SessionSnapshotProvider,
    pub credentials: Option<&'a dyn CredentialProvider>,
    pub cache: &'a dyn SessionCacheInvalidator,
    pub attempt: &'a AuthAttempt,
}

impl AuthOrchestrator<'_> {
    /// 通道按当前策略传入：HTTP → 隐藏 Owned；系统浏览器由上层显式回退信号重新选择。
    /// attempt 与 owner receiver 必须属于同一次连接；跨回退复用 attempt，取消后的新连接重建。
    pub async fn acquire(
        &self,
        channels: &mut [&mut dyn ChannelTrait],
        control: &mut watch::Receiver<Option<AuthControl>>,
    ) -> Result<AcquisitionOutcome, AuthError> {
        if let Some(signal) = self.signal(control) {
            return Ok(AcquisitionOutcome::Control(signal));
        }
        let deadline = self.attempt.deadline(self.config.total_budget);
        let mut stage = None;
        let mut interactive_reason = None;
        let started = Instant::now();
        let mut first_failure = self
            .attempt
            .first_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for channel in channels {
            if let Some(signal) = self.signal(control) {
                return Ok(AcquisitionOutcome::Control(signal));
            }
            let mut manual_requested = false;
            let mut cleanup_failed = false;
            let result = loop {
                let result = self
                    .segment(
                        *channel,
                        control,
                        deadline,
                        &mut stage,
                        &mut manual_requested,
                    )
                    .await;
                let Err(FlowError::Error(error)) = &result else {
                    break result;
                };
                self.events.round_failure(self.attempt.round(), error);
                // 手动渠道/用户主动介入仍由现有控制流负责；自动恢复仅重开可控认证轮次。
                if manual_requested
                    || channel.capabilities().visibility != VisibilityCapability::Controlled
                    || matches!(
                        error.code,
                        AuthErrorCode::UnsupportedOperation | AuthErrorCode::CredentialRequired
                    )
                {
                    break result;
                }
                self.events.failure(error);
                let mut facts =
                    self.attempt
                        .recovery_facts(error, deadline, SessionRecheck::Pending);
                let mut decision = decide_recovery(&facts);
                if decision == RecoveryDecision::RecheckSession {
                    let recheck_deadline =
                        deadline.min(Instant::now() + self.config.cleanup_budget);
                    let recheck = self
                        .recheck_session(*channel, control, recheck_deadline, &mut manual_requested)
                        .await;
                    if let Some(signal) = self.signal(control) {
                        break Err(FlowError::Control(signal));
                    }
                    if Instant::now() >= deadline {
                        let operation = match &recheck {
                            Err(FlowError::Error(error)) => error.operation,
                            _ => AuthOperation::Probe,
                        };
                        break Err(FlowError::Error(AuthError::at(
                            AuthErrorCode::AcquisitionTimeout, operation,
                        )));
                    }
                    match recheck {
                        Ok(Some(session)) => {
                            facts.session_recheck = SessionRecheck::ValidSameOrigin;
                            facts.time_remaining = Instant::now() < deadline;
                            if decide_recovery(&facts) == RecoveryDecision::Complete {
                                log::info!(target: "exv.atrust_auth", "auth.recovery result=session_reused round={:?} elapsed_ms={}", self.attempt.round(), started.elapsed().as_millis());
                                break Ok(SegmentOutcome::Ready(session));
                            }
                        }
                        Err(FlowError::Control(signal)) => break Err(FlowError::Control(signal)),
                        Err(FlowError::Error(diagnostic)) => self.events.failure(&diagnostic),
                        Ok(None) => {}
                    }
                    facts.session_recheck = SessionRecheck::NoValidSession;
                    facts.time_remaining = Instant::now() < deadline;
                    decision = decide_recovery(&facts);
                }
                if decision == RecoveryDecision::Cancelled {
                    break Err(FlowError::Control(AuthControl::Cancel));
                }
                if decision != RecoveryDecision::RestartRound || manual_requested {
                    break result;
                }
                // close 失败只留诊断，保留首次根因，不能在旧资源尚未收束时建第二窗。
                if !self.close(*channel).await {
                    cleanup_failed = true;
                    break result;
                }
                if let Some(signal) = self.signal(control) {
                    break Err(FlowError::Control(signal));
                }
                if Instant::now() >= deadline {
                    break result;
                }
                let reason = recovery_summary(error);
                first_failure = Some(reason.clone());
                self.attempt.restart_round(reason);
                stage = None;
                self.events.progress(AuthProgress::Recovering {
                    round: AuthRound::Second,
                });
                log::info!(target: "exv.atrust_auth", "auth.recovery result=restarting round=2 first_code={} elapsed_ms={}", error.code.machine_code(), started.elapsed().as_millis());
            };
            match result {
                Ok(SegmentOutcome::Interactive(reason)) => {
                    interactive_reason = Some(reason);
                    self.close(*channel).await;
                }
                Ok(SegmentOutcome::Ready(session)) => {
                    log::info!(target: "exv.atrust_auth", "auth.round_result result=success round={:?} recovered={} elapsed_ms={}", self.attempt.round(), first_failure.is_some(), started.elapsed().as_millis());
                    self.close(*channel).await;
                    return Ok(match self.signal(control) {
                        Some(signal) => AcquisitionOutcome::Control(signal),
                        None => AcquisitionOutcome::Ready(session),
                    });
                }
                Err(FlowError::Control(signal)) => {
                    self.close(*channel).await;
                    return Ok(AcquisitionOutcome::Control(signal));
                }
                Err(FlowError::Error(mut error)) => {
                    if let Some(first) = &first_failure {
                        let last = recovery_summary(&error);
                        error.server_detail = Some(format!("首次认证：{first}；自动重试：{last}"));
                    }
                    log::info!(target: "exv.atrust_auth", "auth.round_result result=failed round={:?} recovered={} elapsed_ms={}", self.attempt.round(), first_failure.is_some(), started.elapsed().as_millis());
                    // 局部诊断保留原字段；终结 auth.failed 由 typed owner 唯一汇总。
                    self.events.failure(&error);
                    // 最终失败交现有模态展示汇总；未知/失败页面不能再冒充人工认证任务。
                    if !cleanup_failed {
                        self.close(*channel).await;
                    }
                    return Err(error);
                }
            }
        }
        if let Some(reason) = interactive_reason {
            if let Some(signal) = self.signal(control) {
                return Ok(AcquisitionOutcome::Control(signal));
            }
            Ok(AcquisitionOutcome::Interactive(reason))
        } else {
            Err(AuthError::at(
                AuthErrorCode::UnsupportedOperation,
                AuthOperation::Open,
            ))
        }
    }

    fn signal(&self, control: &watch::Receiver<Option<AuthControl>>) -> Option<AuthControl> {
        if self.attempt.cancelled.load(Ordering::Acquire) {
            return Some(AuthControl::Cancel);
        }
        match *control.borrow() {
            Some(AuthControl::Cancel) => {
                self.attempt.cancelled.store(true, Ordering::Release);
                Some(AuthControl::Cancel)
            }
            Some(AuthControl::UseSystemBrowser) => Some(AuthControl::UseSystemBrowser),
            _ => None,
        }
    }

    async fn operation<T>(
        &self,
        mut future: ChannelFuture<'_, T>,
        operation: AuthOperation,
        control: &mut watch::Receiver<Option<AuthControl>>,
        deadline: Instant,
        manual: &mut bool,
    ) -> FlowResult<T> {
        let mut receiver_alive = true;
        loop {
            if let Some(signal) = self.signal(control) {
                return Err(FlowError::Control(signal));
            }
            if *control.borrow_and_update() == Some(AuthControl::ShowForManual) {
                *manual = true;
            }
            tokio::select! {biased;
                changed=control.changed(),if receiver_alive=>{if changed.is_err(){receiver_alive=false;}},
                _=tokio::time::sleep_until(deadline)=>return Err(FlowError::Error(AuthError::at(AuthErrorCode::AcquisitionTimeout,operation))),
                result=&mut future=>{
                    if let Some(signal)=self.signal(control){return Err(FlowError::Control(signal));}
                    return result.map_err(FlowError::Error);
                }
            }
        }
    }

    fn milestone(&self, stage: &mut Option<AuthMilestone>, next: AuthMilestone) {
        if *stage != Some(next) {
            self.events.milestone(next);
            *stage = Some(next);
        }
    }

    async fn close(&self, channel: &mut dyn ChannelTrait) -> bool {
        match tokio::time::timeout(self.config.cleanup_budget, channel.close()).await {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                self.events.failure(&error);
                false
            }
            Err(_) => {
                self.events.failure(&AuthError::at(
                    AuthErrorCode::ChannelTransportFailed,
                    AuthOperation::Close,
                ));
                false
            }
        }
    }

    async fn recheck_session(
        &self,
        channel: &mut dyn ChannelTrait,
        control: &mut watch::Receiver<Option<AuthControl>>,
        deadline: Instant,
        manual: &mut bool,
    ) -> FlowResult<Option<SessionArtifact>> {
        let status = self
            .operation(
                channel.probe_session(),
                AuthOperation::Probe,
                control,
                deadline,
                manual,
            )
            .await?;
        if decide_session_precedence(&SessionFacts {
            gateway_logged_in: status == SessionStatus::LoggedIn,
            extraction: SessionExtraction::NotAttempted,
        }) != SessionDecision::Extract
        {
            return Ok(None);
        }
        let session = self.extract(channel, control, deadline, manual).await?;
        self.operation(
            channel.complete_session(&session),
            AuthOperation::Extract,
            control,
            deadline,
            manual,
        )
        .await?;
        Ok(Some(session))
    }

    async fn present(
        &self,
        channel: &mut dyn ChannelTrait,
        intent: PresentationIntent,
        control: &mut watch::Receiver<Option<AuthControl>>,
        deadline: Instant,
        manual: &mut bool,
    ) -> FlowResult<PresentationOutcome> {
        if intent == PresentationIntent::Visible && !self.config.allow_interaction {
            let mut error = AuthError::at(AuthErrorCode::CredentialRequired, AuthOperation::Present);
            error.server_detail = Some("学校认证需要人工操作，请主动重新连接".into());
            return Err(FlowError::Error(error));
        }
        self.operation(
            channel.present(intent),
            AuthOperation::Present,
            control,
            deadline,
            manual,
        )
        .await
    }

    async fn segment(
        &self,
        channel: &mut dyn ChannelTrait,
        control: &mut watch::Receiver<Option<AuthControl>>,
        deadline: Instant,
        stage: &mut Option<AuthMilestone>,
        manual: &mut bool,
    ) -> FlowResult<SegmentOutcome> {
        let capabilities = channel.capabilities();
        let start = Instant::now();
        self.milestone(stage, AuthMilestone::Opening);
        self.operation(
            channel.open(ChannelRequest {
                gateway_origin: self.config.gateway_origin.clone(),
                user_agent: self.config.user_agent.clone(),
                initial_presentation: self.config.initial_presentation,
            }),
            AuthOperation::Open,
            control,
            deadline,
            manual,
        )
        .await?;
        if capabilities.observation == ObservationFidelity::NoPage {
            let result = self
                .http_segment(channel, control, deadline, stage, manual)
                .await;
            return match result {
                Ok(result) => Ok(result),
                Err(FlowError::Error(error))
                    if matches!(
                        error.code,
                        AuthErrorCode::SessionProbeFailed
                            | AuthErrorCode::SessionStorageFailed
                            | AuthErrorCode::SessionInvalid
                            | AuthErrorCode::SessionMissing
                    ) =>
                {
                    self.events.failure(&error);
                    Ok(SegmentOutcome::Interactive(match error.code {
                        AuthErrorCode::SessionStorageFailed => InteractiveReason::JarReadFailed,
                        AuthErrorCode::SessionInvalid => InteractiveReason::SessionBuildFailed,
                        AuthErrorCode::SessionMissing => InteractiveReason::SessionMissing,
                        _ => InteractiveReason::NetworkFailed,
                    }))
                }
                Err(error) => Err(error),
            };
        }
        let mut presentation = if capabilities.can_render {
            self.present(
                channel,
                self.config.initial_presentation,
                control,
                deadline,
                manual,
            )
            .await?
        } else {
            PresentationOutcome::NoSurface
        };
        let probe_url = format!("{}{}", self.config.gateway_origin, self.config.auth_path);
        if channel.initial_navigation() == InitialNavigation::Pending {
            self.operation(
                channel.navigate(probe_url.clone()),
                AuthOperation::Navigate,
                control,
                deadline,
                manual,
            )
            .await?;
        }
        let mut policy = PagePolicy::with_gateway_policy(
            self.config.gateway_page_recovery,
            self.config.gateway_stall_from_page || self.config.gateway_page_recovery,
        );
        let mut login_started = channel.initial_navigation() == InitialNavigation::LoginStarted;
        let mut cache_forgotten = false;
        let mut injected_on_channel = false;
        let mut submitted_at = None;
        let mut not_ready_since: Option<(Observation<String>, Instant)> = None;
        let mut last_renavigation: Option<(String, Instant)> = None;
        let mut probe_failure: Option<(Instant, AuthError)> = None;
        loop {
            if let Some(signal) = self.signal(control) {
                return Err(FlowError::Control(signal));
            }
            let page = self
                .operation(
                    channel.observe(),
                    AuthOperation::Observe,
                    control,
                    deadline,
                    manual,
                )
                .await?;
            // 明确学校 origin 与已开始导航是现有登录事实，须先于跨页面未登录分支。
            // 未知外部 origin 不授予此事实，也不扩大 DOM/注入同源权限。
            if is_school_origin(&page) {
                login_started = true;
            }
            let status = match self
                .operation(
                    channel.probe_session(),
                    AuthOperation::Probe,
                    control,
                    deadline,
                    manual,
                )
                .await
            {
                Ok(status) => {
                    probe_failure = None;
                    status
                }
                Err(FlowError::Error(error)) if error.code == AuthErrorCode::SessionProbeFailed => {
                    self.events.failure(&error);
                    let since = probe_failure.as_ref().map_or_else(Instant::now, |(since, _)| *since);
                    probe_failure = Some((since, error));
                    SessionStatus::NotYetObserved
                }
                Err(error) => return Err(error),
            };
            if decide_session_precedence(&SessionFacts {
                gateway_logged_in: matches!(status, SessionStatus::LoggedIn),
                extraction: SessionExtraction::NotAttempted,
            }) == SessionDecision::Extract {
                self.milestone(stage, AuthMilestone::GatewayConfirming);
                let session = self.extract(channel, control, deadline, manual).await?;
                // extract 已核对同源及 sid；页面残留错误仅留诊断。
                if let Err(error) = policy.sample(
                    &page,
                    &status,
                    &self.config.gateway_origin,
                    &self.config.auth_path,
                    login_started,
                    start.elapsed(),
                    self.config.stall_budget,
                ) {
                    self.events.failure(&error);
                }
                if decide_session_precedence(&SessionFacts {
                    gateway_logged_in: true,
                    extraction: SessionExtraction::ValidSameOrigin,
                }) != SessionDecision::Complete
                {
                    return Err(FlowError::Error(AuthError::at(
                        AuthErrorCode::SessionInvalid,
                        AuthOperation::Extract,
                    )));
                }
                self.operation(
                    channel.complete_session(&session),
                    AuthOperation::Extract,
                    control,
                    deadline,
                    manual,
                )
                .await?;
                if *manual && capabilities.can_render {
                    presentation = self
                        .present(
                            channel,
                            PresentationIntent::Visible,
                            control,
                            deadline,
                            manual,
                        )
                        .await?;
                }
                // 外部 UI 的「手动介入」可直接显示本 owner 窗口；成功等待消费实际可见性。
                if capabilities.can_render {
                    presentation = self
                        .present(
                            channel,
                            if matches!(presentation, PresentationOutcome::Hidden) {
                                PresentationIntent::Hidden
                            } else {
                                PresentationIntent::Visible
                            },
                            control,
                            deadline,
                            manual,
                        )
                        .await?;
                }
                if matches!(
                    presentation,
                    PresentationOutcome::Visible | PresentationOutcome::AlwaysVisible
                ) {
                    self.operation(
                        Box::pin(async {
                            tokio::time::sleep(Duration::from_millis(700)).await;
                            Ok(())
                        }),
                        AuthOperation::Wait,
                        control,
                        deadline,
                        manual,
                    )
                    .await?;
                }
                return Ok(SegmentOutcome::Ready(session));
            }
            let page_result = policy.sample(
                &page,
                &status,
                &self.config.gateway_origin,
                &self.config.auth_path,
                login_started,
                start.elapsed(),
                self.config.stall_budget,
            );
            // 明确页面失败保留分类；仅局部停滞或安装提示时保留持续通信失败的末次根因。
            if probe_failure.as_ref().is_some_and(|(since, _)| since.elapsed() >= self.config.stall_budget)
                && !page_result.as_ref().is_err_and(|error| error.failure_kind.is_some())
            {
                return Err(FlowError::Error(probe_failure.take().unwrap().1));
            }
            let latched = page_result.map_err(FlowError::Error)?;
            if let SessionStatus::LoggedOut { first_auth } = &status {
                if !cache_forgotten {
                    // 存储故障是局部事实，不能阻止已可继续的浏览器认证。
                    match self
                        .operation(
                            self.cache.forget(&self.config.gateway_origin),
                            AuthOperation::Extract,
                            control,
                            deadline,
                            manual,
                        )
                        .await
                    {
                        Ok(()) => {}
                        Err(FlowError::Error(error))
                            if error.code == AuthErrorCode::SessionStorageFailed =>
                        {
                            self.events.failure(&error)
                        }
                        Err(error) => return Err(error),
                    }
                    cache_forgotten = true;
                }
                if !login_started {
                    let entry = first_auth
                        .as_deref()
                        .filter(|entry| valid_entry(entry))
                        .ok_or_else(|| {
                            FlowError::Error(AuthError::at(
                                AuthErrorCode::LoginEntryInvalid,
                                AuthOperation::Navigate,
                            ))
                        })?;
                    self.operation(
                        channel.navigate(format!("{}{entry}", self.config.gateway_origin)),
                        AuthOperation::Navigate,
                        control,
                        deadline,
                        manual,
                    )
                    .await?;
                    login_started = true;
                }
            }
            // 实际导航表示进展；短暂缺少位置采样既不清空计时，也不自行触发失败。
            if matches!(page.url, Observation::Observed(_))
                && not_ready_since.as_ref().is_some_and(|(url, _)| url != &page.url)
            {
                not_ready_since = None;
            }
            // 持续采样真实挑战；提交后表单可能已消失或转到学校的挑战路径。
            if is_school_origin(&page)
                && page.coherent
                && matches!(&page.dom, Observation::Observed(dom) if dom.challenge_visible)
            {
                *manual = true;
            }
            let school = is_school_page(&page);
            if school {
                login_started = true;
                self.attempt.count_form(self.events);
                if !*manual && submitted_at.is_none() {
                    self.milestone(stage, AuthMilestone::SchoolLogin);
                }
                if !*manual
                    && !injected_on_channel
                    && capabilities.injection == InjectionCapability::SchoolFormOnce
                {
                    // 取消优先：协商和预算占用通过后才向本次已授权来源请求材料。
                    if let Some(signal) = self.signal(control) {
                        return Err(FlowError::Control(signal));
                    }
                    if let Some(provider) = self.credentials
                        && self.attempt.begin_submission()
                    {
                        if let Some(credentials) = provider.delivery(&self.config.gateway_origin) {
                            injected_on_channel = true;
                            self.milestone(stage, AuthMilestone::Submitting);
                            let injection_started = Instant::now();
                            let injection = self
                                .operation(
                                    channel.inject_once(credentials),
                                    AuthOperation::Inject,
                                    control,
                                    deadline,
                                    manual,
                                )
                                .await;
                            match injection {
                                Ok(outcome) => {
                                    self.attempt.finish_submission(outcome);
                                    if outcome == InjectionOutcome::Submitted {
                                        self.milestone(stage, AuthMilestone::Submitting);
                                        submitted_at = Some(Instant::now());
                                        not_ready_since = None;
                                    } else if outcome == InjectionOutcome::NotReady {
                                        injected_on_channel = false;
                                        // 只给实际反复 NotReady 计时；未知页面/采样缺失不构成失败。
                                        let waiting = not_ready_since
                                            .get_or_insert((page.url.clone(), injection_started));
                                        if !*manual && waiting.1.elapsed() >= self.config.stall_budget {
                                            return Err(FlowError::Error(AuthError::at(
                                                AuthErrorCode::Stalled,
                                                AuthOperation::Wait,
                                            )));
                                        }
                                    } else if outcome == InjectionOutcome::ChallengeVisible {
                                        *manual = true;
                                    } else {
                                        return Err(FlowError::Error(AuthError::at(
                                            if outcome == InjectionOutcome::ServiceMismatch {
                                                AuthErrorCode::LoginEntryInvalid
                                            } else {
                                                AuthErrorCode::InjectionFailed
                                            },
                                            AuthOperation::Inject,
                                        )));
                                    }
                                }
                                Err(FlowError::Error(error))
                                    if error.code == AuthErrorCode::InjectionFailed =>
                                {
                                    self.attempt.finish_submission(InjectionOutcome::Failed);
                                    return Err(FlowError::Error(error));
                                }
                                Err(error) => return Err(error),
                            }
                        } else {
                            self.attempt.finish_submission(InjectionOutcome::NotReady);
                            return Err(FlowError::Error(AuthError::at(
                                AuthErrorCode::CredentialRequired,
                                AuthOperation::Inject,
                            )));
                        }
                    } else if submitted_at.is_none() {
                        *manual = true;
                    }
                } else if capabilities.injection == InjectionCapability::Unavailable
                    || self.credentials.is_none()
                {
                    *manual = true;
                }
                if !*manual && submitted_at
                    .is_some_and(|at: Instant| at.elapsed() >= self.config.submitted_manual_budget)
                {
                    return Err(FlowError::Error(AuthError::at(
                        AuthErrorCode::Stalled,
                        AuthOperation::Wait,
                    )));
                }
            }
            if is_school_origin(&page) && self.credentials.is_none() {
                *manual = true;
            }
            if *manual && capabilities.can_render {
                if matches!(presentation, PresentationOutcome::Hidden) {
                    presentation = self
                        .present(
                            channel,
                            PresentationIntent::Visible,
                            control,
                            deadline,
                            manual,
                        )
                        .await?;
                }
                self.milestone(stage, AuthMilestone::AwaitingManual);
            }
            let url = match &page.url {
                Observation::Observed(url) => Some(url.as_str()),
                _ => None,
            };
            let at_gateway = matches!(&page.origin,Observation::Observed(origin) if origin==&self.config.gateway_origin);
            if login_started
                && !latched
                && at_gateway
                && policy.install_recovery_needed()
                && url.is_some_and(|u| {
                    !super::page_facts::url_is_auth_config_path(u, &self.config.auth_path)
                })
            {
                let url = url.unwrap();
                if last_renavigation
                    .as_ref()
                    .is_none_or(|(old, at)| old != url || at.elapsed() >= Duration::from_secs(3))
                {
                    self.operation(
                        channel.navigate(probe_url.clone()),
                        AuthOperation::Navigate,
                        control,
                        deadline,
                        manual,
                    )
                    .await?;
                    last_renavigation = Some((url.to_owned(), Instant::now()));
                }
            }
            self.operation(
                channel.wait_for_change(self.config.tick),
                AuthOperation::Wait,
                control,
                deadline,
                manual,
            )
            .await?;
        }
    }

    /// HTTP 只执行缓存复核和一次协议复用；回退、取消与预算仍由同一引擎仲裁。
    async fn http_segment(
        &self,
        channel: &mut dyn ChannelTrait,
        control: &mut watch::Receiver<Option<AuthControl>>,
        deadline: Instant,
        stage: &mut Option<AuthMilestone>,
        manual: &mut bool,
    ) -> FlowResult<SegmentOutcome> {
        let status = match self
            .operation(
                channel.probe_session(),
                AuthOperation::Probe,
                control,
                deadline,
                manual,
            )
            .await
        {
            Ok(status) => status,
            Err(FlowError::Error(error))
                if matches!(
                    error.code,
                    AuthErrorCode::SessionProbeFailed | AuthErrorCode::SessionStorageFailed
                ) =>
            {
                self.events.failure(&error);
                SessionStatus::NotYetObserved
            }
            Err(error) => return Err(error),
        };
        let session = if matches!(status, SessionStatus::LoggedIn) {
            self.extract(channel, control, deadline, manual).await?
        } else {
            if matches!(status, SessionStatus::LoggedOut { .. }) {
                match self
                    .operation(
                        self.cache.forget(&self.config.gateway_origin),
                        AuthOperation::Extract,
                        control,
                        deadline,
                        manual,
                    )
                    .await
                {
                    Ok(()) => {}
                    Err(FlowError::Error(error))
                        if error.code == AuthErrorCode::SessionStorageFailed =>
                    {
                        self.events.failure(&error)
                    }
                    Err(error) => return Err(error),
                }
            }
            match self
                .operation(
                    channel.reuse_session(deadline.saturating_duration_since(Instant::now())),
                    AuthOperation::Reuse,
                    control,
                    deadline,
                    manual,
                )
                .await?
            {
                ReuseOutcome::Ready(session) => session,
                ReuseOutcome::Interactive(reason) => {
                    return Ok(SegmentOutcome::Interactive(reason));
                }
            }
        };
        self.validate_session(&session)?;
        self.milestone(stage, AuthMilestone::GatewayConfirming);
        self.operation(
            channel.complete_session(&session),
            AuthOperation::Extract,
            control,
            deadline,
            manual,
        )
        .await?;
        Ok(SegmentOutcome::Ready(session))
    }

    fn validate_session(&self, artifact: &SessionArtifact) -> FlowResult<()> {
        let session = &artifact.session;
        let sid = session
            .cookie_header()
            .split(';')
            .filter_map(|field| field.trim().split_once('='))
            .any(|(name, value)| name == "sid" && !value.trim().is_empty());
        if session.gateway() != self.config.gateway_origin || !sid || session.encode().is_err() {
            return Err(FlowError::Error(AuthError::at(
                AuthErrorCode::SessionInvalid,
                AuthOperation::Extract,
            )));
        }
        Ok(())
    }

    async fn extract(
        &self,
        channel: &mut dyn ChannelTrait,
        control: &mut watch::Receiver<Option<AuthControl>>,
        deadline: Instant,
        manual: &mut bool,
    ) -> FlowResult<SessionArtifact> {
        let browser = self
            .operation(
                channel.extract_session(),
                AuthOperation::Extract,
                control,
                deadline,
                manual,
            )
            .await;
        let mut primary_error = None;
        match browser {
            Ok(Some(session)) => match self.validate_session(&session) {
                Ok(()) => return Ok(session),
                Err(FlowError::Error(error)) => primary_error = Some(error),
                Err(error) => return Err(error),
            },
            Ok(None) => {}
            Err(FlowError::Error(error)) => primary_error = Some(error),
            Err(error) => return Err(error),
        }
        match self
            .operation(
                self.snapshots.snapshot(&self.config.gateway_origin),
                AuthOperation::Extract,
                control,
                deadline,
                manual,
            )
            .await
        {
            Ok(Some(session)) => {
                self.validate_session(&session)?;
                Ok(session)
            }
            Ok(None) => Err(FlowError::Error(primary_error.unwrap_or_else(|| {
                AuthError::at(AuthErrorCode::SessionMissing, AuthOperation::Extract)
            }))),
            Err(error) => Err(error),
        }
    }
}

enum SegmentOutcome {
    Ready(SessionArtifact),
    Interactive(InteractiveReason),
}
enum FlowError {
    Error(AuthError),
    Control(AuthControl),
}
type FlowResult<T> = Result<T, FlowError>;

fn valid_entry(entry: &str) -> bool {
    entry.starts_with('/') && !entry.starts_with("//") && !entry.contains(['\\', '\r', '\n'])
}
fn is_school_page(page: &PageObservation) -> bool {
    matches!(&page.origin,Observation::Observed(origin) if origin==crate::school_auto_login::SCHOOL_LOGIN_ORIGIN)
        && matches!(&page.url,Observation::Observed(url) if url.split(['?','#']).next()==Some("https://sso.ecnu.edu.cn/login"))
}
fn is_school_origin(page: &PageObservation) -> bool {
    matches!(&page.origin, Observation::Observed(origin) if origin == crate::school_auto_login::SCHOOL_LOGIN_ORIGIN)
}

// 活动内嵌线与后续宿主迁移边界见 browser_auth.rs。
