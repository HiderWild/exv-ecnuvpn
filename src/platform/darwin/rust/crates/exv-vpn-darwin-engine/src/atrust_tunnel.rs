//! darwin aTrust 会话 owner（M3：win32 `atrust_tunnel.rs` 的 darwin 等价实现）。
//!
//! 职责与 win32 同构：用已验证 TLS 流 + 认证结果建立 Common
//! [`L3Connection`]（内含 L3 握手 [`open_ipv4_tunnel`]）与 [`PacketEngine`]，
//! 再以 [`run_packets`] 为协议循环、`RawPacketChannels` 为宿主原始包通道
//! （darwin 侧由 `packet::pump` 的原始包泵接 utun）。**不创建网卡或路由**；
//! 宿主 teardown 先标记停机意图（`begin_host_shutdown`），再停 utun 泵，最后
//! [`AtrustPacketTunnel::close`]。
//!
//! ## 心跳归属（设计 §6.1 修订版）
//!
//! L3 心跳由 Common `L3Connection` 内部 worker 独占：write_loop 默认5s发帧，
//! read_loop 消费应答；缺响应不主动终结，实际 I/O/EOF 经 recv/close 返回。
//! `L3Sender::send` 只收 Auth/Data 方向帧，宿主不发心跳帧。
//! 本地夹具不代表 Windows 或 macOS 连续原生业务流通过。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use exv_vpn_atrust::session::{Session, SessionFactory};
use exv_vpn_atrust::transport::l3::connection::{ConnectionError, ConnectionOptions, L3Connection};
use exv_vpn_atrust::transport::l3::packet_engine::{FlowIdentity, FlowLimits, PacketEngine};
use exv_vpn_atrust::transport::l3::packet_session::{PacketEvent, PacketSessionError, run_packets};
use exv_vpn_atrust::{ConnectPurpose, OutboundTarget, Secret};
use std::{
    net::Ipv4Addr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::JoinHandle,
};
use zeroize::Zeroizing;

/// 与原始包泵直接对应的宿主通道（只传原始 IPv4 包，4 字节 AF 头由泵剥离/添加）。
pub(crate) struct RawPacketChannels {
    /// 宿主 → 协议（utun 上行包）。
    pub outgoing: mpsc::Sender<Vec<u8>>,
    /// 协议 → 宿主（`PacketEvent::Packet` 下行包）。
    pub incoming: mpsc::Receiver<Vec<u8>>,
}

/// 本机生成的每流授权身份材料（darwin 进程事实，设计 §6.3）。
///
/// **platform 取值链**：`platform` 参数由信封字段注入（M0 证据暂定 `Mac` 的
/// 唯一来源是 core→信封→本构造；engine 生产代码零 platform 硬编码，
/// **绝不写 "Windows"**）。环境只描述当前进程，不发送 TRUSTED/安全检测声明；
/// `device_id` 每连接新 UUID（不代表持久设备注册），`connection_id` 与 win32
/// 同格式（大写 UUID simple + `-` + 微秒），`process_hash` 为 engine 可执行
/// 路径文本的 SHA-256（大写十六进制），`sign_key` 每连接随机。
pub(crate) struct NodeIdentity {
    pub sign_key: Secret,
    pub device_id: String,
    pub connection_id: String,
    pub process_hash: String,
    pub environment: Option<serde_json::Value>,
}

impl NodeIdentity {
    /// 生成本次连接材料；`platform` 来自信封（协议 platform 同源——M0 证据值）。
    ///
    /// # Errors
    ///
    /// 当前可执行路径不可读/非 Unicode 或签名随机数不可得时返回稳定错误类别
    /// （不含路径之外的系统细节）。
    pub(crate) fn for_current_process(platform: &str) -> Result<Self, &'static str> {
        use sha2::{Digest, Sha256};
        let path = std::env::current_exe().map_err(|_| "aTrust current executable unavailable")?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("aTrust executable name unavailable")?;
        let path_text = path
            .to_str()
            .ok_or("aTrust executable path is not Unicode")?;
        let process_hash =
            Sha256::digest(path_text.as_bytes())
                .iter()
                .fold(String::new(), |mut text, byte| {
                    use std::fmt::Write as _;
                    let _ = write!(text, "{byte:02X}");
                    text
                });
        let device_id = uuid::Uuid::new_v4().simple().to_string();
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "aTrust system clock precedes epoch")?
            .as_micros();
        let connection_id = format!("{}-{micros}", device_id.to_ascii_uppercase());
        let mut key = Zeroizing::new([0_u8; 32]);
        getrandom::fill(key.as_mut()).map_err(|_| "aTrust signing randomness unavailable")?;
        let sign_key = Secret::new(key.iter().fold(String::new(), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02X}");
            text
        }));
        let environment = Some(serde_json::json!({"application":{"runtime":{"process":{
            "name":name,"path":path_text,"platform":platform,"fingerprint":process_hash
        }}}}));
        Ok(Self {
            sign_key,
            device_id,
            connection_id,
            process_hash,
            environment,
        })
    }
}

#[derive(Default)]
struct Notices {
    rejected: AtomicU64,
    dropped: AtomicU64,
    last: Mutex<Option<PacketEvent>>,
}

/// 原 owner 的一次完成观察；克隆只共享原错误，不重新 join 或清理。
#[derive(Clone)]
pub(crate) struct AtrustCloseObservation {
    result: Result<(), &'static str>,
    cause: Option<Arc<PacketSessionError>>,
    // 仅来自本宿主协议 worker 的真实 JoinError，不保存 panic payload。
    join: Option<(bool, bool)>,
}

impl std::fmt::Debug for AtrustCloseObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtrustCloseObservation")
            .field("result", &self.result)
            .field("fields", &self.log_fields())
            .finish()
    }
}

impl AtrustCloseObservation {
    pub(crate) fn from_worker_result(
        result: Result<Result<(), PacketSessionError>, tokio::task::JoinError>,
    ) -> Self {
        match result {
            Ok(Ok(())) => Self {
                result: Ok(()),
                cause: None,
                join: None,
            },
            Ok(Err(error)) => Self {
                result: Err(packet_session_failure_category(&error)),
                cause: Some(Arc::new(error)),
                join: None,
            },
            Err(error) => Self {
                result: Err("worker_stopped"),
                cause: None,
                join: Some((error.is_cancelled(), error.is_panic())),
            },
        }
    }

    pub(crate) fn result(&self) -> Result<(), &'static str> {
        self.result
    }

    pub(crate) fn raw_os_error(&self) -> Option<i32> {
        match self.cause.as_deref() {
            Some(PacketSessionError::Connection(ConnectionError::Io { raw_os_error, .. })) => {
                *raw_os_error
            }
            _ => None,
        }
    }

    /// 只消费 E4 已处理的 I/O 原因；其他协议错误只保稳定类别。
    pub(crate) fn log_fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = Vec::new();
        if let Err(category) = self.result {
            fields.push(("category", category.to_owned()));
        }
        if let Some(PacketSessionError::Connection(ConnectionError::Io {
            stage,
            kind,
            raw_os_error,
            detail,
        })) = self.cause.as_deref()
        {
            fields.extend([
                ("stage", (*stage).to_owned()),
                ("io_kind", format!("{kind:?}")),
                ("raw_os_error", format!("{raw_os_error:?}")),
                ("detail", detail.clone()),
            ]);
        }
        if let Some((cancelled, panicked)) = self.join {
            fields.extend([
                ("worker_cancelled", cancelled.to_string()),
                ("worker_panicked", panicked.to_string()),
            ]);
        }
        fields
    }
}

/// 一个节点组的连接 owner。不创建网卡或路由；宿主 teardown 先停 utun 泵线程，
/// 再关闭它。
pub(crate) struct AtrustPacketTunnel {
    vip: Ipv4Addr,
    lifecycle: Session,
    worker: Option<JoinHandle<Result<(), PacketSessionError>>>,
    completion: Option<AtrustCloseObservation>,
    notices: Arc<Notices>,
    host_shutdown: Arc<AtomicBool>,
}

impl AtrustPacketTunnel {
    /// 用已验证 TLS 流及当前认证结果建立 L3；调用者负责流的 TLS 名称和证书
    /// 验证（[`crate::atrust_node_tls`]）。`node` 必须是该流实际连接的、从所选
    /// 资源节点组选择的地址（`sid` 为 None 时 typed 拒绝——认证产物缺 SID 属
    /// 调用方契约失败）。
    ///
    /// # Errors
    ///
    /// 资源节点不匹配、认证 SID 缺失或 L3 握手失败时返回脱敏错误（Common
    /// `ConnectionError` 的 Display 保留原阶段/kind、可选系统码与脱敏原因，不含 SID 或响应正文）。
    pub(crate) async fn open_with_catalog<S>(
        stream: S,
        catalog: &exv_vpn_atrust::ResourceCatalog,
        sid: Option<&Secret>,
        node: &OutboundTarget,
        node_group: String,
        identity: NodeIdentity,
    ) -> Result<(Self, RawPacketChannels), AtrustOpenError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        if node.purpose != ConnectPurpose::DataNode
            || !catalog.node_groups().iter().any(|group| {
                group.id == node_group
                    && group.addresses.iter().any(|address| {
                        address.host.eq_ignore_ascii_case(&node.host) && address.port == node.port
                    })
            })
        {
            return Err(AtrustOpenError::NodeGroupMismatch);
        }
        let Some(authenticated_sid) = sid else {
            return Err(AtrustOpenError::SidUnavailable);
        };
        let sid = Secret::new(authenticated_sid.expose());
        let lifecycle = SessionFactory::new().open();
        let connection = L3Connection::open(stream, &sid, lifecycle, ConnectionOptions::default())
            .await
            .map_err(AtrustOpenError::L3)?;
        let engine = PacketEngine::new(
            connection.vip(),
            node_group,
            catalog.clone(),
            FlowIdentity {
                sid,
                sign_key: identity.sign_key,
                device_id: identity.device_id,
                connection_id: identity.connection_id,
                process_hash: identity.process_hash,
                environment: identity.environment,
            },
            FlowLimits::default(),
        );
        Ok(Self::start(connection, engine, 64))
    }

    fn start(
        connection: L3Connection,
        engine: PacketEngine,
        capacity: usize,
    ) -> (Self, RawPacketChannels) {
        let vip = connection.vip();
        let lifecycle = connection.session();
        let cancellation = lifecycle.clone();
        let notices = Arc::new(Notices::default());
        let observations = notices.clone();
        let host_shutdown = Arc::new(AtomicBool::new(false));
        let stopping = host_shutdown.clone();
        let (outgoing, packets) = mpsc::channel(capacity);
        let (host_tx, incoming) = mpsc::channel(capacity);
        let (events_tx, mut events) = mpsc::channel(capacity);
        let worker = tokio::spawn(async move {
            let protocol = run_packets(connection, engine, packets, events_tx);
            tokio::pin!(protocol);
            let relay = async {
                while let Some(event) = events.recv().await {
                    match event {
                        PacketEvent::Packet(packet) => {
                            if host_tx.send(packet).await.is_err() {
                                break;
                            }
                        }
                        notice => {
                            match &notice {
                                PacketEvent::Rejected(_) => {
                                    observations.rejected.fetch_add(1, Ordering::Relaxed);
                                }
                                PacketEvent::Dropped(_) => {
                                    observations.dropped.fetch_add(1, Ordering::Relaxed);
                                }
                                PacketEvent::Packet(_) => unreachable!(),
                            }
                            *observations.last.lock().expect("aTrust notice lock") = Some(notice);
                        }
                    }
                }
            };
            tokio::select! {
                biased;
                result = &mut protocol => result,
                () = cancellation.cancelled() => protocol.await,
                () = host_tx.closed() => {
                    let _ = cancellation.close();
                    protocol.await?;
                    if stopping.load(Ordering::Acquire) { Ok(()) }
                    else { Err(PacketSessionError::HostClosed) }
                }
                () = relay => {
                    let _ = cancellation.close();
                    protocol.await?;
                    if stopping.load(Ordering::Acquire) { Ok(()) }
                    else { Err(PacketSessionError::HostClosed) }
                }
            }
        });
        (
            Self {
                vip,
                lifecycle,
                worker: Some(worker),
                completion: None,
                notices,
                host_shutdown,
            },
            RawPacketChannels { outgoing, incoming },
        )
    }

    #[must_use]
    pub(crate) const fn vip(&self) -> Ipv4Addr {
        self.vip
    }

    #[must_use]
    pub(crate) fn is_running(&self) -> bool {
        !self.lifecycle.is_closed()
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
    }

    /// 累计拒绝流/丢弃包次数；只保留最后一项原因，避免逐包诊断产生无界队列。
    #[must_use]
    pub(crate) fn notice_counts(&self) -> (u64, u64) {
        (
            self.notices.rejected.load(Ordering::Relaxed),
            self.notices.dropped.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn take_last_notice(&self) -> Option<PacketEvent> {
        self.notices.last.lock().expect("aTrust notice lock").take()
    }

    /// 正常退出时先标记意图，再停 utun 泵线程，最后 close。
    /// 此方法不取消协议或关闭队列，避免仍在运行的泵线程误报掉线。
    pub(crate) fn begin_host_shutdown(&self) {
        self.host_shutdown.store(true, Ordering::Release);
    }

    /// 隔离验收调用：故障作用于当前真实协议任务，原 join 观察链负责上报。
    pub(crate) fn fail_transport_for_acceptance(&self) {
        if let Some(worker) = &self.worker { worker.abort(); }
    }

    /// 兼容入口：稳定类别来自同一缓存，不重复 join。
    pub(crate) async fn close(&mut self) -> Result<(), &'static str> {
        self.close_observed().await.result()
    }

    /// 等待者取消不会丢失句柄；完成后重复调用共享同一原错误。
    pub(crate) async fn close_observed(&mut self) -> AtrustCloseObservation {
        let _ = self.lifecycle.close();
        if let Some(completion) = &self.completion {
            return completion.clone();
        }
        let result = match self.worker.as_mut() {
            Some(worker) => worker.await,
            None => Ok(Ok(())),
        };
        let completion = AtrustCloseObservation::from_worker_result(result);
        self.worker.take();
        self.completion = Some(completion.clone());
        completion
    }
}

/// `AtrustPacketTunnel::open` 的 typed 失败（稳定 reason 码，秘密零输出）。
#[derive(Debug)]
pub(crate) enum AtrustOpenError {
    /// 节点不属于所选资源节点组（调用方契约失败）。
    NodeGroupMismatch,
    /// 认证产物缺 SID。
    SidUnavailable,
    /// Common L3 握手/连接建立失败（原阶段/kind、可选系统码与必要原因已脱敏）。
    L3(ConnectionError),
}

impl AtrustOpenError {
    /// 稳定 reason 码（进日志/trace）。
    #[must_use]
    pub(crate) const fn reason(&self) -> &'static str {
        match self {
            Self::NodeGroupMismatch => "DARWIN_ATRUST_NODE_GROUP_MISMATCH",
            Self::SidUnavailable => "DARWIN_ATRUST_SID_UNAVAILABLE",
            Self::L3(_) => "DARWIN_ATRUST_L3_HANDSHAKE_FAILED",
        }
    }
}

impl std::fmt::Display for AtrustOpenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::L3(error) => write!(formatter, "{}: {error}", self.reason()),
            _ => formatter.write_str(self.reason()),
        }
    }
}

impl std::error::Error for AtrustOpenError {}

/// 将协议 worker 终止原因压缩为稳定类别（win32 同词表）；不输出底层错误正文、
/// 节点地址或认证材料。darwin 侧前缀 `atrust_` 以与 CSTP 会话失联类别区分
/// （trace/日志稳定 reason）。
pub(crate) fn packet_session_failure_category(error: &PacketSessionError) -> &'static str {
    match error {
        PacketSessionError::Connection(ConnectionError::Io {
            kind: std::io::ErrorKind::UnexpectedEof,
            ..
        }) => "atrust_connection_eof",
        PacketSessionError::Connection(ConnectionError::HeartbeatTimeout) => {
            "atrust_heartbeat_timeout"
        }
        PacketSessionError::Connection(ConnectionError::Codec(_)) => "atrust_codec",
        PacketSessionError::Connection(ConnectionError::UnexpectedDirection) => {
            "atrust_unexpected_direction"
        }
        PacketSessionError::Connection(_) => "atrust_connection_error",
        PacketSessionError::Packet(_) => "atrust_packet_processing",
        PacketSessionError::HostClosed => "atrust_host_closed",
    }
}

impl Drop for AtrustPacketTunnel {
    fn drop(&mut self) {
        let _ = self.lifecycle.close();
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}
