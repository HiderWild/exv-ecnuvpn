//! X3a 内嵌 WebView 宿主原语；分类、预算、取消和认证循环由统一编排器驱动。
use super::*;
use exv_core::browser_auth::*;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

fn error(code: AuthErrorCode, operation: AuthOperation) -> AuthError {
    AuthError {
        failure_kind: None,
        code,
        operation,
        native_code: None,
        server_detail: None,
    }
}

/// 实际窗口呈现策略；用户直接通过旧 UI 显示的窗口也保持可见，不再隐藏回去。
#[derive(Default)]
struct PresentationPolicy;
impl PresentationPolicy {
    async fn apply(window: &WebviewWindow, intent: PresentationIntent) -> PresentationOutcome {
        if intent == PresentationIntent::Visible && !window.is_visible().unwrap_or(false) {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let target = window.clone();
            if window
                .run_on_main_thread(move || {
                    if target.show().is_err() {
                        log::warn!(target: "exv.atrust_auth", "embedded.show_failed");
                    }
                    let _ = tx.send(());
                })
                .is_ok()
            {
                let _ = rx.await;
            } else {
                log::warn!(target: "exv.atrust_auth", "embedded.show_dispatch_failed");
            }
        }
        if window.is_visible().unwrap_or(false) {
            PresentationOutcome::Visible
        } else {
            PresentationOutcome::Hidden
        }
    }
}

pub(super) struct OwnedWebViewAdapter<'a> {
    pub app: AppHandle,
    pub gateway: String,
    pub gateway_url: tauri::Url,
    pub auth_path: String,
    pub profile: PathBuf,
    intent: AuthIntent,
    pub cache_profile: PathBuf,
    pub cached: Option<BrowserSession>,
    pub user_agent: String,
    pub snapshot_user_agent: Arc<std::sync::Mutex<String>>,
    pub generation: u64,
    pub label: String,
    pub snapshot_label: Arc<std::sync::Mutex<String>>,
    round: u8,
    pub login: &'a crate::kernel::connection_request::ConnectionRequests,
    window: Option<WebviewWindow>,
    pub interaction: Arc<AtomicBool>,
    settled: Arc<AtomicBool>,
    destroyed: Arc<AtomicBool>,
    initial: InitialNavigation,
    sequence: u64,
    portal_cleaned: bool,
    completed: bool,
    authenticated_username: Option<String>,
    manual_takeover: bool,
    username_cookie: String,
    poll_tick: tokio::time::Interval,
    page_load: Option<tokio::sync::mpsc::UnboundedReceiver<tauri::webview::PageLoadEvent>>,
    last_location: Option<(String, String)>,
    last_probe: Option<(bool, bool)>,
    last_cookie_names: Option<String>,
    last_probe_status: Option<String>,
    last_eval_ok: Option<bool>,
    last_startup: Option<String>,
    startup_navigation: Arc<AtomicU64>,
    last_probe_ok: std::time::Instant,
    no_probe_warned: bool,
    started: std::time::Instant,
}

impl<'a> OwnedWebViewAdapter<'a> {
    pub(super) fn context_released(&self) -> bool {
        self.window.is_none() || self.destroyed.load(Ordering::Acquire)
    }

    pub fn prepare(
        app: &AppHandle,
        gateway: String,
        generation: u64,
        login: &'a crate::kernel::connection_request::ConnectionRequests,
        intent: AuthIntent,
    ) -> Result<Self, String> {
        Self::prepare_with_profiles(app, gateway, generation, login, intent, None)
    }

    pub(super) fn prepare_with_profiles(
        app: &AppHandle,
        gateway: String,
        generation: u64,
        login: &'a crate::kernel::connection_request::ConnectionRequests,
        intent: AuthIntent,
        profiles: Option<&exv_core::browser_cache_profile::BrowserDataDirs>,
    ) -> Result<Self, String> {
        let gateway_url = gateway.parse().map_err(|_| "aTrust 网关地址无效")?;
        let local_dirs;
        let browser_dirs = match profiles {
            Some(dirs) => dirs,
            None => {
                local_dirs = super::super::browser_cache_profile::browser_data_dirs()?;
                &local_dirs
            }
        };
        let profile = browser_dirs.owned_webview_profile(|| {
            app.path()
                .app_data_dir()
                .map_err(|_| "无法定位 aTrust 内嵌浏览器数据目录".to_owned())
        })?;
        let cache_profile = browser_dirs.cache_profile().to_path_buf();
        // 这里只准备 owner 元数据；HTTP 成功不创建 WebView profile 或读取旧缓存。
        let user_agent = exv_core::atrust_browser::PLAIN_CHROME_USER_AGENT.to_owned();
        let started = std::time::Instant::now();
        Ok(Self {
            app: app.clone(),
            gateway,
            gateway_url,
            auth_path: exv_core::atrust_browser::auth_config_path(false),
            profile,
            intent,
            cache_profile,
            cached: None,
            snapshot_user_agent: Arc::new(std::sync::Mutex::new(user_agent.clone())),
            user_agent,
            generation,
            label: String::new(),
            snapshot_label: Arc::new(std::sync::Mutex::new(String::new())),
            round: 0,
            login,
            window: None,
            interaction: Arc::new(AtomicBool::new(false)),
            settled: Arc::new(AtomicBool::new(false)),
            destroyed: Arc::new(AtomicBool::new(false)),
            initial: InitialNavigation::Pending,
            sequence: 0,
            portal_cleaned: false,
            completed: false,
            authenticated_username: None,
            manual_takeover: false,
            username_cookie: String::new(),
            poll_tick: tokio::time::interval_at(
                tokio::time::Instant::now() + SESSION_POLL_TICK,
                SESSION_POLL_TICK,
            ),
            page_load: None,
            last_location: None,
            last_probe: None,
            last_cookie_names: None,
            last_probe_status: None,
            last_eval_ok: None,
            last_startup: None,
            startup_navigation: Arc::new(AtomicU64::new(0)),
            last_probe_ok: started,
            no_probe_warned: false,
            started,
        })
    }

    fn window(&self, operation: AuthOperation) -> Result<&WebviewWindow, AuthError> {
        self.window
            .as_ref()
            .ok_or_else(|| error(AuthErrorCode::ChannelTransportFailed, operation))
    }

    async fn fill(&mut self, credentials: CredentialDelivery) -> InjectionOutcome {
        use exv_core::school_auto_login::{FillOutcome, RESULT_COOKIE};
        let outcome = match self.window(AuthOperation::Inject) {
            Err(_) => FillOutcome::Error,
            Ok(window) => {
                // 整个 future 持有零化交付与脚本；取消 drop 时不留下临时明文副本。
                let script = credentials.webview_fill_script();
                if window.eval(script.as_str()).is_err() {
                    FillOutcome::Error
                } else {
                    let school_url = exv_core::school_auto_login::SCHOOL_LOGIN_ORIGIN
                        .parse::<tauri::Url>()
                        .expect("学校 origin 常量有效");
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                    loop {
                        if tokio::time::Instant::now() >= deadline {
                            break FillOutcome::Error;
                        }
                        tokio::time::sleep(Duration::from_millis(400)).await;
                        let Ok(cookies) = read_cookies(window.clone(), school_url.clone()).await
                        else {
                            break FillOutcome::Error;
                        };
                        if let Some((_, value)) =
                            cookies.iter().find(|(name, _)| name == RESULT_COOKIE)
                        {
                            let outcome = exv_core::school_auto_login::decode_result_cookie(value)
                                .unwrap_or(FillOutcome::Error);
                            delete_probe_cookie(window, RESULT_COOKIE);
                            break outcome;
                        }
                    }
                }
            }
        };
        log::info!(target: "exv.atrust_auth", "embedded.school_auto_login outcome={}", exv_core::school_auto_login::outcome_log_field(outcome));
        match outcome {
            FillOutcome::Submitted => {
                self.authenticated_username = Some(credentials.username.to_string());
                InjectionOutcome::Submitted
            }
            FillOutcome::FormNotReady | FillOutcome::NotSchoolPage => InjectionOutcome::NotReady,
            FillOutcome::ChallengeVisible => InjectionOutcome::ChallengeVisible,
            FillOutcome::ServiceMismatch => InjectionOutcome::ServiceMismatch,
            FillOutcome::Error => InjectionOutcome::Failed,
        }
    }
}

impl ChannelTrait for OwnedWebViewAdapter<'_> {
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            injection: if self.intent.allows_reuse() {
                InjectionCapability::SchoolFormOnce
            } else {
                InjectionCapability::Unavailable
            },
            can_render: true,
            visibility: VisibilityCapability::Controlled,
            can_read_cookiejar: true,
            can_write_cookiejar: true,
            observation: ObservationFidelity::CookieSideChannel,
            probe_scope: ProbeScope::IndependentHttp,
        }
    }
    fn initial_navigation(&self) -> InitialNavigation {
        self.initial
    }
    fn open(&mut self, _request: ChannelRequest) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            // 重开仍属于原 owner；旧回调只持有旧 settled/导航代次，不能取消第二轮。
            begin_window_round(&mut self.label, &mut self.settled);
            *self
                .snapshot_label
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = self.label.clone();
            self.round += 1;
            self.destroyed = Arc::new(AtomicBool::new(false));
            self.initial = InitialNavigation::Pending;
            self.sequence = 0;
            self.portal_cleaned = false;
            self.completed = false;
            self.authenticated_username = None;
            self.manual_takeover = false;
            self.username_cookie =
                format!("__exv_manual_username_{}", uuid::Uuid::new_v4().simple());
            self.page_load = None;
            self.last_location = None;
            self.last_probe = None;
            self.last_cookie_names = None;
            self.last_probe_status = None;
            self.last_eval_ok = None;
            self.last_startup = None;
            self.startup_navigation = Arc::new(AtomicU64::new(0));
            self.started = std::time::Instant::now();
            self.last_probe_ok = self.started;
            self.no_probe_warned = false;
            self.poll_tick = tokio::time::interval_at(
                tokio::time::Instant::now() + SESSION_POLL_TICK,
                SESSION_POLL_TICK,
            );
            // S4c（2026-10-26）：FreshManual 使用网关级持久 data_directory，跨次登录
            // 与轮次重启复用；已认证 SSO 会话与浏览器登录痕迹随目录保留，不再销毁。
            let data_directory = webview_data_directory(self.intent, &self.profile, &self.gateway)?;
            let (cached, user_agent) =
                gateway_seed(self.intent, &self.cache_profile, &self.gateway);
            self.cached = cached;
            self.user_agent = user_agent;
            *self
                .snapshot_user_agent
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = self.user_agent.clone();
            for (label, stale) in self.app.webview_windows() {
                if label.starts_with("atrust-auth-") {
                    let _ = stale.close();
                }
            }
            let probe_url = format!("{}{}", self.gateway, self.auth_path);
            let initial_target = if self.cached.is_none() {
                let probe_started = std::time::Instant::now();
                // 预探针本身是 open 的有界原语片段，由统一 operation 取消；没有认证循环。
                let probe = exv_core::atrust_session_probe::probe_auth_config(
                    &self.gateway,
                    "",
                    &self.user_agent,
                )
                .await;
                let target = direct_login_target(false, &probe, &self.gateway);
                let result = match &probe {
                    Ok(s) if s.code_ok && !s.is_login => "not_logged_in".to_owned(),
                    Ok(_) => "logged_in".to_owned(),
                    Err(e) => e.label(),
                };
                log::info!(target: "exv.atrust_auth", "embedded.pre_window_probe result={} direct={} elapsed_ms={}", result, target.is_some(), probe_started.elapsed().as_millis());
                target
            } else {
                None
            };
            let direct = initial_target.is_some();
            let initial_url = initial_target
                .unwrap_or(probe_url)
                .parse()
                .map_err(|_| error(AuthErrorCode::LoginEntryInvalid, AuthOperation::Open))?;
            let browser_args = embedded_browser_args();
            log::info!(target: "exv.atrust_auth", "embedded.browser_args args={} data_directory={}", browser_args, data_directory.display());
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let generation = self.generation;
            let round = self.round;
            let startup_navigation = self.startup_navigation.clone();
            let window = WebviewWindowBuilder::new(
                &self.app,
                self.label.clone(),
                WebviewUrl::External(initial_url),
            )
            .user_agent(&self.user_agent)
            .initialization_script(PAGE_STARTUP_SCRIPT)
        .initialization_script_for_all_frames(include_str!("owned_portal_guard.js"))
            .title("学校统一身份认证")
            .inner_size(540.0, 700.0)
            .min_inner_size(380.0, 480.0)
            .initialization_script(owned_username_script(
                self.intent,
                &self.cache_profile,
                &self.gateway,
                &self.username_cookie,
            ))
            .resizable(true)
            .center()
            .visible(self.intent == AuthIntent::FreshManual)
            .general_autofill_enabled(self.intent.allows_reuse())
            .additional_browser_args(&browser_args)
            .data_directory(data_directory)
            .on_new_window(|url, _| {
                if owned_navigation_allowed(&url) {
                    tauri::webview::NewWindowResponse::Allow
                } else {
                    tauri::webview::NewWindowResponse::Deny
                }
            })
            .on_navigation(owned_navigation_allowed)
            .on_page_load(move |_, payload| {
                // 仅约束新增启动诊断；同 URL 完整重载也会使在途采样失效。
                let navigation = startup_navigation.fetch_add(1, Ordering::AcqRel) + 1;
                log::info!(target: "exv.atrust_auth", "embedded.navigation channel=owned_webview owner={} navigation={} event={:?}", generation, navigation, payload.event());
                let _ = tx.send(payload.event());
            })
            .build()
            .map_err(|_| error(AuthErrorCode::ChannelOpenFailed, AuthOperation::Open))?;
            // 在任何下一 await 前登记资源；open 被取消后统一 close 能找到刚建的本 owner 窗口。
            self.window = Some(window.clone());
            self.page_load = Some(rx);
            self.initial = if direct {
                InitialNavigation::LoginStarted
            } else {
                InitialNavigation::ProbeStarted
            };
            set_window_title(&window, WINDOW_TITLE_WAITING);
            let login = self.login.clone();
            let settled = self.settled.clone();
            let destroyed = self.destroyed.clone();
            let generation = self.generation;
            let timer = self.started;
            window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    destroyed.store(true, Ordering::Release);
                    log::info!(target: "exv.atrust_auth", "embedded.window_destroyed elapsed_ms={}", timer.elapsed().as_millis());
                    if !settled.load(Ordering::Acquire) { login.cancel_if_still_owner(generation); }
                }
            });
            log::info!(target: "exv.atrust_auth", "embedded.window_created channel=owned_webview identity={} owner={} round={} elapsed_ms={}", if self.user_agent.contains("aTrustTray-") { "official_tray" } else { "session_browser" }, self.generation, self.round, self.started.elapsed().as_millis());
            if let Some(session) = &self.cached {
                if restore_cached_cookies(&window, &self.gateway, session)
                    .await
                    .is_err()
                {
                    exv_core::atrust_session_cache::forget(&self.cache_profile, &self.gateway);
                    log::warn!(target: "exv.atrust_auth", "embedded.session_cache result=forgotten reason=restore_failed");
                }
            }
            Ok(())
        })
    }
    fn navigate(&mut self, url: String) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            let target = url
                .parse()
                .map_err(|_| error(AuthErrorCode::LoginEntryInvalid, AuthOperation::Navigate))?;
            self.window(AuthOperation::Navigate)?
                .navigate(target)
                .map_err(|_| {
                    error(
                        AuthErrorCode::ChannelTransportFailed,
                        AuthOperation::Navigate,
                    )
                })?;
            log::info!(target: "exv.atrust_auth", "embedded.login_navigation elapsed_ms={}", self.started.elapsed().as_millis());
            Ok(())
        })
    }
    fn observe(&mut self) -> ChannelFuture<'_, PageObservation> {
        Box::pin(async move {
            if self.app.get_webview_window(&self.label).is_none() {
                self.login.cancel_if_still_owner(self.generation);
                return Err(error(
                    AuthErrorCode::ChannelTransportFailed,
                    AuthOperation::Observe,
                ));
            }
            let window = self.window(AuthOperation::Observe)?.clone();
            let before_navigation = self.startup_navigation.load(Ordering::Acquire);
            let before = window.url().ok();
            if let Some(url) = &before {
                let location = (
                    url.host_str().unwrap_or_default().to_owned(),
                    url.path().to_owned(),
                );
                if self.last_location.as_ref() != Some(&location) {
                    log::info!(target: "exv.atrust_auth", "embedded.url_migration host={} path={}", location.0, location.1);
                    self.last_location = Some(location);
                }
                if !self.portal_cleaned && url.origin().ascii_serialization() == self.gateway {
                    self.portal_cleaned = true;
                    let result = if window.eval(PORTAL_STORAGE_CLEANUP_SCRIPT).is_ok() {
                        "ok"
                    } else {
                        "failure"
                    };
                    log::info!(target: "exv.atrust_auth", "embedded.portal_storage_cleanup result={} elapsed_ms={}", result, self.started.elapsed().as_millis());
                }
            }
            self.sequence += 1;
            let mut payload = None;
            let mut startup = None;
            // authConfig JSON 不采 DOM；Cookie side channel 只报告原始事实。
            if let Some(url) = &before
                && !url_is_auth_config_path(url.as_str(), &self.auth_path)
            {
                let mut script = PAGE_FACTS_SCRIPT.replace("'__exv_page_class='", &format!("'__exv_page_class={}~'+encodeURIComponent(location.href).replace(/~/g,'%7E')+'~'", self.sequence));
                // 与事实使用同一次采样序号和位置；诊断失败不能阻断既有页面观察。
                script.push_str(&format!(";try{{document.cookie='__exv_page_startup={}~'+encodeURIComponent(location.href).replace(/~/g,'%7E')+'~'+(window.__exvAuthStartupSample?window.__exvAuthStartupSample():'unavailable')+'; Path=/; Secure; SameSite=Lax';}}catch(_){{}}", self.sequence));
                let ok = window.eval(&script).is_ok();
                if self.last_eval_ok != Some(ok) {
                    log::info!(target: "exv.atrust_auth", "embedded.eval script=page_facts result={}", if ok { "ok" } else { "failure" });
                    self.last_eval_ok = Some(ok);
                }
                if ok {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    if let Ok(cookies) = read_cookies(window.clone(), url.clone()).await {
                        for (name, value) in cookies {
                            if name == PAGE_CLASS_COOKIE {
                                payload = Some(value);
                            } else if name == PAGE_STARTUP_COOKIE {
                                startup = Some(value);
                            }
                        }
                    }
                    delete_probe_cookie(&window, PAGE_CLASS_COOKIE);
                    delete_probe_cookie(&window, PAGE_STARTUP_COOKIE);
                }
            }
            let after = window.url().ok();
            let diagnostic = startup.as_deref().and_then(|value| {
                bound_startup_diagnostic(
                    self.sequence,
                    before.as_ref().map(|u| u.as_str()),
                    after.as_ref().map(|u| u.as_str()),
                    value,
                    (
                        before_navigation,
                        self.startup_navigation.load(Ordering::Acquire),
                    ),
                )
            });
            if self.sequence == 1 || self.sequence % 10 == 0 || self.last_startup != diagnostic {
                log::info!(target: "exv.atrust_auth", "embedded.page_startup channel=owned_webview owner={} round={} navigation={} sample={} {} elapsed_ms={}", self.generation, self.round, before_navigation, self.sequence, diagnostic.as_deref().unwrap_or("result=unavailable"), self.started.elapsed().as_millis());
                crate::kernel::auth_diagnostics::startup(&self.app, self.generation, u64::from(self.round), before_navigation, self.sequence, diagnostic.as_deref());
                self.last_startup = diagnostic.clone();
            }
            let startup_facts = diagnostic
                .as_ref()
                .and_then(|_| startup.as_deref())
                .and_then(|payload| {
                    exv_core::browser_auth::StartupFacts::parse(payload.splitn(3, '~').nth(2)?)
                });
            let mut observation = observed_page(
                self.sequence,
                before.as_ref().map(|u| u.as_str()),
                after.as_ref().map(|u| u.as_str()),
                payload.as_deref(),
                (
                    before_navigation,
                    self.startup_navigation.load(Ordering::Acquire),
                ),
            );
            observation.startup = startup_facts;
            Ok(observation)
        })
    }
    fn probe_session(&mut self) -> ChannelFuture<'_, SessionStatus> {
        Box::pin(async move {
            let window = self.window(AuthOperation::Probe)?.clone();
            let cookies = read_cookies(window, self.gateway_url.clone())
                .await
                .map_err(|_| error(AuthErrorCode::ChannelTransportFailed, AuthOperation::Probe))?;
            let names = gateway_cookie_names(&cookies);
            if self.last_cookie_names.as_ref() != Some(&names) {
                log::info!(target: "exv.atrust_auth", "embedded.gateway_cookies names={names}");
                self.last_cookie_names = Some(names);
            }
            match probe_gateway_cookies(
                &self.gateway,
                &cookies,
                &self.user_agent,
                |gateway, header, agent| {
                    Box::pin(exv_core::atrust_session_probe::probe_auth_config(
                        gateway, header, agent,
                    ))
                },
            )
            .await
            {
                Ok(state) => {
                    self.last_probe_ok = std::time::Instant::now();
                    if self.last_probe != Some((state.code_ok, state.is_login)) {
                        log::info!(target: "exv.atrust_auth", "embedded.host_probe result=ok code_ok={} is_login={}", state.code_ok, state.is_login);
                        self.last_probe = Some((state.code_ok, state.is_login));
                    }
                    Ok(if state.is_login {
                        SessionStatus::LoggedIn
                    } else {
                        SessionStatus::LoggedOut {
                            first_auth: state.first_auth,
                        }
                    })
                }
                Err(reason) => {
                    let label = reason.label();
                    if self.last_probe_status.as_ref() != Some(&label) {
                        log::info!(target: "exv.atrust_auth", "embedded.host_probe result={label}");
                        self.last_probe_status = Some(label);
                    }
                    Err(error(
                        AuthErrorCode::SessionProbeFailed,
                        AuthOperation::Probe,
                    ))
                }
            }
        })
    }
    fn reuse_session(&mut self, _budget: Duration) -> ChannelFuture<'_, ReuseOutcome> {
        Box::pin(async {
            Err(error(
                AuthErrorCode::UnsupportedOperation,
                AuthOperation::Reuse,
            ))
        })
    }
    fn inject_once(
        &mut self,
        credentials: CredentialDelivery,
    ) -> ChannelFuture<'_, InjectionOutcome> {
        Box::pin(async move {
            if !self.intent.allows_reuse() {
                return Err(error(
                    AuthErrorCode::UnsupportedOperation,
                    AuthOperation::Inject,
                ));
            }
            Ok(self.fill(credentials).await)
        })
    }
    fn extract_session(&mut self) -> ChannelFuture<'_, Option<SessionArtifact>> {
        Box::pin(async move {
            let cookies = read_cookies(
                self.window(AuthOperation::Extract)?.clone(),
                self.gateway_url.clone(),
            )
            .await
            .map_err(|_| {
                error(
                    AuthErrorCode::ChannelTransportFailed,
                    AuthOperation::Extract,
                )
            })?;
            if !has_session_cookie(&cookies) {
                return Ok(None);
            }
            let session = build_session_from_webview(&self.gateway, cookies, &self.user_agent)
                .map_err(|_| error(AuthErrorCode::SessionInvalid, AuthOperation::Extract))?;
            Ok(Some(SessionArtifact {
                session,
                source: exv_core::browser_auth::SessionSource::BrowserCookieJar,
            }))
        })
    }
    fn complete_session<'a>(&'a mut self, session: &'a SessionArtifact) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            if self.completed {
                return Ok(());
            }
            let window = self.window(AuthOperation::Extract)?.clone();
            // 原 UI 可直接显示 owner 窗口；成功前读取真实可见性也要撤销旧 provider。
            if window.is_visible().unwrap_or(false) {
                take_manual_identity(
                    &mut self.authenticated_username,
                    &mut self.manual_takeover,
                    self.completed,
                );
            }
            // 可取消的导出读取先完成；若等待中取消，不写成功缓存或标题。
            export_sso_session_jar(&window, &self.cache_profile).await;
            let manual = self.intent == AuthIntent::FreshManual || self.manual_takeover;
            let observed = if manual {
                read_cookies(
                    window.clone(),
                    exv_core::school_auto_login::SCHOOL_LOGIN_ORIGIN
                        .parse()
                        .expect("学校地址有效"),
                )
                .await
                .ok()
            } else {
                None
            };
            let saved = save_completed_identity(
                &self.cache_profile,
                &self.gateway,
                &session.session,
                &mut self.authenticated_username,
                manual,
                observed.as_deref(),
                &self.username_cookie,
            );
            match saved {
                Ok(()) => {
                    log::info!(target: "exv.atrust_auth", "embedded.session_cache_write result=ok exists=true")
                }
                Err(_) => {
                    log::warn!(target: "exv.atrust_auth", "embedded.session_cache_write result=error exists=unknown")
                }
            }
            set_window_title(&window, WINDOW_TITLE_SUCCESS);
            self.completed = true;
            log::info!(target: "exv.atrust_auth", "embedded.session_ready elapsed_ms={}", self.started.elapsed().as_millis());
            Ok(())
        })
    }
    fn present(&mut self, intent: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome> {
        Box::pin(async move {
            if intent == PresentationIntent::Visible {
                take_manual_identity(
                    &mut self.authenticated_username,
                    &mut self.manual_takeover,
                    self.completed,
                );
            }
            let outcome =
                PresentationPolicy::apply(self.window(AuthOperation::Present)?, intent).await;
            if matches!(
                outcome,
                PresentationOutcome::Visible | PresentationOutcome::AlwaysVisible
            ) {
                take_manual_identity(
                    &mut self.authenticated_username,
                    &mut self.manual_takeover,
                    self.completed,
                );
                self.interaction.store(true, Ordering::Release);
            }
            Ok(outcome)
        })
    }
    fn wait_for_change(&mut self, max_wait: Duration) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            if !self.no_probe_warned && self.last_probe_ok.elapsed() >= PROBE_IDLE_WARN_DURATION {
                self.no_probe_warned = true;
                log::warn!(target: "exv.atrust_auth", "embedded.probe_idle idle_ms={} elapsed_ms={} last_status={}", self.last_probe_ok.elapsed().as_millis(), self.started.elapsed().as_millis(), self.last_probe_status.as_deref().unwrap_or("none"));
            }
            if let Some(rx) = &mut self.page_load {
                tokio::select! { biased;
                    _ = self.poll_tick.tick() => {},
                    _ = tokio::time::sleep(max_wait) => {},
                    event = rx.recv() => { if event.is_none() { self.page_load = None; tokio::time::sleep(max_wait).await; } }
                }
            } else {
                tokio::time::sleep(max_wait).await;
            }
            Ok(())
        })
    }
    fn close(&mut self) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.settled.store(true, Ordering::Release);
            if let Some(window) = self.window.as_ref() {
                if window.is_visible().unwrap_or(false) {
                    self.interaction.store(true, Ordering::Release);
                }
                let result = window.close();
                log::info!(target: "exv.atrust_auth", "embedded.close_requested result={} elapsed_ms={}", if result.is_ok() { "success" } else { "failure" }, self.started.elapsed().as_millis());
                result.map_err(|_| {
                    error(AuthErrorCode::ChannelTransportFailed, AuthOperation::Close)
                })?;
                // close() 只发出原生关闭请求；确认本轮 Destroyed 后才允许重开。
                while !self.destroyed.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                self.window = None;
                self.page_load = None;
            }
            // S4c：手动持久目录（manual-persistent）跨轮次/跨次登录保留，close 不删除。
            Ok(())
        })
    }
}

impl Drop for OwnedWebViewAdapter<'_> {
    fn drop(&mut self) {
        if self.intent != AuthIntent::FreshManual {
            return;
        }
        self.settled.store(true, Ordering::Release);
        if let Some(window) = self.window.take() {
            if let Err(cause) = window.close() {
                log::warn!(target: "exv.atrust_auth", "embedded.drop_close_failed error={cause}");
            }
        }
        // 持久 manual-persistent 目录保留（S4c）：已认证 SSO 与登录痕迹供下次登录复用。
    }
}

fn begin_window_round(label: &mut String, settled: &mut Arc<AtomicBool>) {
    settled.store(true, Ordering::Release);
    *label = format!("atrust-auth-{}", uuid::Uuid::new_v4().simple());
    *settled = Arc::new(AtomicBool::new(false));
}

/// AppHandle、档案路径和 agent 快照留在宿主，统一引擎只见稳定事件及 v2 产物。
pub(super) struct HostDependencies {
    pub app: AppHandle,
    pub generation: u64,
    pub label: Arc<std::sync::Mutex<String>>,
    pub user_agent: Arc<std::sync::Mutex<String>>,
    pub cache_profile: PathBuf,
    pub intent: AuthIntent,
    pub silent: bool,
}
impl AuthEventSink for HostDependencies {
    fn ticket_refresh(&self, reason: TicketRefreshReason) {
        if !self.silent {
            super::super::commands::emit_browser_ticket_refresh(&self.app, self.generation, reason);
        }
    }
    fn round_failure(&self, round: AuthRound, error: &AuthError) {
        crate::kernel::auth_diagnostics::round_failure(&self.app, self.generation, round, error);
    }
    fn progress(&self, progress: AuthProgress) {
        if self.silent {
            return;
        }
        if let AuthProgress::Recovering { round } = progress {
            log::info!(target: "exv.atrust_auth", "embedded.recovering owner={} round={:?}", self.generation, round);
            super::super::commands::emit_browser_login_stage(
                &self.app,
                self.generation,
                "recovering",
            );
        }
    }
    fn milestone(&self, milestone: AuthMilestone) {
        if self.silent {
            return;
        }
        let stage = match milestone {
            AuthMilestone::Opening => STAGE_OPENING,
            AuthMilestone::SchoolLogin => STAGE_SCHOOL_LOGIN,
            AuthMilestone::Submitting => STAGE_SUBMITTING,
            AuthMilestone::GatewayConfirming => STAGE_GATEWAY_CONFIRMING,
            AuthMilestone::AwaitingManual => STAGE_AWAITING_MANUAL,
        };
        super::super::commands::emit_browser_login_stage(&self.app, self.generation, stage);
        if milestone == AuthMilestone::GatewayConfirming
            && let Some(w) = self
                .app
                .get_webview_window(&self.label.lock().unwrap_or_else(|e| e.into_inner()))
        {
            set_window_title(&w, WINDOW_TITLE_CONFIRMING);
        }
    }
    fn form_login_reached(&self) {
        if self.silent {
            return;
        }
        record_form_login_once(&self.app);
    }
}
impl SessionCacheInvalidator for HostDependencies {
    fn forget<'a>(&'a self, gateway: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            if !self.intent.allows_reuse() {
                return Ok(());
            }
            exv_core::atrust_session_cache::forget(&self.cache_profile, gateway);
            log::warn!(target: "exv.atrust_auth", "embedded.session_cache result=forgotten reason=not_logged_in");
            Ok(())
        })
    }
}
impl SessionSnapshotProvider for HostDependencies {
    fn snapshot<'a>(&'a self, gateway: &'a str) -> ChannelFuture<'a, Option<SessionArtifact>> {
        Box::pin(async move {
            if !self.intent.allows_reuse() {
                return Ok(None);
            }
            let snapshot = super::super::local_agent::session_snapshot(&self.app);
            let user_agent = self
                .user_agent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let session = snapshot.as_ref().and_then(|s| {
                build_session_from_snapshot(
                    gateway,
                    &user_agent,
                    &s.gateway,
                    s.cookie_header.expose(),
                )
            });
            Ok(session.map(|session| SessionArtifact {
                session,
                source: exv_core::browser_auth::SessionSource::AgentSnapshot,
            }))
        })
    }
}

// 单次 HTTP I/O 边界只接配置网关与网关 cookie，没有当前页面 origin 条件。
type GatewayProbeFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    exv_core::atrust_session_probe::SessionProbe,
                    exv_core::atrust_session_probe::ProbeError,
                >,
            > + Send
            + 'a,
    >,
>;

async fn probe_gateway_cookies<P>(
    gateway: &str,
    cookies: &[(String, String)],
    user_agent: &str,
    probe: P,
) -> Result<exv_core::atrust_session_probe::SessionProbe, exv_core::atrust_session_probe::ProbeError>
where
    P: for<'a> FnOnce(&'a str, &'a str, &'a str) -> GatewayProbeFuture<'a>,
{
    let header = zeroize::Zeroizing::new(cookie_header(cookies));
    probe(gateway, &header, user_agent).await
}

/// S4c（2026-10-26）：内嵌窗 data_directory 选择。
/// - `FreshManual`：`<webview 根>/manual-persistent/<sha256(gateway)>/`（哈希与
///   `manual-username-<sha>` 同源，对齐 darwin S4a 手动通道形态），跨次登录与
///   轮次重启复用；EXV 不删除该目录——轮次重启（编排器 close→open 重导航）不再
///   销毁已认证 SSO 会话，自动通道本就用持久基础 profile，两通道语义一致。
/// - `Automatic`：持久基础 profile（`atrust-webview`/`webview`）不变。
fn webview_data_directory(
    intent: AuthIntent,
    profile: &std::path::Path,
    gateway: &str,
) -> Result<PathBuf, AuthError> {
    let path = if intent == AuthIntent::FreshManual {
        exv_core::browser_cache_profile::manual_persistent_profile(profile, gateway)
    } else {
        std::fs::create_dir_all(profile)
            .map(|_| profile.to_path_buf())
            .map_err(|e| format!("无法创建 EXV 浏览器配置目录：{e}"))
    };
    path.map_err(|_| error(AuthErrorCode::ChannelOpenFailed, AuthOperation::Open))
}

fn gateway_seed(
    intent: AuthIntent,
    profile: &std::path::Path,
    gateway: &str,
) -> (Option<BrowserSession>, String) {
    if intent.allows_reuse() {
        current_gateway_seed(profile, gateway)
    } else {
        (
            None,
            exv_core::atrust_browser::PLAIN_CHROME_USER_AGENT.to_owned(),
        )
    }
}

fn current_gateway_seed(
    profile: &std::path::Path,
    gateway: &str,
) -> (Option<BrowserSession>, String) {
    let loaded = exv_core::atrust_session_cache::load(profile, gateway);
    match &loaded {
        Ok(Some(_)) => {
            log::info!(target: "exv.atrust_auth", "embedded.session_cache result=hit exists=true")
        }
        Ok(None) => {
            log::info!(target: "exv.atrust_auth", "embedded.session_cache result=miss exists=false")
        }
        Err(_) => {
            log::warn!(target: "exv.atrust_auth", "embedded.session_cache result=error exists=unknown")
        }
    }
    let cached = loaded.ok().flatten();
    // 旧身份的有效候选已由 HTTP 先复核；需要新浏览器事务时使用实际具备的普通浏览器能力。
    let user_agent = exv_core::atrust_browser::PLAIN_CHROME_USER_AGENT.to_owned();
    (cached, user_agent)
}

fn bound_startup_diagnostic(
    sequence: u64,
    before: Option<&str>,
    after: Option<&str>,
    payload: &str,
    navigation: (u64, u64),
) -> Option<String> {
    let mut parts = payload.splitn(3, '~');
    let sampled_sequence = parts.next()?.parse::<u64>().ok()?;
    let sampled_url = percent_decode_component(parts.next()?);
    if navigation.0 != navigation.1
        || sequence != sampled_sequence
        || before != after
        || after != Some(sampled_url.as_str())
    {
        return None;
    }
    startup_diagnostic(parts.next()?)
}

fn observed_page(
    sequence: u64,
    before: Option<&str>,
    after: Option<&str>,
    payload: Option<&str>,
    navigation: (u64, u64),
) -> PageObservation {
    let url = after.and_then(|u| u.parse::<tauri::Url>().ok());
    let facts = payload.and_then(|value| {
        let mut parts = value.splitn(3, '~');
        let sampled_sequence = parts.next()?.parse::<u64>().ok()?;
        let sampled_url = percent_decode_component(parts.next()?);
        if navigation.0 != navigation.1
            || sampled_sequence != sequence
            || before != after
            || after != Some(sampled_url.as_str())
        {
            return None;
        }
        parse_page_facts(parts.next()?)
    });
    let coherent = facts.is_some();
    PageObservation {
        sequence,
        url: url.as_ref().map_or(Observation::NotYetObserved, |u| {
            Observation::Observed(u.as_str().to_owned())
        }),
        origin: url.as_ref().map_or(Observation::NotYetObserved, |u| {
            Observation::Observed(u.origin().ascii_serialization())
        }),
        dom: facts.map_or(Observation::NotYetObserved, |f| {
            Observation::Observed(DomFacts {
                challenge_visible: f.challenge_visible,
                client_install_marker: f.marker,
                shortcut_page: f.shortcut,
                download_page: f.download,
                body_has_failure_phrase: f.has_fail,
                body_char_count: f.len,
                error_container_text: f.error_text,
                short_body_text: f.body_head,
            })
        }),
        startup: None,
        coherent,
    }
}

// 每轮使用独有的观察 cookie；隐藏自动填交不写人工观察，旧窗口/旧尝试不能提供身份。
fn owned_username_script(
    intent: AuthIntent,
    profile: &std::path::Path,
    gateway: &str,
    cookie: &str,
) -> String {
    let remembered = if intent == AuthIntent::FreshManual {
        exv_core::browser_cache_profile::remembered_username(profile, gateway)
    } else {
        None
    };
    exv_core::browser_cache_profile::username_memory_script(remembered.as_deref())
        .replace(exv_core::browser_cache_profile::USERNAME_COOKIE, cookie)
        .replace(
            "const observe=()=>{",
            "const observe=()=>{if(document.visibilityState!=='visible')return;",
        )
}

fn take_manual_identity(username: &mut Option<String>, manual: &mut bool, completed: bool) {
    if !completed && !*manual {
        *username = None;
        *manual = true;
    }
}

// 与成功会话一起落盘；人工接管后只接纳本轮可见页面的提交观察。
fn save_completed_identity(
    profile: &std::path::Path,
    gateway: &str,
    session: &BrowserSession,
    username: &mut Option<String>,
    manual: bool,
    cookies: Option<&[(String, String)]>,
    username_cookie: &str,
) -> Result<(), String> {
    if manual {
        *username = cookies
            .and_then(|cookies| cookies.iter().find(|(key, _)| key == username_cookie))
            .and_then(|(_, value)| exv_core::browser_cache_profile::observed_username(value));
        if let Some(username) = username.as_deref() {
            let _ = exv_core::browser_cache_profile::remember_username(profile, gateway, username);
        } else {
            // 普通 save 对同一 session 保留旧身份；本次人工身份未知时须先撤销该绑定。
            exv_core::atrust_session_cache::forget(profile, gateway);
        }
    }
    if let Some(username) = username.as_deref() {
        exv_core::atrust_session_cache::save_with_username(profile, session, username)
    } else {
        exv_core::atrust_session_cache::save(profile, session)
    }
}

// 与doc-start请求拦截使用同一实证官方SDK范围，保留学校及其他本机应用导航。
fn owned_navigation_allowed(url: &tauri::Url) -> bool {
    let official = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost.sangfor.com.cn")
    ) && matches!(
        url.port(),
        Some(54630 | 54641 | 54652 | 54631 | 54642 | 54653)
    ) && matches!(
        url.path(),
        "/v1/detect"
            | "/v1/service/init"
            | "/v1/service/start"
            | "/v1/service/online"
            | "/v1/service/offline"
            | "/v1/service/logout"
            | "/v1/service/status"
            | "/v1/service/events"
            | "/v1/service/checkMITMAttack"
            | "/v1/service/reportEnv"
            | "/v1/service/reportEnvBeforeLogin"
            | "/v1/service/PopupPhishingRiskConfirm"
            | "/v1/update/check"
            | "/v1/update/status"
            | "/v1/update/start"
            | "/v1/base/setinfo"
            | "/v1/base/getter"
            | "/v1/base/setter"
            | "/v1/base/printLog"
    );
    url.scheme() == "https" && !official
}
