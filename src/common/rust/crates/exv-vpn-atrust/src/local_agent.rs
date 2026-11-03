
use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::task::JoinSet;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use zeroize::Zeroize;

use crate::Secret;

/// 默认监听端口（门户硬编码探测族主口；绑定 IP 族即应答竞速，hosts 条目可省——已实证）。
pub const DEFAULT_AGENT_PORT: u16 = 54630;

/// 默认监听地址：回环 IPv4（`https://127.0.0.1:54630`）。
pub const DEFAULT_AGENT_ADDR: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// 请求信封未携带 `addr` 时的应答回显缺省（ECNU 门户 origin）。
pub const DEFAULT_GATEWAY: &str = "https://vpn.ecnu.edu.cn";

/// 门户环境核销交还浏览器的协议码（官方 `TRUSTCORE_BASIC_ERROR`）。
/// 2026-10-08 Windows 四连成功与 generation19 SDK 分支证据：门户收到此码后
/// 由自身浏览器会话发出 reportEnv；本机保持零网关请求、保留后续 start 票据。
pub const REFUSE_PRELOGIN_CODE: i64 = 11119001;

/// 请求头尺寸上限（字节）；超限按畸形请求断连。
const HEAD_LIMIT: usize = 64 * 1024;

/// 请求体尺寸上限（字节）：门户信封远小于该值；超限应答 413 并断连。
const BODY_LIMIT: usize = 1024 * 1024;

/// 日志目标（与本 crate 既有诊断一致的可检索前缀）。
const LOG_TARGET: &str = "exv.local_agent";

/// `/v1/service/start` 到达即调用的票据消费者（路线 A 主源：票据由门户直送本地 Agent）。
///
/// 实现方（darwin core / win32 host 生产接线）负责真正的 `sessionIdExchange` 兑换链
/// 与成败处理（含日志、75500304 用户文案等）；服务端只保证「票据全值到达即派发」且
/// 派发不阻断 HTTP 应答（与浏览器 authCheck 竞速——R2 时序实证：reportEnv→start→
/// authCheck，竞速窗口真实存在；窗口内即时兑换的成败由 C3 真机验证）。
///
/// 返回 `Err(固定分类词)` 只用于服务端补一条失败日志，不进应答、不携带票据内容。
pub trait TicketConsumer: Send + Sync {
    /// 消费一张 `sidTicket`（全值，[`Secret`] 承载、Drop 零化）。
    fn consume_ticket(
        &self,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send + '_>>;
}

/// 闭包适配：`Fn(Secret) -> Future`（Future 须 `'static`——不借用闭包环境）自动实现。
/// 需要借用自身状态的实现直接 impl [`TicketConsumer`]（trait 方法允许借用 `&self`）。
impl<F, Fut> TicketConsumer for F
where
    F: Fn(Secret) -> Fut + Send + Sync,
    Fut: Future<Output = Result<(), &'static str>> + Send + 'static,
{
    fn consume_ticket(
        &self,
        ticket: Secret,
    ) -> Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send + '_>> {
        Box::pin(self(ticket))
    }
}

/// `/v1/service/online` 到达即调用的免密自动登录触发器（C5 门户免密通道的原生协议位）。
///
/// 语义：门户页「一键登录」（`psw_free`）命令客户端用自有凭据自登录；登录由触发方
/// 异步执行，服务端只回「已接受」。连接级去重（R2 实证同一会话 online 可重试数十次）
/// 由实现方保证——服务端每次到达都透传。
pub trait AutoLoginTrigger: Send + Sync {
    /// 触发一次免密自动登录。
    fn trigger_auto_login(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// 闭包适配：`Fn() -> Future`（Future 须 `'static`）自动实现；同 [`TicketConsumer`]。
impl<F, Fut> AutoLoginTrigger for F
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn trigger_auto_login(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self())
    }
}

/// init 应答 `data.status` 的缺省值（逆向①：`firstLogin`/`offline` → 门户 startTunnel）。
pub const INIT_STATUS_FIRST_LOGIN: &str = "firstLogin";

/// 服务端配置：TLS 材料由 C2 证书管理提供（叶子证书 SAN 需含 `IP:127.0.0.1`），
/// 消费/触发点由宿主注入。
#[derive(Clone)]
pub struct AgentServerConfig {
    /// 监听地址（默认 [`DEFAULT_AGENT_ADDR`]）。
    pub bind: IpAddr,
    /// 监听端口（默认 [`DEFAULT_AGENT_PORT`]；传 `0` 由系统分配临时端口，供测试）。
    pub port: u16,
    /// TLS 叶子证书 PEM 文件路径（C2 提供；应含叶子及可选链）。
    pub cert_path: PathBuf,
    /// TLS 私钥 PEM 文件路径（与叶子证书配对）。
    pub key_path: PathBuf,
    /// 请求信封未携带 `addr` 时的应答回显缺省（默认 [`DEFAULT_GATEWAY`]）。
    pub gateway: String,
    /// `/v1/service/start` 票据消费者（到达即调）。
    pub ticket_consumer: Arc<dyn TicketConsumer>,
    /// `/v1/service/online` 免密登录触发器（到达即调；去重由实现方保证）。
    pub auto_login: Arc<dyn AutoLoginTrigger>,
    /// init 应答注入的 `data.username`（C5：门户渲染 psw_free 一键登录视图的必要
    /// 条件）。`None` 时键缺省——psw_free 不渲染；空串视同 `None`。
    pub init_username: Option<String>,
    /// init 应答 `data.status` 的初始覆盖值（`None` → [`INIT_STATUS_FIRST_LOGIN`]；
    /// 运行时可经 [`AgentHandle::set_init_status`] 更新，如免密登录成功后的
    /// `online`）。
    pub init_status: Option<String>,
}

impl AgentServerConfig {
    /// 以 TLS 材料与注入点构造（bind/port/gateway/init 注入取默认值）。
    #[must_use]
    pub fn new(
        cert_path: PathBuf,
        key_path: PathBuf,
        ticket_consumer: Arc<dyn TicketConsumer>,
        auto_login: Arc<dyn AutoLoginTrigger>,
    ) -> Self {
        Self {
            bind: DEFAULT_AGENT_ADDR,
            port: DEFAULT_AGENT_PORT,
            cert_path,
            key_path,
            gateway: DEFAULT_GATEWAY.to_owned(),
            ticket_consumer,
            auto_login,
            init_username: None,
            init_status: None,
        }
    }
}

/// 本地 Agent 服务端启动错误（原样上抛，宿主呈现给用户；不含任何敏感材料）。
#[derive(Debug)]
pub enum AgentServerError {
    /// 监听端口绑定失败（如官方客户端占用 54630）。
    /// 通过 [`Self::bind_error`] 读取原始 OS 错误，地址上下文由 [`Self::bind_address`] 提供。
    Bind(std::io::Error),
    /// TLS 材料文件读取失败。
    MaterialIo(std::io::Error),
    /// 叶子证书 PEM 解析失败。
    CertPem(rustls::pki_types::pem::Error),
    /// 私钥 PEM 解析失败。
    KeyPem(rustls::pki_types::pem::Error),
    /// 证书文件不含任何 CERTIFICATE 块。
    EmptyChain,
    /// rustls 服务端配置构造失败。
    Tls(rustls::Error),
    /// 监听组必须至少包含一个宿主配置。
    EmptyGroup,
    /// 同轮配置必须引用相同的消费与登录对象。
    InconsistentGroup,
}

impl std::fmt::Display for AgentServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(inner) => write!(f, "本地 Agent 端口绑定失败: {inner}"),
            Self::MaterialIo(inner) => write!(f, "TLS 材料读取失败: {inner}"),
            Self::CertPem(inner) => write!(f, "叶子证书 PEM 解析失败: {inner}"),
            Self::KeyPem(inner) => write!(f, "私钥 PEM 解析失败: {inner}"),
            Self::EmptyChain => write!(f, "证书文件不含任何 CERTIFICATE 块"),
            Self::Tls(inner) => write!(f, "TLS 服务端配置构造失败: {inner}"),
            Self::EmptyGroup => write!(f, "本地 Agent 监听组没有配置"),
            Self::InconsistentGroup => write!(f, "本地 Agent 监听组配置引用了不同的认证消费对象"),
        }
    }
}

/// 绑定失败的地址上下文；原始 OS 错误保存在 source 中。
#[derive(Debug)]
struct AgentBindContext {
    address: std::net::SocketAddr,
    source: std::io::Error,
}

impl std::fmt::Display for AgentBindContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.address, self.source)
    }
}

impl std::error::Error for AgentBindContext {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl AgentServerError {
    /// 返回绑定失败的原始 OS 错误，供宿主读取 kind 和 raw_os_error。
    #[must_use]
    pub fn bind_error(&self) -> Option<&std::io::Error> {
        match self {
            Self::Bind(error) => Some(
                error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<AgentBindContext>())
                    .map_or(error, |context| &context.source),
            ),
            _ => None,
        }
    }

    /// 返回绑定失败的实际配置地址。
    #[must_use]
    pub fn bind_address(&self) -> Option<std::net::SocketAddr> {
        match self {
            Self::Bind(error) => error
                .get_ref()
                .and_then(|source| source.downcast_ref::<AgentBindContext>())
                .map(|context| context.address),
            _ => None,
        }
    }
}

impl std::error::Error for AgentServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.bind_error()
            .map(|error| error as &(dyn std::error::Error + 'static))
    }
}

/// 本地 Agent 服务端入口（命名空间；实例状态全部在 [`AgentHandle`] 与内部任务里）。
pub struct AgentServer;

impl AgentServer {
    /// 启动单个监听器；沿用既有宿主入口。
    pub async fn start(config: AgentServerConfig) -> Result<AgentHandle, AgentServerError> {
        Ok(AgentHandle {
            group: Self::start_group(vec![config]).await?,
        })
    }

    /// 按宿主提供的地址与 TLS 配置启动同轮监听组。
    /// 全部绑定成功后才受理请求，绑定失败时释放已绑定项。
    pub async fn start_group(
        configs: Vec<AgentServerConfig>,
    ) -> Result<AgentGroupHandle, AgentServerError> {
        let first = configs.first().ok_or(AgentServerError::EmptyGroup)?;
        if configs.iter().any(|config| {
            !Arc::ptr_eq(&first.ticket_consumer, &config.ticket_consumer)
                || !Arc::ptr_eq(&first.auto_login, &config.auto_login)
        }) {
            return Err(AgentServerError::InconsistentGroup);
        }
        let owned = Arc::new(OwnedTasks::default());
        let seen_tickets = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let mut bound = Vec::with_capacity(configs.len());
        for config in configs {
            let tls = load_tls_server_config(&config.cert_path, &config.key_path)?;
            let address = std::net::SocketAddr::new(config.bind, config.port);
            let listener = TcpListener::bind(address).await.map_err(|error| {
                AgentServerError::Bind(std::io::Error::new(
                    error.kind(),
                    AgentBindContext {
                        address,
                        source: error,
                    },
                ))
            })?;
            let port = listener
                .local_addr()
                .map_err(AgentServerError::Bind)?
                .port();
            bound.push((config, tls, listener, port));
        }
        let (shutdown, shutdown_rx) = watch::channel(false);
        let mut accept_tasks = JoinSet::new();
        let mut ports = Vec::with_capacity(bound.len());
        let mut init_status = Vec::with_capacity(bound.len());
        for (config, tls, listener, port) in bound {
            let status = Arc::new(RwLock::new(config.init_status));
            let state = Arc::new(ServerState {
                gateway: config.gateway,
                ticket_consumer: config.ticket_consumer,
                seen_referer_tickets: Arc::clone(&seen_tickets),
                auto_login: config.auto_login,
                init_username: config.init_username,
                init_status: Arc::clone(&status),
                owned: Arc::clone(&owned),
            });
            let acceptor = TlsAcceptor::from(tls);
            accept_tasks.spawn(run_accept_loop(
                listener,
                state,
                shutdown_rx.clone(),
                move |tcp| {
                    let acceptor = acceptor.clone();
                    async move {
                        match acceptor.accept(tcp).await {
                            Ok(stream) => Some(stream),
                            Err(error) => {
                                tracing::debug!(target: LOG_TARGET, %error, "TLS 握手失败");
                                None
                            }
                        }
                    }
                },
            ));
            ports.push(port);
            init_status.push(status);
        }
        Ok(AgentGroupHandle {
            shutdown,
            accept_tasks,
            owned,
            ports,
            init_status,
        })
    }
}

/// 本轮关闭失败的实际步骤和任务收尾事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentShutdownError {
    pub step: &'static str,
    pub timed_out: bool,
    pub aborted_tasks: usize,
    pub failed_tasks: usize,
}

impl std::fmt::Display for AgentShutdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "本地 Agent 关闭步骤 {}：超时={}，已中止任务={}，失败任务={}",
            self.step, self.timed_out, self.aborted_tasks, self.failed_tasks
        )
    }
}
impl std::error::Error for AgentShutdownError {}

/// 单监听兼容句柄，内部拥有完整的一轮服务端任务。
#[derive(Debug)]
pub struct AgentHandle {
    group: AgentGroupHandle,
}
impl AgentHandle {
    #[must_use]
    pub fn port(&self) -> u16 {
        self.group.ports[0]
    }

    pub fn set_init_status(&self, status: Option<String>) {
        self.group.set_init_status(status);
    }

    /// 停止受理并在独立的最多五秒窗口内完成响应和消费任务。
    pub async fn shutdown(self) {
        if let Err(error) = self
            .shutdown_until(Instant::now() + Duration::from_secs(5))
            .await
        {
            tracing::warn!(target: LOG_TARGET, %error, "本地 Agent 关闭任务收尾失败");
        }
    }

    pub async fn shutdown_until(self, deadline: Instant) -> Result<(), AgentShutdownError> {
        self.group.shutdown_until(deadline).await
    }
}

/// 同轮监听、TLS/HTTP 连接、消费与 online 回调的共同所有者。
#[derive(Debug)]
pub struct AgentGroupHandle {
    shutdown: watch::Sender<bool>,
    accept_tasks: JoinSet<()>,
    owned: Arc<OwnedTasks>,
    ports: Vec<u16>,
    init_status: Vec<Arc<RwLock<Option<String>>>>,
}
impl AgentGroupHandle {
    #[must_use]
    pub fn ports(&self) -> &[u16] {
        &self.ports
    }

    pub fn set_init_status(&self, status: Option<String>) {
        for slot in &self.init_status {
            *slot.write().expect("init status 锁") = status.clone();
        }
    }

    /// deadline 是调用方给出的独立 drain 期限，与认证期限无关；上限五秒。
    /// 返回前完成全部 join，超时先 abort 再 join，释放所有监听和连接。
    pub async fn shutdown_until(mut self, deadline: Instant) -> Result<(), AgentShutdownError> {
        let deadline =
            tokio::time::Instant::from_std(deadline.min(Instant::now() + Duration::from_secs(5)));
        let _ = self.shutdown.send(true);
        let mut failed_tasks = 0;
        let mut aborted_tasks = 0;
        let mut timed_out = false;
        let mut step = "join_tasks";
        if tokio::time::timeout_at(deadline, async {
            while let Some(result) = self.accept_tasks.join_next().await {
                failed_tasks += usize::from(result.is_err());
            }
        })
        .await
        .is_err()
        {
            timed_out = true;
            step = "stop_accept";
            aborted_tasks += self.accept_tasks.len();
            self.accept_tasks.abort_all();
            while let Some(result) = self.accept_tasks.join_next().await {
                failed_tasks += usize::from(result.is_err_and(|error| !error.is_cancelled()));
            }
        }
        if tokio::time::timeout_at(deadline, self.owned.join_all(&mut failed_tasks))
            .await
            .is_err()
        {
            timed_out = true;
            step = "drain_tasks";
            aborted_tasks += self.owned.abort_all();
            self.owned.join_all(&mut failed_tasks).await;
        }
        if timed_out || failed_tasks != 0 {
            Err(AgentShutdownError {
                step,
                timed_out,
                aborted_tasks,
                failed_tasks,
            })
        } else {
            Ok(())
        }
    }
}
impl Drop for AgentGroupHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        self.accept_tasks.abort_all();
        self.owned.abort_all();
    }
}

#[derive(Debug, Default)]
struct OwnedTasks {
    tasks: Mutex<JoinSet<()>>,
    aborting: std::sync::atomic::AtomicBool,
}
impl OwnedTasks {
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self.tasks.lock().expect("Agent task 锁");
        if !self.aborting.load(std::sync::atomic::Ordering::Relaxed) {
            tasks.spawn(task);
        }
    }

    fn abort_all(&self) -> usize {
        let mut tasks = self.tasks.lock().expect("Agent task 锁");
        let count = tasks.len();
        self.aborting
            .store(true, std::sync::atomic::Ordering::Relaxed);
        tasks.abort_all();
        count
    }

    async fn join_all(&self, failed: &mut usize) {
        // 只在 poll 时短暂持锁；Pending 时释放，让在途 dispatch 注册其子任务。
        while let Some(result) =
            std::future::poll_fn(|cx| self.tasks.lock().expect("Agent task 锁").poll_join_next(cx))
                .await
        {
            *failed += usize::from(result.is_err_and(|error| !error.is_cancelled()));
        }
    }
}

/// 每连接共享的服务端状态。
struct ServerState {
    /// 同轮所有监听、Referer 与 service/start 的已派发票据。
    seen_referer_tickets: Arc<Mutex<std::collections::HashSet<String>>>,
    owned: Arc<OwnedTasks>,
    /// 信封缺省 `addr`（应答回显用）。
    gateway: String,
    ticket_consumer: Arc<dyn TicketConsumer>,
    auto_login: Arc<dyn AutoLoginTrigger>,
    /// init 应答注入的用户名（启动时快照；键缺省即 psw_free 不渲染）。
    init_username: Option<String>,
    /// init 应答 `data.status` 覆盖值（与 [`AgentHandle`] 共享，运行时可更新）。
    init_status: Arc<RwLock<Option<String>>>,
}

/// 读取并解析 TLS 材料（PEM），构造 rustls 服务端配置。
fn load_tls_server_config(
    cert_path: &Path,
    key_path: &Path,
) -> Result<Arc<rustls::ServerConfig>, AgentServerError> {
    let cert_pem = std::fs::read(cert_path).map_err(AgentServerError::MaterialIo)?;
    let key_pem = std::fs::read(key_path).map_err(AgentServerError::MaterialIo)?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(AgentServerError::CertPem)?;
    if certs.is_empty() {
        return Err(AgentServerError::EmptyChain);
    }
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(AgentServerError::KeyPem)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(AgentServerError::Tls)?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(AgentServerError::Tls)?;
    Ok(Arc::new(config))
}

/// 接受循环：停机信号优先；每连接一个 tokio 任务。
///
/// `wrap` 把 TCP 流变成服务端要处理的流（生产为 TLS 包裹；测试复用同一循环注入
/// 明文恒等包裹，驱动完全相同的连接处理代码）。
async fn run_accept_loop<S, F, Fut>(
    listener: TcpListener,
    state: Arc<ServerState>,
    mut shutdown: watch::Receiver<bool>,
    wrap: F,
) where
    F: Fn(TcpStream) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = Option<S>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((tcp, _)) => {
                    let state = Arc::clone(&state);
                    let wrap = wrap.clone();
                    let mut conn_shutdown = shutdown.clone();
                    let owned = Arc::clone(&state.owned);
                    owned.spawn(async move {
                        if let Some(stream) = wrap(tcp).await {
                            handle_connection(stream, state, &mut conn_shutdown).await;
                        }
                    });
                }
                Err(error) => tracing::debug!(target: LOG_TARGET, %error, "accept 失败"),
            },
        }
    }
}

/// 单连接处理：HTTP/1.1 keep-alive 循环（请求头 + content-length 请求体 + 应答）。
async fn handle_connection<S>(
    mut stream: S,
    state: Arc<ServerState>,
    shutdown: &mut watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buffer: Vec<u8> = Vec::with_capacity(4096);
    loop {
        if *shutdown.borrow_and_update() {
            let _ = stream.shutdown().await;
            return;
        }
        let Some(head_end) = read_head(&mut stream, &mut buffer, shutdown).await else {
            let _ = stream.shutdown().await;
            return;
        };
        let head_text = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
        let Some(parsed) = ParsedHead::parse(&head_text) else {
            return;
        };
        if parsed.content_length > BODY_LIMIT {
            let outcome = Dispatched {
                status: 413,
                body: Vec::new(),
                referer_valid: true,
            };
            let response = build_response(&parsed, &outcome, true);
            let _ = stream.write_all(&response).await;
            // 主动断连前完成 TLS 关闭（close_notify），对明文流等价直接关闭。
            let _ = stream.shutdown().await;
            return;
        }
        let body_start = head_end + 4;
        let body_end = body_start + parsed.content_length;
        if !read_body(&mut stream, &mut buffer, body_end).await {
            return;
        }
        let request_body = buffer[body_start..body_end].to_vec();
        buffer.drain(..body_end);
        let outcome = dispatch(&state, &parsed, &request_body);
        let close = parsed
            .header("connection")
            .is_some_and(|value| value.to_ascii_lowercase().contains("close"));
        let response = build_response(&parsed, &outcome, close);
        if stream.write_all(&response).await.is_err() {
            return;
        }
        if close || *shutdown.borrow() {
            // 主动断连前完成 TLS 关闭（close_notify），对明文流等价直接关闭。
            let _ = stream.shutdown().await;
            return;
        }
    }
}

/// 读到完整请求头（含停机/对端断开/超限三种失败形态，均返回 `None` 断连）。
async fn read_head<S>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<usize>
where
    S: AsyncRead + Unpin,
{
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(position) = find_header_end(buffer) {
            return Some(position);
        }
        if buffer.len() > HEAD_LIMIT {
            return None;
        }
        let read = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                let _ = changed;
                return None;
            }
            read = stream.read(&mut chunk) => read,
        };
        match read {
            Ok(0) | Err(_) => return None,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
}

/// 读满定长请求体（受 `content-length <= BODY_LIMIT` 约束，无停机分支）。
async fn read_body<S>(stream: &mut S, buffer: &mut Vec<u8>, body_end: usize) -> bool
where
    S: AsyncRead + Unpin,
{
    let mut chunk = [0u8; 8192];
    while buffer.len() < body_end {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
    true
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// 解析后的请求头（只取本服务需要的字段）。
struct ParsedHead {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    content_length: usize,
}

impl ParsedHead {
    fn parse(head: &str) -> Option<Self> {
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next()?.split_whitespace();
        let method = request_line.next()?.to_owned();
        let path = request_line.next()?.to_owned();
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
            .collect();
        let content_length = headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse().ok())
            .unwrap_or(0);
        Some(Self {
            method,
            path,
            headers,
            content_length,
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// 路由产物：状态码 + 应答体 + checkReferValid 结果（决定 CORS 档位）。
struct Dispatched {
    status: u16,
    body: Vec<u8>,
    /// `true` = 完整 CORS（回显 Origin）；`false` = 降级（`Access-Control-Allow-Origin: null`）。
    referer_valid: bool,
}

impl Dispatched {
    fn json(value: &Value, referer_valid: bool) -> Self {
        Self {
            status: 200,
            body: serde_json::to_vec(value).expect("serde_json::Value 序列化不应失败"),
            referer_valid,
        }
    }
}

/// init 臂的注入快照（其余端点忽略；`dispatch` 每请求构造一次）。
struct InitOverrides<'a> {
    /// 有效凭据对用户名（`None`/空 → init 应答不带 `username` 键，psw_free 不渲染）。
    username: Option<&'a str>,
    /// 覆盖后的 `data.status`（`None` → [`INIT_STATUS_FIRST_LOGIN`]）。
    status: &'a str,
}

/// 请求分发：OPTIONS 预检 / 非 POST 兜底 404 / POST 路由表 + 动态端点（start/online/printLog）。
///
/// 动态副作用（票据消费/免密触发）交给本轮任务集合，不在应答路径上等待——
/// 「到达即兑换」与浏览器 authCheck 竞速（计划 C1 路线 A 主源）。
fn dispatch(state: &Arc<ServerState>, request: &ParsedHead, body: &[u8]) -> Dispatched {
    if request.method.eq_ignore_ascii_case("OPTIONS") {
        // 预检无请求体（无信封 addr 可校验）：按完整 CORS 回显。
        return Dispatched {
            status: 204,
            body: Vec::new(),
            referer_valid: true,
        };
    }
    if !request.method.eq_ignore_ascii_case("POST") {
        // 与 sim 一致：非 POST（含未实现的 WS 升级 GET）以 404 错误信封应答。
        return Dispatched {
            status: 404,
            body: serde_json::to_vec(&json!({"code": -1, "data": {}}))
                .expect("serde_json::Value 序列化不应失败"),
            referer_valid: true,
        };
    }
    let envelope_addr = envelope_addr(body);
    let addr = envelope_addr
        .clone()
        .unwrap_or_else(|| state.gateway.clone());
    let referer_valid = check_refer_valid(request.header("referer"), envelope_addr.as_deref());
    let path = path_only(&request.path);
    // Referer 票据提取已禁用（2026-10-28 E2E 实证）：
    // shortcut 票据是浏览器绑定票——我们的独立会话兑换必然 TicketInvalid，
    // 且兑换尝试会消费/标记票据，导致浏览器自己的 authCheck 也失败。
    // 正确流：让浏览器自然走完 shortcut 链（authCheck 用票据成功）→ SPA 加载
    // → _startClient→ticketExchange 产生无绑定新票 → service/start 直送本 Agent。
    // 后续如需启用，设 EXV_REFERER_EXTRACT=1 环境变量。
    if std::env::var("EXV_REFERER_EXTRACT").as_deref() == Ok("1") {
        if let Some(ticket) = extract_ticket_from_referer(request.header("referer")) {
            let mut seen = state
                .seen_referer_tickets
                .lock()
                .expect("referer ticket 去重锁");
            let ticket_key = ticket.expose().to_owned();
            if !seen.contains(&ticket_key) {
                seen.insert(ticket_key);
                drop(seen);
                eprintln!(
                    "[local_agent] Referer 票据提取（EXV_REFERER_EXTRACT=1）len={}",
                    ticket.len()
                );
                let consumer = Arc::clone(&state.ticket_consumer);
                state.owned.spawn(async move {
                    if let Err(class) = consumer.consume_ticket(ticket).await {
                        tracing::warn!(target: LOG_TARGET, class, "Referer 票据消费失败");
                    }
                });
            }
        }
    }
    let init_status = state.init_status.read().expect("init status 锁").clone();
    let init = InitOverrides {
        username: state.init_username.as_deref(),
        status: init_status.as_deref().unwrap_or(INIT_STATUS_FIRST_LOGIN),
    };
    match path {
        "/v1/service/start" => {
            match extract_sid_ticket(body) {
                Some(ticket) => {
                    tracing::info!(
                        target: LOG_TARGET,
                        len = ticket.len(),
                        "service/start 票据到达，立即派发兑换（应答不受阻断）"
                    );
                    eprintln!(
                        "[local_agent] service/start 票据到达 len={}，派发兑换",
                        ticket.len()
                    );
                    // 路线 A 主源：先派发消费再写应答——与浏览器 authCheck 竞速；
                    // 消费成败由消费方处理，失败只记日志、不影响应答。
                    let first = state
                        .seen_referer_tickets
                        .lock()
                        .expect("ticket 去重锁")
                        .insert(ticket.expose().to_owned());
                    if first {
                        let consumer = Arc::clone(&state.ticket_consumer);
                        state.owned.spawn(async move {
                            if let Err(class) = consumer.consume_ticket(ticket).await {
                                tracing::warn!(target: LOG_TARGET, class, "票据消费失败（成败由消费方处理）");
                            }
                        });
                    }
                }
                None => {
                    tracing::debug!(
                        target: LOG_TARGET,
                        "service/start 未携带 sidTicket（空票据不派发）"
                    );
                }
            }
            Dispatched::json(&endpoint_response(path, &addr, &init), referer_valid)
        }
        "/v1/service/online" => {
            tracing::info!(
                target: LOG_TARGET,
                "service/online 到达，派发免密自动登录（去重由触发方保证）"
            );
            // 免密触发：登录由触发方异步执行，此处只回「已接受」；每次到达都透传。
            let trigger = Arc::clone(&state.auto_login);
            state.owned.spawn(async move {
                trigger.trigger_auto_login().await;
            });
            Dispatched::json(&endpoint_response(path, &addr, &init), referer_valid)
        }
        "/v1/base/printLog" => {
            // 门户把自身控制台日志流给客户端：载荷可能含票据，只记 kind+path+长度，
            // 不落原文。
            tracing::debug!(
                target: LOG_TARGET,
                kind = "printLog",
                path,
                len = body.len(),
                "门户 printLog 上调（载荷不落原文）"
            );
            Dispatched::json(&endpoint_response(path, &addr, &init), referer_valid)
        }
        _ => Dispatched::json(&endpoint_response(path, &addr, &init), referer_valid),
    }
}

/// 静态端点应答表（形状与 `tools/research/atrust_r2_sim.py` 逐字段一致）。
///
/// 动态端点（start/online/printLog）的副作用在 [`dispatch`]；此处只承载应答形状。
/// init 臂的 `data.username`/`data.status` 由 [`InitOverrides`] 注入（C5 免密通道），
/// 默认（无注入）与 sim 表逐字一致。
/// 兜底：未知路径（含全部 `/v1/*`）一律最小成功信封（计划 C1：`{code:0,data:{}}`）。
// 形状表按端点逐条钉死（即使多条臂应答相同也不合并）：清单即协议契约文档，
// 合并臂会掩盖「哪些端点存在」这一事实。
#[allow(clippy::match_same_arms)]
#[must_use]
fn endpoint_response(path: &str, addr: &str, init: &InitOverrides<'_>) -> Value {
    match path {
        "/v1/detect" => json!({"code": 0, "data": {}}),
        "/v1/service/init" => {
            let mut data = json!({
                // 逆向①：status 是分岔开关——firstLogin/offline → 门户 startTunnel
                // （网关 ticketExchange 新票 → service/start 直送本地，先于浏览器烧票）。
                // C5：免密登录成功后宿主切 `online`（init_status 运行时覆盖）。
                "status": init.status,
                "addr": addr,
                "sidTicket": "",
                // R2 第四轮实证：check_environment 硬性要求 capacities.antiMIMTAttack
                // truthy，缺失即判「客户端版本过低」→升级失败页。其余能力保守不开启。
                "capacities": {"antiMIMTAttack": {"enable": 0, "version": "1"}},
            });
            // C5 免密通道：data.username 在位时门户渲染 psw_free/quick_logon 一键登录
            // 视图；缺失（或空串）时键缺省——门户 assert「username is empty」后视图
            // 回落（R2 实证安全），不渲染一键登录。
            if let Some(username) = init.username.filter(|username| !username.is_empty()) {
                data["username"] = json!(username);
            }
            json!({
                "code": 0,
                "message": "",
                "data": data,
                // 逆向①：security 在顶层（非 data 内）；riskType 恒空串。
                "security": {"origin": "", "target": "", "riskType": "", "antiUninstall": false},
            })
        }
        "/v1/service/checkMITMAttack" => json!({"code": 0, "data": {}}),
        // 浏览器绑定事务由门户自己的 Cookie 会话核销，直接交还浏览器处理。
        "/v1/service/reportEnvBeforeLogin" => {
            json!({"code": REFUSE_PRELOGIN_CODE, "message": "", "data": {}})
        }
        "/v1/service/status" => json!({
            "code": 0,
            "message": "",
            "data": {"status": "offline", "addr": addr, "tunnelStatus": "tunnelUnavailable"},
        }),
        "/v1/service/start" => json!({"code": 0, "message": "", "data": {}}),
        "/v1/service/online" => json!({
            "code": 0,
            "data": {"status": "online", "addr": addr, "tunnelStatus": "tunnelAvailable"},
        }),
        "/v1/service/logout" | "/v1/service/offline" => json!({"code": 0, "data": {}}),
        "/v1/update/status" => json!({"code": 0, "data": {}}),
        "/v1/update/check" => json!({"code": 0, "data": {"type": "noUpdate"}}),
        "/v1/update/skip" | "/v1/update/start" => json!({"code": 0, "data": {}}),
        "/v1/base/printLog" | "/v1/base/setinfo" => json!({"code": 0, "data": {}}),
        // 顶层非信封——与 sim 一致（门户只读 uemStatus）。
        "/v1/aUem/statusV2" => json!({"uemStatus": 0}),
        "/v1/service/queryAppModuleInfo" => json!({"code": 0, "resourceReady": true, "data": {}}),
        // HTTP 长轮询：立即空事件应答（WS 移出 C1，事件通道由宿主后续补）。
        "/v1/service/events" => json!({"code": 0, "data": {"id": 0, "events": []}}),
        _ => json!({"code": 0, "data": {}}),
    }
}

/// checkReferValid（逆向④：官方 Core 同款）：请求信封 `addr` 与 `Referer` 前缀匹配。
///
/// 判据 `Referer.startswith(addr + "/portal/")`——必须前缀而非等值：`shortcut.html`
/// 等带路径/查询的 Referer 在等值匹配下会被误拒（计划 C1 复审必改）。
/// `addr` 取自请求 JSON 信封；无信封/无 Referer 场景以 `None` 传入（判不匹配）。
/// 从 Referer 头提取 shortcut URL 中的 sidTicket。
///
/// Referer 形态（R2 实证）：
/// `https://<gateway>/portal/shortcut.html?...&data={"ticket":"<uuid>_<uuid>","env":{...}}&...`
/// 票据为 URL 编码的 JSON 中的 `ticket` 字段值（两个下划线连接的 UUID）。
fn extract_ticket_from_referer(referer: Option<&str>) -> Option<Secret> {
    let referer = referer?;
    // 快速前置过滤：必须含 shortcut.html 和 ticket 关键词
    if !referer.contains("shortcut.html") || !referer.contains("ticket") {
        return None;
    }
    // URL 解码 data 参数
    let decoded = urldecode(referer)?;
    // 提取 "ticket":"..." 的值
    let marker = "\"ticket\":\"";
    let start = decoded.find(marker)? + marker.len();
    let end = decoded[start..].find('"')? + start;
    let ticket = &decoded[start..end];
    // 验证形态：UUID_UUID（两个 UUID 用下划线连接）
    if ticket.len() >= 36 && ticket.contains('_') && !ticket.contains(' ') {
        return Some(Secret::new(ticket.to_owned()));
    }
    None
}

/// 最小 URL 解码（%XX → 字节）。
fn urldecode(input: &str) -> Option<String> {
    let mut result = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &input[i + 1..i + 3];
            let byte = u8::from_str_radix(hex, 16).ok()?;
            result.push(byte);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).ok()
}

fn check_refer_valid(referer: Option<&str>, addr: Option<&str>) -> bool {
    match (referer, addr) {
        (Some(referer), Some(addr)) => referer
            .strip_prefix(addr)
            .is_some_and(|rest| rest.starts_with("/portal/")),
        _ => false,
    }
}

/// 去掉查询串，只保留路径。
fn path_only(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

/// 提取请求信封 `addr`（非空字符串；非法/缺省 JSON 返回 `None`）。
fn envelope_addr(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .get("addr")
        .and_then(Value::as_str)
        .filter(|addr| !addr.is_empty())
        .map(str::to_owned)
}

/// 提取 `/v1/service/start` 信封的 `data.sidTicket` 全值（非空才返回；`Secret` 承载）。
///
/// 中间结构体实现 `Drop` 零化，票据字符串不在解析残骸中留明文（与 win32
/// agent-sim `consume.rs` 同款纪律）。
fn extract_sid_ticket(body: &[u8]) -> Option<Secret> {
    #[derive(Deserialize)]
    struct Data {
        #[serde(rename = "sidTicket", default)]
        sid_ticket: Option<String>,
    }
    impl Drop for Data {
        fn drop(&mut self) {
            if let Some(ticket) = &mut self.sid_ticket {
                ticket.zeroize();
            }
        }
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        data: Option<Data>,
    }
    let envelope: Envelope = serde_json::from_slice(body).ok()?;
    let mut data = envelope.data?;
    let ticket = data.sid_ticket.take().filter(|ticket| !ticket.is_empty())?;
    Some(Secret::new(ticket))
}

/// 组装 HTTP/1.1 应答字节：CORS/PNA 头 + 状态行 + 定长正文。
fn build_response(request: &ParsedHead, outcome: &Dispatched, close: bool) -> Vec<u8> {
    let origin = request.header("origin");
    let allow_origin = if outcome.referer_valid {
        origin.unwrap_or("*")
    } else {
        // checkReferValid 不通过 → CORS 降级（浏览器以 null origin 隔离，不再携带凭据）。
        "null"
    };
    let mut head = String::with_capacity(256);
    head.push_str("HTTP/1.1 ");
    head.push_str(&outcome.status.to_string());
    head.push(' ');
    head.push_str(reason_phrase(outcome.status));
    head.push_str("\r\n");
    head.push_str("Access-Control-Allow-Origin: ");
    head.push_str(allow_origin);
    head.push_str("\r\n");
    // 通配 `*` 在 withCredentials 下被拒：回显具体 Origin + 允许凭据（知识库一、CORS）。
    head.push_str("Access-Control-Allow-Credentials: true\r\n");
    head.push_str("Vary: Origin\r\n");
    if request.method.eq_ignore_ascii_case("OPTIONS") {
        head.push_str("Access-Control-Allow-Methods: POST, OPTIONS, GET\r\n");
        let requested = request
            .header("access-control-request-headers")
            .unwrap_or("*");
        head.push_str("Access-Control-Allow-Headers: ");
        head.push_str(requested);
        head.push_str("\r\n");
        head.push_str("Access-Control-Max-Age: 600\r\n");
        // Chrome LNA/PNA 对冲：仅在浏览器声明该请求头时回设（官方 agent 二进制证据，
        // win32 agent-sim S4；R2 sim 同款）。
        if request
            .header("access-control-request-private-network")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        {
            head.push_str("Access-Control-Allow-Private-Network: true\r\n");
        }
    }
    if !outcome.body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str("Content-Length: ");
    head.push_str(&outcome.body.len().to_string());
    head.push_str("\r\n");
    head.push_str(if close {
        "Connection: close\r\n"
    } else {
        "Connection: keep-alive\r\n"
    });
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&outcome.body);
    bytes
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        204 => "No Content",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "OK",
    }
}
