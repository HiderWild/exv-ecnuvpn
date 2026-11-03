//! Windows Engine 认证交接回放。仅诊断入口，无网卡、路由或官方客户端操作。
//! stdin 沿用受保护采集驱动的 Cookie/UA；显式选择物理接口或回环代理，不接受命令行凭据。
use exv_engine::atrust_auth::authenticate_browser_session;
use exv_vpn_atrust::{
    ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay, UnderlayError,
    UnderlayErrorKind,
};
use exv_vpn_win32_ipc::browser_session::BrowserSession;
use std::{io::Read, net::SocketAddr, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};
#[path = "support/atrust_node_replay.rs"]
mod node_replay;

#[derive(serde::Deserialize, Zeroize, ZeroizeOnDrop)]
struct Input {
    cookie_header: String,
    user_agent: String,
    #[zeroize(skip)]
    proxy: SocketAddr,
    #[serde(default)]
    #[zeroize(skip)]
    physical_ifindex: Option<u32>,
    #[serde(default)]
    #[zeroize(skip)]
    node_probe: Option<node_replay::NodeOptions>,
}
struct Proxy(SocketAddr);
impl Underlay for Proxy {
    fn resolve(&self, _: &str, _: u16) -> ResolveFuture<'_> {
        Box::pin(async {
            Err(UnderlayError::new(
                UnderlayErrorKind::ResolveFailed,
                ConnectPurpose::AuthGateway,
            ))
        })
    }
    fn connect(&self, target: &OutboundTarget) -> ConnectFuture<'_> {
        let allowed = target.host == "vpn.ecnu.edu.cn"
            && target.port == 443
            && target.purpose != ConnectPurpose::DataNode;
        let purpose = target.purpose;
        Box::pin(async move {
            let failed = || UnderlayError::new(UnderlayErrorKind::ConnectFailed, purpose);
            if !allowed || !self.0.ip().is_loopback() {
                return Err(failed());
            }
            let mut socket = tokio::net::TcpStream::connect(self.0)
                .await
                .map_err(|_| failed())?;
            socket
                .write_all(
                    b"CONNECT vpn.ecnu.edu.cn:443 HTTP/1.1\r\nHost: vpn.ecnu.edu.cn:443\r\n\r\n",
                )
                .await
                .map_err(|_| failed())?;
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                if header.len() >= 4096 {
                    return Err(failed());
                }
                header.push(socket.read_u8().await.map_err(|_| failed())?);
            }
            let line = std::str::from_utf8(&header)
                .map_err(|_| failed())?
                .lines()
                .next()
                .ok_or_else(failed)?;
            let mut parts = line.split_ascii_whitespace();
            if !matches!(parts.next(), Some("HTTP/1.1" | "HTTP/1.0")) || parts.next() != Some("200")
            {
                return Err(failed());
            }
            Ok(Box::new(socket) as Box<dyn exv_vpn_atrust::transport::UnderlayStream>)
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::stdin()
        .take(131_073)
        .read_to_end(&mut bytes)
        .map_err(|_| "stdin-read")?;
    if bytes.len() > 131_072 {
        return Err("stdin-too-large".into());
    }
    let mut input: Input = serde_json::from_slice(&bytes).map_err(|_| "stdin-json")?;
    drop(bytes);
    let browser = BrowserSession::new(
        "https://vpn.ecnu.edu.cn".into(),
        std::mem::take(&mut input.cookie_header),
        std::mem::take(&mut input.user_agent),
    )
    .map_err(|e| e.to_string())?;
    // 实际编码/解码新的 IPC 载荷，验证接收对象随后能被服务器接受。
    let wire = browser.encode().map_err(|e| e.to_string())?;
    drop(browser);
    let received = BrowserSession::decode(&wire).map_err(|e| e.to_string())?;
    drop(wire);
    let underlay: Arc<dyn Underlay> = match input.physical_ifindex {
        Some(ifindex) => Arc::new(exv_engine::atrust_underlay::PhysicalUnderlay::new(ifindex)?),
        None => Arc::new(Proxy(input.proxy)),
    };
    let session =
        authenticate_browser_session(received, "https://vpn.ecnu.edu.cn", underlay.clone())
            .await
            .map_err(|e| e.to_string())?;
    println!("{{\"stage\":\"saved-session\",\"is_login\":true}}");
    println!(
        "{{\"stage\":\"native-ticket-exchange\",\"initial_is_login\":false,\"final_is_login\":true}}"
    );
    println!(
        "{{\"stage\":\"gateway-resource-only\",\"bytes\":{},\"apps\":{},\"entries\":{},\"skipped\":{},\"node_groups\":{}}}",
        session.resource_bytes(),
        session.catalog().apps().len(),
        session.catalog().entry_count(),
        session.catalog().skipped_count(),
        session.catalog().node_groups().len()
    );
    if let Some(options) = input.node_probe.take() {
        node_replay::run(options, &session, underlay.as_ref()).await?;
    }
    Ok(())
}
