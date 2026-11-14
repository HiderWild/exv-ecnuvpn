//! 请求级本地Agent宿主：同轮六监听共享票据消费者，关闭等待整组任务完成。
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use exv_vpn_atrust::Secret;
use exv_vpn_atrust::auth::browser_session::BrowserSession;
use exv_vpn_atrust::auth::client_identity::AuthClientIdentity;
use exv_vpn_atrust::auth::{AuthClient, AuthClientOptions, AuthHttpError};
use exv_vpn_atrust::local_agent::{
    AgentGroupHandle, AgentServer, AgentServerConfig, AgentServerError, AutoLoginTrigger,
    DEFAULT_AGENT_ADDR, DEFAULT_AGENT_PORT, TicketConsumer,
};
use exv_vpn_atrust::transport::Underlay;
use tokio::sync::mpsc;

use crate::DarwinUiConfig;
use crate::agent_certs;
use crate::atrust_http_underlay::HostAuthUnderlay;

/// 票据兑换网关错误码：`75500304 = TICKET_INVALID`（门户常量表，知识库 §票据事实）。
pub const TICKET_INVALID_CODE: i64 = 75_500_304;

/// 兑换链单请求预算（`authConfig` + `sessionIdExchange` 两次串行请求）；
/// 与 win32 agent-sim `CONSUMER_REQUEST_BUDGET` 同值语义。
const EXCHANGE_REQUEST_BUDGET: Duration = Duration::from_secs(8);

/// 日志目标（`log` crate 的 `target:` 字符串字面量形式）。
const LOG_TARGET_S: &str = "exv.local_agent_host";

/// 票据兑换失败分类（脱敏；无票据、无 Cookie、无自由文本原因）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeFailure {
    /// 网关判票据无效（`75500304`；无重取源，等浏览器侧重新登录产生新票）。
    TicketInvalid,
    /// 其余网关业务码拒绝（码原样保留供日志/诊断）。
    Gateway(i64),
    /// 本地侧失败（传输/CSRF/会话形状等固定分类词）。
    Local(&'static str),
}

impl ExchangeFailure {
    /// 服务端日志用的固定分类词（C1 `TicketConsumer` 契约：失败上抛分类词）。
    fn log_class(self) -> &'static str {
        match self {
            Self::TicketInvalid => "ticket_invalid",
            Self::Gateway(_) => "gateway",
            Self::Local(class) => class,
        }
    }
}

/// 票据兑换原语（生产 [`AuthClientExchange`]；测试注入假实现）。
///
/// 语义：`gateway` 为等待方冻结的网关 origin；`user_agent` 为等待方冻结的 UA；
/// 立即执行（不等任何外部应答），成功返回可提交的候选会话。
pub trait GatewayExchange: Send + Sync {
    /// 兑换一张 `sidTicket`（全值，[`Secret`] 承载）。
    fn exchange<'a>(
        &'a self,
        gateway: &'a str,
        user_agent: &'a str,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<BrowserSession, ExchangeFailure>> + Send + 'a>>;

    /// 非秘密账户随同一兑换结果返回；旧兑换端口未提供时保持缺值。
    fn exchange_with_username<'a>(
        &'a self,
        gateway: &'a str,
        user_agent: &'a str,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(BrowserSession, String), ExchangeFailure>> + Send + 'a>> {
        Box::pin(async move { self.exchange(gateway, user_agent, ticket).await.map(|session| (session, String::new())) })
    }
}

/// 生产兑换实现：每张票据新建 `AuthClient`（R2 配方 `r2_ticket_consume.rs` 同链）：
/// `AuthClientIdentity::browser("Mac","zh-CN")` + 冻结 UA → `auth_config(true)` CSRF
/// 自举 → `exchange_sid_ticket` → 会话字段组 `BrowserSession`。
pub struct AuthClientExchange {
    underlay: Arc<dyn Underlay>,
}

impl AuthClientExchange {
    /// 生产构造（`HostAuthUnderlay` 出口）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            underlay: Arc::new(HostAuthUnderlay),
        }
    }
}

impl Default for AuthClientExchange {
    fn default() -> Self {
        Self::new()
    }
}

fn exchange_local_class(error: &AuthHttpError) -> &'static str {
    match error {
        AuthHttpError::InvalidConfiguration => "config",
        AuthHttpError::InvalidCookie => "cookie",
        AuthHttpError::Timeout => "timeout",
        AuthHttpError::Underlay(_) => "underlay",
        AuthHttpError::Tls { .. } => "tls",
        AuthHttpError::Http { .. } => "http",
        AuthHttpError::HttpStatus { .. } => "http_status",
        AuthHttpError::SizeLimit => "size_limit",
        AuthHttpError::Protocol => "protocol",
        AuthHttpError::MissingCsrf => "csrf",
        AuthHttpError::MissingSignKey => "sign_key",
        AuthHttpError::Rejected { .. } => "rejected",
    }
}

fn classify_exchange_error(error: AuthHttpError) -> ExchangeFailure {
    match error {
        AuthHttpError::Rejected {
            code: TICKET_INVALID_CODE,
        } => ExchangeFailure::TicketInvalid,
        AuthHttpError::Rejected { code } => ExchangeFailure::Gateway(code),
        other => ExchangeFailure::Local(exchange_local_class(&other)),
    }
}

impl GatewayExchange for AuthClientExchange {
    fn exchange<'a>(
        &'a self,
        gateway: &'a str,
        user_agent: &'a str,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<BrowserSession, ExchangeFailure>> + Send + 'a>> {
        Box::pin(async move { self.exchange_with_username(gateway, user_agent, ticket).await.map(|(session, _)| session) })
    }

    fn exchange_with_username<'a>(
        &'a self,
        gateway: &'a str,
        user_agent: &'a str,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(BrowserSession, String), ExchangeFailure>> + Send + 'a>> {
        let underlay = Arc::clone(&self.underlay);
        Box::pin(async move {
            let identity = AuthClientIdentity::browser("Mac", "zh-CN")
                .map_err(|_| ExchangeFailure::Local("identity"))?;
            let mut options = AuthClientOptions::new(identity);
            options.timeout = EXCHANGE_REQUEST_BUDGET;
            // UA 对齐（§7.6）：等待方冻结 UA 优先；空则保留官方 aTrust UA 缺省。
            let effective_user_agent = if user_agent.trim().is_empty() {
                options.user_agent.clone()
            } else {
                user_agent.to_owned()
            };
            options.user_agent = effective_user_agent.clone();
            let mut client =
                AuthClient::new(gateway, underlay, options).map_err(classify_exchange_error)?;
            client
                .auth_config(true)
                .await
                .map_err(classify_exchange_error)?;
            let exchange = client
                .exchange_sid_ticket(ticket)
                .await
                .map_err(classify_exchange_error)?;
            eprintln!(
                "[local_agent] exchange 解析结果: sid={} sig={} legacy={} trace={}",
                exchange.sid.is_some(),
                exchange.sid_sig.is_some(),
                exchange.sid_legacy.is_some(),
                exchange.straceid.is_some(),
            );
            eprintln!(
                "[local_agent] COOKIE_JAR_CHECK is_empty={} sid_in_jar={}",
                exchange.is_empty(),
                client.sid().is_some()
            );
            // 网关 sessionIdExchange 以 Set-Cookie 交付会话（body 只含 onlineInfo）；
            // body 解析为空时从 AuthClient cookie jar 导出（sid 在位才返回 Some）。
            let header = if exchange.is_empty() {
                match client.gateway_session_cookie_header() {
                    Some(header) => {
                        eprintln!(
                            "[local_agent] body 解析为空，从 Set-Cookie 导出会话（len={}）",
                            header.len()
                        );
                        header
                    }
                    None => {
                        eprintln!("[local_agent] 兑换后 cookie jar 无 sid，会话建立失败");
                        return Err(ExchangeFailure::Local("empty_session"));
                    }
                }
            } else {
                exchange.cookie_header().to_string()
            };
            BrowserSession::new(gateway.to_owned(), header, effective_user_agent)
                .map(|session| (session, exchange.authenticated_username.unwrap_or_default()))
                .map_err(|_| ExchangeFailure::Local("session_shape"))
        })
    }
}

/// 等待方收到的票据兑换交付。
#[derive(Debug)]
pub enum TicketDelivery {
    /// 兑换响应同时提供了实际账户。
    AuthenticatedSession { session: BrowserSession, username: String },
    /// 兑换成功：可提交的候选会话（网关=等待方冻结网关）。
    Session(BrowserSession),
    /// 兑换失败（分类见 [`ExchangeFailure`]）；等待方决定是否继续等下一张票。
    Failed(ExchangeFailure),
}

/// 注册一次原生系统浏览器等待后取得的交付通道。
pub struct TicketWait {
    rx: mpsc::Receiver<TicketDelivery>,
}

impl TicketWait {
    /// 交付接收端（编排等待循环在 `select!` 中消费；单测直接注入交付）。
    pub fn as_receiver(&mut self) -> &mut mpsc::Receiver<TicketDelivery> {
        &mut self.rx
    }
}

/// 注册中的等待（单槽；新注册整体替换旧等待，旧通道随即关闭）。
struct ActiveWait {
    ticket_consumed: Arc<AtomicBool>,
    gateway: String,
    user_agent: String,
    tx: mpsc::Sender<TicketDelivery>,
}

/// 免密触发回调的运行闸门：Drop 时清除去重标记（回调把闸门移动进异步任务，
/// 任务结束自动归还触发资格）。
pub struct AutoLoginGate {
    inner: Arc<HostInner>,
}

impl Drop for AutoLoginGate {
    fn drop(&mut self) {
        self.inner
            .auto_login_running
            .store(false, Ordering::Release);
    }
}

/// 免密自动登录回调：收到 `service/online` 触发时调用（同步；实现方自行派发异步
/// 任务并把 [`AutoLoginGate`] 移动进任务持有到结束）。
pub type AutoLoginCallback = Arc<dyn Fn(AutoLoginGate) + Send + Sync>;

/// 免密通道的保存凭据源（C5）：返回 EXV 凭据存储中**有效凭据对**（用户名 + 已保存
/// 密码都在位）的用户名；无有效对（未保存/未记住密码/存储不可读）一律 `None`。
///
/// 同一来源服务两个职责：
/// * 启动快照注入 init 应答 `data.username`（门户渲染 psw_free 一键登录视图的条件）；
/// * `service/online` 触发的凭据在位门（`None` → 静默忽略，免密失败用户手动登录）。
///
/// 生产实现见 [`saved_school_username`]（读 `DarwinUiConfig` 凭据对）；测试注入假源。
pub type SavedCredentialSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

struct HostInner {
    wait: Mutex<Option<ActiveWait>>,
    auto_login_running: AtomicBool,
    auto_login: AutoLoginCallback,
    credentials: SavedCredentialSource,
    exchange: Arc<dyn GatewayExchange>,
}

/// 常驻本地 Agent 句柄（应用运行期持有；克隆只共享内部状态）。
#[derive(Clone)]
pub struct NativeAgentHandle {
    inner: Arc<HostInner>,
    server: Arc<OnceServer>,
}

/// 服务端停机只执行一次（`AgentHandle::shutdown` 消费自身）。
struct OnceServer {
    handle: Mutex<Option<AgentGroupHandle>>,
    closed: tokio::sync::watch::Sender<
        Option<Result<(), exv_vpn_atrust::local_agent::AgentShutdownError>>,
    >,
    port: u16,
}

impl NativeAgentHandle {
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.server
            .handle
            .lock()
            .expect("native agent server")
            .is_some()
    }

    /// 实际监听端口。
    #[must_use]
    pub fn port(&self) -> u16 {
        self.server.port
    }

    /// 注册一次原生系统浏览器票据等待（单槽：新等待整体替换旧等待）。
    ///
    /// 返回交付通道；编排方在等待结束（成功/取消/超时）后直接丢弃即可，下一次
    /// 注册天然清理旧槽。`gateway`/`user_agent` 为本次连接冻结值——兑换以冻结
    /// 网关为目标，UA 按 §7.6 对齐配方冻结到兑换客户端。
    ///
    /// # Panics
    /// 内部状态锁被毒化时 panic（锁内临界区不 panic，实际不可达）。
    #[must_use]
    pub fn register_ticket_wait(&self, gateway: String, user_agent: String) -> TicketWait {
        let (tx, rx) = mpsc::channel(1);
        *self.inner.wait.lock().expect("native agent wait") = Some(ActiveWait {
            ticket_consumed: Arc::new(AtomicBool::new(false)),
            gateway,
            user_agent,
            tx,
        });
        TicketWait { rx }
    }

    /// 运行时切换 init 应答的 `data.status`（C5：免密登录受理成功后切
    /// `Some("online")`；`None` 恢复缺省 `firstLogin`）。服务端已停机后为 no-op。
    ///
    /// # Panics
    /// 内部状态锁被毒化时 panic（锁内临界区不 panic，实际不可达）。
    pub fn set_init_status(&self, status: Option<String>) {
        let guard = self.server.handle.lock().expect("native agent server");
        if let Some(server) = guard.as_ref() {
            server.set_init_status(status);
        }
    }

    /// 优雅停机（应用退出时调用；释放监听端口；重复调用幂等）。
    ///
    /// # Panics
    /// 内部状态锁被毒化时 panic（锁内临界区不 panic，实际不可达）。
    pub async fn shutdown(&self) {
        let _ = self
            .shutdown_until(std::time::Instant::now() + Duration::from_secs(5))
            .await;
    }

    /// 整组幂等关闭；第二调用者等待第一调用者真正完成join。
    pub async fn shutdown_until(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), exv_vpn_atrust::local_agent::AgentShutdownError> {
        let mut closed = self.server.closed.subscribe();
        let group = self
            .server
            .handle
            .lock()
            .expect("native agent server")
            .take();
        if let Some(group) = group {
            let server = self.server.clone();
            tokio::spawn(async move {
                log::info!(target: LOG_TARGET_S, "agent.closing");
                let result = group.shutdown_until(deadline).await;
                match &result {
                    Ok(()) => log::info!(target: LOG_TARGET_S, "agent.closed"),
                    Err(error) => {
                        log::error!(target: LOG_TARGET_S, "agent.close_failed error={error}")
                    }
                }
                server.closed.send_replace(Some(result));
            });
        }
        loop {
            if let Some(result) = closed.borrow_and_update().clone() {
                return result;
            }
            // self.server 持有 sender，worker 完成前通道持续存在。
            closed
                .changed()
                .await
                .expect("native agent shutdown completion");
        }
    }
}

impl std::fmt::Debug for NativeAgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeAgentHandle")
            .field("port", &self.server.port)
            .finish_non_exhaustive()
    }
}

/// 本次请求宿主启动错误（原样上抛给宿主呈现给用户；不含敏感材料）。
#[derive(Debug)]
pub enum NativeAgentStartError {
    /// C2 证书管理失败（生成/落盘/钥匙串）。
    Cert(crate::agent_certs::AgentCertError),
    /// C1 服务端启动失败（端口被占/TLS 材料）。
    Server(AgentServerError),
}

impl std::fmt::Display for NativeAgentStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cert(error) => write!(f, "本地登录接收服务证书未就绪：{error}"),
            Self::Server(error) => write!(f, "本地登录接收服务启动失败：{error}"),
        }
    }
}

impl std::error::Error for NativeAgentStartError {}

/// 宿主配置（生产经 [`start_prepared`] 组装；测试直接构造以注入假兑换/临时端口）。
pub struct NativeAgentConfig {
    /// TLS 叶子证书 PEM 路径。
    pub cert_path: PathBuf,
    /// TLS 叶子私钥 PEM 路径。
    pub key_path: PathBuf,
    /// 监听端口（`0` = 系统分配临时端口，供测试）。
    pub port: u16,
    /// `/v1/service/online` 免密触发回调（连接级去重与凭据在位门由宿主保证）。
    pub auto_login: AutoLoginCallback,
    /// 保存凭据源（C5：init username 快照 + online 触发凭据在位门）。
    pub credentials: SavedCredentialSource,
    /// 票据兑换原语（生产 [`AuthClientExchange`]）。
    pub exchange: Arc<dyn GatewayExchange>,
}

/// 启动常驻本地 Agent（服务端 + 消费/触发接线）。
///
/// 启动时经 `credentials` 快照一次有效凭据对用户名注入 init 应答（快照后不随存储
/// 变化自动刷新；凭据在位门在每次 online 触发时实时重查）。
///
/// # Errors
/// 端口绑定失败（如官方客户端占用 `{DEFAULT_AGENT_PORT}`）或 TLS 材料不可用时返回
/// [`NativeAgentStartError`]，宿主原样呈现给用户。
pub async fn start_agent(
    config: NativeAgentConfig,
) -> Result<NativeAgentHandle, NativeAgentStartError> {
    start_agent_for_gateway(config, exv_vpn_atrust::local_agent::DEFAULT_GATEWAY).await
}

async fn start_agent_for_gateway(
    config: NativeAgentConfig,
    gateway: &str,
) -> Result<NativeAgentHandle, NativeAgentStartError> {
    let init_username = (config.credentials)();
    if init_username.is_some() {
        log::info!(
            target: LOG_TARGET_S,
            "免密通道就绪：保存凭据在位，init 应答携带 username（门户渲染一键登录）"
        );
    } else {
        log::info!(
            target: LOG_TARGET_S,
            "免密通道未就绪：无有效凭据对，init 应答不带 username（psw_free 不渲染）"
        );
    }
    let inner = Arc::new(HostInner {
        wait: Mutex::new(None),
        auto_login_running: AtomicBool::new(false),
        auto_login: config.auto_login,
        credentials: config.credentials,
        exchange: config.exchange,
    });
    let consumer: Arc<dyn TicketConsumer> = Arc::new(HostTicketConsumer {
        inner: inner.clone(),
    });
    let auto_login: Arc<dyn AutoLoginTrigger> = Arc::new(HostAutoLoginTrigger {
        inner: inner.clone(),
    });
    let ports = if config.port == DEFAULT_AGENT_PORT {
        vec![54630, 54641, 54652, 54631, 54642, 54653]
    } else {
        vec![config.port]
    };
    let configs = ports
        .into_iter()
        .map(|port| AgentServerConfig {
            bind: DEFAULT_AGENT_ADDR,
            port,
            cert_path: config.cert_path.clone(),
            key_path: config.key_path.clone(),
            gateway: gateway.to_owned(),
            ticket_consumer: consumer.clone(),
            auto_login: auto_login.clone(),
            init_username: init_username.clone(),
            init_status: None,
        })
        .collect();
    let server = AgentServer::start_group(configs).await.map_err(|error| {
        log::error!(target: LOG_TARGET_S, "agent.start_failed address={:?} os_code={:?} error={error}", error.bind_address(), error.bind_error().and_then(std::io::Error::raw_os_error));
        NativeAgentStartError::Server(error)
    })?;
    let port = server.ports()[0];
    log::info!(target: LOG_TARGET_S, "agent.listening ports={:?}", server.ports());
    Ok(NativeAgentHandle {
        inner,
        server: Arc::new(OnceServer {
            handle: Mutex::new(Some(server)),
            closed: tokio::sync::watch::channel(None).0,
            port,
        }),
    })
}

/// 生产保存凭据源：EXV 凭据存储（默认配置目录）中有效凭据对的用户名。
///
/// 无有效对（未保存用户名/密码、未记住密码或存储不可读/密钥损坏）一律 `None`——
/// 存储错误与「未保存」同视（免密通道静默不渲染；真实错误由手动登录/凭据窗口路径
/// 呈现给用户）。
fn saved_school_username() -> Option<String> {
    let dir = DarwinUiConfig::default().config_dir().to_path_buf();
    let config = DarwinUiConfig::load_from_dir(&dir).ok()?;
    crate::atrust_auth_storage::school_credentials(&config, None)
        .ok()
        .flatten()
        .map(|pair| pair.username.as_str().to_owned())
}

/// 使用本次已确认的TLS材料；该入口不安装信任。
pub async fn start_prepared(
    prepared: agent_certs::PreparedAgentTls,
    gateway: &str,
    auto_login: AutoLoginCallback,
) -> Result<NativeAgentHandle, NativeAgentStartError> {
    start_agent_for_gateway(
        NativeAgentConfig {
            cert_path: prepared.cert_path,
            key_path: prepared.key_path,
            port: DEFAULT_AGENT_PORT,
            auto_login,
            credentials: Arc::new(saved_school_username),
            exchange: Arc::new(AuthClientExchange::new()),
        },
        gateway,
    )
    .await
}

/// `/v1/service/start` 消费者：票据到达即兑换，交付到注册等待。
struct HostTicketConsumer {
    inner: Arc<HostInner>,
}

impl TicketConsumer for HostTicketConsumer {
    fn consume_ticket(
        &self,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send + '_>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            // 快照等待槽（不摘除）：兑换失败时等待方继续等下一张票——浏览器内
            // 重新登录会产生新票（每次登录新票，JSONL 铁证）。
            let wait = {
                let guard = inner.wait.lock().expect("native agent wait");
                guard.as_ref().map(|wait| {
                    (
                        wait.gateway.clone(),
                        wait.user_agent.clone(),
                        wait.tx.clone(),
                        wait.ticket_consumed.clone(),
                    )
                })
            };
            let Some((gateway, user_agent, tx, ticket_consumed)) = wait else {
                log::info!(
                    target: LOG_TARGET_S,
                    "票据到达但当前无等待中的原生系统浏览器认证，不兑换（len={}；票据只消费一次，等待浏览器内下一次登录）",
                    ticket.len()
                );
                return Ok(());
            };
            // 单票闸门：本轮等待已在兑换/已兑换时忽略后续票据（门户双发 300ms 铁证，
            // 并发第二张兑换会踢掉第一会话——2026-10-29 E2E 铁证）。
            if ticket_consumed.swap(true, Ordering::AcqRel) {
                log::info!(
                    target: LOG_TARGET_S,
                    "二次票据到达，本轮等待已消费一张，忽略（len={}；单会话保护，等待浏览器内下一次登录）",
                    ticket.len()
                );
                eprintln!(
                    "[local_agent] 二次票据到达，本轮已消费一张，忽略（len={}）",
                    ticket.len()
                );
                return Ok(());
            }
            log::info!(
                target: LOG_TARGET_S,
                "票据到达，立即发起 sessionIdExchange 兑换（与浏览器 authCheck 竞速；len={} gateway={}）",
                ticket.len(),
                gateway
            );
            eprintln!(
                "[local_agent] 开始兑换票据 gateway={gateway} ua_len={}",
                user_agent.len()
            );
            let outcome = inner.exchange.exchange_with_username(&gateway, &user_agent, ticket).await;
            match &outcome {
                Ok(_) => eprintln!("[local_agent] 票据兑换成功"),
                Err(failure) => eprintln!("[local_agent] 票据兑换失败: {failure:?}"),
            }
            let failure = match &outcome {
                Ok(_) => {
                    log::info!(target: LOG_TARGET_S, "票据兑换成功 gateway={gateway}");
                    None
                }
                Err(ExchangeFailure::TicketInvalid) => {
                    log::warn!(
                        target: LOG_TARGET_S,
                        "票据被网关判失效（code={TICKET_INVALID_CODE}；浏览器侧需重新登录产生新票）"
                    );
                    Some(ExchangeFailure::TicketInvalid)
                }
                Err(ExchangeFailure::Gateway(code)) => {
                    log::warn!(target: LOG_TARGET_S, "票据兑换被网关拒绝 code={code}");
                    Some(ExchangeFailure::Gateway(*code))
                }
                Err(ExchangeFailure::Local(class)) => {
                    log::warn!(target: LOG_TARGET_S, "票据兑换本地失败 class={class}");
                    Some(ExchangeFailure::Local(class))
                }
            };
            if failure.is_some() {
                // 兑换失败归还消费资格：浏览器内重新登录产生的新票（每次登录新票）
                // 到达时可立即再兑换；成功则保持占用至新等待注册。
                ticket_consumed.store(false, Ordering::Release);
            }
            let delivery = match outcome {
                Ok((session, username)) if !username.is_empty() => TicketDelivery::AuthenticatedSession { session, username },
                Ok((session, _)) => TicketDelivery::Session(session),
                Err(failure) => TicketDelivery::Failed(failure),
            };
            if tx.send(delivery).await.is_err() {
                // 等待方已离开（取消/超时/被新等待替换）：交付没有接收方，如实记录。
                log::info!(target: LOG_TARGET_S, "兑换交付无接收方（等待已结束或被替换）");
            }
            match failure {
                Some(failure) => Err(failure.log_class()),
                None => Ok(()),
            }
        })
    }
}

/// `/v1/service/online` 触发器：凭据在位门 → 连接级去重 → 调用注入回调。
///
/// 凭据门在去重交换**之前**检查：凭据不完整时静默忽略（既不占闸门也不派发登录，
/// 免密失败用户手动登录——psw_free 视图本就不该在无凭据时渲染）。
struct HostAutoLoginTrigger {
    inner: Arc<HostInner>,
}

impl AutoLoginTrigger for HostAutoLoginTrigger {
    fn trigger_auto_login(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            if (inner.credentials)().is_none() {
                log::info!(
                    target: LOG_TARGET_S,
                    "service/online 触达，但保存凭据不完整，免密登录静默忽略（请在门户或 EXV 手动登录）"
                );
                return;
            }
            if inner.auto_login_running.swap(true, Ordering::AcqRel) {
                log::info!(
                    target: LOG_TARGET_S,
                    "service/online 触达，但一次免密登录已在进行，本次触发不重复执行"
                );
                return;
            }
            log::info!(target: LOG_TARGET_S, "service/online 触达，派发免密自动登录");
            let gate = AutoLoginGate {
                inner: Arc::clone(&inner),
            };
            (inner.auto_login)(gate);
        })
    }
}
