
//! 生产 VGDC 双线可信网关解析（DoH 直连 → 绑物理网卡 UDP/53 兜底）——PRD G-⑤ / C4。
//!
//! 本模块是 VGDC 双线 DNS 的**生产**实现，自 acceptance `direct_connect.rs` 提升而来
//! （`提升复用`，不 fork 两份；acceptance 侧改为薄重导出）。engine 解析 VPN 服务器
//! 域名时经 [`resolve_gateway_dual_line`] 走直连，不经 Mihomo fake-ip DNS 劫持：
//!
//! - **L1 DoH（默认）**：HTTPS 查询阿里 `https://223.5.5.5/resolve` → Cloudflare
//!   `https://1.1.1.1/dns-query`，走 proxy-safe TLS wiring（rustls +
//!   rustls-platform-verifier；测试注入 TestRoots）。查询路径不被 Mihomo fake-ip
//!   DNS 劫持。
//! - **L2 UDP/53 绑物理网卡**：UDP 查询公共 resolver（1.1.1.1 / 8.8.8.8 /
//!   114.114.114.114），socket 以 `IP_UNICAST_IF` 绑物理网卡 ifindex（不经 Mihomo TUN
//!   默认路由）。
//! - **fake-ip 过滤**：198.18.0.0/15（RFC 2544 / Clash fake-ip 池）在每条线路内过滤。
//! - **逐层 typed 失败**：一条线路失败 → 记录 typed 错误 → 下一条线路；全部失败 →
//!   [`ResolveLayerError`] 向量（证据记录 `describe()`）。
//!
//! 与 C3（`exv-vpn-cstp` connector `socket_binder` seam）衔接：本模块的
//! [`resolve_gateway_dual_line`] 产出真实物理出口 IP 供 CSTP connector 连接；
//! [`socket_binder_for_ifindex`] 供给 `BootstrapConfig.socket_binder`（IP_UNICAST_IF
//! 绑物理网卡，控制面连接不走 Mihomo 默认路由）。生产装配点（engine 拉起后、connect
//! 前）见 `exv-engine::vgdc_connect`。
//!
//! 与 acceptance 的差异：`resolve_gateway_dual_line` 改为 **async**（tokio 原生，DoH
//! 直接 await，不 block_on 外部 runtime）；acceptance 侧提供同步 shim 兼容既有调用面。

use std::net::{Ipv4Addr, SocketAddr};
use std::os::windows::io::AsRawSocket;
use std::sync::Arc;

use exv_vpn_cstp::connector::SocketBinder;
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{
    setsockopt, AF_INET, IPPROTO_IP, IP_UNICAST_IF, SOCKET, SOCKET_ADDRESS, SOCKADDR_INET,
};

/// 每层解析 deadline（spec VGDC-05：全程有 deadline，不再出现无限挂起）。
const LAYER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
/// UDP/53 单次查询读写超时（proxy_safe_resolver 同款 2s）。
const DNS_UDP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// DNS 查询 ID（proxy_safe_resolver 同款冻结值）。
const DNS_QUERY_ID: [u8; 2] = [0x12, 0x34];
/// `IF_TYPE_ETHERNET_CSMACD`（物理以太网）。
const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
/// `IF_TYPE_IEEE80211`（物理 Wi-Fi）。
const IF_TYPE_IEEE80211: u32 = 71;

/// 物理网卡发现结果（`GetAdaptersAddresses`；UDP/53 绑网卡 + /32 路由共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NicInfo {
    /// 网卡 LUID（`NET_LUID_LH.Value`）。
    pub luid: u64,
    /// 网卡 ifIndex（`IP_UNICAST_IF` 绑网卡 + 路由行共用）。
    pub ifindex: u32,
    /// 网卡网关（`/32` 路由的 next-hop）。
    pub gateway: Ipv4Addr,
    /// 网卡 IPv4 单播地址（源地址选择；发现辅助判据）。
    pub local_ip: Ipv4Addr,
    /// 友好名（人类可读；排除虚拟/隧道适配器用）。
    pub friendly_name: String,
    /// 网卡 IPv4 接口度量（候选排序：越小越优先）。
    pub ipv4_metric: u32,
}

/// 解析来源（证据 `resolution_source`：`"doh"` / `"udp53"` / `"system"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    /// gateway host 本身就是 IPv4 字面量（无需 DNS；fake-ip 过滤后直接采用）。
    System,
    /// DoH 线路成功（记录 endpoint）。
    Doh(Ipv4Addr),
    /// 绑物理网卡 UDP/53 线路成功（记录 resolver）。
    Udp53(Ipv4Addr),
}

impl ResolutionSource {
    /// 证据值：`"system"` / `"doh"` / `"udp53"`。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Doh(_) => "doh",
            Self::Udp53(_) => "udp53",
        }
    }
}

/// DoH 线路 typed 错误（逐层记录；不 panic）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DohErrorKind {
    /// TCP 连接失败（端点不可达）。
    ConnectFailed(String),
    /// TLS 握手失败（证书链/网络）。
    TlsFailed(String),
    /// HTTP 状态非 200。
    HttpStatus(u16),
    /// DoH JSON `Status` 非 0（DNS 层失败）。
    DnsStatus(u64),
    /// 响应无法解析（非 HTTP / 非 JSON / 缺字段）。
    BadResponse(String),
    /// 过滤 fake-ip 后无 IPv4 结果。
    NoIpv4Result,
    /// deadline 超时。
    DeadlineExceeded,
    /// 请求构造失败（非法 host / server name）。
    BadRequest(String),
}

impl DohErrorKind {
    /// 证据 predicate 片段（`doh:<endpoint>:<片段>`）。
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::ConnectFailed(e) => format!("connect-failed:{e}"),
            Self::TlsFailed(e) => format!("tls-failed:{e}"),
            Self::HttpStatus(code) => format!("http-status-{code}"),
            Self::DnsStatus(code) => format!("dns-status-{code}"),
            Self::BadResponse(detail) => format!("bad-response:{detail}"),
            Self::NoIpv4Result => "no-ipv4-result".to_string(),
            Self::DeadlineExceeded => "deadline".to_string(),
            Self::BadRequest(detail) => format!("bad-request:{detail}"),
        }
    }
}

/// UDP/53 线路 typed 错误（逐层记录；不 panic）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Udp53ErrorKind {
    /// 发送失败（socket/网卡绑定的系统错误）。
    QueryFailed(String),
    /// 响应无法解析（短包 / ID 不匹配 / 非响应 / 截断）。
    BadResponse(String),
    /// DNS RCODE 非 0。
    DnsRcode(u8),
    /// 过滤 fake-ip 后无 IPv4 结果。
    NoIpv4Result,
    /// 读取超时（deadline 内无响应）。
    Timeout,
    /// 查询构造失败（非法 host：空 label / label > 63）。
    BadQuery(String),
}

impl Udp53ErrorKind {
    /// 证据 predicate 片段（`udp53:<resolver>:<片段>`）。
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::QueryFailed(e) => format!("query-failed:{e}"),
            Self::BadResponse(detail) => format!("bad-response:{detail}"),
            Self::DnsRcode(code) => format!("dns-rcode-{code}"),
            Self::NoIpv4Result => "no-ipv4-result".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::BadQuery(detail) => format!("bad-query:{detail}"),
        }
    }
}

/// 单层解析错误（L1 DoH 或 L2 UDP/53；证据逐层记录）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveLayerError {
    /// DoH 线路失败。
    Doh {
        /// 失败 endpoint。
        endpoint: Ipv4Addr,
        /// typed 错误。
        kind: DohErrorKind,
    },
    /// UDP/53 线路失败。
    Udp53 {
        /// 失败 resolver。
        resolver: Ipv4Addr,
        /// typed 错误。
        kind: Udp53ErrorKind,
    },
    /// gateway host 是 fake-ip 字面量（198.18.0.0/15）——直接拒绝。
    FakeIpLiteral(Ipv4Addr),
}

impl ResolveLayerError {
    /// 证据 predicate 片段（`doh:223.5.5.5:tls-failed:...` / `udp53:1.1.1.1:timeout`）。
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Doh { endpoint, kind } => format!("doh:{endpoint}:{}", kind.describe()),
            Self::Udp53 { resolver, kind } => format!("udp53:{resolver}:{}", kind.describe()),
            Self::FakeIpLiteral(ip) => format!("fake-ip-literal:{ip}"),
        }
    }
}

/// DoH endpoint 配置（endpoint IP + 查询路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DohEndpoint {
    /// DoH 服务器 IP（TCP 443）。
    pub ip: Ipv4Addr,
    /// JSON API 路径（阿里 `/resolve`；Cloudflare `/dns-query`）。
    pub path: &'static str,
}

/// 双线解析配置（可注入——机制测试用假 endpoint/假 verifier；生产用
/// [`DualLineConfig::production`]）。
#[derive(Debug, Clone)]
pub struct DualLineConfig {
    /// DoH endpoint 列表（按优先级；一条失败 → 下一条）。
    pub doh_endpoints: Vec<DohEndpoint>,
    /// TLS 客户端配置（生产 = rustls-platform-verifier；测试 = 注入 TestRoots）。
    pub doh_client_config: std::sync::Arc<rustls::ClientConfig>,
    /// UDP/53 resolver 列表（按优先级；proxy_safe_resolver 同款三公共 resolver）。
    pub udp53_resolvers: Vec<SocketAddr>,
    /// 绑物理网卡 ifindex（None = 不绑——仅测试假 socket 使用；生产恒为 Some）。
    pub udp53_ifindex: Option<u32>,
}

impl DualLineConfig {
    /// 生产配置：DoH `223.5.5.5/resolve` → `1.1.1.1/dns-query`（默认线路），
    /// UDP/53 `1.1.1.1` → `8.8.8.8` → `114.114.114.114`（fallback 线路，绑物理网卡）。
    ///
    /// # Errors
    ///
    /// rustls-platform-verifier 初始化失败 → `String` 描述。
    pub fn production(ifindex: Option<u32>) -> Result<Self, String> {
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
            .map_err(|_| "platform-verifier".to_string())?;
        let client_cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| "client-config".to_string())?
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(verifier))
            .with_no_client_auth();
        Ok(Self {
            doh_endpoints: vec![
                DohEndpoint {
                    ip: Ipv4Addr::new(223, 5, 5, 5),
                    path: "/resolve",
                },
                DohEndpoint {
                    ip: Ipv4Addr::new(1, 1, 1, 1),
                    path: "/dns-query",
                },
            ],
            doh_client_config: std::sync::Arc::new(client_cfg),
            udp53_resolvers: vec![
                SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 53)),
                SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 53)),
                SocketAddr::from((Ipv4Addr::new(114, 114, 114, 114), 53)),
            ],
            udp53_ifindex: ifindex,
        })
    }
}

/// fake-ip 检测：198.18.0.0/15（RFC 2544 / Clash fake-ip 池；proxy_safe_resolver
/// `is_fake_ip_v4` 同款语义）。
#[must_use]
pub fn is_fake_ip_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

// ---------------------------------------------------------------------------
// L1：DoH（HTTPS；proxy-safe TLS wiring——school.rs / controlled.rs 同款）。
// ---------------------------------------------------------------------------

/// 对单个 DoH endpoint 执行 `name=A` JSON 查询；fake-ip 结果过滤。
///
/// `endpoint` 是 TCP 目标（生产 `(ip, 443)`；测试指向假 HTTPS 服务器）；
/// `server_name` 是 TLS SNI + 证书校验名（生产 = endpoint IP；测试 = 注入根 SAN）。
///
/// # Errors
///
/// [`DohErrorKind`]（typed；connect/TLS/HTTP/DNS 层逐层分类）。
pub async fn doh_query(
    client_config: std::sync::Arc<rustls::ClientConfig>,
    endpoint: SocketAddr,
    server_name: String,
    path: &str,
    host: &str,
) -> Result<Ipv4Addr, DohErrorKind> {
    let connector = tokio_rustls::TlsConnector::from(client_config);
    let server_name = rustls::pki_types::ServerName::try_from(server_name)
        .map_err(|_| DohErrorKind::BadRequest("server-name".to_string()))?;
    let tcp = tokio::time::timeout(LAYER_DEADLINE, tokio::net::TcpStream::connect(endpoint))
        .await
        .map_err(|_| DohErrorKind::DeadlineExceeded)?
        .map_err(|e| DohErrorKind::ConnectFailed(e.to_string()))?;
    let mut stream = tokio::time::timeout(LAYER_DEADLINE, connector.connect(server_name, tcp))
        .await
        .map_err(|_| DohErrorKind::DeadlineExceeded)?
        .map_err(|e| DohErrorKind::TlsFailed(e.to_string()))?;
    let query = format!(
        "GET {path}?name={}&type=A HTTP/1.1\r\nHost: {}\r\nAccept: application/dns-json\r\nConnection: close\r\n\r\n",
        url_encode_host(host),
        endpoint.ip()
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, query.as_bytes())
        .await
        .map_err(|e| DohErrorKind::ConnectFailed(e.to_string()))?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = tokio::time::timeout(
            LAYER_DEADLINE,
            tokio::io::AsyncReadExt::read(&mut stream, &mut chunk),
        )
        .await
        .map_err(|_| DohErrorKind::DeadlineExceeded)?
        .map_err(|e| DohErrorKind::TlsFailed(e.to_string()))?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    parse_doh_response(&raw)
}

/// DoH JSON 响应解析（HTTP 状态 + `Status` + `Answer[]` 过滤 fake-ip 后的首个 A 记录）。
///
/// # Errors
///
/// [`DohErrorKind`]（HTTP 层 / DNS 层 / 无 IPv4 结果）。
fn parse_doh_response(raw: &[u8]) -> Result<Ipv4Addr, DohErrorKind> {
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| DohErrorKind::BadResponse("no-http-header-end".to_string()))?;
    let head = String::from_utf8_lossy(&raw[..header_end]);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| DohErrorKind::BadResponse("no-status-line".to_string()))?;
    if status != 200 {
        return Err(DohErrorKind::HttpStatus(status));
    }
    let body = &raw[header_end + 4..];
    let json: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| DohErrorKind::BadResponse(format!("json:{e}")))?;
    let dns_status = json
        .get("Status")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| DohErrorKind::BadResponse("no-status".to_string()))?;
    if dns_status != 0 {
        return Err(DohErrorKind::DnsStatus(dns_status));
    }
    let answers = json
        .get("Answer")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| DohErrorKind::BadResponse("no-answer".to_string()))?;
    for entry in answers {
        let is_a = entry.get("type").and_then(serde_json::Value::as_u64) == Some(1);
        if !is_a {
            continue;
        }
        let data = entry
            .get("data")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if let Ok(ip) = data.parse::<Ipv4Addr>()
            && !is_fake_ip_v4(ip)
        {
            return Ok(ip);
        }
    }
    Err(DohErrorKind::NoIpv4Result)
}

/// URL 查询参数编码（host 只允许 `[a-zA-Z0-9.-]`；其余字节 `%XX`）。
#[must_use]
fn url_encode_host(host: &str) -> String {
    let mut out = String::with_capacity(host.len());
    for b in host.bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// L2：UDP/53 绑物理网卡（proxy_safe_resolver `dns_a_query_bound` 的 Rust 移植；
// 绑网卡 = `IP_UNICAST_IF` setsockopt，Windows 语义等价物）。
// ---------------------------------------------------------------------------

/// 构造 DNS A 查询（id `0x1234`、RD=1、qdcount=1；proxy_safe_resolver 同款字节布局）。
///
/// # Errors
///
/// 空 label / label > 63 → [`Udp53ErrorKind::BadQuery`]。
fn build_dns_query(host: &str) -> Result<Vec<u8>, Udp53ErrorKind> {
    let mut q = Vec::with_capacity(host.len() + 16);
    q.extend_from_slice(&DNS_QUERY_ID);
    q.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(Udp53ErrorKind::BadQuery(format!("label:{label}")));
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // type A, class IN
    Ok(q)
}

/// 跳过 DNS name（普通标签与压缩指针均支持；返回 name 之后的偏移）。
///
/// 语义与 proxy_safe_resolver 的 `skip_name` 一致：遇到压缩指针（`0xC0`）即认为
/// name 结束于指针之后（answer 记录边界在指针之后；question name 不会压缩）。
fn skip_name(resp: &[u8], mut off: usize) -> Result<usize, Udp53ErrorKind> {
    loop {
        let Some(&lab) = resp.get(off) else {
            return Err(Udp53ErrorKind::BadResponse("name-eof".to_string()));
        };
        if lab == 0 {
            return Ok(off + 1);
        }
        if lab & 0xC0 == 0xC0 {
            if off + 2 > resp.len() {
                return Err(Udp53ErrorKind::BadResponse("ptr-eof".to_string()));
            }
            return Ok(off + 2);
        }
        if lab > 63 {
            return Err(Udp53ErrorKind::BadResponse("label-too-long".to_string()));
        }
        off += 1 + usize::from(lab);
        if off > resp.len() {
            return Err(Udp53ErrorKind::BadResponse("label-eof".to_string()));
        }
    }
}

/// 解析 DNS 响应：ID/QR/RCODE 校验 + question 跳过 + Answer 的 A 记录（fake-ip 过滤）。
///
/// # Errors
///
/// 响应非法（短包 / ID 不匹配 / 非响应 / 截断 / 非零 RCODE）或过滤后无 IPv4 →
/// [`Udp53ErrorKind`]。
fn parse_dns_response(resp: &[u8], query_id: &[u8; 2]) -> Result<Vec<Ipv4Addr>, Udp53ErrorKind> {
    if resp.len() < 12 {
        return Err(Udp53ErrorKind::BadResponse("short".to_string()));
    }
    if &resp[..2] != query_id {
        return Err(Udp53ErrorKind::BadResponse("id-mismatch".to_string()));
    }
    if resp[2] & 0x80 == 0 {
        return Err(Udp53ErrorKind::BadResponse("not-response".to_string()));
    }
    let rcode = resp[3] & 0x0F;
    if rcode != 0 {
        return Err(Udp53ErrorKind::DnsRcode(rcode));
    }
    let qdcount = u16::from_be_bytes([resp[4], resp[5]]);
    let ancount = u16::from_be_bytes([resp[6], resp[7]]);
    if qdcount == 0 {
        return Err(Udp53ErrorKind::BadResponse("no-question".to_string()));
    }
    let mut off = 12usize;
    for _ in 0..qdcount {
        off = skip_name(resp, off)?;
        off += 4; // question type + class
    }
    let mut out = Vec::new();
    for _ in 0..ancount {
        off = skip_name(resp, off)?;
        if off + 10 > resp.len() {
            return Err(Udp53ErrorKind::BadResponse("answer-truncated".to_string()));
        }
        let atype = u16::from_be_bytes([resp[off], resp[off + 1]]);
        let rdlen = u16::from_be_bytes([resp[off + 8], resp[off + 9]]);
        off += 10;
        if off + usize::from(rdlen) > resp.len() {
            return Err(Udp53ErrorKind::BadResponse("rdlen-eof".to_string()));
        }
        if atype == 1 && rdlen == 4 {
            let ip = Ipv4Addr::new(resp[off], resp[off + 1], resp[off + 2], resp[off + 3]);
            if !is_fake_ip_v4(ip) && !out.contains(&ip) {
                out.push(ip);
            }
        }
        off += usize::from(rdlen);
    }
    if out.is_empty() {
        return Err(Udp53ErrorKind::NoIpv4Result);
    }
    Ok(out)
}

/// 绑物理网卡 UDP/53 A 查询（`proxy_safe_resolver::dns_a_query_bound` 的 Win32 移植）。
///
/// `ifindex` 为 `Some` 时以 `IP_UNICAST_IF` 强制出口为物理网卡（绕过 Mihomo TUN 的
/// 0.0.0.0/0 默认路由）；`None` 不绑（仅测试假 socket 使用）。生产 `resolver` 端口
/// 恒为 53；测试可注入任意端口。
///
/// # Errors
///
/// 查询构造 / 发送 / 读取 / 解析失败 → [`Udp53ErrorKind`]（typed；不 panic）。
pub fn dns_a_query_udp(
    host: &str,
    resolver: SocketAddr,
    ifindex: Option<u32>,
) -> Result<Vec<Ipv4Addr>, Udp53ErrorKind> {
    let query = build_dns_query(host)?;
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| Udp53ErrorKind::QueryFailed(format!("bind:{e}")))?;
    if let Some(index) = ifindex {
        // SAFETY: sock 句柄有效；IP_UNICAST_IF 的 optval 是网卡索引的网络字节序
        // DWORD（Windows 文档：IPv4 的索引值必须以网络字节序存储）。
        let rc = unsafe {
            setsockopt(
                SOCKET(sock.as_raw_socket() as usize),
                IPPROTO_IP.0,
                IP_UNICAST_IF,
                Some(&index.to_be_bytes()[..]),
            )
        };
        if rc != 0 {
            return Err(Udp53ErrorKind::QueryFailed(format!("ip-unicast-if:{rc}")));
        }
    }
    sock.set_read_timeout(Some(DNS_UDP_TIMEOUT))
        .map_err(|e| Udp53ErrorKind::QueryFailed(format!("read-timeout:{e}")))?;
    sock.set_write_timeout(Some(DNS_UDP_TIMEOUT))
        .map_err(|e| Udp53ErrorKind::QueryFailed(format!("write-timeout:{e}")))?;
    sock.send_to(&query, resolver)
        .map_err(|e| Udp53ErrorKind::QueryFailed(format!("send:{e}")))?;
    let mut resp = [0u8; 512];
    let (n, _peer) = sock
        .recv_from(&mut resp)
        .map_err(|e| Udp53ErrorKind::QueryFailed(format!("recv:{e}")))?;
    if n == 0 {
        return Err(Udp53ErrorKind::BadResponse("empty".to_string()));
    }
    parse_dns_response(&resp[..n], &DNS_QUERY_ID)
}

// ---------------------------------------------------------------------------
// 双线解析（L1 DoH → L2 UDP/53；一条失败 → 下一条；全部失败 → typed 向量）。
// ---------------------------------------------------------------------------

/// 双线可信解析（fake-ip 过滤；逐层 typed 错误收集）——**async 生产入口**。
///
/// gateway host 为 IPv4 字面量时直接采用（`ResolutionSource::System`；fake-ip 字面量
/// 拒绝）；否则按配置顺序试 DoH 全部 endpoint，再试 UDP/53 全部 resolver。
///
/// UDP/53 线路是 std 同步 socket（`read_timeout` 2s 有界阻塞），在 async 上下文内直接
/// 调用——与 acceptance 参考语义一致，单次查询以 DNS_UDP_TIMEOUT 为界、整体由
/// LAYER_DEADLINE 约束，不 panic。
///
/// # Errors
///
/// 全部线路失败 → [`ResolveLayerError`] 向量（每层一条；证据 `describe()`）。
pub async fn resolve_gateway_dual_line(
    cfg: &DualLineConfig,
    host: &str,
) -> Result<(Ipv4Addr, ResolutionSource), Vec<ResolveLayerError>> {
    // R0 计时：整体 + 逐层（LAYER_DEADLINE 5s / DNS_UDP_TIMEOUT 2s 是上界，实际耗时
    // 逐层落盘——归因 vgdc_dns 是否吃 apply 段之外的时间）。
    let _t = crate::timing::Timed::new("resource.vgdc_dns.resolve_total");
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        if is_fake_ip_v4(ip) {
            return Err(vec![ResolveLayerError::FakeIpLiteral(ip)]);
        }
        return Ok((ip, ResolutionSource::System));
    }
    let mut layer_errors: Vec<ResolveLayerError> = Vec::new();
    for ep in &cfg.doh_endpoints {
        let layer_start = std::time::Instant::now();
        match doh_query(
            std::sync::Arc::clone(&cfg.doh_client_config),
            SocketAddr::from((ep.ip, 443)),
            ep.ip.to_string(),
            ep.path,
            host,
        )
        .await
        {
            Ok(ip) => {
                crate::timing::record_elapsed(
                    &format!("resource.vgdc_dns.doh_ok:{}", ep.ip),
                    layer_start,
                );
                return Ok((ip, ResolutionSource::Doh(ep.ip)));
            }
            Err(kind) => {
                crate::timing::record_elapsed(
                    &format!("resource.vgdc_dns.doh_fail:{}", ep.ip),
                    layer_start,
                );
                layer_errors.push(ResolveLayerError::Doh {
                    endpoint: ep.ip,
                    kind,
                });
            }
        }
    }
    for resolver in &cfg.udp53_resolvers {
        // MVP 是 IPv4-only：V6 resolver 视为该层 typed 失败（不 panic）。
        let resolver_ip = match resolver.ip() {
            std::net::IpAddr::V4(ip) => ip,
            std::net::IpAddr::V6(_) => {
                layer_errors.push(ResolveLayerError::Udp53 {
                    resolver: Ipv4Addr::UNSPECIFIED,
                    kind: Udp53ErrorKind::BadQuery("ipv6-resolver".to_string()),
                });
                continue;
            }
        };
        let layer_start = std::time::Instant::now();
        match dns_a_query_udp(host, *resolver, cfg.udp53_ifindex) {
            Ok(mut ips) => {
                let ip = ips.remove(0);
                crate::timing::record_elapsed(
                    &format!("resource.vgdc_dns.udp53_ok:{}", resolver_ip),
                    layer_start,
                );
                return Ok((ip, ResolutionSource::Udp53(resolver_ip)));
            }
            Err(kind) => {
                crate::timing::record_elapsed(
                    &format!("resource.vgdc_dns.udp53_fail:{}", resolver_ip),
                    layer_start,
                );
                layer_errors.push(ResolveLayerError::Udp53 {
                    resolver: resolver_ip,
                    kind,
                });
            }
        }
    }
    Err(layer_errors)
}

// ---------------------------------------------------------------------------
// 物理网卡发现（GetAdaptersAddresses；UDP/53 绑网卡 + /32 路由共用）。
// ---------------------------------------------------------------------------

/// `SOCKET_ADDRESS` → IPv4 地址（非 `AF_INET` 返回 `None`）。
#[must_use]
fn ipv4_of(sa: &SOCKET_ADDRESS) -> Option<Ipv4Addr> {
    if sa.lpSockaddr.is_null() {
        return None;
    }
    // SAFETY: lpSockaddr 指向 SOCKADDR_INET 兼容缓冲区（sockaddr 存储足以容纳）；
    // AF_INET 分支下 sin_addr 有效（routes.rs 同款读取路径）。
    let inet = unsafe { &*sa.lpSockaddr.cast::<SOCKADDR_INET>() };
    if unsafe { inet.si_family } != AF_INET {
        return None;
    }
    // SAFETY: AF_INET 分支下 Ipv4.sin_addr 有效；S_addr 按网络字节序存于内存
    // （x86 LE：to_le_bytes 还原字节顺序，routes.rs 同款实测修正）。
    let octets = unsafe { inet.Ipv4.sin_addr.S_un.S_addr }.to_le_bytes();
    Some(Ipv4Addr::from(octets))
}

/// 适配器第一个 IPv4 单播地址。
#[must_use]
fn first_ipv4_unicast(a: &IP_ADAPTER_ADDRESSES_LH) -> Option<Ipv4Addr> {
    let mut cur = a.FirstUnicastAddress;
    while !cur.is_null() {
        // SAFETY: 系统链表（GetAdaptersAddresses 所有权）；非空指针有效。
        let ua = unsafe { &*cur };
        if let Some(ip) = ipv4_of(&ua.Address) {
            return Some(ip);
        }
        cur = ua.Next;
    }
    None
}

/// 适配器第一个 IPv4 网关。
#[must_use]
fn first_ipv4_gateway(a: &IP_ADAPTER_ADDRESSES_LH) -> Option<Ipv4Addr> {
    let mut cur = a.FirstGatewayAddress;
    while !cur.is_null() {
        // SAFETY: 系统链表（GetAdaptersAddresses 所有权）；非空指针有效。
        let ga = unsafe { &*cur };
        if let Some(ip) = ipv4_of(&ga.Address) {
            return Some(ip);
        }
        cur = ga.Next;
    }
    None
}

/// 虚拟/隧道适配器名称标记（排除 Mihomo TUN / Wintun / Hyper-V 虚拟交换机等）。
#[must_use]
fn is_virtual_adapter_name(name: &str) -> bool {
    const MARKERS: [&str; 15] = [
        "wintun", "mihomo", "clash", "meta", "tun", "loopback", "virtual", "vehernet",
        "hyper-v", "docker", "tailscale", "zerotier", "vmware", "virtualbox", "exv",
    ];
    let lower = name.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

/// 发现全部物理网卡（以太网/802.11、OperStatus Up、有 IPv4 单播 + 网关、非虚拟）；
/// 按 `ipv4_metric` 升序（越小越优先）。
///
/// # Errors
///
/// `GetAdaptersAddresses` 失败 → `String` 描述。
pub fn find_physical_nics() -> Result<Vec<NicInfo>, String> {
    let mut size: u32 = 0;
    // SAFETY: 首次调用只查询所需缓冲区大小（adapter 缓冲区 None；size 由系统填充）。
    let rc = unsafe {
        GetAdaptersAddresses(
            u32::from(AF_INET.0),
            GAA_FLAG_INCLUDE_GATEWAYS,
            None,
            None,
            &raw mut size,
        )
    };
    if rc != 0 && rc != ERROR_BUFFER_OVERFLOW {
        return Err(format!("GetAdaptersAddresses rc={rc}"));
    }
    // 对齐：u64 缓冲区（IP_ADAPTER_ADDRESSES_LH 需要 8 字节对齐）。
    let mut buf = vec![0u64; size as usize / 8 + 2];
    let ptr = buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    let mut out_size = (buf.len() * 8) as u32;
    // SAFETY: 缓冲区按请求大小分配且对齐；系统填充链表（同一调用返回后 buf 存活）。
    let rc = unsafe {
        GetAdaptersAddresses(
            u32::from(AF_INET.0),
            GAA_FLAG_INCLUDE_GATEWAYS,
            None,
            Some(ptr),
            &raw mut out_size,
        )
    };
    if rc != 0 {
        return Err(format!("GetAdaptersAddresses rc={rc}"));
    }
    let mut out: Vec<NicInfo> = Vec::new();
    // SAFETY: 系统填充的链表；Next 链按 Length 约束（Windows 保证有效）。
    let mut cur = ptr;
    while !cur.is_null() {
        let a = unsafe { &*cur };
        let is_physical = a.IfType == IF_TYPE_ETHERNET_CSMACD || a.IfType == IF_TYPE_IEEE80211;
        if is_physical && a.OperStatus == IfOperStatusUp && !a.FirstGatewayAddress.is_null() {
            let friendly_name = unsafe { a.FriendlyName.to_string() }.unwrap_or_default();
            if !is_virtual_adapter_name(&friendly_name)
                && let (Some(local_ip), Some(gateway)) =
                    (first_ipv4_unicast(a), first_ipv4_gateway(a))
            {
                // SAFETY: 读 union 成员（Anonymous1 的 IfIndex、Luid.Value；
                // routes.rs 同款读取路径）。
                let (ifindex, luid) = unsafe { (a.Anonymous1.Anonymous.IfIndex, a.Luid.Value) };
                out.push(NicInfo {
                    luid,
                    ifindex,
                    gateway,
                    local_ip,
                    friendly_name,
                    ipv4_metric: a.Ipv4Metric,
                });
            }
        }
        cur = a.Next;
    }
    out.sort_by_key(|n| n.ipv4_metric);
    Ok(out)
}

/// `ERROR_BUFFER_OVERFLOW`（`GetAdaptersAddresses` 首次 size 查询的预期返回）。
const ERROR_BUFFER_OVERFLOW: u32 = 111;

// ---------------------------------------------------------------------------
// C3 生产供给：CSTP/TLS 控制面 socket 出口绑定（IP_UNICAST_IF；自 acceptance
// `socket_binder.rs` 提升，供 `BootstrapConfig.socket_binder` 注入）。
// ---------------------------------------------------------------------------

/// 由物理出口 ifindex 构造 CSTP/TLS 控制面 socket 出口绑定闭包
/// （`IP_UNICAST_IF`，网络字节序，绑定点 = TCP connect 之前）。
///
/// `ifindex == 0`（无效/未探测）返回 `None`，调用方可回退到不绑定（默认路由）。
///
/// # Errors
///
/// 无（绑定的 setsockopt 失败在闭包调用时以 `std::io::Error` 返回，connect 会以
/// `BootstrapError::ConnectFailed` 中止）。
#[must_use]
pub fn socket_binder_for_ifindex(ifindex: u32) -> Option<Arc<SocketBinder>> {
    if ifindex == 0 {
        return None;
    }
    Some(Arc::new(move |socket: &tokio::net::TcpSocket| -> std::io::Result<()> {
        let raw = socket.as_raw_socket();
        // SAFETY: socket 句柄有效（tokio TcpSocket 持有）；IP_UNICAST_IF 的 optval
        // 是网卡索引的网络字节序 DWORD（Windows 文档：IPv4 索引必须网络字节序存储，
        // 否则 WSAEADDRNOTAVAIL，见 dev.22 实测）。
        let rc = unsafe {
            setsockopt(
                SOCKET(raw as usize),
                IPPROTO_IP.0,
                IP_UNICAST_IF,
                Some(&ifindex.to_be_bytes()[..]),
            )
        };
        if rc != 0 {
            // SOCKET_ERROR=-1 只是失败标记，实际原因必须紧随失败调用读取。
            let code = unsafe { windows::Win32::Networking::WinSock::WSAGetLastError() }.0;
            return Err(std::io::Error::from_raw_os_error(code));
        }
        Ok(())
    }))
}

// ---------------------------------------------------------------------------
// 机制测试（非 elevated）：DoH 假 HTTPS 服务器（注入 TestRoots）、UDP/53 假 socket、
// fake-ip 过滤、层回退矩阵、物理网卡发现、socket binder 参数形态。
// ---------------------------------------------------------------------------
