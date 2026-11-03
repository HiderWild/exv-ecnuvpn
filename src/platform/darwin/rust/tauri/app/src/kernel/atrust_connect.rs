//! aTrust认证候选与既有本地连接请求的薄接缝；不拥有连接、缓存或隧道生命周期。
use super::client::auth_deadline_app_error;
use super::{
    browser_auth::{self, AuthArtifact, AuthContext, AuthMode, AuthOptions, AuthSource},
    client::{ConfigPayload, ConnectIntent, CoreClient, CoreState},
    connection_request::ConnectionRequest,
    error::AppError,
    state::OperationReply,
};
use exv_vpn_atrust::auth::{browser_session::BrowserSession, origin::GatewayOrigin};
use exv_vpn_darwin_ipc::auth_deadline::AuthDeadline;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use zeroize::Zeroize;
const CONTEXT_CHANGED: &str = "认证期间连接设置已改变，本次候选不会提交。请使用新设置重新连接。";
#[derive(Clone)]
pub(super) struct FrozenLogin {
    context: AuthContext,
    mode: AuthMode,
    configured_mode: AuthMode,
    auto_login: bool,
    config_dir: std::path::PathBuf,
}
impl FrozenLogin {
    pub(super) fn from_config(config: &ConfigPayload) -> Result<Option<Self>, AppError> {
        config.ensure_readable()?;
        let value = |key: &str| {
            config
                .items
                .iter()
                .find(|item| item.key == key)
                .map(|item| item.value.as_str())
                .unwrap_or_default()
        };
        match value("protocol") {
            "cisco" | "" => Ok(None),
            "atrust" => {
                let configured_mode = match value("atrust_auth_mode") {
                    "browser" => AuthMode::Browser,
                    "headless" => AuthMode::Headless,
                    "embedded" => AuthMode::Embedded,
                    "" => AuthMode::Browser,
                    _ => {
                        return Err(AppError::Internal(
                            "当前网页登录方式无效，请重新选择认证方式。".into(),
                        ));
                    }
                };
                let context = AuthContext::freeze(value("atrust_server"), value("user_agent"))
                    .map_err(AppError::Internal)?;
                let auto_login = configured_mode == AuthMode::Headless;
                let mode = resolved_mode(configured_mode, auto_login);
                // Default 仅组装既有目录，不加载或初始化配置；Core 子进程继承同一环境/cwd。
                let config_dir = exv_vpn_darwin_core::DarwinUiConfig::default()
                    .config_dir()
                    .to_path_buf();
                Ok(Some(Self {
                    context,
                    mode,
                    configured_mode,
                    auto_login,
                    config_dir,
                }))
            }
            _ => Err(AppError::ProtocolUnavailable(
                "当前连接协议不可用，请重新选择。".into(),
            )),
        }
    }
    fn ensure_current(&self, current: &ConfigPayload) -> Result<(), AppError> {
        current.ensure_readable()?;
        let Some(current) =
            Self::from_config(current).map_err(|_| AppError::Internal(CONTEXT_CHANGED.into()))?
        else {
            return Err(AppError::Internal(CONTEXT_CHANGED.into()));
        };
        if self.context.gateway != current.context.gateway
            || self.mode != current.mode
            || self.configured_mode != current.configured_mode
            || self.auto_login != current.auto_login
            || self.config_dir != current.config_dir
            || self.context.user_agent != current.context.user_agent
        {
            return Err(AppError::Internal(CONTEXT_CHANGED.into()));
        }
        Ok(())
    }
    fn validate_reauthentication(&self, origin: &str) -> Result<(), AppError> {
        let raw_origin = if origin.starts_with("https://") {
            origin.to_owned()
        } else {
            format!("https://{origin}")
        };
        let origin = GatewayOrigin::parse(&raw_origin)
            .map_err(|_| AppError::Internal("需要重新认证的网关地址无效，请重新连接。".into()))?;
        if origin.endpoint() != self.context.gateway {
            return Err(AppError::Internal(
                "当前网关已改变，请使用新网关重新连接。".into(),
            ));
        }
        Ok(())
    }
}
fn resolved_mode(configured: AuthMode, _auto_login: bool) -> AuthMode {
    configured
}

/// 认证阶段错误→AppError kind 映射（S4b，2026-10-26）：取消/超时/凭据缺失维持原
/// 分流；其余认证失败改映射 `browser_login_failed` 形态并保留净化后的原中文
/// message——前端 `command-error.ts` 对该 kind 原样透传，真实原因不再被
/// `internal` 通用文案顶掉（对齐 win32 `AppError::browser_auth_failed`）。
fn auth_failure_app_error(error: String) -> AppError {
    if error.starts_with(super::client::AUTH_RECEIVER_UPDATE_REQUIRED) {
        AppError::ServiceConnectFailed(error)
    } else if error == "DARWIN_AUTH_CANCELLED" {
        AppError::ConnectionCancelled
    } else if error == browser_auth::TIMEOUT {
        AppError::BrowserLoginTimeout
    } else if error == browser_auth::CREDENTIAL_REQUIRED {
        AppError::CredentialRequired {
            code: Some("DARWIN_CORE_CONNECT_CREDENTIALS_MISSING".into()),
            message: "学校自动登录需要完整账号密码，请在凭据窗口输入后重试。".into(),
        }
    } else {
        AppError::BrowserLoginFailed(error)
    }
}
#[derive(Clone, serde::Serialize)]
struct BrowserLoginEvent {
    active: bool,
    generation: u64,
    mode: &'static str,
    milestone: browser_auth::AuthMilestone,
    /// 自动恢复轮次；原里程碑事件不要求此字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<RecoveryPresentation>,
}
#[derive(Clone, serde::Serialize)]
struct RecoveryPresentation {
    mode: &'static str,
    auto_login: bool,
}
impl BrowserLoginEvent {
    fn project_temporary_mode(&mut self, app: &tauri::AppHandle) {
        if let Some(mode) = app.try_state::<CoreState>().and_then(|state| state.connection_requests.temporary_auth_mode(self.generation)) {
            self.mode = mode;
            self.recovery = Some(RecoveryPresentation { mode, auto_login: mode == "headless" });
        }
    }
}

pub(super) fn emit_browser_channel_selected(
    app: &tauri::AppHandle,
    generation: u64,
    mode: &'static str,
    auto_login: bool,
) {
    let _ = app.emit("exv://browser-login", serde_json::json!({
        "active": true, "generation": generation, "mode": mode, "milestone": "opening",
        "channel_override": true, "recovery": { "mode": mode, "auto_login": auto_login },
    }));
}

pub(super) struct LoginEventGuard {
    app: tauri::AppHandle,
    event: BrowserLoginEvent,
    ended: bool,
}
impl LoginEventGuard {
    fn new(app: &tauri::AppHandle, generation: u64, mode: AuthMode) -> Self {
        Self::begin(app, generation, mode, None)
    }
    pub(super) fn recovery(
        app: &tauri::AppHandle,
        generation: u64,
        mode: AuthMode,
        auto_login: bool,
    ) -> Self {
        Self::begin(app, generation, mode, Some(auto_login))
    }
    fn begin(
        app: &tauri::AppHandle,
        generation: u64,
        mode: AuthMode,
        recovery: Option<bool>,
    ) -> Self {
        let event = BrowserLoginEvent {
            active: true,
            generation,
            mode: match mode {
                AuthMode::Embedded => "embedded",
                AuthMode::Browser => "browser",
                AuthMode::Headless => "headless",
            },
            milestone: browser_auth::AuthMilestone::Opening,
            round: None,
            outcome: None,
            recovery: recovery.map(|auto_login| RecoveryPresentation {
                mode: match mode {
                    AuthMode::Embedded => "embedded",
                    AuthMode::Browser => "browser",
                    AuthMode::Headless => "headless",
                },
                auto_login,
            }),
        };
        let _ = app.emit("exv://browser-login", event.clone());
        Self {
            app: app.clone(),
            event,
            ended: false,
        }
    }
    pub(super) fn cancelled(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.event.active = false;
        self.event.outcome = Some("cancelled");
        self.event.recovery = None;
        self.event.project_temporary_mode(&self.app);
        let _ = self.app.emit("exv://browser-login", self.event.clone());
    }
    pub(super) fn authenticated(&mut self, interactive: bool) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.event.active = false;
        self.event.outcome = Some("authenticated");
        self.event.recovery = None;
        self.event.project_temporary_mode(&self.app);
        let _ = self.app.emit("exv://browser-login", self.event.clone());
        if interactive
            && self
                .app
                .get_webview_window("main")
                .is_some_and(|window| window.is_visible().unwrap_or(false))
        {
            crate::lifecycle::show_main_window(&self.app);
        }
    }
}
impl LoginEventGuard {
    pub(super) fn milestone_sink(&self) -> Arc<dyn Fn(browser_auth::AuthMilestone) + Send + Sync> {
        let app = self.app.clone();
        let event = self.event.clone();
        Arc::new(move |milestone| {
            let mut update = event.clone();
            update.recovery = None;
            update.milestone = milestone;
            update.round =
                matches!(milestone, browser_auth::AuthMilestone::Recovering).then_some(2);
            update.project_temporary_mode(&app);
            let _ = app.emit("exv://browser-login", update);
        })
    }
}
impl Drop for LoginEventGuard {
    fn drop(&mut self) {
        if !self.ended {
            self.event.active = false;
            self.event.recovery = None;
            self.event.project_temporary_mode(&self.app);
        let _ = self.app.emit("exv://browser-login", self.event.clone());
        }
    }
}
struct SecretIntent(ConnectIntent);
impl Drop for SecretIntent {
    fn drop(&mut self) {
        if let Some(secret) = &mut self.0.secret_payload {
            secret.zeroize();
        }
    }
}
fn candidate_intent(
    intent: &ConnectIntent,
    artifact: AuthArtifact,
) -> Result<SecretIntent, AppError> {
    let fresh = artifact.fresh_authentication();
    let payload = exv_vpn_darwin_core::UiAtrustCredentialPackage::new(artifact.session, fresh)
        .with_authenticated_username(artifact.authenticated_username)
        .to_bytes()
        .map_err(|_| AppError::Internal("无法组装aTrust连接候选，请重新登录。".into()))?;
    let secret = String::from_utf8(payload.to_vec())
        .map_err(|_| AppError::Internal("无法编码aTrust连接候选。".into()))?;
    Ok(SecretIntent(ConnectIntent {
        profile_ref: intent.profile_ref.clone(),
        secret_payload: Some(secret),
        ..ConnectIntent::default()
    }))
}
/// 配置和请求身份校验成功后才生成待提交秘密；测试可直接注入候选而不打开浏览器。
fn prepare_candidate(
    frozen: &FrozenLogin,
    current: &ConfigPayload,
    intent: &ConnectIntent,
    artifact: AuthArtifact,
    request: &ConnectionRequest,
) -> Result<SecretIntent, AppError> {
    request.begin_submit()?;
    frozen.ensure_current(current)?;
    request.begin_submit()?;
    candidate_intent(intent, artifact)
}
/// 连接请求覆盖缓存尝试、浏览器准备和最终提交；认证结果只在原请求下消费。
pub(crate) async fn connect(
    app: &tauri::AppHandle,
    state: &CoreState,
    intent: &ConnectIntent,
    request: Arc<ConnectionRequest>,
    deadline: AuthDeadline,
) -> Result<OperationReply, AppError> {
    connect_with_authenticator(
        Some(app),
        state,
        intent,
        request,
        deadline,
        |context, mode, request, options| async move {
            browser_auth::authenticate_with_options(
                app,
                &context,
                mode,
                &state.browser_selection,
                &request,
                options,
            )
            .await
        },
    )
    .await
}

/// 生产连接与局部回归共用同一薄编排；只注入认证原语，不另建请求或计时循环。
pub(super) async fn connect_with_authenticator<F, Fut>(
    app: Option<&tauri::AppHandle>,
    state: &CoreState,
    intent: &ConnectIntent,
    request: Arc<ConnectionRequest>,
    deadline: AuthDeadline,
    authenticate: F,
) -> Result<OperationReply, AppError>
where
    F: FnOnce(AuthContext, AuthMode, Arc<ConnectionRequest>, AuthOptions) -> Fut,
    Fut: std::future::Future<Output = Result<AuthArtifact, String>>,
{
    let config_deadline = deadline.tokio_deadline().map_err(auth_deadline_app_error)?;
    let config = tokio::select! { biased;
        _ = request.cancelled() => return Err(AppError::ConnectionCancelled),
        result = tokio::time::timeout_at(config_deadline, CoreClient.config_get(state)) => result.map_err(|_| AppError::Internal("读取认证配置超过本次连接总期限".into()))??,
    };
    let Some(frozen) = FrozenLogin::from_config(&config)? else {
        if intent.atrust_reauthenticate.is_some() {
            return Err(AppError::Internal(
                "当前已切换连接协议，请重新连接。".into(),
            ));
        }
        return CoreClient.connect_for_request(state, intent, request).await;
    };
    request.set_auth_control_deadline(deadline.monotonic_ns());
    request.begin_submit()?;
    tokio::select! { biased;
        _ = request.cancelled() => return Err(AppError::ConnectionCancelled),
        result = CoreClient.prepare_auth_control(state, deadline) => result?,
    }
    persist_school_credentials_for_request(
        state,
        intent,
        &request,
        deadline.tokio_deadline().map_err(auth_deadline_app_error)?,
    )
    .await?;
    if let Some(origin) = &intent.atrust_reauthenticate {
        frozen.validate_reauthentication(origin)?;
    } else if frozen.auto_login {
        let cache_intent = ConnectIntent {
            profile_ref: intent.profile_ref.clone(),
            ..ConnectIntent::default()
        };
        match CoreClient
            .connect_for_request_with_deadline(state, &cache_intent, Arc::clone(&request), deadline)
            .await
        {
            Err(AppError::CredentialRequired { .. }) => {}
            result => {
                request.begin_submit()?;
                return result;
            }
        }
    }
    request.begin_submit()?;
    deadline.remaining().map_err(auth_deadline_app_error)?;
    let mut events = app.map(|app| LoginEventGuard::new(app, request.generation(), frozen.mode));
    let options = AuthOptions {
        deadline: Some(deadline.tokio_deadline().map_err(auth_deadline_app_error)?),
        auto_login: frozen.auto_login,
        hidden: frozen.mode == AuthMode::Headless,
        silent: false,
        config_dir: Some(frozen.config_dir.clone()),
        explicit: intent.credentials.as_ref().map(|input| {
            exv_vpn_darwin_core::atrust_auth_storage::SchoolCredentialInput {
                username: zeroize::Zeroizing::new(input.username.clone()),
                password: zeroize::Zeroizing::new(input.password.clone()),
            }
        }),
        milestone: events.as_ref().map(LoginEventGuard::milestone_sink),
        diagnostic: app.map(|app| super::auth_diagnostics::failure_sink(app, request.generation())),
    };
    let artifact = authenticate(
        frozen.context.clone(),
        frozen.mode,
        Arc::clone(&request),
        options,
    )
    .await
    .map_err(auth_failure_app_error)?;
    request.begin_submit()?;
    if !GatewayOrigin::parse(artifact.session.gateway())
        .is_ok_and(|origin| origin.endpoint() == frozen.context.gateway)
    { return Err(AppError::Internal(CONTEXT_CHANGED.into())); }
    if let Some(events) = events.as_mut() {
        events.authenticated(artifact.interactive);
    }
    submit_candidate_bound(
        state,
        &frozen,
        intent,
        artifact,
        request,
        deadline.tokio_deadline().map_err(auth_deadline_app_error)?,
        Some(deadline),
    )
    .await
}

/// 取消结束连接准备，不宣称已经派发的 ConfigSet 已撤销；其结果沿配置刷新确认。
pub(super) async fn persist_school_credentials_for_request(
    state: &CoreState,
    intent: &ConnectIntent,
    request: &ConnectionRequest,
    deadline: tokio::time::Instant,
) -> Result<(), AppError> {
    tokio::select! { biased;
        _ = request.cancelled() => Err(AppError::ConnectionCancelled),
        result = tokio::time::timeout_at(deadline, persist_school_credentials(state, intent)) => result.map_err(|_| AppError::Internal("等待学校凭据保存超过本次认证期限，连接尚未提交；已派发的配置保存结果请刷新确认。".into()))?,
    }
}
/// 学校显式持久化仍走既有 Core ConfigSet 和配置锁；不把学校密码编码为 Cisco Connect 包。
pub(super) async fn persist_school_credentials(
    state: &CoreState,
    intent: &ConnectIntent,
) -> Result<(), AppError> {
    if let Some(input) = &intent.credentials {
        if input.persist && !input.username.trim().is_empty() && !input.password.is_empty() {
            use super::client::ConfigItem;
            let saved = CoreClient
                .config_set(
                    state,
                    vec![
                        ConfigItem {
                            key: "username".into(),
                            value: input.username.clone(),
                        },
                        ConfigItem {
                            key: "password".into(),
                            value: input.password.clone(),
                        },
                        ConfigItem {
                            key: "remember_password".into(),
                            value: "true".into(),
                        },
                    ],
                )
                .await?;
            if !saved {
                return Err(AppError::Internal(
                    "学校凭据未保存，本次连接尚未开始认证。请重试保存。".into(),
                ));
            }
        }
    }
    Ok(())
}
pub(super) async fn submit_candidate_with_deadline(
    state: &CoreState,
    frozen: &FrozenLogin,
    intent: &ConnectIntent,
    artifact: AuthArtifact,
    request: Arc<ConnectionRequest>,
    deadline: tokio::time::Instant,
) -> Result<OperationReply, AppError> {
    submit_candidate_bound(state, frozen, intent, artifact, request, deadline, None).await
}

async fn submit_candidate_bound(
    state: &CoreState,
    frozen: &FrozenLogin,
    intent: &ConnectIntent,
    artifact: AuthArtifact,
    request: Arc<ConnectionRequest>,
    deadline: tokio::time::Instant,
    auth_deadline: Option<AuthDeadline>,
) -> Result<OperationReply, AppError> {
    request.begin_submit()?;
    let gate = Arc::clone(&state.connection_requests.commit_gate);
    let gate_deadline = candidate_local_deadline(deadline, auth_deadline)?;
    let permit = tokio::select! { biased;
        _ = request.cancelled() => return Err(AppError::ConnectionCancelled),
        permit = tokio::time::timeout_at(gate_deadline, gate.lock_owned()) => permit.map_err(|_| auth_deadline_error())?,
    };
    request.begin_submit()?;
    let config_deadline = candidate_local_deadline(deadline, auth_deadline)?;
    let current = tokio::select! { biased;
        _ = request.cancelled() => return Err(AppError::ConnectionCancelled),
        reply = tokio::time::timeout_at(config_deadline, CoreClient.config_get(state)) => reply.map_err(|_| auth_deadline_error())??,
    };
    if let Some(absolute) = auth_deadline {
        absolute.remaining().map_err(auth_deadline_app_error)?;
    }
    if tokio::time::Instant::now() >= deadline {
        return Err(auth_deadline_error());
    }
    let candidate = prepare_candidate(frozen, &current, intent, artifact, &request)?;
    // 只有尚未提交的等待使用认证期限；派发后仍沿既有真实 RPC 收束，不丢弃未来冒充停止。
    CoreClient
        .connect_with_permit_and_auth_deadline(state, &candidate.0, request, permit, auth_deadline)
        .await
}
fn candidate_local_deadline(
    local: tokio::time::Instant,
    absolute: Option<AuthDeadline>,
) -> Result<tokio::time::Instant, AppError> {
    absolute.map_or(Ok(local), |deadline| {
        deadline.tokio_deadline().map_err(auth_deadline_app_error)
    })
}

fn auth_deadline_error() -> AppError {
    AppError::Internal("本次认证总期限已耗尽，候选未提交，请重新连接。".into())
}

