//! 系统浏览器每次请求独占六端口Agent；启动应用只登记空状态。
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use exv_vpn_atrust::Secret;
use exv_vpn_atrust::local_agent::{
    AgentGroupHandle, AgentServer, AgentServerConfig, AutoLoginTrigger, TicketConsumer,
};
use exv_vpn_win32_agent_sim::cert;
pub use exv_vpn_win32_agent_sim::cert::{
    CertificateError, CertificatePreparation, PreparedAgentTls,
};
use exv_vpn_win32_agent_sim::consume::{
    ConsumeError, GatewayConsumer, GatewayTicketConsumer, SessionSnapshot, SessionStore,
};
use exv_vpn_win32_agent_sim::trust::{CertutilTrustStore, SystemCommandRunner};
use tauri::{AppHandle, Manager};
use tokio::sync::watch;

/// 日志目标（延续 agent-sim 既有可检索前缀，便于跨版本日志归并）。
const LOG_TARGET: &str = "exv.local_agent";

/// 首次把 EXV 根 CA 装入当前用户 Root 存储时，Windows 会弹出系统「安全警告」
/// 对话框（根证书保护机制，需用户点一次「是」）。本常量是该场景的产品说明文案
/// （win32 侧常量；统一表述由 darwin C3 定义，C6 汇总核对）。
pub const CA_INSTALL_DIALOG_COPY: &str = "首次使用系统浏览器登录时，Windows 会弹出安全警告：EXV 需要把本地登录服务的证书加入当前用户的受信任根证书，请点击「是」继续。";

/// 网关拒绝码 `75500304`（`TICKET_INVALID`，门户常量表）：等待超时分类为
/// 「登录票据已失效」的判据（知识库 §票据事实；R2 两次独立兑换均此码）。
pub const TICKET_INVALID_CODE: i64 = 75500304;

/// 空状态只追踪当前请求的关闭句柄；票据结果每轮重新分配。
#[derive(Default)]
pub struct LocalAgentState {
    server: Mutex<std::sync::Weak<RequestAgent>>,
    exit_deadline: Arc<Mutex<Option<std::time::Instant>>>,
    exchange: Mutex<Arc<TicketExchange>>,
}
impl LocalAgentState {
    async fn start_request(
        &self,
        generation: u64,
        start: impl Future<Output = Result<AgentGroupHandle, AgentStartFailure>>,
    ) -> Result<Arc<RequestAgent>, AgentStartFailure> {
        let previous = self.server.lock().expect("agent server").upgrade();
        if let Some(previous) = previous {
            let deadline = self
                .exit_deadline
                .lock()
                .expect("agent exit deadline")
                .unwrap_or_else(|| std::time::Instant::now() + std::time::Duration::from_secs(5));
            // 旧轮结果已由关闭worker记录；返回时其全部任务已join，再创建新轮。
            let _ = previous.finish(deadline).await;
        }
        if self
            .exit_deadline
            .lock()
            .expect("agent exit deadline")
            .is_some()
        {
            return Err(AgentStartFailure {
                message: "应用正在退出，本次登录已取消".into(),
                native_code: None,
            });
        }
        let group = start.await?;
        let owner = Arc::new(RequestAgent {
            exit_deadline: self.exit_deadline.clone(),
            group: Mutex::new(Some(group)),
            closed: watch::channel(None).0,
            generation,
        });
        *self.server.lock().expect("agent server") = Arc::downgrade(&owner);
        Ok(owner)
    }

    fn new_exchange(&self, gateway: &str) -> Arc<TicketExchange> {
        let exchange = Arc::new(TicketExchange::new());
        exchange.set_gateway(gateway);
        *self.exchange.lock().expect("agent exchange") = exchange.clone();
        exchange
    }
}

const PORTAL_PORTS: [u16; 6] = [54630, 54641, 54652, 54631, 54642, 54653];
struct RequestAgent {
    exit_deadline: Arc<Mutex<Option<std::time::Instant>>>,
    group: Mutex<Option<AgentGroupHandle>>,
    closed: watch::Sender<Option<Result<(), String>>>,
    generation: u64,
}
impl RequestAgent {
    async fn finish(self: &Arc<Self>, deadline: std::time::Instant) -> Result<(), String> {
        let mut closed = self.closed.subscribe();
        // 同步移交所有权后才 await；取消任何等待者不会销毁整组关闭任务。
        let group = self.group.lock().expect("agent group").take();
        if let Some(group) = group {
            let deadline = self
                .exit_deadline
                .lock()
                .expect("agent exit deadline")
                .map_or(deadline, |outer| outer.min(deadline));
            let owner = self.clone();
            tokio::spawn(async move {
                log::info!(target: LOG_TARGET, "agent.closing generation={}", owner.generation);
                let result = group
                    .shutdown_until(deadline)
                    .await
                    .map_err(|error| error.to_string());
                match &result {
                    Ok(()) => {
                        log::info!(target: LOG_TARGET, "agent.closed generation={}", owner.generation)
                    }
                    Err(error) => {
                        log::error!(target: LOG_TARGET, "agent.close_failed generation={} error={error}", owner.generation)
                    }
                }
                owner.closed.send_replace(Some(result));
            });
        }
        loop {
            if let Some(result) = closed.borrow_and_update().clone() {
                return result;
            }
            closed
                .changed()
                .await
                .map_err(|_| "本地登录服务关闭任务中断".to_owned())?;
        }
    }
}

pub(super) struct SystemAuthSession {
    owner: Arc<RequestAgent>,
    pub(super) exchange: Arc<TicketExchange>,
}
impl SystemAuthSession {
    pub(super) async fn finish(&self, deadline: std::time::Instant) -> Result<(), String> {
        self.owner.finish(deadline).await
    }
}
impl Drop for SystemAuthSession {
    fn drop(&mut self) {
        let owner = self.owner.clone();
        // 非正常future销毁仍持有整组至abort/join完成；正常路径先显式await finish。
        tauri::async_runtime::spawn(async move {
            let _ = owner
                .finish(std::time::Instant::now() + std::time::Duration::from_secs(5))
                .await;
        });
    }
}

#[derive(Debug)]
pub(super) struct AgentStartFailure {
    pub(super) message: String,
    pub(super) native_code: Option<i64>,
}

pub(super) async fn prepare_system_auth(
    app: &AppHandle,
    generation: u64,
    gateway: &str,
    prepared: PreparedAgentTls,
) -> Result<SystemAuthSession, AgentStartFailure> {
    let state = app.state::<LocalAgentState>();
    if state
        .exit_deadline
        .lock()
        .expect("agent exit deadline")
        .is_some()
    {
        return Err(AgentStartFailure {
            message: "应用正在退出，本次登录已取消".into(),
            native_code: None,
        });
    }
    let exchange = state.new_exchange(gateway);
    let consumer: Arc<dyn TicketConsumer> = Arc::new(TicketExchangeConsumer {
        exchange: exchange.clone(),
        consumer: GatewayTicketConsumer::new(),
    });
    let auto_login: Arc<dyn AutoLoginTrigger> = Arc::new(SavedCredentialAutoLogin {
        app: app.clone(),
        generation,
        gateway: gateway.to_owned(),
    });
    let configs = PORTAL_PORTS
        .into_iter()
        .map(|port| {
            let mut config = AgentServerConfig::new(
                prepared.cert_path.clone(),
                prepared.key_path.clone(),
                consumer.clone(),
                auto_login.clone(),
            );
            config.port = port;
            config.gateway = gateway.to_owned();
            config
        })
        .collect();
    log::info!(target: LOG_TARGET, "agent.starting generation={generation}");
    let owner = state.start_request(generation, async {
    let group = AgentServer::start_group(configs).await.map_err(|error| {
        log::error!(target: LOG_TARGET, "agent.start_failed generation={generation} address={:?} os_code={:?} error={error}", error.bind_address(), error.bind_error().and_then(std::io::Error::raw_os_error));
        AgentStartFailure { message: format!("本地登录服务启动失败：{error}"), native_code: error.bind_error().and_then(std::io::Error::raw_os_error).map(i64::from) }
    })?;
    log::info!(target: LOG_TARGET, "agent.listening generation={generation} ports={:?}", group.ports());
        Ok(group)
    }).await?;
    let exit_deadline = *state.exit_deadline.lock().expect("agent exit deadline");
    if let Some(deadline) = exit_deadline {
        let _ = owner.finish(deadline).await;
        return Err(AgentStartFailure {
            message: "应用正在退出，本次登录已取消".into(),
            native_code: None,
        });
    }
    Ok(SystemAuthSession { owner, exchange })
}

/// 票据兑换共享态：`/v1/service/start` 到达 → 立即兑换 → 结果落地并通知等待方。
///
/// 换代语义：每轮独立分配对象，打开浏览器前设置本轮网关——
/// 上一轮遗留的成功/失败不参与本轮判读；`notify` 每次终结都广播，等待方据此
/// 从等待循环醒来（门户对同一登录可能重试多张票据，R2 实证 5 次到达）。
pub struct TicketExchange {
    /// 兑换成功的会话落地（覆盖语义：新成功整体替换旧会话；失败不动既有状态）。
    store: SessionStore,
    /// 当前目标网关（消费者据此调 `sessionIdExchange`；缺省 ECNU 门户 origin）。
    gateway: RwLock<String>,
    /// 本轮网关下是否已成功兑换过（`set_gateway` 复位）。
    exchanged_for_gateway: AtomicBool,
    /// 当前轮次首票资格：兑换中或成功后占用，失败归还，新轮次复位。
    ticket_consumed: AtomicBool,
    /// 最近一次失败业务码（仅用于超时分类文案；成功清除）。
    last_failure_code: Mutex<Option<i64>>,
    /// 终结广播（已终结计数；等待方 select 此通道）。
    notify: watch::Sender<u64>,
    /// 已终结计数。
    completed: AtomicU64,
}

impl TicketExchange {
    fn new() -> Self {
        let (notify, _) = watch::channel(0);
        Self {
            store: SessionStore::default(),
            gateway: RwLock::new(exv_vpn_atrust::local_agent::DEFAULT_GATEWAY.to_owned()),
            exchanged_for_gateway: AtomicBool::new(false),
            ticket_consumed: AtomicBool::new(false),
            last_failure_code: Mutex::new(None),
            notify,
            completed: AtomicU64::new(0),
        }
    }

    /// 设定本轮目标网关并复位本轮判定（打开浏览器前调用）。
    pub fn set_gateway(&self, gateway: &str) {
        if let Ok(mut guard) = self.gateway.write() {
            *guard = gateway.to_owned();
        }
        self.exchanged_for_gateway.store(false, Ordering::Release);
        self.ticket_consumed.store(false, Ordering::Release);
        if let Ok(mut guard) = self.last_failure_code.lock() {
            *guard = None;
        }
    }

    fn claim_ticket(&self) -> bool {
        !self.ticket_consumed.swap(true, Ordering::AcqRel)
    }

    fn release_ticket(&self) {
        self.ticket_consumed.store(false, Ordering::Release);
    }

    /// 当前目标网关（缺省 ECNU 门户 origin）。
    #[must_use]
    pub fn current_gateway(&self) -> String {
        self.gateway
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| exv_vpn_atrust::local_agent::DEFAULT_GATEWAY.to_owned())
    }

    /// 兑换成功落地：空 cookie 头不落地会话、不置成功标志（网关 code=0 但无会话
    /// 字段不是成功——调用方按失败分支处理，此处只保底通知等待方一次终结）。
    /// 跨轮迟到的成功（`set_gateway` 换网关前发起的兑换）如实入店，但不置本轮
    /// 成功标志——本轮等待方经同源校验不会误收别网关会话。
    fn record_success(&self, gateway: &str, cookie_header: &str) {
        if cookie_header.is_empty() {
            self.bump();
            return;
        }
        self.store.store(gateway, cookie_header);
        if self.current_gateway() == gateway {
            self.exchanged_for_gateway.store(true, Ordering::Release);
            if let Ok(mut guard) = self.last_failure_code.lock() {
                *guard = None;
            }
        }
        self.bump();
    }

    /// 兑换失败记录（不覆盖已落地的会话；门户可能重发新票据，等待方继续等）。
    fn record_failure(&self, code: Option<i64>) {
        if let Ok(mut guard) = self.last_failure_code.lock() {
            *guard = code;
        }
        self.bump();
    }

    fn bump(&self) {
        let value = self.completed.fetch_add(1, Ordering::AcqRel) + 1;
        self.notify.send_replace(value);
    }

    /// 本轮网关下是否已有成功兑换的会话。
    #[must_use]
    pub fn exchanged_for_gateway(&self) -> bool {
        self.exchanged_for_gateway.load(Ordering::Acquire)
    }

    /// 最近一次失败业务码（本轮内；成功后清除）。
    #[must_use]
    pub fn last_failure_code(&self) -> Option<i64> {
        self.last_failure_code.lock().ok().and_then(|code| *code)
    }

    /// 当前会话快照（网关 origin + `sid=…` Cookie 头，`Secret` 承载；无会话 `None`）。
    #[must_use]
    pub fn snapshot(&self) -> Option<SessionSnapshot> {
        self.store.snapshot()
    }

    /// 终结广播接收端（等待方 select 此通道在每次兑换终结时醒来）。
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.notify.subscribe()
    }
}

impl Default for TicketExchange {
    fn default() -> Self {
        Self::new()
    }
}

/// `/v1/service/start` 票据消费者：到达即兑换（C1 派发不等 HTTP 应答；这里立即
/// 执行网关 `sessionIdExchange`——与浏览器 authCheck 竞速，R2 时序实证窗口真实
/// 存在）。结果落 [`TicketExchange`] 供等待方判读；语言取 `zh-CN` 缺省（C1 派发
/// 不携带信封 `lang`；与 agent-sim S2 评审修正 3 的缺省一致）。
struct TicketExchangeConsumer {
    exchange: Arc<TicketExchange>,
    consumer: GatewayTicketConsumer,
}

impl TicketConsumer for TicketExchangeConsumer {
    fn consume_ticket(
        &self,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send + '_>> {
        Box::pin(async move {
            if !self.exchange.claim_ticket() {
                tracing::info!(target: LOG_TARGET, "本轮已有票据进入兑换，忽略后续票据");
                return Ok(());
            }
            let gateway = self.exchange.current_gateway();
            log::info!(target: LOG_TARGET, "agent.ticket_received exchange.started");
            match self
                .consumer
                .exchange_sid_ticket(&gateway, "zh-CN", &ticket)
                .await
            {
                Ok(exchanged) => {
                    log::info!(target: LOG_TARGET, "agent.exchange_received");
                    let header = exchanged.cookie_header();
                    if header.is_empty() {
                        // 网关 code=0 但无会话字段：如实按失败记录，不动既有会话。
                        self.exchange.release_ticket();
                        self.exchange.record_failure(None);
                        tracing::warn!(
                            target: LOG_TARGET,
                            gateway = %gateway,
                            "service/start 票据兑换成功但网关未返回会话字段"
                        );
                        return Err("empty_session");
                    }
                    self.exchange.record_success(&gateway, header.as_str());
                    log::info!(target: LOG_TARGET, "agent.exchange_succeeded");
                    tracing::info!(
                        target: LOG_TARGET,
                        gateway = %gateway,
                        "service/start 票据兑换成功，会话已落地（cookie 不记录）"
                    );
                    Ok(())
                }
                Err(ConsumeError::Gateway { code }) => {
                    log::warn!(target: LOG_TARGET, "agent.exchange_failed stage=gateway code={code}");
                    self.exchange.release_ticket();
                    self.exchange.record_failure(Some(code));
                    tracing::warn!(
                        target: LOG_TARGET,
                        gateway = %gateway,
                        code,
                        "service/start 票据兑换被网关拒绝"
                    );
                    Err("gateway")
                }
                Err(ConsumeError::Local { class }) => {
                    log::warn!(target: LOG_TARGET, "agent.exchange_failed stage=local class={class}");
                    self.exchange.release_ticket();
                    self.exchange.record_failure(None);
                    tracing::warn!(
                        target: LOG_TARGET,
                        gateway = %gateway,
                        class,
                        "service/start 票据兑换本地失败"
                    );
                    Err(class)
                }
            }
        })
    }
}

/// 门户一键登录也切回本次请求的既有自动登录渠道，先结束Agent再继续。
struct SavedCredentialAutoLogin {
    app: AppHandle,
    generation: u64,
    gateway: String,
}
impl AutoLoginTrigger for SavedCredentialAutoLogin {
    fn trigger_auto_login(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if super::client::school_auto_login_saved(
                &self.gateway,
                true,
                &exv_vpn_win32_config::config_dir(),
            )
            .is_none()
            {
                log::info!(target: LOG_TARGET, "agent.saved_login_unavailable generation={}", self.generation);
                return;
            }
            if let Some(state) = self.app.try_state::<super::client::CoreState>() {
                let _ = state.connection_requests.respond_auth_preparation(
                    self.generation,
                    super::connection_request::AuthPreparationAction::TrySavedLogin,
                );
            }
        })
    }
}

/// 只读检查系统浏览器登录证书，沿用既有用户证书目录。
pub fn inspect_agent_certificate() -> Result<CertificatePreparation, CertificateError> {
    let dir = cert::default_cert_dir().ok_or_else(|| {
        CertificateError::Material(cert::CertError::Io("无法解析默认证书目录".into()))
    })?;
    cert::inspect_agent_certificate_with_store(&dir, &CertutilTrustStore::new(SystemCommandRunner))
}

/// 本次连接确认后准备 TLS；未获确认时只复用现有根信任。
pub fn prepare_agent_certificate(approved: bool) -> Result<PreparedAgentTls, CertificateError> {
    let dir = cert::default_cert_dir().ok_or_else(|| {
        CertificateError::Material(cert::CertError::Io("无法解析默认证书目录".into()))
    })?;
    cert::prepare_agent_certificate_with_store(
        &dir,
        &CertutilTrustStore::new(SystemCommandRunner),
        approved,
    )
}

/// 应用退出兜底与请求终态共用真正等待join的关闭入口。
pub async fn stop_until<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    deadline: std::time::Instant,
) {
    let owner = app.try_state::<LocalAgentState>().and_then(|state| {
        let mut outer = state.exit_deadline.lock().ok()?;
        *outer = Some(outer.map_or(deadline, |existing| existing.min(deadline)));
        state.server.lock().ok()?.upgrade()
    });
    if let Some(owner) = owner {
        let _ = owner.finish(deadline).await;
    }
}

pub fn session_snapshot(app: &AppHandle) -> Option<SessionSnapshot> {
    app.try_state::<LocalAgentState>()?
        .exchange
        .lock()
        .ok()?
        .snapshot()
}

/// 一键卸载自建 CA（当前用户 Root），返回被删除的 SHA-1 指纹。
///
/// # Errors
/// 证书目录不可解析或 `certutil -delstore -user Root` 失败时返回错误文本。
pub fn uninstall_ca() -> Result<Option<String>, String> {
    exv_vpn_win32_agent_sim::uninstall_trusted_ca().map_err(|error| error.to_string())
}
