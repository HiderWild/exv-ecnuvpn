//! 系统浏览器小原语；分类、latch、里程碑和认证循环只在统一编排器。
use super::*;
use crate::browser_auth::page_facts::text_is_failed_like;
use crate::browser_auth::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

type PendingTarget<'a> =
    Pin<Box<dyn Future<Output = (&'a mut Cdp, Result<Value, String>)> + Send + 'a>>;

/// pending 按值持有 CDP 借用；open 的在途 future 被丢弃不会丢弃已提交创建请求。
struct SystemBrowserAdapter<'a> {
    cdp: Option<&'a mut Cdp>,
    pending: Option<PendingTarget<'a>>,
    committed: Arc<AtomicBool>,
    target: Option<String>,
    session: Option<String>,
    profile: &'a Path,
    intent: AuthIntent,
    gateway: String,
    user_agent: String,
    sequence: u64,
    create_settle: Duration,
    cleanup_unconfirmed: bool,
    cleanup_started: Option<tokio::time::Instant>,
    closed: bool,
    completed: bool,
    legacy_failure: Option<String>,
}

impl<'a> SystemBrowserAdapter<'a> {
    fn new(cdp: &'a mut Cdp, profile: &'a Path, create_settle: Duration) -> Self {
        Self::new_with_intent(cdp, profile, create_settle, AuthIntent::Automatic)
    }
    fn new_with_intent(cdp: &'a mut Cdp, profile: &'a Path, create_settle: Duration, intent: AuthIntent) -> Self {
        let committed = Arc::new(AtomicBool::new(false));
        let flag = committed.clone();
        let pending = Box::pin(async move {
            let result = async {
                let version = cdp.call("Browser.getVersion", json!({}), None).await?;
                if !version["userAgent"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
                {
                    return Err("浏览器未返回标识".to_owned());
                }
                cdp.call_with_commit(
                    "Target.createTarget",
                    json!({"url":"about:blank"}),
                    None,
                    Some(&flag),
                )
                .await
            }
            .await;
            (cdp, result)
        });
        Self {
            cdp: None,
            pending: Some(pending),
            committed,
            target: None,
            session: None,
            profile,
            intent,
            gateway: String::new(),
            user_agent: PLAIN_CHROME_USER_AGENT.into(),
            sequence: 0,
            create_settle,
            cleanup_unconfirmed: false,
            cleanup_started: None,
            closed: false,
            completed: false,
            legacy_failure: None,
        }
    }
    fn accept_created(
        &mut self,
        cdp: &'a mut Cdp,
        result: Result<Value, String>,
    ) -> Result<(), AuthError> {
        self.cdp = Some(cdp);
        self.pending = None;
        self.target = result
            .as_ref()
            .ok()
            .and_then(|v| v["targetId"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if self.target.is_none() {
            self.cleanup_unconfirmed = self.committed.load(Ordering::Acquire);
            self.legacy_failure = Some(
                result
                    .err()
                    .unwrap_or_else(|| "浏览器未返回登录标签页标识".into()),
            );
            return Err(AuthError::at(
                AuthErrorCode::ChannelOpenFailed,
                AuthOperation::Open,
            ));
        }
        Ok(())
    }
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        operation: AuthOperation,
    ) -> Result<Value, AuthError> {
        let result = match self.cdp.as_deref_mut() {
            Some(cdp) => cdp.call(method, params, self.session.as_deref()).await,
            None => Err("浏览器调试连接已关闭".into()),
        };
        result.map_err(|message| {
            self.legacy_failure = Some(message);
            AuthError::at(AuthErrorCode::ChannelTransportFailed, operation)
        })
    }
}

impl ChannelTrait for SystemBrowserAdapter<'_> {
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            injection: InjectionCapability::Unavailable,
            can_render: true,
            visibility: VisibilityCapability::AlwaysVisible,
            can_read_cookiejar: true,
            can_write_cookiejar: true,
            observation: ObservationFidelity::EvaluatedDom,
            probe_scope: ProbeScope::IndependentHttp,
        }
    }
    fn open(&mut self, request: ChannelRequest) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.gateway = request.gateway_origin;
            let (cdp, result) = self.pending.as_mut().expect("one open per owner").await;
            self.accept_created(cdp, result)?;
            let target = self.target.clone().expect("accepted own target");
            let attached = self
                .call(
                    "Target.attachToTarget",
                    json!({"targetId":target,"flatten":true}),
                    AuthOperation::Open,
                )
                .await?;
            self.session = attached["sessionId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            if self.session.is_none() {
                self.legacy_failure = Some("无法绑定登录标签页".into());
                return Err(AuthError::at(
                    AuthErrorCode::ChannelOpenFailed,
                    AuthOperation::Open,
                ));
            }
            self.call("Network.enable", json!({}), AuthOperation::Open)
                .await?;
            if self.intent == AuthIntent::FreshManual {
                let script = crate::browser_cache_profile::username_memory_script(
                    crate::browser_cache_profile::remembered_username(self.profile, &self.gateway).as_deref());
                // 账户记忆是可选功能；脚本失败不改变认证结果。
                // Page.enable 是生效前提：缺省时 Chrome 受理注册并返回 identifier
                // 但脚本对后续导航一律不执行（2026-10-26 darwin 真机确证同款缺陷）。
                if let Err(error) = self.call("Page.enable", json!({}), AuthOperation::Open).await {
                    log::warn!(target: "exv.atrust_auth", "auth.username_memory page_enable failed: {}", error.diagnostic_log_line());
                } else {
                    match self
                        .call(
                            "Page.addScriptToEvaluateOnNewDocument",
                            json!({"source": script, "runImmediately": true}),
                            AuthOperation::Open,
                        )
                        .await
                    {
                        Ok(reply) if reply.get("identifier").is_some_and(|id| !id.is_null()) => {}
                        Ok(_) => {
                            log::warn!(target: "exv.atrust_auth", "auth.username_memory register missing identifier")
                        }
                        Err(error) => {
                            log::warn!(target: "exv.atrust_auth", "auth.username_memory register failed: {}", error.diagnostic_log_line())
                        }
                    }
                }
            }
            let cached = if self.intent.allows_reuse() {
                crate::atrust_session_cache::load(self.profile, &self.gateway)
            } else { Ok(None) };
            log::info!(target: "exv.atrust_auth", "auth.session_cache result={} exists={}",
                match &cached { Ok(Some(_)) => "hit", Ok(None) => "miss", Err(_) => "error" },
                match &cached { Ok(Some(_)) => "true", Ok(None) => "false", Err(_) => "unknown" });
            let gateway = self.gateway.clone();
            if let Ok(cookies) = self
                .call(
                    "Network.getCookies",
                    json!({"urls":[gateway]}),
                    AuthOperation::Open,
                )
                .await
            {
                log::info!(target: "exv.atrust_auth", "auth.profile_cookies names={}", cookie_name_set(&cookies["cookies"]));
            }
            if let Ok(Some(saved)) = cached {
                let cookies: Vec<Value> = saved.cookie_header().split(';').filter_map(|field| {
                    let (name, value) = field.trim().split_once('=')?;
                    Some(json!({"name":name,"value":value,"url":self.gateway,"secure":true,"httpOnly":true}))
                }).collect();
                self.call(
                    "Network.setCookies",
                    json!({"cookies":cookies}),
                    AuthOperation::Open,
                )
                .await?;
            }
            self.call(
                "Network.setUserAgentOverride",
                json!({"userAgent":self.user_agent}),
                AuthOperation::Open,
            )
            .await?;
            self.call(
                "Network.setBlockedURLs",
                json!({"urls":BLOCKED_URL_PATTERNS}),
                AuthOperation::Open,
            )
            .await?;
            Ok(())
        })
    }
    fn navigate(&mut self, url: String) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.call("Page.navigate", json!({"url":url}), AuthOperation::Navigate)
                .await?;
            Ok(())
        })
    }
    fn observe(&mut self) -> ChannelFuture<'_, PageObservation> {
        Box::pin(async move {
            self.sequence += 1;
            let result = self
                .call(
                    "Runtime.evaluate",
                    json!({"expression":PAGE_FACTS_EXPRESSION,"returnByValue":true}),
                    AuthOperation::Observe,
                )
                .await;
            let facts = result.as_ref().ok().and_then(parse_page_facts);
            if result.is_err() {
                let target = self.target.clone().unwrap_or_default();
                if self
                    .cdp
                    .as_deref_mut()
                    .unwrap()
                    .call("Target.getTargetInfo", json!({"targetId":target}), None)
                    .await
                    .is_err()
                {
                    self.legacy_failure = Some("登录窗口已关闭".into());
                    return Err(AuthError::at(
                        AuthErrorCode::ChannelTransportFailed,
                        AuthOperation::Observe,
                    ));
                }
            }
            Ok(match facts {
                Some(f) => PageObservation {
                    sequence: self.sequence,
                    url: Observation::Observed(f.href),
                    origin: Observation::Observed(f.origin),
                    dom: Observation::Observed(DomFacts {
                        challenge_visible: false,
                        client_install_marker: f.marker,
                        shortcut_page: f.shortcut,
                        download_page: f.download,
                        body_has_failure_phrase: f.has_fail
                            || f.body_head.as_deref().is_some_and(text_is_failed_like),
                        body_char_count: f.len,
                        error_container_text: f.error_text,
                        short_body_text: f.body_head,
                    }),
                    startup: None,
                    coherent: true,
                },
                None => PageObservation {
                    sequence: self.sequence,
                    url: Observation::NotYetObserved,
                    origin: Observation::NotYetObserved,
                    dom: Observation::NotYetObserved,
                    startup: None,
                    coherent: false,
                },
            })
        })
    }
    fn probe_session(&mut self) -> ChannelFuture<'_, SessionStatus> {
        Box::pin(async move {
            let gateway = self.gateway.clone();
            let cookies = self
                .call(
                    "Network.getCookies",
                    json!({"urls":[gateway]}),
                    AuthOperation::Probe,
                )
                .await?;
            let cookies = cookies["cookies"].as_array().ok_or_else(|| {
                AuthError::at(AuthErrorCode::SessionInvalid, AuthOperation::Probe)
            })?;
            // 仅取配置网关允许集合；无 sid 仍可明确探测未登录，不等同提取会话。
            let header = zeroize::Zeroizing::new(
                exv_vpn_atrust::auth::browser_session::GATEWAY_COOKIE_NAMES
                    .iter()
                    .filter_map(|name| {
                        cookies
                            .iter()
                            .find(|c| c["name"].as_str() == Some(*name))
                            .and_then(|c| c["value"].as_str())
                            .map(|value| format!("{name}={value}"))
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            #[cfg(test)]
            let injected = self
                .cdp
                .as_deref_mut()
                .and_then(|cdp| cdp.probe_io.as_mut())
                .map(|probe| {
                    probe(
                        self.gateway.clone(),
                        header.to_string(),
                        self.user_agent.clone(),
                    )
                });
            #[cfg(test)]
            let probe = match injected {
                Some(probe) => probe.await,
                None => {
                    crate::atrust_session_probe::probe_auth_config(
                        &self.gateway,
                        &header,
                        &self.user_agent,
                    )
                    .await
                }
            };
            #[cfg(not(test))]
            let probe = crate::atrust_session_probe::probe_auth_config(
                &self.gateway,
                &header,
                &self.user_agent,
            )
            .await;
            crate::browser_auth::probe_status(probe)
        })
    }
    fn reuse_session(&mut self, _: Duration) -> ChannelFuture<'_, ReuseOutcome> {
        Box::pin(async {
            Err(AuthError::at(
                AuthErrorCode::UnsupportedOperation,
                AuthOperation::Reuse,
            ))
        })
    }
    fn inject_once(&mut self, _: CredentialDelivery) -> ChannelFuture<'_, InjectionOutcome> {
        Box::pin(async {
            Err(AuthError::at(
                AuthErrorCode::UnsupportedOperation,
                AuthOperation::Inject,
            ))
        })
    }
    fn extract_session(&mut self) -> ChannelFuture<'_, Option<SessionArtifact>> {
        Box::pin(async move {
            let gateway = self.gateway.clone();
            let cookies = self
                .call(
                    "Network.getCookies",
                    json!({"urls":[gateway]}),
                    AuthOperation::Extract,
                )
                .await?;
            let pairs = cookies["cookies"]
                .as_array()
                .ok_or_else(|| {
                    AuthError::at(AuthErrorCode::SessionInvalid, AuthOperation::Extract)
                })?
                .iter()
                .filter_map(|c| {
                    Some((
                        c["name"].as_str()?.to_owned(),
                        c["value"].as_str()?.to_owned(),
                    ))
                })
                .collect();
            match session_from_cookie_pairs(&self.gateway, &self.user_agent, pairs) {
                Ok(session) => Ok(Some(SessionArtifact {
                    session,
                    source: SessionSource::BrowserCookieJar,
                })),
                Err(CookieSessionError::MissingSession) => Ok(None),
                Err(_) => Err(AuthError::at(
                    AuthErrorCode::SessionInvalid,
                    AuthOperation::Extract,
                )),
            }
        })
    }
    fn complete_session<'a>(&'a mut self, artifact: &'a SessionArtifact) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            if self.completed {
                return Ok(());
            }
            let username = if self.intent == AuthIntent::FreshManual {
                self.call("Network.getCookies", json!({"urls":[crate::school_auto_login::SCHOOL_LOGIN_ORIGIN]}), AuthOperation::Extract).await.ok()
                    .and_then(|reply| reply["cookies"].as_array().and_then(|cookies| cookies.iter().find(|cookie| cookie["name"] == crate::browser_cache_profile::USERNAME_COOKIE)).and_then(|cookie| cookie["value"].as_str()).and_then(crate::browser_cache_profile::observed_username))
            } else { None };
            if let Some(username) = &username {
                let _ = crate::browser_cache_profile::remember_username(self.profile, &self.gateway, username);
            }
            let saved = if let Some(username) = &username {
                crate::atrust_session_cache::save_with_username(self.profile, &artifact.session, username)
            } else { crate::atrust_session_cache::save(self.profile, &artifact.session) };
            log::info!(target: "exv.atrust_auth", "auth.session_cache_write result={} exists={}", if saved.is_ok() { "ok" } else { "error" }, if saved.is_ok() { "true" } else { "unknown" });
            saved.map_err(|message| {
                self.legacy_failure = Some(message);
                AuthError::at(AuthErrorCode::SessionStorageFailed, AuthOperation::Extract)
            })?;
            self.completed = true;
            Ok(())
        })
    }
    fn present(&mut self, _: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome> {
        Box::pin(async { Ok(PresentationOutcome::AlwaysVisible) })
    }
    fn wait_for_change(&mut self, max_wait: Duration) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            tokio::time::sleep(max_wait).await;
            Ok(())
        })
    }
    fn close(&mut self) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.cleanup_started.get_or_insert_with(tokio::time::Instant::now);
            if self.closed {
                return Ok(());
            }
            if self.pending.is_some() {
                if self.committed.load(Ordering::Acquire) {
                    match tokio::time::timeout(self.create_settle, self.pending.as_mut().unwrap())
                        .await
                    {
                        Ok((cdp, result)) => {
                            let _ = self.accept_created(cdp, result);
                        }
                        Err(_) => {
                            self.cleanup_unconfirmed = true;
                            self.pending = None;
                        }
                    }
                } else {
                    self.pending = None;
                }
            }
            if let (Some(cdp), Some(target)) = (self.cdp.as_deref_mut(), self.target.take()) {
                let _ = finish_target(cdp, &target, Ok(())).await;
            }
            if self.cleanup_unconfirmed {
                log::warn!(target: "exv.atrust_auth", "auth.close_target cleanup_unconfirmed reason=target_id_unavailable");
            }
            self.closed = true;
            Ok(())
        })
    }
}

pub(super) struct DefaultDependencies<'a> {
    pub profile: &'a Path,
}
impl AuthEventSink for DefaultDependencies<'_> {
    fn milestone(&self, milestone: AuthMilestone) {
        log::info!(target:"exv.atrust_auth", "auth.stage stage={milestone:?}");
    }
    fn form_login_reached(&self) {}
}
impl SessionSnapshotProvider for DefaultDependencies<'_> {
    fn snapshot<'a>(&'a self, _: &'a str) -> ChannelFuture<'a, Option<SessionArtifact>> {
        Box::pin(async { Ok(None) })
    }
}
impl SessionCacheInvalidator for DefaultDependencies<'_> {
    fn forget<'a>(&'a self, gateway: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            crate::atrust_session_cache::forget(self.profile, gateway);
            Ok(())
        })
    }
}

pub(super) async fn acquire_connected(
    cdp: &mut Cdp,
    gateway: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    create_settle: Duration,
    attempt: &AuthAttempt,
    events: &dyn AuthEventSink,
    snapshots: &dyn SessionSnapshotProvider,
) -> Result<BrowserSession, BrowserAuthError> {
    acquire_connected_with_intent(cdp, gateway, profile, action, create_settle, attempt, events, snapshots, AuthIntent::Automatic).await.0
}

/// 手动候选只来自本次浏览器，旧全局快照端口保持未调用。
struct IntentDependencies<'a> {
    intent: AuthIntent,
    source: &'a dyn SessionSnapshotProvider,
    profile: &'a Path,
}
impl SessionSnapshotProvider for IntentDependencies<'_> {
    fn snapshot<'a>(&'a self, gateway: &'a str) -> ChannelFuture<'a, Option<SessionArtifact>> {
        Box::pin(async move {
            if self.intent.allows_reuse() { self.source.snapshot(gateway).await }
            else { Ok(None) }
        })
    }
}

impl SessionCacheInvalidator for IntentDependencies<'_> {
    fn forget<'a>(&'a self, gateway: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            if self.intent.allows_reuse() { crate::atrust_session_cache::forget(self.profile, gateway); }
            Ok(())
        })
    }
}

pub(super) async fn acquire_connected_with_intent(
    cdp: &mut Cdp,
    gateway: &str,
    profile: &Path,
    action: &mut watch::Receiver<LoginAction>,
    create_settle: Duration,
    attempt: &AuthAttempt,
    events: &dyn AuthEventSink,
    snapshots: &dyn SessionSnapshotProvider,
    intent: AuthIntent,
) -> (Result<BrowserSession, BrowserAuthError>, tokio::time::Instant) {
    let dependencies = IntentDependencies { intent, source: snapshots, profile };
    let mut adapter = SystemBrowserAdapter::new_with_intent(cdp, profile, create_settle, intent);
    let mut config = AuthConfig::new(
        gateway.into(),
        PLAIN_CHROME_USER_AGENT.into(),
        auth_config_path(false),
    );
    config.initial_presentation = PresentationIntent::Visible;
    config.gateway_stall_from_page = true;
    config.cleanup_budget = create_settle + Duration::from_secs(2) + Duration::from_millis(100);
    let cleanup_budget = config.cleanup_budget;
    let orchestrator = AuthOrchestrator {
        config,
        events,
        snapshots: &dependencies,
        credentials: None,
        cache: &dependencies,
        attempt,
    };
    let signal = |action| {
        if action == LoginAction::Cancel {
            Some(AuthControl::Cancel)
        } else {
            None
        }
    };
    let (sender, mut receiver) = watch::channel(signal(*action.borrow()));
    let result = {
        let mut channels: [&mut dyn ChannelTrait; 1] = [&mut adapter];
        let acquisition = orchestrator.acquire(&mut channels, &mut receiver);
        tokio::pin!(acquisition);
        let mut alive = true;
        loop {
            tokio::select! { biased;
                changed=action.changed(), if alive => {
                    if changed.is_err() { alive=false; sender.send_replace(Some(AuthControl::Cancel)); }
                    else { sender.send_replace(signal(*action.borrow_and_update())); }
                }
                result=&mut acquisition => break result,
            }
        }
    };
    // 系统线的页面故障也收束本 target；内嵌故障面的保留语义由其宿主继续决定。
    match tokio::time::timeout(cleanup_budget, adapter.close()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => events.failure(&error),
        Err(_) => events.failure(&AuthError::at(
            AuthErrorCode::ChannelTransportFailed,
            AuthOperation::Close,
        )),
    }
    let result = match result {
        Ok(AcquisitionOutcome::Ready(artifact)) => Ok(artifact.session),
        Ok(AcquisitionOutcome::Control(control)) => {
            Err(BrowserAuthError::control(control, "浏览器登录已取消"))
        }
        Ok(AcquisitionOutcome::Interactive(_)) => {
            Err(BrowserAuthError::Host("浏览器尚未取得网关会话".into()))
        }
        Err(error) => {
            let message = match error.code {
                AuthErrorCode::ClientInstallRequired
                | AuthErrorCode::FailedPage
                | AuthErrorCode::GatewayThrottled
                | AuthErrorCode::Stalled => error.legacy_message(),
                AuthErrorCode::AcquisitionTimeout => "浏览器登录超时，请重试".into(),
                AuthErrorCode::SessionMissing => "浏览器尚未取得网关会话".into(),
                AuthErrorCode::SessionInvalid => "浏览器会话格式无效".into(),
                AuthErrorCode::LoginEntryInvalid => "网关浏览器认证入口不是有效的本站路径".into(),
                _ => adapter
                    .legacy_failure
                    .take()
                    .unwrap_or_else(|| error.legacy_message()),
            };
            Err(BrowserAuthError::auth(error, message))
        }
    };
    let result = result.map_err(|mut reason| {
        if adapter.cleanup_unconfirmed {
            match &mut reason {
                BrowserAuthError::Auth {
                    compatibility_message,
                    ..
                }
                | BrowserAuthError::Control {
                    compatibility_message,
                    ..
                } => compatibility_message.push_str("；本次标签页清理未确认"),
                BrowserAuthError::Host(message) => message.push_str("；本次标签页清理未确认"),
                BrowserAuthError::FallBackEmbedded => {},
            }
        }
        reason
    });
    (result, adapter.cleanup_started.unwrap_or_else(tokio::time::Instant::now))
}
