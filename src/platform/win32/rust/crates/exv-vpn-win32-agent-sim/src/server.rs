//! 本地 HTTPS/WSS 服务：监听 `127.0.0.1:54630`（默认再加 54631），按端点契约应答。
//!
//! 传输（规格 §3/§5）：TCP + TLS（自建 CA 签发的叶子），HTTP/1.1 手写解析（请求体小、
//! 客户端固定），WebSocket 由 `tokio-tungstenite` 完成握手与分帧。每个 HTTP 响应都带
//! CORS 头（回显 `Origin`，无则 `*`）；OPTIONS 预检在浏览器声明
//! `Access-Control-Request-Private-Network` 时按官方行为应答
//! `Access-Control-Allow-Private-Network: true`（S4，见 [`build_http_response`]）；
//! WS 响应回显 `sdpTraceId`。
//!
//! S2 动态端点：`/v1/service/reportEnvBeforeLogin` 与 `/v1/service/start` 不走静态契约，
//! 由 [`crate::consume`] 经注入的 [`crate::consume::GatewayConsumer`] 真实触达网关
//! （核销票据/交换会话）；连接处理运行在 tokio 任务内，出站调用同池执行。
//!
//! 生命周期：启动前断言官方客户端未运行；[`AgentSim::stop`] 结束即释放端口。

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;

use crate::cert::{self, CertError, CertMaterial, CertOrigin};
use crate::consume::{self, GatewayConsumer, SessionStore};
use crate::contract;
use crate::probe::{self, HostProbe, OfficialClientPresent, SystemProbe};
use crate::trust::{self, CertutilTrustStore, SystemCommandRunner, TrustError, TrustStore};

/// 默认伪装网关地址（ECNU 门户 origin；`/v1/service/init`/`status` 的 `addr`）。
pub const DEFAULT_GATEWAY: &str = "https://vpn.ecnu.edu.cn";

/// 默认监听端口（门户硬编码候选的前两位）。
pub const DEFAULT_PORTS: [u16; 2] = [54630, 54631];

/// 仿真服务配置。
#[derive(Clone)]
pub struct AgentSimConfig {
    /// 伪装网关地址。
    pub gateway: String,
    /// 监听端口（默认 54630/54631）。测试可传 `0` 取临时端口。
    pub ports: Vec<u16>,
    /// 证书材料目录。
    pub cert_dir: PathBuf,
    /// 网关侧消费者（S2 核销）：生产默认 [`consume::GatewayTicketConsumer`]（直连出口），
    /// 测试注入假实现。
    pub consumer: Arc<dyn GatewayConsumer>,
}

impl Default for AgentSimConfig {
    fn default() -> Self {
        Self {
            gateway: DEFAULT_GATEWAY.to_owned(),
            ports: DEFAULT_PORTS.to_vec(),
            cert_dir: cert::default_cert_dir()
                .unwrap_or_else(|| PathBuf::from("exv-agent-sim")),
            consumer: Arc::new(consume::GatewayTicketConsumer::new()),
        }
    }
}

/// 启动/运行错误。
#[derive(Debug)]
pub enum AgentSimError {
    /// 官方客户端在运行（或端口被占）。
    OfficialClientPresent(OfficialClientPresent),
    /// 证书生成/持久化失败。
    Cert(CertError),
    /// 用户 Root 安装失败。
    Trust(TrustError),
    /// TLS 配置失败。
    Tls(String),
    /// 全部端口都无法绑定。
    Bind { ports: Vec<u16> },
}

impl std::fmt::Display for AgentSimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OfficialClientPresent(inner) => write!(f, "{inner}"),
            Self::Cert(inner) => write!(f, "{inner}"),
            Self::Trust(inner) => write!(f, "{inner}"),
            Self::Tls(message) => write!(f, "TLS 配置失败: {message}"),
            Self::Bind { ports } => write!(f, "无法绑定任何端口: {ports:?}"),
        }
    }
}

impl std::error::Error for AgentSimError {}

/// 运行中的仿真服务句柄。
pub struct AgentSim {
    shutdown: watch::Sender<bool>,
    accept_tasks: Vec<tokio::task::JoinHandle<()>>,
    bound_ports: Vec<u16>,
    ca_sha1_hex: String,
    ca_installed_now: bool,
    /// `/v1/service/start` 成功后的会话落地（S3 经 [`Self::session_snapshot`] 读取）。
    session: Arc<SessionStore>,
}

impl std::fmt::Debug for AgentSim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSim")
            .field("bound_ports", &self.bound_ports)
            .field("ca_sha1_hex", &self.ca_sha1_hex)
            .field("ca_installed_now", &self.ca_installed_now)
            .finish_non_exhaustive()
    }
}

impl AgentSim {
    /// 用系统探测（进程/端口）与系统 `certutil` 信任存储启动。
    ///
    /// # Errors
    /// 官方客户端在运行、证书/信任/TLS 失败或端口全占用时返回 [`AgentSimError`]。
    pub async fn start(config: AgentSimConfig) -> Result<Self, AgentSimError> {
        let probe = SystemProbe;
        let store = CertutilTrustStore::new(SystemCommandRunner);
        Self::start_with(config, &probe, &store).await
    }

    /// 注入探测与信任存储的启动入口（测试用）。
    ///
    /// # Errors
    /// 同 [`AgentSim::start`]。
    pub async fn start_with(
        config: AgentSimConfig,
        probe: &dyn HostProbe,
        store: &dyn TrustStore,
    ) -> Result<Self, AgentSimError> {
        probe::assert_official_client_absent(probe, &config.ports)
            .map_err(AgentSimError::OfficialClientPresent)?;

        let (material, origin) =
            cert::ensure_material(&config.cert_dir).map_err(AgentSimError::Cert)?;
        if let CertOrigin::Generated { replaced_sha1: Some(old) } = &origin {
            // 旧 CA 已不再被新叶子使用：best-effort 清理，避免用户 Root 累积孤立根。
            if let Err(error) = store.uninstall(old) {
                tracing::warn!(target: "exv.agent_sim", %error, "旧 CA 清理失败（忽略）");
            }
        }
        let tls = tls_server_config(&material)?;

        let mut listeners = Vec::new();
        for &port in &config.ports {
            match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => listeners.push(listener),
                Err(error) => {
                    tracing::warn!(target: "exv.agent_sim", port, %error, "端口绑定失败，跳过");
                }
            }
        }
        if listeners.is_empty() {
            return Err(AgentSimError::Bind { ports: config.ports.clone() });
        }

        let (shutdown, receiver) = watch::channel(false);
        let mut accept_tasks = Vec::new();
        let mut bound_ports = Vec::new();
        let session = Arc::new(SessionStore::default());
        for listener in listeners {
            let port = listener.local_addr().map(|addr| addr.port()).unwrap_or_default();
            bound_ports.push(port);
            let context = Arc::new(ServerContext {
                gateway: config.gateway.clone(),
                acceptor: TlsAcceptor::from(Arc::clone(&tls)),
                consumer: Arc::clone(&config.consumer),
                session: Arc::clone(&session),
            });
            let mut receiver = receiver.clone();
            accept_tasks.push(tokio::spawn(async move {
                run_accept_loop(listener, context, &mut receiver).await;
            }));
        }

        // 监听已就绪后再装 CA：首次会弹系统确认框（Windows 根证书保护），
        // 即使确认框未处理，端口也已开始应答，不影响已绑定的服务。
        let ca_installed_now = trust::ensure_trusted(store, &config.cert_dir, &material)
            .map_err(AgentSimError::Trust)?;

        tracing::info!(
            target: "exv.agent_sim",
            ports = ?bound_ports,
            ca_sha1 = %material.ca_sha1_hex,
            ca_installed_now,
            "L3 本机 agent 仿真已启动"
        );
        Ok(Self {
            shutdown,
            accept_tasks,
            bound_ports,
            ca_sha1_hex: material.ca_sha1_hex,
            ca_installed_now,
            session,
        })
    }

    /// 实际绑定的端口。
    #[must_use]
    pub fn bound_ports(&self) -> &[u16] {
        &self.bound_ports
    }

    /// 当前 CA 的 SHA-1 指纹（大写十六进制）。
    #[must_use]
    pub fn ca_sha1_hex(&self) -> &str {
        &self.ca_sha1_hex
    }

    /// 本次启动是否真正把 CA 装入用户 Root（否则为复用既有信任）。
    #[must_use]
    pub fn ca_installed_now(&self) -> bool {
        self.ca_installed_now
    }

    /// `/v1/service/start` 成功后的会话快照（网关 origin + `sid=…` Cookie 头；
    /// `Secret` 承载，调用方负责不落日志）。无会话返回 `None`。S3 据此可选注入
    /// WebView cookie jar。
    #[must_use]
    pub fn session_snapshot(&self) -> Option<consume::SessionSnapshot> {
        self.session.snapshot()
    }

    /// 停止服务并等待监听套接字释放。
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        for task in self.accept_tasks {
            let _ = task.await;
        }
    }
}

struct ServerContext {
    gateway: String,
    acceptor: TlsAcceptor,
    /// S2 网关侧消费者（核销/会话交换）。
    consumer: Arc<dyn GatewayConsumer>,
    /// 会话共享状态（`/v1/service/start` 成功后落地）。
    session: Arc<SessionStore>,
}

fn tls_server_config(material: &CertMaterial) -> Result<Arc<rustls::ServerConfig>, AgentSimError> {
    let certs = vec![
        CertificateDer::from(material.leaf_der.clone()),
        CertificateDer::from(material.ca_der.clone()),
    ];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(material.leaf_key_der.clone()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| AgentSimError::Tls(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| AgentSimError::Tls(e.to_string()))?;
    Ok(Arc::new(config))
}

async fn run_accept_loop(
    listener: TcpListener,
    context: Arc<ServerContext>,
    shutdown: &mut watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((tcp, _peer)) => {
                        let context = Arc::clone(&context);
                        tokio::spawn(async move {
                            if let Ok(tls) = context.acceptor.accept(tcp).await {
                                handle_connection(tls, context).await;
                            }
                        });
                    }
                    Err(error) => {
                        tracing::debug!(target: "exv.agent_sim", %error, "accept 失败");
                    }
                }
            }
        }
    }
}

/// 单连接处理：先读请求头判定 HTTP/WS，再分别处理。
async fn handle_connection<S>(mut stream: S, context: Arc<ServerContext>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buffer = Vec::new();
    loop {
        if !read_until_header_end(&mut stream, &mut buffer).await {
            return;
        }
        let header_end = find_header_end(&buffer).expect("已确认存在头部结束标记");
        let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
        let Some(parsed) = ParsedHead::parse(&head) else {
            return;
        };
        let body_end = header_end + 4 + parsed.content_length;
        while buffer.len() < body_end {
            if !read_more(&mut stream, &mut buffer).await {
                return;
            }
        }

        if parsed.is_websocket_upgrade() {
            // 把已读到的握手请求头（连同可能的额外字节）喂给 tungstenite。
            let prefixed = PrefixedIo::new(std::mem::take(&mut buffer), stream);
            handle_websocket(prefixed, &context.gateway).await;
            return;
        }

        // 请求体副本：动态端点（S2 核销）需要解析门户信封（含票据，不落日志）。
        let request_body = buffer[header_end + 4..body_end].to_vec();
        buffer.drain(..body_end);
        let response = build_http_response(
            &parsed,
            &context.gateway,
            &request_body,
            context.consumer.as_ref(),
            &context.session,
        )
        .await;
        if stream.write_all(&response.bytes).await.is_err() {
            return;
        }
        if response.close {
            return;
        }
    }
}

async fn handle_websocket<S>(stream: S, gateway: &str)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
        tracing::debug!(target: "exv.agent_sim", "WebSocket 握手失败");
        return;
    };
    while let Some(message) = socket.next().await {
        match message {
            Ok(Message::Text(text)) => {
                // 回送失败（对端已断）即结束该连接；无回复的消息直接略过。
                if let Some(reply) = contract::ws_reply(text.as_ref(), gateway)
                    && socket.send(Message::Text(reply.into())).await.is_err()
                {
                    break;
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

async fn read_until_header_end<S>(stream: &mut S, buffer: &mut Vec<u8>) -> bool
where
    S: AsyncRead + Unpin,
{
    loop {
        if find_header_end(buffer).is_some() {
            return true;
        }
        if !read_more(stream, buffer).await {
            return false;
        }
    }
}

async fn read_more<S>(stream: &mut S, buffer: &mut Vec<u8>) -> bool
where
    S: AsyncRead + Unpin,
{
    let mut chunk = [0u8; 8192];
    match stream.read(&mut chunk).await {
        Ok(0) | Err(_) => false,
        Ok(read) => {
            buffer.extend_from_slice(&chunk[..read]);
            true
        }
    }
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
        Some(Self { method, path, headers, content_length })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn is_websocket_upgrade(&self) -> bool {
        self.method.eq_ignore_ascii_case("GET")
            && self
                .header("upgrade")
                .is_some_and(|value| value.to_ascii_lowercase().contains("websocket"))
    }
}

struct HttpResponse {
    bytes: Vec<u8>,
    close: bool,
}

/// 组装 HTTP 响应：动态端点（S2 核销）先于静态契约分发；未实现端点记日志（可观测）。
async fn build_http_response(
    request: &ParsedHead,
    gateway: &str,
    body: &[u8],
    consumer: &dyn GatewayConsumer,
    session: &SessionStore,
) -> HttpResponse {
    let origin = request.header("origin").map(str::to_owned);
    let allow_origin = origin.clone().unwrap_or_else(|| "*".to_owned());
    let close = request
        .header("connection")
        .is_some_and(|value| value.to_ascii_lowercase().contains("close"));

    let (status, body) = if request.method.eq_ignore_ascii_case("OPTIONS") {
        ("204 No Content", Vec::new())
    } else {
        // S2 动态端点：真实触达网关（票据核销/会话交换），成功当且仅当网关调用成功。
        let value = match contract::path_only(&request.path) {
            "/v1/service/reportEnvBeforeLogin" => {
                consume::handle_report_env_before_login(consumer, body).await
            }
            "/v1/service/start" => consume::handle_service_start(consumer, session, body).await,
            path => {
                let value = contract::http_body(&request.path, gateway);
                if value.get("code").and_then(Value::as_i64) == Some(contract::AGENT_INNER_ERROR_CODE)
                {
                    // 可观测：显式记录未实现端点被调用（不记录请求体，防票据泄漏）。
                    tracing::warn!(
                        target: "exv.agent_sim",
                        path,
                        "未实现的 agent 端点，已显式错误应答（不吞请求）"
                    );
                }
                value
            }
        };
        ("200 OK", value.to_string().into_bytes())
    };

    let mut head = format!("HTTP/1.1 {status}\r\n");
    head.push_str(&format!("access-control-allow-origin: {allow_origin}\r\n"));
    if origin.is_some() {
        // 回显具体 Origin 时才允许携带凭据（与 `*` 互斥）。
        head.push_str("access-control-allow-credentials: true\r\n");
    }
    head.push_str("access-control-allow-methods: POST, GET, OPTIONS\r\n");
    head.push_str("access-control-allow-headers: content-type\r\n");
    // S4 CORS 补缺：Private Network Access。官方 agent 只在 OPTIONS 预检（且浏览器带
    // `Access-Control-Request-Private-Network: true` 请求头）时应答该头——证据：官方
    // aTrustAgent.exe 的 `HttpServer::preHandleMessage` 同时引用 `OPTIONS` 与
    // `Access-Control-Request-Private-Network`（二进制字符串实测），`Allow-Private-Network`
    // 字符串位于其 HttpResponse 头部写出代码。缺此头时 Chrome 对「公网站点 → 回环」的
    // 请求可能按 PNA 拦截，是 L3 的静默失败点；非预检响应不带（与官方一致）。
    if request.method.eq_ignore_ascii_case("OPTIONS")
        && request.header("access-control-request-private-network").is_some()
    {
        head.push_str("access-control-allow-private-network: true\r\n");
    }
    head.push_str("vary: origin\r\n");
    if !body.is_empty() {
        head.push_str("content-type: application/json; charset=utf-8\r\n");
        head.push_str(&format!("content-length: {}\r\n", body.len()));
    } else {
        head.push_str("content-length: 0\r\n");
    }
    head.push_str(if close { "connection: close\r\n" } else { "connection: keep-alive\r\n" });
    head.push_str("\r\n");

    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&body);
    HttpResponse { bytes, close }
}

/// 把「已预读的前缀字节」先喂给底层流（WS 握手需要重放已读请求头）。
struct PrefixedIo<S> {
    prefix: Vec<u8>,
    offset: usize,
    inner: S,
}

impl<S> PrefixedIo<S> {
    fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self { prefix, offset: 0, inner }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixedIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.offset < this.prefix.len() {
            let end = (this.offset + buf.remaining()).min(this.prefix.len());
            let chunk = this.prefix[this.offset..end].to_vec();
            buf.put_slice(&chunk);
            this.offset = end;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixedIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
