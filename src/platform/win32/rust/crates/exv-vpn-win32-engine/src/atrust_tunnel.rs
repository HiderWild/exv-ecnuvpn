//! Windows aTrust 会话 owner：协议包循环与 Wintun 原始有界队列之间的桥接。
// 由产品 assemble 的 aTrust 分支持有；真实产品流尚待验收。
// 当前接线边界见 docs/research/atrust/t07-runtime-assembly.md。

use crate::atrust_auth::AuthenticatedAtrustSession;
use exv_vpn_atrust::session::{Session, SessionFactory};
use exv_vpn_atrust::transport::l3::{
    connection::{ConnectionError, ConnectionOptions, L3Connection},
    packet_engine::{FlowIdentity, FlowLimits, PacketEngine},
    packet_session::{PacketEvent, PacketSessionError, run_packets},
};
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

/// 与 spawn_raw_packet_data_plane 的参数直接对应，队列只传原始 IP 包。
pub struct RawPacketChannels {
    pub outgoing: mpsc::Sender<Vec<u8>>,
    pub incoming: mpsc::Receiver<Vec<u8>>,
}

/// 本机生成的身份材料，不接受调用方另外覆盖已认证 SID。
pub struct NodeIdentity {
    pub sign_key: Secret,
    pub device_id: String,
    pub connection_id: String,
    pub process_hash: String,
    pub environment: Option<serde_json::Value>,
}

impl NodeIdentity {
    /// 生成本次连接材料；环境只描述当前进程，不声明设备可信或检测通过。
    /// 当前设备标识随连接生成，不代表持久设备注册。
    pub fn for_current_process() -> Result<Self, String> {
        use sha2::{Digest, Sha256};
        use zeroize::Zeroizing;
        let path = std::env::current_exe()
            .map_err(|_| "aTrust current executable unavailable".to_owned())?;
        let name = path.file_name().and_then(|name| name.to_str())
            .ok_or("aTrust executable name unavailable")?;
        let path_text = path.to_str().ok_or("aTrust executable path is not Unicode")?;
        let process_hash = Sha256::digest(path_text.as_bytes()).iter()
            .map(|byte| format!("{byte:02X}")).collect::<String>();
        let device_id = uuid::Uuid::new_v4().simple().to_string();
        let micros = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "aTrust system clock precedes epoch")?.as_micros();
        let connection_id = format!("{}-{micros}", device_id.to_ascii_uppercase());
        let mut key = Zeroizing::new([0u8; 32]);
        rustls::crypto::ring::default_provider().secure_random.fill(key.as_mut())
            .map_err(|_| "aTrust signing randomness unavailable")?;
        let sign_key = Secret::new(key.iter().map(|b| format!("{b:02X}")).collect::<String>());
        let environment = Some(serde_json::json!({"application":{"runtime":{"process":{
            "name":name,"path":path_text,"platform":"Windows","fingerprint":process_hash
        }}}}));
        Ok(Self { sign_key, device_id, connection_id, process_hash, environment })
    }
}

#[derive(Default)]
struct Notices {
    rejected: AtomicU64,
    dropped: AtomicU64,
    last: Mutex<Option<PacketEvent>>,
}

/// 一个节点组的连接 owner。不创建网卡或路由；宿主 teardown 先停网卡线程，再关闭它。
pub struct AtrustPacketTunnel {
    vip: Ipv4Addr,
    lifecycle: Session,
    worker: Option<JoinHandle<Result<(), PacketSessionError>>>,
    notices: Arc<Notices>,
    host_shutdown: Arc<AtomicBool>,
}

impl AtrustPacketTunnel {
    /// 用已验证 TLS 流及当前认证结果建立 L3；调用者负责流的 TLS 名称和证书验证。
    /// node 必须是该流实际连接的、从所选资源节点组选择的地址。
    /// # Errors
    /// 资源节点不匹配、认证 SID 缺失或 L3 握手失败时返回脱敏错误。
    pub async fn open<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        stream: S,
        authenticated: &AuthenticatedAtrustSession,
        node: &OutboundTarget,
        node_group: String,
        identity: NodeIdentity,
    ) -> Result<(Self, RawPacketChannels), String> {
        if node.purpose != ConnectPurpose::DataNode
            || !authenticated.catalog().node_groups().iter().any(|group| {
                group.id == node_group
                    && group.addresses.iter().any(|address| {
                        address.host.eq_ignore_ascii_case(&node.host) && address.port == node.port
                    })
            })
        {
            return Err("aTrust node does not belong to selected resource group".into());
        }
        let sid = Secret::new(
            authenticated
                .sid()
                .ok_or("aTrust authenticated SID unavailable")?
                .expose(),
        );
        let lifecycle = SessionFactory::new().open();
        let connection = L3Connection::open(stream, &sid, lifecycle, ConnectionOptions::default())
            .await
            .map_err(|error| format!("aTrust L3 open: {error}"))?;
        let engine = PacketEngine::new(
            connection.vip(),
            node_group,
            authenticated.catalog().clone(),
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
                notices,
                host_shutdown,
            },
            RawPacketChannels { outgoing, incoming },
        )
    }

    #[must_use]
    pub fn vip(&self) -> Ipv4Addr {
        self.vip
    }
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.lifecycle.is_closed()
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
    }
    /// 累计拒绝流/丢弃包次数；只保留最后一项原因，避免逐包诊断产生无界队列。
    #[must_use]
    pub fn notice_counts(&self) -> (u64, u64) {
        (
            self.notices.rejected.load(Ordering::Relaxed),
            self.notices.dropped.load(Ordering::Relaxed),
        )
    }
    pub fn take_last_notice(&self) -> Option<PacketEvent> {
        self.notices.last.lock().expect("aTrust notice lock").take()
    }
    /// 正常退出时先标记意图，再 stop_and_join 网卡线程，最后 close。
    /// 此方法不取消协议或关闭队列，避免仍在运行的网卡线程误报掉线。
    pub fn begin_host_shutdown(&self) {
        self.host_shutdown.store(true, Ordering::Release);
    }

    /// 显式关闭并等待协议释放连接；可重复调用。
    /// # Errors
    /// 保留协议失败的脱敏原因；worker panic/cancel 不回显任务内部内容。
    pub async fn close(&mut self) -> Result<(), String> {
        let _ = self.lifecycle.close();
        if let Some(worker) = self.worker.take() {
            match worker.await {
                Err(_) => return Err("worker_stopped".to_owned()),
                Ok(Err(error)) => return Err(packet_session_failure_category(&error).to_owned()),
                Ok(Ok(())) => {}
            }
        }
        Ok(())
    }
}

/// 将协议 worker 终止原因压缩为稳定类别；不输出底层错误正文、节点地址或认证材料。
fn packet_session_failure_category(error: &PacketSessionError) -> &'static str {
    match error {
        PacketSessionError::Connection(ConnectionError::Io(std::io::ErrorKind::UnexpectedEof)) => {
            "connection_eof"
        }
        PacketSessionError::Connection(ConnectionError::HeartbeatTimeout) => "heartbeat_timeout",
        PacketSessionError::Connection(ConnectionError::Codec(_)) => "codec",
        PacketSessionError::Connection(ConnectionError::UnexpectedDirection) => {
            "unexpected_direction"
        }
        PacketSessionError::Connection(_) => "connection_error",
        PacketSessionError::Packet(_) => "packet_processing",
        PacketSessionError::HostClosed => "host_closed",
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

// 产品 assemble 已有 aTrust 分支并接入本 owner；真实产品流尚待验收。
// 验收须覆盖真实浏览器、IPC、L3/Wintun 和路由恢复；见 docs/research/atrust/t07-runtime-assembly.md。
