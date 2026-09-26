//! 隧道 apply 前的 hostname → 真实 IPv4 解析；结果在本连接内缓存。
//!
//! 代理 TUN 的 fake-ip 污染是已观察事实（Windows vgdc 双线 DNS 针对同一问题）：
//! 系统解析器与未绑定的 UDP/53 查询都会被 TUN 截走并返回 `198.18.0.0/15` 假址。
//! 因此主解析线是 **`IP_BOUND_IF` 绑定物理网卡直接查公共 DNS**（与 Windows
//! "bound-NIC UDP/53" 行为对齐），系统解析器只作兜底且拒绝 fake-ip 段。
//! IPv6、未指定地址与非主机名输入一律 fail closed；解析失败不触发任何网络修改。

// DNS 二进制解析在边界检查后做窄化转换；中文文档中的技术术语不逐个加反引号。
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::doc_markdown)]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use crate::egress::EgressError;

/// RFC 2544 基准段：代理 TUN fake-ip 的惯用地址范围（已观察污染段）。
fn is_fake_ip(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

/// 直连公共 DNS 服务器（经物理网卡绑定的 UDP/53）。
const PUBLIC_DNS_SERVERS: [Ipv4Addr; 2] = [
    Ipv4Addr::new(223, 5, 5, 5),    // AliDNS
    Ipv4Addr::new(119, 29, 29, 29), // DNSPod
];

const DNS_TIMEOUT: Duration = Duration::from_secs(3);

/// 把网关主机名解析为 `port` 上的真实 IPv4 socket 地址。
///
/// `ifindex` 是已探测的物理出口；`None` 时跳过绑定线（仅测试/兜底语义）。
///
/// # Errors
///
/// 两条解析线都失败、无 IPv4 结果或结果为 IPv6/未指定/fake-ip 时返回 typed 错误。
pub async fn resolve_gateway_ipv4(
    hostname: &str,
    port: u16,
    ifindex: Option<u32>,
) -> Result<SocketAddr, EgressError> {
    resolve_gateway_ipv4_observed(hostname, port, ifindex)
        .await
        .map(|(address, _)| address)
}

/// 返回本次实际采用的解析路径；不额外查询或改变回退顺序。
pub(crate) async fn resolve_gateway_ipv4_observed(
    hostname: &str,
    port: u16,
    ifindex: Option<u32>,
) -> Result<(SocketAddr, &'static str), EgressError> {
    // IP 字面量直接采用。
    if let Ok(literal) = hostname.parse::<Ipv4Addr>() {
        if literal.is_unspecified() || is_fake_ip(literal) {
            return Err(EgressError::NoIpv4Answer);
        }
        return Ok((SocketAddrV4::new(literal, port).into(), "ipv4_literal"));
    }

    // 主线：绑定物理网卡的 UDP/53 直查公共 DNS（绕过 TUN 截留）。
    if let Some(ifindex) = ifindex {
        for server in PUBLIC_DNS_SERVERS {
            match query_dns_a(hostname, server, ifindex).await {
                Ok(Some(address)) if !address.is_unspecified() && !is_fake_ip(address) => {
                    return Ok((
                        SocketAddrV4::new(address, port).into(),
                        if server == PUBLIC_DNS_SERVERS[0] {
                            "bound_public_dns_primary"
                        } else {
                            "bound_public_dns_secondary"
                        },
                    ));
                }
                _ => {}
            }
        }
    }

    // 兜底线：系统解析器（拒绝 fake-ip / 未指定地址 / IPv6）。
    let candidates = tokio::net::lookup_host((hostname, port))
        .await
        .map_err(EgressError::Resolve)?;
    for candidate in candidates {
        if let std::net::IpAddr::V4(ipv4) = candidate.ip()
            && ipv4 != Ipv4Addr::UNSPECIFIED
            && !is_fake_ip(ipv4)
        {
            return Ok((SocketAddrV4::new(ipv4, port).into(), "system_resolver"));
        }
    }
    Err(EgressError::NoIpv4Answer)
}

/// 构造最小 A 记录查询（RD=1，单 question）。
fn build_dns_query(id: u16, hostname: &str) -> Option<Vec<u8>> {
    let mut query = Vec::with_capacity(12 + hostname.len() + 6);
    query.extend_from_slice(&id.to_be_bytes());
    query.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
    for label in hostname.split('.') {
        let bytes = label.as_bytes();
        if bytes.is_empty() || bytes.len() > 63 {
            return None;
        }
        query.push(bytes.len() as u8);
        query.extend_from_slice(bytes);
    }
    query.push(0);
    query.extend_from_slice(&[0, 1, 0, 1]); // QTYPE=A, QCLASS=IN
    Some(query)
}

/// 解析 DNS 应答中的第一条 A 记录（处理压缩指针）。
fn parse_dns_a(reply: &[u8]) -> Option<Ipv4Addr> {
    if reply.len() < 12 {
        return None;
    }
    let mut cursor = 12usize;
    // 跳过 question 的 QNAME（标签序列 + 终止零；指针只会出现在 answer 中）。
    loop {
        let &length = reply.get(cursor)?;
        if length == 0 {
            cursor += 1;
            break;
        }
        if length & 0xC0 != 0 {
            return None;
        }
        cursor += 1 + usize::from(length);
    }
    cursor += 4; // QTYPE + QCLASS
    let answers = u16::from_be_bytes([*reply.get(6)?, *reply.get(7)?]);
    for _ in 0..answers {
        // name：标签序列或压缩指针。
        loop {
            let &length = reply.get(cursor)?;
            if length & 0xC0 == 0xC0 {
                cursor += 2;
                break;
            }
            if length == 0 {
                cursor += 1;
                break;
            }
            cursor += 1 + usize::from(length);
        }
        let r#type = reply.get(cursor..cursor + 2)?;
        let rdlength = usize::from(u16::from_be_bytes([
            *reply.get(cursor + 8)?,
            *reply.get(cursor + 9)?,
        ]));
        let rdata = reply.get(cursor + 10..cursor + 10 + rdlength)?;
        if r#type == [0, 1] && rdlength == 4 {
            return Some(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]));
        }
        cursor += 10 + rdlength;
    }
    None
}

/// 经 `ifindex` 绑定的 UDP socket 直查 `server` 的 A 记录。
async fn query_dns_a(
    hostname: &str,
    server: Ipv4Addr,
    ifindex: u32,
) -> std::io::Result<Option<Ipv4Addr>> {
    let socket = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).await?;
    {
        use std::os::fd::AsRawFd;
        let index: libc::c_uint = ifindex;
        // SAFETY: socket is live; the option writes 4 bytes of c_uint. 端口 53 与
        // IP_BOUND_IF 同为 <netinet/in.h> 常量；此处与 binder.rs 保持一致。
        let status = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IP,
                25, // IP_BOUND_IF
                std::ptr::addr_of!(index).cast(),
                std::mem::size_of::<libc::c_uint>() as libc::socklen_t,
            )
        };
        if status != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    let mut id_bytes = [0_u8; 2];
    getrandom::fill(&mut id_bytes).map_err(std::io::Error::other)?;
    let id = u16::from_be_bytes(id_bytes);
    let Some(query) = build_dns_query(id, hostname) else {
        return Ok(None);
    };
    socket
        .send_to(&query, SocketAddrV4::new(server, 53))
        .await?;

    let mut reply = vec![0_u8; 1_500];
    let read = tokio::time::timeout(DNS_TIMEOUT, socket.recv(&mut reply)).await;
    match read {
        Ok(Ok(n)) if n >= 12 => Ok(parse_dns_a(&reply[..n])),
        _ => Ok(None),
    }
}

/// 本连接内缓存的真实网关地址（resolver 结果只解析一次）。
#[derive(Clone, Copy, Debug)]
pub struct CachedGateway {
    resolved: SocketAddr,
}

impl CachedGateway {
    /// 用已解析结果构造缓存。
    #[must_use]
    pub const fn new(resolved: SocketAddr) -> Self {
        Self { resolved }
    }

    /// 返回缓存的网关地址。
    #[must_use]
    pub const fn get(&self) -> SocketAddr {
        self.resolved
    }
}
