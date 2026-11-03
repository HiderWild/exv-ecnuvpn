//! Darwin认证编排：一个期限、一条采样循环；通道只拥有页面原语，不拥有连接提交。
//! 自动登录沿用既有数据域；手动登录使用网关级持久浏览器目录（2026-10-26 S4a，
//! `<根>/manual-persistent/<sha256(gateway)>`），登录痕迹跨次复用，EXV 不删除。
use super::connection_request::ConnectionRequest;
use exv_vpn_atrust::auth::browser_auth_policy::{
    AuthFailureKind, AuthRound, RecoveryDecision, RecoveryFacts, RoundBudget, SessionDecision,
    SessionExtraction, SessionFacts, SessionRecheck, SubmissionOutcome, SubmissionState,
    decide_recovery, decide_session_precedence,
};
use exv_vpn_atrust::auth::{
    browser_session::BrowserSession, client_identity::AuthClientIdentity, origin::GatewayOrigin,
};
use exv_vpn_atrust::browser_selection::{
    BrowserKind, Decision, Detection, MACOS_SUPPORTED, SelectionCache,
};
use exv_vpn_darwin_core::atrust_session_probe::{ProbeError, SessionProbe};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
mod embedded;
mod external;
mod http_probe;
mod native;
mod native_eval;
mod page_facts;
mod page_policy;
mod school_auto_login;
use page_facts::{Observation, PageObservation, SessionStatus};
use school_auto_login::{CredentialDelivery, InjectionOutcome};
pub(crate) const CREDENTIAL_REQUIRED: &str = "DARWIN_SCHOOL_CREDENTIAL_REQUIRED";
pub(crate) use external::{SupportedBrowser, browser_for_wire_name};
const CANCELLED: &str = "DARWIN_AUTH_CANCELLED";
pub(super) const TIMEOUT: &str = "网页登录超时，请重试。";
pub(crate) const AUTH_BUDGET: Duration = Duration::from_secs(300);
pub(crate) type ChannelFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthMode {
    Embedded,
    Browser,
    Headless,
}
/// 本次冻结的认证意图；手动认证只消费本次页面上下文。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthIntent {
    Automatic,
    FreshManual,
}
impl AuthIntent {
    fn from_auto_login(automatic: bool) -> Self {
        if automatic {
            Self::Automatic
        } else {
            Self::FreshManual
        }
    }
    fn initial_mode(self, requested: AuthMode) -> AuthMode {
        match (self, requested) {
            (Self::Automatic, _) => AuthMode::Headless,
            (Self::FreshManual, AuthMode::Headless) => AuthMode::Embedded,
            (Self::FreshManual, mode) => mode,
        }
    }
    fn fresh(self) -> bool {
        self == Self::FreshManual
    }
}
/// 仅承载新认证来源；Core 缓存候选继续由 Core 构造 fresh=false。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthSource {
    HttpCas,
    Embedded,
    SystemBrowser,
}
pub(crate) struct AuthArtifact {
    pub(crate) session: BrowserSession,
    pub(crate) source: AuthSource,
    pub(crate) interactive: bool,
    pub(crate) authenticated_username: String,
}
impl AuthArtifact {
    pub(crate) fn new(session: BrowserSession, source: AuthSource) -> Self {
        Self {
            session,
            source,
            interactive: false,
            authenticated_username: String::new(),
        }
    }
    pub(crate) fn fresh_authentication(&self) -> bool {
        match self.source {
            AuthSource::HttpCas | AuthSource::Embedded | AuthSource::SystemBrowser => true,
        }
    }
}
impl std::fmt::Debug for AuthArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthArtifact")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}
pub(crate) enum ReuseOutcome {
    Ready(BrowserSession),
    Interactive(&'static str),
}
/// 连接入口冻结一次。后续浏览器动作不读取可变化的配置，也不转交连接权限。
#[derive(Clone)]
pub(crate) struct AuthContext {
    pub(crate) gateway: String,
    pub(crate) auth_path: String,
    pub(crate) user_agent: String,
}
impl AuthContext {
    pub(crate) fn freeze(server: &str, user_agent: &str) -> Result<Self, String> {
        let gateway = if server.starts_with("https://") {
            server.trim_end_matches('/').to_owned()
        } else {
            format!("https://{}", server.trim_matches('/'))
        };
        let gateway = GatewayOrigin::parse(&gateway)
            .map_err(|_| "aTrust网关地址无效".to_owned())?
            .endpoint()
            .to_owned();
        let identity = AuthClientIdentity::browser("Mac", "zh-CN")
            .map_err(|_| "aTrust认证身份无效".to_owned())?;
        Ok(Self {
            gateway,
            auth_path: identity.auth_config_path(false),
            user_agent: user_agent.to_owned(),
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationSurface {
    Controlled,
    AlwaysVisible,
    NoSurface,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationIntent {
    Hidden,
    Visible,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentationOutcome {
    Hidden,
    Visible,
    AlwaysVisible,
    NoSurface,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChannelCapabilities {
    pub(crate) can_render: bool,
    pub(crate) can_extract: bool,
    pub(crate) can_inject: bool,
    pub(crate) surface: PresentationSurface,
}
/// 首导航决策：`url` 为本轮首个导航目标；`school_login=true` 表示目标已是
/// 学校认证入口（编排器据此置 `login_started`，跳过重复的入口导航）。
pub(crate) struct FirstNavigation {
    pub(crate) url: String,
    pub(crate) school_login: bool,
}
/// 通道只执行一次原语；统一循环持有期限、提交预算和控制事件。
pub(crate) trait ChannelTrait: Send {
    fn capabilities(&self) -> ChannelCapabilities;
    fn navigate<'a>(&'a mut self, url: &'a str) -> ChannelFuture<'a, ()>;
    /// 每轮首次导航的目标决策（S4b）。默认保持 authConfig 原语义；内嵌通道用
    /// 空 cookie 预探针在冷缓存+明确未登录时直达学校登录页。
    fn first_navigation<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, FirstNavigation> {
        let url = format!("{}{}", context.gateway, context.auth_path);
        Box::pin(std::future::ready(Ok(FirstNavigation {
            url,
            school_login: false,
        })))
    }
    /// 轮次原地重启能力：true 表示 RestartRound 可保留通道（窗口与 data store）
    /// 仅重新导航；默认 false，维持关窗重建原语义。
    fn supports_in_place_restart(&self) -> bool {
        false
    }
    /// 原地重启时的轮次同步（仅诊断用途）。
    fn begin_round(&mut self, _round: u64) {}
    fn observe_page(&mut self, sequence: u64) -> ChannelFuture<'_, PageObservation>;
    fn probe_session<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, SessionStatus>;
    fn extract<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, Option<BrowserSession>>;
    fn present(&mut self, _intent: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome> {
        let surface = self.capabilities().surface;
        Box::pin(async move {
            Ok(match surface {
                PresentationSurface::NoSurface => PresentationOutcome::NoSurface,
                _ => PresentationOutcome::AlwaysVisible,
            })
        })
    }
    fn inject_once(
        &mut self,
        _delivery: CredentialDelivery,
    ) -> ChannelFuture<'_, InjectionOutcome> {
        Box::pin(async { Err("当前通道不接受学校凭据".into()) })
    }
    fn observed_username(&mut self) -> ChannelFuture<'_, Option<String>> {
        Box::pin(async { Ok(None) })
    }
    fn collect_sso(&mut self) -> ChannelFuture<'_, Vec<(String, String)>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn reuse_session<'a>(
        &'a mut self,
        _context: &'a AuthContext,
        _budget: Duration,
    ) -> ChannelFuture<'a, ReuseOutcome> {
        Box::pin(async { Ok(ReuseOutcome::Interactive("not_http")) })
    }
    fn closed(&self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(std::future::pending())
    }
    fn cleanup(&mut self) -> ChannelFuture<'_, ()>;
}
/// 浏览器选择宿主状态：会话内显式偏好 `chosen`（wire 兼容保留——前端选择器已
/// 退场，生产无写入方）+ 进程内选择缓存（壳进程生命周期，B1 缓存四律由
/// [`SelectionCache`] 承载：Use 落槽、回退内嵌永不落槽、命中轻量复核可用即短路）。
#[derive(Debug, Default)]
pub(crate) struct BrowserSelection {
    chosen: Mutex<Option<SupportedBrowser>>,
    cache: std::sync::Mutex<SelectionCache>,
}
impl BrowserSelection {
    /// 会话内显式选择（wire 兼容面）。写入同时失效缓存——缓存命中路径不重读
    /// `chosen`（B1 缓存契约）。
    pub(crate) fn choose(&self, wire: &str) -> Result<(), String> {
        let browser = browser_for_wire_name(wire).ok_or("请选择Chrome或Edge。")?;
        *self.chosen.lock().expect("browser selection") = Some(browser);
        self.cache.lock().expect("selection cache").invalidate();
        Ok(())
    }
    fn chosen(&self) -> Option<SupportedBrowser> {
        *self.chosen.lock().expect("browser selection")
    }
}

/// 阶梯+缓存求值核心（探测面注入；生产=LaunchServices 双查询，测试=注入闭包）。
///
/// 宿主侧两段：① 缓存命中先对缓存种类做轻量复核（`recheck` 注入；B1 约束：
/// 二进制在位级别，禁止会话级探测），可用即短路返回、不可用交由共享 resolve
/// 清槽；② 未短路时跑完整探测（`detect` 注入），结论经共享
/// [`SelectionCache::resolve`] 按缓存四律落槽。锁不跨 await。
async fn resolve_browser_decision_with<D, R, FD, FR>(
    selection: &BrowserSelection,
    detect: D,
    recheck: R,
) -> Result<Decision, String>
where
    D: FnOnce() -> FD,
    R: Fn(BrowserKind) -> FR,
    FD: Future<Output = Result<Detection, String>>,
    FR: Future<Output = Result<bool, String>>,
{
    let cached = selection
        .cache
        .lock()
        .expect("selection cache")
        .cached();
    if let Some(kind) = cached
        && recheck(kind).await?
    {
        return Ok(Decision::Use(kind));
    }
    let detection = detect().await?;
    Ok(selection
        .cache
        .lock()
        .expect("selection cache")
        .resolve(
            MACOS_SUPPORTED,
            || detection,
            // 到达此处时命中路径已提前返回；残留槽位（复核失败或并发写入）经
            // false 复核清槽后用本次探测结论重建，不会误短路。
            |_| false,
        ))
}

/// 生产探测面：缓存轻量复核=单浏览器 LaunchServices 查询；完整探测=双查询。
///
/// C3 换岗注记：系统浏览器模式改走原生零控制通道（`native.rs`）后，生产分派暂不
/// 消费本阶梯；B1-B3 基础设施保留，供自动登录隐藏 Chromium 选择（计划 §C3）复用。
#[allow(dead_code)]
async fn resolve_browser_decision(
    app: &tauri::AppHandle,
    selection: &BrowserSelection,
) -> Result<Decision, String> {
    resolve_browser_decision_with(
        selection,
        || external::detect_browsers(app, selection.chosen()),
        |kind| external::browser_kind_available(app, kind),
    )
    .await
}

/// 阶梯结论 → 通道工厂意图（纯函数，可单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserDispatch {
    /// 打开 CDP 外部通道（Chrome/Edge）。
    External(SupportedBrowser),
    /// 可用集为空：本次连接静默改派内嵌通道（不报错、不改用户认证设置）。
    EmbeddedFallback,
}

fn browser_dispatch(decision: Decision) -> BrowserDispatch {
    match decision {
        Decision::Use(kind) => BrowserDispatch::External(SupportedBrowser::from_kind(kind)),
        Decision::FallBackEmbedded => BrowserDispatch::EmbeddedFallback,
    }
}

/// [`open_browser_channel`] 的结论：已开通道（Chrome/Edge），
/// 或零可用时的静默内嵌改派（由调用方打开内嵌通道）。
///
/// C3 换岗注记：同 [`resolve_browser_decision`]——阶梯基础设施保留，换岗自动登录。
#[allow(dead_code)]
enum BrowserOpenOutcome<T> {
    Channel(T),
    EmbeddedFallback,
}

/// 浏览器通道直开（B3 阶梯+缓存；探测与开通道注入，可编排级单测）：
/// `resolve` 跑阶梯 → External(Chrome/Edge) 直开（错误原样上抛）；
/// 零可用 → [`BrowserOpenOutcome::EmbeddedFallback`]（既有静默内嵌路径）。
#[allow(dead_code)]
async fn open_browser_channel<T, D, DF, E, EF>(
    mut resolve: D,
    mut open_external: E,
) -> Result<BrowserOpenOutcome<T>, String>
where
    D: FnMut() -> DF,
    DF: Future<Output = Result<Decision, String>>,
    E: FnMut(SupportedBrowser) -> EF,
    EF: Future<Output = Result<T, String>>,
{
    match browser_dispatch(resolve().await?) {
        BrowserDispatch::External(browser) => {
            open_external(browser).await.map(BrowserOpenOutcome::Channel)
        }
        BrowserDispatch::EmbeddedFallback => Ok(BrowserOpenOutcome::EmbeddedFallback),
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct BrowserStatus {
    pub(crate) default_browser: Option<String>,
    pub(crate) installed: Vec<String>,
    pub(crate) chosen: Option<String>,
}
pub(crate) async fn browser_status(
    app: &tauri::AppHandle,
    selection: &BrowserSelection,
) -> Result<BrowserStatus, String> {
    let installed = external::discover_installed_browsers(app).await?;
    let default = external::discover_default_browser(app).await?;
    Ok(BrowserStatus {
        default_browser: default.map(|c| c.browser.wire_name().into()),
        installed: installed
            .iter()
            .map(|c| c.browser.wire_name().into())
            .collect(),
        chosen: selection.chosen().map(|b| b.wire_name().into()),
    })
}
struct HostFactory<'a> {
    app: &'a tauri::AppHandle,
    /// C3 换岗注记：系统浏览器模式改走原生通道后本字段暂不消费（B1-B3 阶梯保留
    /// 给自动登录 Chromium 选择）；`authenticate_with_options` 签字保持稳定。
    #[allow(dead_code)]
    selection: &'a BrowserSelection,
    context: &'a AuthContext,
    config_dir: std::path::PathBuf,
    generation: u64,
    round: AtomicU64,
    intent: AuthIntent,
}
trait ChannelFactory: Send + Sync {
    fn initial_mode(&self, mode: AuthMode) -> AuthMode {
        mode
    }
    fn begin_round(&self, _round: AuthRound) {}
    fn open(
        &self,
        mode: AuthMode,
        presentation: PresentationIntent,
    ) -> ChannelFuture<'_, Option<Box<dyn ChannelTrait>>>;
}
impl ChannelFactory for HostFactory<'_> {
    fn initial_mode(&self, mode: AuthMode) -> AuthMode {
        self.intent.initial_mode(mode)
    }
    fn begin_round(&self, round: AuthRound) {
        self.round.store(
            if round == AuthRound::First { 1 } else { 2 },
            Ordering::Release,
        );
    }
    fn open(
        &self,
        mode: AuthMode,
        presentation: PresentationIntent,
    ) -> ChannelFuture<'_, Option<Box<dyn ChannelTrait>>> {
        Box::pin(async move {
            if mode == AuthMode::Headless {
                return Ok(Some(
                    Box::new(http_probe::HttpChannel::new(self.config_dir.clone()))
                        as Box<dyn ChannelTrait>,
                ));
            }
            if mode == AuthMode::Embedded {
                return embedded::EmbeddedChannel::open(
                    self.app,
                    self.context,
                    presentation,
                    self.generation,
                    self.round.load(Ordering::Acquire),
                    self.intent,
                )
                .map(|c| Some(Box::new(c) as Box<dyn ChannelTrait>));
            }
            // C3：系统浏览器模式在 `authenticate_with_options` 入口改走原生零控制
            // 通道（`native`），编排器/通道工厂不再打开 CDP 外部通道——此分支不可
            // 达，如实报告内部状态错配。
            Err("系统浏览器模式已改由原生通道处理，不应进入页面编排器。".into())
        })
    }
}
#[derive(Default)]
pub(crate) struct AuthOptions {
    pub(crate) deadline: Option<tokio::time::Instant>,
    pub(crate) auto_login: bool,
    pub(crate) hidden: bool,
    pub(crate) silent: bool,
    pub(crate) config_dir: Option<std::path::PathBuf>,
    pub(crate) explicit: Option<exv_vpn_darwin_core::atrust_auth_storage::SchoolCredentialInput>,
    pub(crate) milestone: Option<std::sync::Arc<dyn Fn(AuthMilestone) + Send + Sync>>,
    pub(crate) diagnostic: Option<std::sync::Arc<dyn Fn(AuthFailureDiagnostic) + Send + Sync>>,
}
/// 每轮真实失败的有限诊断，学校摘录仅来自同次页面采样。
#[derive(Clone, Debug)]
pub(crate) struct AuthFailureDiagnostic {
    pub(crate) round: u8,
    pub(crate) kind: &'static str,
    pub(crate) operation: &'static str,
    pub(crate) detail: Option<String>,
    pub(crate) school_hint: Option<String>,
    pub(crate) gateway_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthMilestone {
    Opening,
    SchoolLogin,
    Submitting,
    GatewayConfirming,
    AwaitingManual,
    Recovering,
    /// C3 原生系统浏览器通道：票据被网关判失效（75500304）——提示用户在浏览器中
    /// 重新登录；等待继续（新票到达自动再兑换）。
    TicketInvalid,
}
impl AuthOptions {
    fn emit(&self, milestone: AuthMilestone) {
        if self.silent {
            return;
        }
        if let Some(sink) = &self.milestone {
            sink(milestone);
        }
    }
}

pub(crate) async fn authenticate_context(
    app: &tauri::AppHandle,
    context: &AuthContext,
    mode: AuthMode,
    selection: &BrowserSelection,
    request: &ConnectionRequest,
) -> Result<AuthArtifact, String> {
    authenticate_with_options(
        app,
        context,
        mode,
        selection,
        request,
        AuthOptions::default(),
    )
    .await
}
pub(crate) async fn authenticate_with_options(
    app: &tauri::AppHandle,
    context: &AuthContext,
    mut mode: AuthMode,
    selection: &BrowserSelection,
    request: &ConnectionRequest,
    mut options: AuthOptions,
) -> Result<AuthArtifact, String> {
    let started = std::time::Instant::now();
    eprintln!(
        "aTrust auth.begin owner={} channel={:?} round=1 auto_login={} hidden={}",
        request.generation(),
        mode,
        options.auto_login,
        options.hidden
    );
    // C3：系统浏览器模式改走原生零控制通道（open 默认浏览器 + 本次请求本地 Agent 收
    // 票据），不进入页面编排器——没有可控页面。内嵌/自动登录/HTTP 探测保持原编排。
    if mode == AuthMode::Browser {
        let result = native::authenticate(app, context, request, &options).await;
        eprintln!(
            "aTrust auth.result owner={} channel=browser(native) outcome={} elapsed_ms={}",
            request.generation(),
            match &result {
                Ok(_) => "authenticated",
                Err(error) if error == CANCELLED => "cancelled",
                Err(_) => "error",
            },
            started.elapsed().as_millis()
        );
        match result {
            Err(error) if error == native::USE_EMBEDDED => {
                request.set_temporary_auth_mode("embedded");
                super::atrust_connect::emit_browser_channel_selected(app, request.generation(), "embedded", false);
                mode = AuthMode::Embedded; options.auto_login = false; options.hidden = false; options.explicit = None;
            }
            Err(error) if error == native::TRY_SAVED_LOGIN => {
                request.set_temporary_auth_mode("headless");
                super::atrust_connect::emit_browser_channel_selected(app, request.generation(), "headless", true);
                mode = AuthMode::Headless; options.auto_login = true; options.hidden = true; options.explicit = None;
            }
            result => return result,
        }
    }
    let factory = HostFactory {
        app,
        selection,
        context,
        generation: request.generation(),
        round: AtomicU64::new(1),
        intent: AuthIntent::from_auto_login(options.auto_login),
        config_dir: options.config_dir.clone().unwrap_or_else(|| {
            exv_vpn_darwin_core::DarwinUiConfig::default()
                .config_dir()
                .to_path_buf()
        }),
    };
    let orchestrator = AuthOrchestrator {
        context,
        deadline: options
            .deadline
            .unwrap_or_else(|| tokio::time::Instant::now() + AUTH_BUDGET),
        tick: Duration::from_millis(250),
    };
    let result = orchestrator
        .run_with_options(&factory, mode, request.cancelled(), options)
        .await;
    eprintln!(
        "aTrust auth.result owner={} channel={:?} outcome={} elapsed_ms={}",
        request.generation(),
        mode,
        match &result {
            Ok(_) => "authenticated",
            Err(error) if error == CANCELLED => "cancelled",
            Err(error) if error == TIMEOUT => "timeout",
            Err(_) => "error",
        },
        started.elapsed().as_millis()
    );
    result
}
struct AuthOrchestrator<'a> {
    context: &'a AuthContext,
    deadline: tokio::time::Instant,
    tick: Duration,
}
impl AuthOrchestrator<'_> {
    async fn operation<T>(&self, future: ChannelFuture<'_, T>) -> Result<T, String> {
        tokio::time::timeout_at(self.deadline, future)
            .await
            .map_err(|_| TIMEOUT.to_owned())?
    }
    async fn run_with_options(
        &self,
        factory: &dyn ChannelFactory,
        mut mode: AuthMode,
        cancel: impl Future<Output = ()> + Send,
        mut options: AuthOptions,
    ) -> Result<AuthArtifact, String> {
        tokio::pin!(cancel);
        let requested_mode = mode;
        mode = factory.initial_mode(mode);
        let mut channel: Option<Box<dyn ChannelTrait>> = None;
        let mut interactive = false;
        let mut navigated = false;
        let mut login_started = false;
        let mut missing_since = None;
        let mut manual_requested = false;
        let mut show_requested = false;
        let mut hidden = (options.auto_login && options.hidden && mode == AuthMode::Embedded)
            || mode == AuthMode::Headless;
        let mut http_fallback = false;
        let mut round = AuthRound::First;
        factory.begin_round(round);
        let mut budget = RoundBudget::default();
        let mut submitted_at = None;
        let mut injected_username: Option<String> = None;
        // 仅在连接开始时选择一次显式/保存来源；错误延迟到确实需要填交时返回。
        let credentials = if options.auto_login {
            FrozenCredentials::freeze(&mut options)
        } else {
            Ok(FrozenCredentials::Missing)
        };
        let mut pending_failure: Option<(String, AuthFailureKind)> = None;
        // 仅受控内嵌通道的真实探针错误使用局部连续预算；页面未知事实不启动预算。
        let mut probe_failure: Option<(tokio::time::Instant, String)> = None;
        let mut first_failure: Option<(AuthFailureKind, &'static str)> = None;
        let mut failure_school_hint: Option<String> = None;
        let mut failure_gateway_code: Option<i32> = None;
        let mut failure_operation = "authentication";
        let mut failure_cause: Option<&'static str> = None;
        let mut failure_kind = AuthFailureKind::TransportFailed;
        let mut credential_failure = false;
        let mut challenge_visible = false;
        let mut not_ready_since = None;
        let mut last_observed_url: Option<String> = None;
        let mut sequence = 0;
        let started = tokio::time::Instant::now();
        let browser_origin = tauri::Url::parse(&self.context.gateway)
            .map_err(|_| "认证冻结网关无效".to_owned())?
            .origin()
            .ascii_serialization();
        let mut policy = page_policy::PagePolicy::with_gateway_recovery(mode == AuthMode::Embedded);
        let mut install_since = None;
        options.emit(AuthMilestone::Opening);
        // 操作、控制事件与采样共享同一循环和绝对期限；没有第二个连接 owner。
        let mut result = loop {
            tokio::select! { biased;
                _ = &mut cancel => break Err(CANCELLED.into()),
                _ = tokio::time::sleep_until(self.deadline) => break Err(TIMEOUT.into()),
                _ = std::future::ready(()) => {}
            }
            if http_fallback {
                if let Some(mut old) = channel.take() {
                    let cleanup = tokio::select! { biased;
                        _ = &mut cancel => break Err(CANCELLED.into()),
                        result = tokio::time::timeout_at(self.deadline.min(tokio::time::Instant::now() + Duration::from_secs(3)), old.cleanup()) => result,
                    };
                    let cleanup_error = match cleanup {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error),
                        Err(error) => Some(format!("HTTP复用旧认证通道收尾超时：{error}")),
                    };
                    if let Some(error) = cleanup_error {
                        failure_kind = AuthFailureKind::TransportFailed;
                        failure_operation = "cleanup";
                        failure_cause = Some("旧认证通道收尾失败");
                        if let Some(sink) = &options.diagnostic {
                            sink(AuthFailureDiagnostic {
                                round: if round == AuthRound::First { 1 } else { 2 },
                                kind: failure_kind_code(failure_kind),
                                operation: failure_operation,
                                detail: super::auth_diagnostics::local_error_detail(&error),
                                school_hint: None,
                                gateway_code: None,
                            });
                        }
                        break Err("HTTP复用结束后未能完成旧认证通道收尾。".into());
                    }
                }
                mode = if requested_mode == AuthMode::Browser && !manual_requested {
                    AuthMode::Browser
                } else {
                    AuthMode::Embedded
                };
                hidden = !manual_requested
                    && mode == AuthMode::Embedded
                    && (options.auto_login || requested_mode == AuthMode::Headless);
                navigated = false;
                login_started = false;
                missing_since = None;
                show_requested = manual_requested;
                http_fallback = false;
                probe_failure = None;
                policy = page_policy::PagePolicy::with_gateway_recovery(mode == AuthMode::Embedded);
                options.emit(if manual_requested {
                    AuthMilestone::AwaitingManual
                } else {
                    AuthMilestone::Opening
                });
            }
            if let Some((error, kind)) = pending_failure.take() {
                failure_kind = kind;
                if let Some(sink) = &options.diagnostic {
                    sink(AuthFailureDiagnostic {
                        round: if round == AuthRound::First { 1 } else { 2 },
                        kind: failure_kind_code(kind),
                        operation: failure_operation,
                        detail: if failure_operation == "page_policy" { None } else { super::auth_diagnostics::local_error_detail(&error) },
                        school_hint: failure_school_hint.take(),
                        gateway_code: failure_gateway_code.take(),
                    });
                }
                eprintln!(
                    "aTrust auth.round_result round={:?} outcome=failed kind={:?} elapsed_ms={}",
                    round,
                    kind,
                    started.elapsed().as_millis()
                );
                let mut facts = RecoveryFacts {
                    round,
                    failure: kind,
                    cancelled: error == CANCELLED,
                    time_remaining: tokio::time::Instant::now() < self.deadline,
                    session_recheck: SessionRecheck::Pending,
                    challenge_visible,
                    submission: budget.state,
                };
                let mut decision = decide_recovery(&facts);
                if decision == RecoveryDecision::RecheckSession {
                    let closed = channel.as_ref().map(|current| current.closed());
                    let closed = async move {
                        match closed {
                            Some(closed) => closed.await,
                            None => std::future::pending().await,
                        }
                    };
                    let recheck = async {
                        if let Some(current) = channel.as_mut() {
                            if matches!(
                                current.probe_session(self.context).await?,
                                SessionStatus::LoggedIn
                            ) {
                                if let Some(candidate) = current.extract(self.context).await? {
                                    if GatewayOrigin::parse(candidate.gateway()).is_ok_and(
                                        |origin| origin.endpoint() == self.context.gateway,
                                    ) {
                                        return Ok(Some((
                                            candidate,
                                            channel_auth_source(current.capabilities().surface),
                                        )));
                                    }
                                }
                            }
                        }
                        Ok::<_, String>(None)
                    };
                    let recheck_deadline = self
                        .deadline
                        .min(tokio::time::Instant::now() + Duration::from_secs(2));
                    tokio::select! { biased;
                        _ = &mut cancel => break Err(CANCELLED.into()),
                        _ = closed => break Err(CANCELLED.into()),
                        result = tokio::time::timeout_at(recheck_deadline, recheck) => {
                            match result {
                                Ok(Ok(Some((candidate, source)))) => {
                                    // 取消和绝对期限仍优先于刚复核得到的候选。
                                    facts.session_recheck = SessionRecheck::ValidSameOrigin;
                                    facts.time_remaining = tokio::time::Instant::now() < self.deadline;
                                    if decide_recovery(&facts) != RecoveryDecision::Complete { break Err(TIMEOUT.into()); }
                                    let mut artifact = AuthArtifact::new(candidate, source);
                                    artifact.interactive = interactive;
                                    eprintln!("aTrust auth.recheck round={:?} outcome=authenticated elapsed_ms={}", round, started.elapsed().as_millis());
                                    break Ok(artifact);
                                }
                                Ok(Err(ref recheck_error)) if recheck_error == CANCELLED => break Err(CANCELLED.into()),
                                _ => {}
                            }
                        }
                    }
                    // 局部复核耗尽可恢复；原总期限耗尽必须保留既有超时边界。
                    tokio::select! { biased;
                        _ = &mut cancel => break Err(CANCELLED.into()),
                        _ = std::future::ready(()) => {}
                    }
                    facts.session_recheck = SessionRecheck::NoValidSession;
                    facts.time_remaining = tokio::time::Instant::now() < self.deadline;
                    if !facts.time_remaining {
                        break Err(TIMEOUT.into());
                    }
                    decision = decide_recovery(&facts);
                }
                match decision {
                    RecoveryDecision::Cancelled => break Err(CANCELLED.into()),
                    RecoveryDecision::AwaitChallenge => {
                        if options.silent {
                            break Err(error);
                        }
                        manual_requested = true;
                        show_requested = true;
                        options.emit(AuthMilestone::AwaitingManual);
                    }
                    RecoveryDecision::RestartRound => {
                        // 原地重启（S4b）：轮次模式不变且通道支持时保留窗口与 data
                        // store，仅重新导航——彻底切断"关窗=销毁已认证 SSO"；关窗
                        // 收尾只留给终局失败/取消/模式切换。重导航目标由
                        // first_navigation（direct_login_target）决策。
                        let restart_mode = factory.initial_mode(requested_mode);
                        let in_place = restart_mode == mode
                            && channel
                                .as_ref()
                                .is_some_and(|current| current.supports_in_place_restart());
                        if in_place {
                            if let Some(current) = channel.as_mut() {
                                current.begin_round(2);
                            }
                            eprintln!(
                                "aTrust auth.recovery round=2 outcome=in_place_restart elapsed_ms={}",
                                started.elapsed().as_millis()
                            );
                        } else {
                            // 本轮资源必须确认收尾；失败时保留触发原因，不继续建窗。
                            if let Some(mut old) = channel.take() {
                                let cleanup_deadline = self
                                    .deadline
                                    .min(tokio::time::Instant::now() + Duration::from_secs(3));
                                let cleanup = tokio::select! { biased;
                                    _ = &mut cancel => break Err(CANCELLED.into()),
                                    result = tokio::time::timeout_at(cleanup_deadline, old.cleanup()) => result,
                                };
                                if !matches!(cleanup, Ok(Ok(()))) {
                                    eprintln!(
                                        "aTrust auth.cleanup outcome=failed before_recovery=true"
                                    );
                                    break Err(error);
                                }
                            }
                            eprintln!(
                                "aTrust auth.recovery round=2 outcome=opening elapsed_ms={}",
                                started.elapsed().as_millis()
                            );
                        }
                        first_failure = Some((kind, presentation_cause(kind, failure_operation, failure_cause)));
                        round = AuthRound::Second;
                        factory.begin_round(round);
                        mode = restart_mode;
                        budget = RoundBudget::default();
                        submitted_at = None;
                        injected_username = None;
                        not_ready_since = None;
                        probe_failure = None;
                        last_observed_url = None;
                        navigated = false;
                        login_started = false;
                        missing_since = None;
                        install_since = None;
                        challenge_visible = false;
                        sequence = 0;
                        hidden =
                            !manual_requested && mode != AuthMode::Browser && options.auto_login;
                        show_requested = manual_requested;
                        policy = page_policy::PagePolicy::with_gateway_recovery(
                            mode == AuthMode::Embedded,
                        );
                        options.emit(AuthMilestone::Recovering);
                        continue;
                    }
                    _ => break Err(error),
                }
            }
            if options.silent && (manual_requested || show_requested || mode == AuthMode::Browser) {
                break Err("学校认证需要人工操作，请主动重新连接。".into());
            }
            failure_kind = AuthFailureKind::TransportFailed;
            failure_operation = "authentication";
            failure_cause = None;
            let opening = channel.is_none();
            if opening {
                failure_operation = "window_open";
                tokio::select! { biased;
                    _ = &mut cancel => break Err(CANCELLED.into()),
                    opened = self.operation(factory.open(mode, if hidden { PresentationIntent::Hidden } else { PresentationIntent::Visible })) => {
                        match opened {
                            Ok(opened) => channel = opened,
                            Err(error) if error == CANCELLED || error == TIMEOUT => break Err(error),
                            Err(error) => { pending_failure = Some((error, AuthFailureKind::StartupFailed)); continue; }
                        }
                    }
                }
            }
            // 建窗和本轮原语分开：第一次采样/注入挂起时，当前窗口关闭同样立即取消。
            let closed = channel.as_ref().map(|current| current.closed());
            let closed = async move {
                match closed {
                    Some(closed) => closed.await,
                    None => std::future::pending().await,
                }
            };
            let step = async {
                let Some(channel) = channel.as_mut() else {
                    return Ok(None);
                };
                let capabilities = channel.capabilities();
                interactive |= opening
                    && mode != AuthMode::Browser
                    && !hidden
                    && capabilities.surface != PresentationSurface::NoSurface;
                if !capabilities.can_extract {
                    return Err("当前认证通道不能采集网页登录会话。".into());
                }
                if capabilities.surface == PresentationSurface::NoSurface {
                    match self
                        .operation(
                            channel.reuse_session(
                                self.context,
                                self.deadline
                                    .saturating_duration_since(tokio::time::Instant::now()),
                            ),
                        )
                        .await?
                    {
                        ReuseOutcome::Ready(session) => {
                            if !GatewayOrigin::parse(session.gateway())
                                .is_ok_and(|origin| origin.endpoint() == self.context.gateway)
                            {
                                return Err("HTTP候选与冻结网关不一致".into());
                            }
                            return Ok(Some(AuthArtifact::new(session, AuthSource::HttpCas)));
                        }
                        ReuseOutcome::Interactive(_reason) => {
                            http_fallback = true;
                            return Ok(None);
                        }
                    }
                }
                if show_requested && (mode != AuthMode::Browser || navigated) {
                    failure_operation = "presentation";
                    let presentation = self
                        .operation(channel.present(PresentationIntent::Visible))
                        .await;
                    if matches!(
                        presentation,
                        Ok(PresentationOutcome::Visible | PresentationOutcome::AlwaysVisible)
                    ) {
                        interactive = true;
                    }
                    presentation?;
                    hidden = false;
                    show_requested = false;
                }
                if !navigated {
                    failure_operation = "navigation";
                    // 首导航目标先问通道（S4b）：内嵌窗冷缓存+明确未登录时直达
                    // 学校登录页，跳过 authConfig JSON 闪现；其余通道默认原语义。
                    let first = self
                        .operation(channel.first_navigation(self.context))
                        .await?;
                    self.operation(channel.navigate(&first.url)).await?;
                    navigated = true;
                    if first.school_login {
                        login_started = true;
                        options.emit(AuthMilestone::SchoolLogin);
                    }
                    if mode == AuthMode::Browser {
                        failure_operation = "presentation";
                        let presentation = self
                            .operation(channel.present(PresentationIntent::Visible))
                            .await?;
                        interactive |= presentation == PresentationOutcome::Visible;
                        show_requested = false;
                        hidden = false;
                    }
                }
                sequence += 1;
                failure_operation = "page_observation";
                let page = self.operation(channel.observe_page(sequence)).await?;
                if let Observation::Observed(url) = &page.url {
                    if last_observed_url.as_ref().is_some_and(|old| old != url) {
                        not_ready_since = None;
                    }
                    last_observed_url = Some(url.clone());
                }
                let school_page = page.coherent
                    && matches!(&page.origin, Observation::Observed(origin) if origin == school_auto_login::SCHOOL_LOGIN_ORIGIN);
                challenge_visible = school_page
                    && matches!(&page.dom, Observation::Observed(dom) if dom.challenge_visible);
                failure_operation = "session_probe";
                let mut probe_failed_now = false;
                let status = match self.operation(channel.probe_session(self.context)).await {
                    Ok(status) => {
                        if matches!(status, SessionStatus::LoggedIn | SessionStatus::LoggedOut(_)) {
                            probe_failure = None;
                        }
                        status
                    }
                    Err(error) if error == CANCELLED || error == TIMEOUT => return Err(error),
                    Err(error) if capabilities.surface == PresentationSurface::Controlled => {
                        let since = probe_failure
                            .as_ref()
                            .map_or_else(tokio::time::Instant::now, |(since, _)| *since);
                        probe_failure = Some((since, error));
                        probe_failed_now = true;
                        SessionStatus::Unavailable
                    }
                    Err(error) => return Err(error),
                };
                let probe_expired = probe_failed_now
                    && probe_failure.as_ref().is_some_and(|(since, _)| {
                        since.elapsed() >= page_policy::PAGE_STALL_TIMEOUT
                    });
                if decide_session_precedence(&SessionFacts {
                    gateway_logged_in: matches!(status, SessionStatus::LoggedIn),
                    extraction: SessionExtraction::NotAttempted,
                }) == SessionDecision::Extract
                {
                    options.emit(AuthMilestone::GatewayConfirming);
                    failure_operation = "session_extract";
                    if let Some(candidate) = self.operation(channel.extract(self.context)).await? {
                        if !GatewayOrigin::parse(candidate.gateway())
                            .is_ok_and(|origin| origin.endpoint() == self.context.gateway)
                        {
                            return Err("浏览器候选与本次冻结网关不一致。".into());
                        }
                        if decide_session_precedence(&SessionFacts {
                            gateway_logged_in: true,
                            extraction: SessionExtraction::ValidSameOrigin,
                        }) != SessionDecision::Complete
                        {
                            return Err("浏览器候选会话验证失败。".into());
                        }
                        if let Err(error) = policy.sample(
                            &page,
                            &status,
                            &browser_origin,
                            &self.context.auth_path,
                            login_started,
                            started.elapsed(),
                            page_policy::PAGE_STALL_TIMEOUT,
                        ) {
                            eprintln!(
                                "aTrust auth.diagnostic residual_page code={:?} kind={:?}",
                                error.code, error.failure_kind
                            );
                        }
                        // SSO 是可选附加缓存；失败只诊断，不能否决已可用候选。
                        if let Some(dir) = &options.config_dir {
                            if capabilities.can_inject {
                                // 可选采集只借用短预算，为可见成功呈现保留时间。
                                let reserve = if hidden {
                                    Duration::from_millis(1)
                                } else {
                                    Duration::from_millis(701)
                                };
                                let capture_budget = self
                                    .deadline
                                    .saturating_duration_since(tokio::time::Instant::now())
                                    .saturating_sub(reserve)
                                    .min(Duration::from_secs(1));
                                match tokio::time::timeout(capture_budget, channel.collect_sso())
                                    .await
                                {
                                    Ok(Ok(mut pairs)) => {
                                        use exv_vpn_darwin_core::{
                                            atrust_auth_storage as store,
                                            sso_session_jar::{SSO_HOST, SsoSessionJar},
                                        };
                                        let jar = match store::load_sso_jar(dir, SSO_HOST) {
                                            Ok(Some(jar)) => Ok(jar),
                                            Ok(None) => SsoSessionJar::new_fresh(),
                                            Err(_) => {
                                                eprintln!("aTrust SSO 档案读取失败，保留原档案");
                                                Err(exv_vpn_darwin_core::sso_session_jar::SsoJarError::Format)
                                            }
                                        };
                                        if let Ok(mut jar) = jar {
                                            for (name, value) in &pairs {
                                                let _ = jar.insert_cookie(name, value);
                                            }
                                            if store::save_sso_jar(dir, SSO_HOST, &jar).is_err() {
                                                eprintln!("aTrust SSO 档案保存失败");
                                            }
                                        }
                                        for (_, value) in &mut pairs {
                                            zeroize::Zeroize::zeroize(value);
                                        }
                                    }
                                    Ok(Err(_)) | Err(_) => {
                                        eprintln!("aTrust SSO 原生采集失败或短预算耗尽")
                                    }
                                }
                            }
                        }
                        if !hidden && capabilities.surface == PresentationSurface::Controlled {
                            self.operation(Box::pin(async {
                                tokio::time::sleep(Duration::from_millis(700)).await;
                                Ok(())
                            }))
                            .await?;
                        }
                        let mut artifact = AuthArtifact::new(
                            candidate,
                            channel_auth_source(capabilities.surface),
                        );
                        artifact.interactive = interactive;
                        return Ok(Some(artifact));
                    }
                    let since = *missing_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= Duration::from_secs(10) {
                        return Err("学校已确认登录，但尚未取得网关会话，请重试。".into());
                    }
                    return Ok(None);
                }
                failure_operation = "page_policy";
                let latched = match policy.sample(
                    &page,
                    &status,
                    &browser_origin,
                    &self.context.auth_path,
                    login_started,
                    started.elapsed(),
                    page_policy::PAGE_STALL_TIMEOUT,
                ) {
                    Ok(latched) => latched,
                    Err(error) => {
                        // 明确页面业务失败先处理；局部停滞或安装页不能覆盖已到期的真实通信故障。
                        if probe_expired
                            && !matches!(
                                error.code,
                                page_policy::PolicyErrorCode::FailedPage
                                    | page_policy::PolicyErrorCode::GatewayThrottled
                            )
                        {
                            failure_operation = "session_probe";
                            failure_kind = AuthFailureKind::TransportFailed;
                            return Err(probe_failure.take().unwrap().1);
                        }
                        failure_kind = error.failure_kind.unwrap_or(match error.code {
                            page_policy::PolicyErrorCode::Stalled => AuthFailureKind::LocalTimeout,
                            page_policy::PolicyErrorCode::GatewayThrottled => AuthFailureKind::GatewayThrottled,
                            _ => AuthFailureKind::UnknownFailure,
                        });
                        failure_cause = Some(if error.code == page_policy::PolicyErrorCode::ClientInstallRequired {
                            "网关要求启动或安装客户端"
                        } else { failure_kind_label(failure_kind) });
                        failure_school_hint = error.detail.clone();
                        failure_gateway_code = page.startup.and_then(|facts| facts.gateway_code);
                        return Err(policy_message(error));
                    }
                };
                if probe_expired {
                    failure_operation = "session_probe";
                    failure_kind = AuthFailureKind::TransportFailed;
                    return Err(probe_failure.take().unwrap().1);
                }
                if policy.install_recovery_needed() && !latched {
                    let since = *install_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= Duration::from_secs(3) {
                        failure_operation = "navigation";
                        self.operation(channel.navigate(&format!(
                            "{}{}",
                            self.context.gateway, self.context.auth_path
                        )))
                        .await?;
                        install_since = None;
                    }
                } else {
                    install_since = None;
                }
                if let SessionStatus::LoggedOut(entry) = status {
                    if !login_started {
                        let entry = entry.ok_or("网关未提供学校认证入口。")?;
                        let url = login_url(&self.context.gateway, &entry)?;
                        failure_operation = "navigation";
                        self.operation(channel.navigate(&url)).await?;
                        login_started = true;
                        options.emit(AuthMilestone::SchoolLogin);
                    }
                }
                if school_page && hidden && (!options.auto_login || manual_requested) {
                    show_requested = true;
                    options.emit(AuthMilestone::AwaitingManual);
                }
                let form_ready =
                    matches!(&page.dom, Observation::Observed(dom) if dom.school_form_ready);
                if challenge_visible && capabilities.can_inject {
                    if options.silent {
                        return Err("学校认证出现验证码，需要人工操作，请主动重新连接。".into());
                    }
                    manual_requested = true;
                    show_requested = true;
                    options.emit(AuthMilestone::AwaitingManual);
                }
                if school_page
                    && form_ready
                    && options.auto_login
                    && !manual_requested
                    && budget.state == SubmissionState::NotStarted
                    && capabilities.can_inject
                {
                    let material = credentials
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(FrozenCredentials::deliver);
                    let credentials = match material {
                        Ok(Some(credentials)) => credentials,
                        Ok(None) => {
                            credential_failure = true;
                            return Err(CREDENTIAL_REQUIRED.into());
                        }
                        Err(error) => {
                            credential_failure = true;
                            return Err(error);
                        }
                    };
                    let submitted_username = credentials.username.to_string();
                    let delivery = CredentialDelivery {
                        username: credentials.username,
                        password: credentials.password,
                        gateway_origin: browser_origin.clone(),
                    };
                    // 调用前占用预算；pending/错误/控制中断都不能在回退后再次提交。
                    if !budget.begin_submission() {
                        return Ok(None);
                    }
                    let injection_started = tokio::time::Instant::now();
                    failure_operation = "credential_submission";
                    let injection = self.operation(channel.inject_once(delivery)).await;
                    match injection {
                        Ok(InjectionOutcome::NotReady) => {
                            budget.finish_submission(SubmissionOutcome::NotReady);
                            let since = *not_ready_since.get_or_insert(injection_started);
                            if since.elapsed() >= page_policy::PAGE_STALL_TIMEOUT {
                                failure_kind = AuthFailureKind::LocalTimeout;
                                failure_cause = Some("学校登录表单持续未就绪");
                                return Err("学校表单持续未就绪，自动填交尚未完成。".into());
                            }
                        }
                        Ok(InjectionOutcome::Submitted) => {
                            injected_username = Some(submitted_username);
                            budget.finish_submission(SubmissionOutcome::Submitted);
                            not_ready_since = None;
                            submitted_at = Some(tokio::time::Instant::now());
                            options.emit(AuthMilestone::Submitting);
                        }
                        Ok(InjectionOutcome::ChallengeVisible) => {
                            if options.silent {
                                return Err(
                                    "学校认证出现验证码，需要人工操作，请主动重新连接。".into()
                                );
                            }
                            budget.finish_submission(SubmissionOutcome::Finished);
                            manual_requested = true;
                            challenge_visible = true;
                            show_requested = true;
                            options.emit(AuthMilestone::AwaitingManual);
                        }
                        Ok(InjectionOutcome::ServiceMismatch) => {
                            budget.finish_submission(SubmissionOutcome::Finished);
                            failure_kind = AuthFailureKind::UnknownFailure;
                            failure_cause = Some("学校表单的认证目标与本次网关不匹配");
                            return Err("学校表单的认证目标与本次冻结网关不一致。".into());
                        }
                        Err(error) if error == CANCELLED || error == TIMEOUT => return Err(error),
                        failed @ (Ok(InjectionOutcome::Failed) | Err(_)) => {
                            budget.finish_submission(SubmissionOutcome::Finished);
                            failure_kind = AuthFailureKind::StartupFailed;
                            failure_cause = Some("学校认证自动填交失败");
                            return Err(match failed {
                                Err(error) => error,
                                _ => "学校页面自动填交失败，未取得网关会话。".into(),
                            });
                        }
                    }
                }
                if !manual_requested
                    && submitted_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(15))
                {
                    failure_kind = AuthFailureKind::LocalTimeout;
                    failure_cause = Some("学校登录后网关会话持续未就绪");
                    return Err("学校表单已提交，但网关会话持续未就绪。".into());
                }
                Ok(None)
            };
            tokio::select! { biased;
                _ = &mut cancel => break Err(CANCELLED.into()),
                _ = closed => break Err(CANCELLED.into()),
                outcome = tokio::time::timeout_at(self.deadline, step) => match outcome {
                    Ok(Ok(Some(candidate))) => {
                        if tokio::time::Instant::now() >= self.deadline { break Err(TIMEOUT.into()); }
                        break Ok(candidate);
                    }, Ok(Ok(None)) => {},
                    Ok(Err(error)) => {
                        if error == CANCELLED || error == TIMEOUT || credential_failure { break Err(error); }
                        pending_failure = Some((error, failure_kind));
                        continue;
                    }, Err(_) => break Err(TIMEOUT.into()),
                }
            }
            tokio::select! { biased;
                _ = &mut cancel => break Err(CANCELLED.into()),
                _ = tokio::time::sleep_until(self.deadline) => break Err(TIMEOUT.into()),
                _ = tokio::time::sleep(self.tick) => {}
            }
        };
        let mut final_control = None;
        if let Ok(artifact) = &mut result {
            let observed = if let Some(channel) = channel.as_mut() {
                tokio::select! { biased;
                    _ = &mut cancel => { final_control = Some(CANCELLED); None },
                    _ = tokio::time::sleep_until(self.deadline) => { final_control = Some(TIMEOUT); None },
                    observed = tokio::time::timeout_at(self.deadline.min(tokio::time::Instant::now() + Duration::from_secs(1)), channel.observed_username()) => observed.ok().and_then(Result::ok).flatten(),
                }
            } else {
                None
            };
            if final_control.is_none() {
                artifact.authenticated_username = observed
                    .or_else(|| {
                        if !manual_requested {
                            injected_username
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        options.config_dir.as_ref().and_then(|dir| {
                            exv_vpn_darwin_core::atrust_auth_storage::username_for_session(
                                dir,
                                &artifact.session,
                            )
                        })
                    })
                    .unwrap_or_default();
                if !artifact.authenticated_username.is_empty() {
                    if let Some(dir) = &options.config_dir {
                        let _ =
                            exv_vpn_darwin_core::atrust_auth_storage::save_session_with_username(
                                dir,
                                &artifact.session,
                                &artifact.authenticated_username,
                            );
                    }
                    if !options.auto_login || manual_requested {
                        if let Ok(root) = external::profile_root() {
                            let _ = remember_username(
                                &root,
                                &self.context.gateway,
                                &artifact.authenticated_username,
                            );
                        }
                    }
                }
            }
        }
        if let Some(control) = final_control {
            result = Err(control.into());
        }
        eprintln!(
            "aTrust auth.round_result round={:?} outcome={} elapsed_ms={}",
            round,
            match &result {
                Ok(_) => "authenticated",
                Err(error) if error == CANCELLED => "cancelled",
                Err(error) if error == TIMEOUT => "timeout",
                Err(_) => "failed",
            },
            started.elapsed().as_millis()
        );
        if let Some(mut channel) = channel {
            // 浏览器模式自 B3 起可能已静默改派内嵌通道，收尾诊断按通道实际表面
            // 归类（外部 CDP 通道才按登录浏览器收尾记日志）。
            let external_channel =
                channel.capabilities().surface == PresentationSurface::AlwaysVisible;
            let outcome = tokio::time::timeout(Duration::from_secs(3), channel.cleanup()).await;
            if let Some(error) = browser_cleanup_diagnostic(external_channel, outcome) {
                eprintln!("EXV 登录浏览器收尾失败：{error}");
            }
        }
        match (result, first_failure) {
            (Err(error), Some(first))
                if error != CANCELLED && error != CREDENTIAL_REQUIRED && error != TIMEOUT
                    && !credential_failure && !error.contains("人工操作") =>
            {
                Err(authentication_failure_summary_causes(first, (failure_kind, presentation_cause(failure_kind, failure_operation, failure_cause))))
            }
            (result, _) => result.map_err(|error| {
                if [CANCELLED, CREDENTIAL_REQUIRED, TIMEOUT].contains(&error.as_str()) || credential_failure || error.contains("人工操作") {
                    error
                } else {
                    single_failure_summary_cause(failure_kind, presentation_cause(failure_kind, failure_operation, failure_cause))
                }
            }),
        }
    }
}

/// toast 只在终局组合一次；每轮细节经诊断回调记录。
fn failure_kind_code(kind: AuthFailureKind) -> &'static str {
    match kind {
        AuthFailureKind::CredentialsRejected => "credentials_rejected",
        AuthFailureKind::PolicyRestricted => "policy_restricted",
        AuthFailureKind::GatewayThrottled => "gateway_throttled",
        AuthFailureKind::StartupFailed => "browser_startup_failed",
        AuthFailureKind::LocalTimeout => "browser_auth_stalled",
        AuthFailureKind::TransportFailed => "browser_auth_transport",
        AuthFailureKind::UnknownFailure => "browser_auth_unknown_page",
    }
}
fn failure_kind_label(kind: AuthFailureKind) -> &'static str {
    match kind {
        AuthFailureKind::CredentialsRejected => "学校拒绝了账户或密码",
        AuthFailureKind::PolicyRestricted => "学校或网关拒绝了本次登录权限",
        AuthFailureKind::GatewayThrottled => "网关限流或登录会话冲突",
        AuthFailureKind::StartupFailed => "认证页面启动失败",
        AuthFailureKind::LocalTimeout => "认证页面持续没有进展",
        AuthFailureKind::TransportFailed => "认证页面通信失败",
        AuthFailureKind::UnknownFailure => "认证页面报告登录失败",
    }
}
fn presentation_cause(kind: AuthFailureKind, operation: &str, cause: Option<&'static str>) -> &'static str {
    cause.unwrap_or(match operation {
        "window_open" => "认证窗口打开失败",
        "navigation" => "学校认证页面加载失败",
        "presentation" => "认证页面显示失败",
        "credential_submission" => "学校认证自动填交失败",
        _ => failure_kind_label(kind),
    })
}
fn single_failure_summary_cause(kind: AuthFailureKind, cause: &str) -> String {
    format!("{}（诊断编号 {}）。请重试或查看日志。", cause, failure_kind_code(kind))
}
fn single_failure_summary(kind: AuthFailureKind) -> String {
    single_failure_summary_cause(kind, failure_kind_label(kind))
}
fn authentication_failure_summary_causes(first: (AuthFailureKind, &str), second: (AuthFailureKind, &str)) -> String {
    if first == second {
        format!("{}，自动恢复后仍失败（诊断编号 {}）。请重试或查看日志。", second.1, failure_kind_code(second.0))
    } else {
        format!("首次{}，自动恢复后{}（诊断编号 {}）。请重试或查看日志。", first.1, second.1, failure_kind_code(second.0))
    }
}
/// 复用原存储读取入口，仅冻结本次来源；每次注入仍按值交付零化副本。
enum FrozenCredentials {
    Explicit(exv_vpn_darwin_core::atrust_auth_storage::SchoolCredentialInput),
    Saved(exv_vpn_darwin_core::DarwinUiConfig),
    Missing,
}
impl FrozenCredentials {
    fn freeze(options: &mut AuthOptions) -> Result<Self, String> {
        // 显式半对也必须占据当前选择，不能与保存密码混配。
        if let Some(input) = options.explicit.take() {
            return Ok(Self::Explicit(input));
        }
        options
            .config_dir
            .as_ref()
            .map(exv_vpn_darwin_core::DarwinUiConfig::load_from_dir)
            .transpose()
            .map(|config| config.map(Self::Saved).unwrap_or(Self::Missing))
            .map_err(|_| "读取冻结配置的学校凭据失败，请检查配置后重试。".to_owned())
    }
    fn deliver(
        &self,
    ) -> Result<Option<exv_vpn_darwin_core::atrust_auth_storage::SchoolCredentials>, String> {
        use exv_vpn_darwin_core::atrust_auth_storage::{SchoolCredentials, school_credentials};
        match self {
            // 单次显式输入没有存储provider；保留本次已授权零化材料供最多两轮按次交付。
            Self::Explicit(input) => Ok((!input.username.trim().is_empty()
                && !input.password.is_empty())
            .then(|| SchoolCredentials {
                username: zeroize::Zeroizing::new(input.username.to_string()),
                password: zeroize::Zeroizing::new(input.password.to_string()),
            })),
            // 冻结配置内只有原账号及密文，按次由原provider解密，未跨轮保存明文。
            Self::Saved(config) => school_credentials(config, None)
                .map_err(|_| "解密已保存学校凭据失败，请重新输入凭据。".to_owned()),
            Self::Missing => Ok(None),
        }
    }
}
/// 认证来源按通道实际表面判定（B3 起浏览器模式可能静默改派内嵌通道，不能只看
/// 请求模式）：外部 CDP 通道=SystemBrowser，受控（内嵌）通道=Embedded，无表面
/// （HTTP 复用）=HttpCas。
fn channel_auth_source(surface: PresentationSurface) -> AuthSource {
    match surface {
        PresentationSurface::NoSurface => AuthSource::HttpCas,
        PresentationSurface::AlwaysVisible => AuthSource::SystemBrowser,
        PresentationSurface::Controlled => AuthSource::Embedded,
    }
}

/// 只消费外部浏览器通道的原收尾结果；认证结果不参与，也不被清理诊断改写。
fn browser_cleanup_diagnostic(
    external_channel: bool,
    outcome: Result<Result<(), String>, tokio::time::error::Elapsed>,
) -> Option<String> {
    if !external_channel {
        return None;
    }
    match outcome {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(elapsed) => Some(format!("超过原三秒等待预算：{elapsed}")),
    }
}

fn policy_message(error: page_policy::PolicyError) -> String {
    single_failure_summary(error.failure_kind.unwrap_or(match error.code {
        page_policy::PolicyErrorCode::GatewayThrottled => AuthFailureKind::GatewayThrottled,
        page_policy::PolicyErrorCode::Stalled => AuthFailureKind::LocalTimeout,
        _ => AuthFailureKind::UnknownFailure,
    }))
}
fn login_url(gateway: &str, path: &str) -> Result<String, String> {
    if !path.starts_with('/') || path.starts_with("//") || path.contains(['\\', '\r', '\n']) {
        return Err("网关返回了无效的学校认证入口。".into());
    }
    Ok(format!("{gateway}{path}"))
}

/// 纯决策（S4b，对齐 win32 embedded_auth.rs `direct_login_target`）：本轮首导航
/// 是否直达学校认证入口（`{gateway}{first_auth}`），跳过 authConfig JSON 页闪现。
///
/// 条件（全部满足才直达）：无缓存种子（`cache_present=false`——darwin 内嵌窗从不
/// 向窗口预置 EXV 侧 cookie，恒为冷缓存，保留参数与 win32 同形）；空 cookie 预探针
/// Ok 且明示未登录（darwin `probe_auth_config` Ok 即 win32 的 `code_ok`）；`first_auth`
/// 是合法本站路径（复用 [`login_url`] 校验）。其余一切情形（探针失败登录态未知、已
/// 登录、入口缺失/非法）一律保持 authConfig 初始页，与既有行为逐字节一致。
///
/// 安全论证（同 win32）：初始 authConfig 页不参与任何检测判据——网关 cookie 由
/// CAS 302 链自行播种；页面策略对 authConfig 路径不适用失败页判定；安装页恢复
/// 重导航独立发起、只复用 URL 字符串；候选采集从不依赖初始页面内容。持久 store
/// 命中时直达入口同样安全：CAS 链用既有 SSO 会话自动走完并落地已登录页，探针
/// 随即确认。跳过只省时间，不削弱错误识别。
fn direct_login_target(
    cache_present: bool,
    probe: &Result<SessionProbe, ProbeError>,
    gateway: &str,
) -> Option<String> {
    if cache_present {
        return None;
    }
    let state = probe.as_ref().ok()?;
    // 探针 Ok 即传输与应答均有效（等价 win32 code_ok）；仍显式校验未登录。
    if state.is_login {
        return None;
    }
    let first_auth = state.first_auth.as_deref().unwrap_or_default();
    login_url(gateway, first_auth).ok()
}
/// 网关键控哈希（SHA-256 小写十六进制）；`manual-username-<sha>` 与
/// `manual-persistent/<sha>` 目录共用同一哈希源，保证跨次登录与账户记忆同键
/// （与 win32 `browser_cache_profile::gateway_hash` 同源同形）。
pub(super) fn gateway_hash(gateway: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(gateway.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 仅保存上次成功的学校账户，用于新鲜手动表单预填；不包含密码或会话。
fn manual_username_path(root: &std::path::Path, gateway: &str) -> std::path::PathBuf {
    root.join(format!("manual-username-{}.json", gateway_hash(gateway)))
}
pub(super) fn remembered_username(root: &std::path::Path, gateway: &str) -> Option<String> {
    let bytes = std::fs::read(manual_username_path(root, gateway)).ok()?;
    serde_json::from_slice::<String>(&bytes)
        .ok()
        .filter(|name| !name.trim().is_empty())
}
pub(super) fn remember_username(
    root: &std::path::Path,
    gateway: &str,
    username: &str,
) -> Result<(), String> {
    if username.trim().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(root).map_err(|_| "无法创建学校账户记忆目录")?;
    let bytes = serde_json::to_vec(username.trim()).map_err(|_| "学校账户记忆编码失败")?;
    std::fs::write(manual_username_path(root, gateway), bytes)
        .map_err(|_| "无法保存学校账户记忆".into())
}
pub(super) const USERNAME_COOKIE: &str = "__exv_manual_username";
pub(super) fn observed_username(value: &str) -> Option<String> {
    if value.len() > 2048 || value.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<_>> = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect();
    String::from_utf8(bytes?)
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}
/// 只预填学校账户；仅在本次用户点击或提交时观察账户，不读取密码或代替提交。
pub(super) fn username_memory_script(username: Option<&str>) -> String {
    let username = serde_json::to_string(username.unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
    let body = r#"(()=>{if(location.origin!=='https://sso.ecnu.edu.cn'||location.pathname!=='/login')return;
const remembered=__USERNAME__;let attached=false;
const setup=()=>{if(attached)return true;const form=document.querySelector('form#normalLoginForm')||document.querySelector('form[action*="/login"]');
const input=form&&form.querySelector('input[name="username"]');if(!input)return false;attached=true;
if(!input.value&&remembered){input.value=remembered;input.dispatchEvent(new Event('input',{bubbles:true}));input.dispatchEvent(new Event('change',{bubbles:true}));}
const observe=()=>{const value=input.value.trim();if(!value)return;const hex=Array.from(new TextEncoder().encode(value),b=>b.toString(16).padStart(2,'0')).join('');document.cookie='__exv_manual_username='+hex+'; Path=/; Secure; SameSite=Lax';};
form.addEventListener('submit',observe,true);const button=form.querySelector('button#submitBtn');if(button)button.addEventListener('click',observe,true);return true;};
if(!setup()){const observer=new MutationObserver(()=>{if(setup())observer.disconnect();});observer.observe(document,{childList:true,subtree:true});addEventListener('pagehide',()=>observer.disconnect(),{once:true});}})()"#;
    body.replace("__USERNAME__", &username)
}
