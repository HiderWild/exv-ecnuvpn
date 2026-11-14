
pub use exv_vpn_atrust::auth::browser_auth_policy::{AuthProgress, AuthRound, StartupFacts};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use exv_vpn_atrust::auth::browser_session::BrowserSession;
use zeroize::Zeroizing;

mod http_probe;
mod orchestrator;
pub mod page_facts;
mod policy;
pub use http_probe::HttpProbeAdapter;
pub(crate) use http_probe::probe_status;
pub use orchestrator::*;
pub use policy::PagePolicy;

/// 本次连接冻结的认证意图；渠道选择不隐式授予历史会话复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthIntent {
    Automatic,
    FreshManual,
}
impl AuthIntent {
    pub fn from_auto_login(automatic: bool) -> Self {
        if automatic { Self::Automatic } else { Self::FreshManual }
    }
    pub fn allows_reuse(self) -> bool { self == Self::Automatic }
}

/// 可由编排器动态持有的异步端口；借用结束前完成，丢弃 future 表示取消本次操作。
pub type ChannelFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AuthError>> + Send + 'a>>;

/// 自动填交权限；CDP 的技术可达性不授予系统浏览器填交权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionCapability {
    Unavailable,
    SchoolFormOnce,
}

/// 页面事实采集的实际保真度；不作为未知页面的允许白名单。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationFidelity {
    NoPage,
    CookieSideChannel,
    EvaluatedDom,
}

/// 窗口可见性能力。当前显示或隐藏状态由呈现策略决定，不能据此命名通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityCapability {
    NoSurface,
    Controlled,
    AlwaysVisible,
}

/// 会话探针可达范围；跨 origin 不可采样不能归一为未登录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeScope {
    GatewayPageOnly,
    IndependentHttp,
}

/// 能力协商只描述机制与授权；页面、凭据和会话结果另行观测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelCapabilities {
    pub injection: InjectionCapability,
    pub can_render: bool,
    pub visibility: VisibilityCapability,
    pub can_read_cookiejar: bool,
    pub can_write_cookiejar: bool,
    pub observation: ObservationFidelity,
    pub probe_scope: ProbeScope,
}

/// 一项局部事实的可用性；未采到或机制不可达都不表示失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation<T> {
    Observed(T),
    NotYetObserved,
    Unavailable,
}

/// DOM 原始事实。适配器只采集，分类策略及交接页 latch 由编排器统一消费。
/// 文本是受限摘录，沿用现有 60 字符净化规则；不得写入普通日志。
#[derive(Clone, PartialEq, Eq)]
pub struct DomFacts {
    /// 同次学校页面实际可见的验证码；不包含字段值。
    pub challenge_visible: bool,
    pub client_install_marker: bool,
    pub shortcut_page: bool,
    pub download_page: bool,
    pub body_has_failure_phrase: bool,
    pub body_char_count: usize,
    pub error_container_text: Option<String>,
    pub short_body_text: Option<String>,
}

/// 一次页面采样。URL 可能含票据，故不提供 Debug；不是错误、分类或里程碑。
/// URL 和 DOM 不保证原子一致，适配器报告采样序号，编排器不得把旧 DOM 绑定新 URL。
#[derive(Clone, PartialEq, Eq)]
pub struct PageObservation {
    pub sequence: u64,
    pub url: Observation<String>,
    pub origin: Observation<String>,
    pub dom: Observation<DomFacts>,
    /// URL 与 DOM 同次采样时为 true；否则仅使用可独立判定的事实。
    pub coherent: bool,
    /// 无诊断回包时为 None；有值时已经核对文档代次。
    pub startup: Option<exv_vpn_atrust::auth::browser_auth_policy::StartupFacts>,
}

/// authConfig 事实。明确 code=0 且 isLogin=0 才产生 LoggedOut；探针失败返回 AuthError。
#[derive(Clone, PartialEq, Eq)]
pub enum SessionStatus {
    LoggedIn,
    LoggedOut { first_auth: Option<String> },
    NotYetObserved,
    Unavailable,
}

/// 非秘密来源标签；SessionArtifact 的来源不影响上层连接语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSource {
    BrowserCookieJar,
    AgentSnapshot,
    GatewayCache,
    HttpCasReuse,
}

/// 经过同源和非空 sid 校验的既有 v2 会话；不克隆、不派生 Debug、不改变 wire 格式。
/// 提取成功不等于隧道连接成功；编码、缓存和交接由现有 BrowserSession 契约约束。
pub struct SessionArtifact {
    pub session: BrowserSession,
    pub source: SessionSource,
}

/// 一次性交付的凭据。编排器跨回退持有唯一预算，批准后按值交给可注入适配器。
/// 适配器不得保存、克隆或记录材料；操作返回或取消时零化，不交给 HTTP/系统浏览器。
pub struct CredentialDelivery {
    pub username: Zeroizing<String>,
    pub password: Zeroizing<String>,
    /// 已验证的当前网关 origin；用于既有学校表单 service 匹配。
    pub gateway_origin: String,
}

impl CredentialDelivery {
    /// WebView 按次生成并零化脚本；复用既有学校表单脚本与状态 cookie 协议。
    pub fn webview_fill_script(&self) -> Zeroizing<String> {
        // 参数编码及最终脚本都随本次交付释放零化；不生成持久材料副本。
        let username = Zeroizing::new(
            serde_json::to_string(self.username.as_str()).expect("username serializes"),
        );
        let password = Zeroizing::new(
            serde_json::to_string(self.password.as_str()).expect("password serializes"),
        );
        Zeroizing::new(format!(
            "({})({},{},{}).then((status)=>{{const hex=Array.from(new TextEncoder().encode(status)).map((b)=>b.toString(16).padStart(2,'0')).join('');document.cookie='{}='+hex+'; Max-Age=60; Path=/; Secure; SameSite=Lax';}}).catch(()=>{{document.cookie='{}='+'6572726f72; Max-Age=60; Path=/; Secure; SameSite=Lax';}});void 0",
            include_str!("school_login_fill.js"),
            username.as_str(),
            password.as_str(),
            serde_json::to_string(&self.gateway_origin).expect("origin serializes"),
            crate::school_auto_login::RESULT_COOKIE,
            crate::school_auto_login::RESULT_COOKIE,
        ))
    }
}

/// 填交结果是事实；NotReady 不消耗提交预算，Submitted 和 ChallengeVisible 结束尝试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionOutcome {
    NotReady,
    Submitted,
    ChallengeVisible,
    ServiceMismatch,
    Failed,
}

/// HTTP 复用失败后的局部回退信号，不是用户认证失败；零凭据、零学校表单 POST。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractiveReason {
    JarMissing,
    JarInsufficient,
    JarReadFailed,
    FormRequired,
    NetworkFailed,
    SessionMissing,
    SessionBuildFailed,
    BudgetExpired,
    CasConfigurationInvalid,
}

/// HTTP 复用原语的结果。编排器决定静默转隐藏内嵌窗口，适配器不能自己打开窗口。
pub enum ReuseOutcome {
    Ready(SessionArtifact),
    Interactive(InteractiveReason),
}

/// 与现有 UI 五阶段同义；由编排器发出，不由适配器推断用户已完成手动认证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMilestone {
    Opening,
    SchoolLogin,
    Submitting,
    GatewayConfirming,
    AwaitingManual,
}

/// 由本次缓存读取和网关探测事实决定的重取原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketRefreshReason {
    Missing,
    Expired,
}
impl TicketRefreshReason {
    pub const fn as_str(self) -> &'static str {
        match self { Self::Missing => "missing", Self::Expired => "expired" }
    }
}

/// 用户控制信号与真实错误分开；转人工和渠道回退不是错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthControl {
    Cancel,
    ShowForManual,
    UseSystemBrowser,
}

/// 编排器的呈现意图；对 AlwaysVisible 请求 Hidden 时不得伪称已隐藏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationIntent {
    Hidden,
    Visible,
}

/// 呈现结果用于能力协商后的事实记录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationOutcome {
    Hidden,
    Visible,
    AlwaysVisible,
    NoSurface,
}

/// 单词汇表。前四项沿用现有 browser_auth_* 用户码；其余为新层内部稳定分类。
/// 每项只响应已经观察到的事实，未采到/未知页面不能自动生成错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthErrorCode {
    /// 明确安装/启动页，且分类策略未被交接 latch 抑制。
    ClientInstallRequired,
    /// 非空 errorData 或命中已知短失败正文；authConfig JSON 页排除。
    FailedPage,
    /// 已开始认证、无 latch 冻结且连续墙钟无进展达到停滞预算。
    Stalled,
    /// 网关结果字段明确显示限流或并发登录冲突。
    GatewayThrottled,
    /// 本次获取会话的总预算耗尽；与局部停滞分别记录。
    AcquisitionTimeout,
    /// 适配器建窗/启动浏览器/创建受控标签页实际失败。
    ChannelOpenFailed,
    /// 浏览器窗口/调试连接已关闭或命令传输失败。
    ChannelTransportFailed,
    /// 实际采样回包无法解析；不能转为 LoggedOut。
    ObservationMalformed,
    /// HTTP authConfig 调用失败；保留局部失败，不把它解释成未登录。
    SessionProbeFailed,
    /// 网关已确认登录，全部本次适用来源仍缺少有效 sid。
    SessionMissing,
    /// 缓存读取/解密/写入实际失败。
    SessionStorageFailed,
    /// 会话同源、sid 或既有 v2 载荷验证失败。
    SessionInvalid,
    /// firstAuth 缺失或不是有效本站入口。
    LoginEntryInvalid,
    /// 已调用的填交操作失败；挑战与未就绪由 InjectionOutcome 表达。
    InjectionFailed,
    /// 当前自动填交来源缺少材料，沿用既有凭据入口。
    CredentialRequired,
    /// 调用了能力协商明确不支持的操作，属于调用方错误。
    UnsupportedOperation,
    /// 真实发生但尚无法归入以上条件的失败，不扩张为未知状态门禁。
    Unclassified,
}

/// 错误发生的局部步骤；用于描述请求经过何层、执行何操作及业务影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOperation {
    Open,
    Navigate,
    Observe,
    Probe,
    Reuse,
    Inject,
    Extract,
    Present,
    Wait,
    Close,
}

/// 错误只携带分类、步骤、原生数字码与经过净化的短服务端摘录。
/// 摘录不派生 Debug；兼容文案只在最终边界转换，不复制散落分类器。
pub struct AuthError {
    pub code: AuthErrorCode,
    pub operation: AuthOperation,
    pub native_code: Option<i64>,
    pub server_detail: Option<String>,
    pub failure_kind: Option<exv_vpn_atrust::auth::browser_auth_policy::AuthFailureKind>,
}

impl AuthErrorCode {
    pub fn machine_code(self) -> &'static str {
        match self {
            Self::ClientInstallRequired => "browser_auth_client_install_required",
            Self::FailedPage => "browser_auth_failed_page",
            Self::Stalled => "browser_auth_stalled",
            Self::GatewayThrottled => "browser_auth_gateway_throttled",
            Self::AcquisitionTimeout => "browser_auth_acquisition_timeout",
            Self::ChannelOpenFailed => "browser_auth_channel_open_failed",
            Self::ChannelTransportFailed => "browser_auth_channel_transport_failed",
            Self::ObservationMalformed => "browser_auth_observation_malformed",
            Self::SessionProbeFailed => "browser_auth_session_probe_failed",
            Self::SessionMissing => "browser_auth_session_missing",
            Self::SessionStorageFailed => "browser_auth_session_storage_failed",
            Self::SessionInvalid => "browser_auth_session_invalid",
            Self::LoginEntryInvalid => "browser_auth_login_entry_invalid",
            Self::CredentialRequired => "auto_login_no_credentials",
            Self::InjectionFailed => "browser_auth_injection_failed",
            Self::UnsupportedOperation => "browser_auth_unsupported_operation",
            Self::Unclassified => "browser_auth_unclassified",
        }
    }
}

impl AuthError {
    /// 唯一旧串桥接；上层仍可识别既有四码和 ASCII 分隔符。
    pub fn legacy_message(&self) -> String {
        let mut text = self.code.machine_code().to_owned();
        if let Some(detail) = self.clean_detail() {
            text.push('\u{1f}');
            text.push_str(&detail);
        }
        text
    }
    /// owner 的终结汇总直接使用原分类；局部事实使用 diagnostic_log_line。
    pub fn log_line(&self) -> String {
        self.role_log_line("auth.failed")
    }
    pub fn diagnostic_log_line(&self) -> String {
        self.role_log_line("auth.diagnostic")
    }
    fn role_log_line(&self, role: &str) -> String {
        let mut text = format!(
            "{role} code={} operation={:?}",
            self.code.machine_code(),
            self.operation
        );
        if let Some(kind) = self.failure_kind {
            text.push_str(&format!(" failure_kind={kind:?}"));
        }
        if let Some(code) = self.native_code {
            text.push_str(&format!(" native_code={code}"));
        }
        if let Some(detail) = self.clean_detail() {
            text.push_str(" server_detail=");
            text.push_str(&detail);
        }
        text
    }
    fn clean_detail(&self) -> Option<String> {
        self.server_detail
            .as_deref()
            .map(page_facts::sanitize_server_detail)
            .filter(|s| !s.is_empty())
    }
    /// 宿主及 Tauri 渠道共用的分类错误构造入口。
    pub fn at(code: AuthErrorCode, operation: AuthOperation) -> Self {
        Self {
            code,
            operation,
            failure_kind: None,
            native_code: None,
            server_detail: None,
        }
    }
}

/// 渠道打开所需的非凭据配置；URL/UA 不进入普通日志。
pub struct ChannelRequest {
    pub gateway_origin: String,
    pub user_agent: String,
    pub initial_presentation: PresentationIntent,
}

/// open 实际启动的首次导航；避免覆盖冷缓存直达学校入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InitialNavigation {
    #[default]
    Pending,
    ProbeStarted,
    LoginStarted,
}

/// 统一适配器的小原语端口；不得把旧 acquire/run_target_auth 循环包进此 trait。
/// 编排器独占里程碑、分类表、latch、一次预算、取消和回退；适配器仅执行本次操作。
/// 未来实现需要在取消 future 后收束已创建资源，close 幂等且不取消其他 owner。
/// 系统浏览器 observe 可以使用 CDP，但 inject_once 永远 UnsupportedOperation。
pub trait ChannelTrait: Send {
    fn ticket_refresh_reason(&self) -> Option<TicketRefreshReason> { None }
    fn capabilities(&self) -> ChannelCapabilities;
    fn open(&mut self, request: ChannelRequest) -> ChannelFuture<'_, ()>;
    fn initial_navigation(&self) -> InitialNavigation {
        InitialNavigation::Pending
    }
    fn navigate(&mut self, url: String) -> ChannelFuture<'_, ()>;
    fn observe(&mut self) -> ChannelFuture<'_, PageObservation>;
    fn probe_session(&mut self) -> ChannelFuture<'_, SessionStatus>;
    fn reuse_session(&mut self, budget: Duration) -> ChannelFuture<'_, ReuseOutcome>;
    fn inject_once(
        &mut self,
        credentials: CredentialDelivery,
    ) -> ChannelFuture<'_, InjectionOutcome>;
    /// None 表示本通道当前无有效 sid，编排器继续同源快照等来源；不是终端 SessionMissing。
    fn extract_session(&mut self) -> ChannelFuture<'_, Option<SessionArtifact>>;
    /// 所有来源验证完成后的附加宿主收尾；成功标题、会话缓存和 SSO 导出只执行一次。
    /// 仍通过编排器的可取消 operation；不得在提取失败或预取消时调用。
    fn complete_session<'a>(&'a mut self, _session: &'a SessionArtifact) -> ChannelFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn present(&mut self, intent: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome>;
    /// 页面事件或兜底节拍到达即返回；max_wait 有界，取消由编排器 select/drop。
    fn wait_for_change(&mut self, max_wait: Duration) -> ChannelFuture<'_, ()>;
    fn close(&mut self) -> ChannelFuture<'_, ()>;
}

/// AppHandle 相关注入面。编排器统一发阶段及每 acquire 至多一次表单计数。
pub trait AuthEventSink: Send + Sync {
    fn ticket_refresh(&self, _reason: TicketRefreshReason) {}
    fn milestone(&self, milestone: AuthMilestone);
    fn progress(&self, _progress: AuthProgress) {}
    /// 每轮原始失败，供宿主持久诊断；呈现摘要与恢复策略保持各自流程。
    fn round_failure(&self, _round: AuthRound, _error: &AuthError) {}
    fn form_login_reached(&self);
    /// 编排器局部事实与清理失败走诊断；终结汇总由外层 owner 负责。
    fn failure(&self, error: &AuthError) {
        log::warn!(target: "exv.atrust_auth", "{}", error.diagnostic_log_line());
    }
}

/// agent 仿真快照的小端口。提供方先校验所选网关同源，再返回可交接 v2 会话。
/// None 是当前无快照，编排器继续其他来源；该端口不规定 AppHandle 或跨宿主机制。
pub trait SessionSnapshotProvider: Send + Sync {
    fn snapshot<'a>(
        &'a self,
        gateway_origin: &'a str,
    ) -> ChannelFuture<'a, Option<SessionArtifact>>;
}

