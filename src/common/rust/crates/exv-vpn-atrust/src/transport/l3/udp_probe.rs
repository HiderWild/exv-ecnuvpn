//! 无网卡 UDP 单包探针：初始 VIP → 每流授权 → 数据请求 → 匹配的 UDP 响应。
//! 供真实节点互通实验使用，不是多连接生产收发循环；DNS 事务语义仍由调用方校验。
use super::codec::split_ipv4_packets;
use super::connection::{ConnectionError, ConnectionOptions, L3Connection};
use super::flow_auth::{
    FlowAuthError, FlowAuthRequest, FlowAuthorization, Ipv4Flow, parse_response, sign_request,
};
use super::handshake::{IpRequestError, IpRequestOptions};
use super::{DataFrame, L3Frame};
use crate::{Secret, error::L3CodecError, session::SessionFactory};
use std::{fmt, net::Ipv4Addr, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{sleep, timeout};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum UdpProbeError {
    #[error("invalid UDP probe input")]
    InvalidInput,
    #[error("UDP probe timed out")]
    Timeout,
    #[error("UDP probe I/O failed: {0:?}")]
    Io(std::io::ErrorKind),
    #[error("UDP probe handshake: {0}")]
    Handshake(#[from] IpRequestError),
    #[error("UDP probe authorization: {0}")]
    Authorization(#[from] FlowAuthError),
    #[error("UDP probe codec: {0}")]
    Codec(#[from] L3CodecError),
    #[error("unexpected UDP probe frame")]
    UnexpectedFrame,
    #[error("UDP probe authorization retry limit")]
    RetryLimit,
    #[error("UDP probe connection: {0}")]
    Connection(#[from] ConnectionError),
}

pub struct UdpProbeReply {
    pub vip: Ipv4Addr,
    pub payload: Zeroizing<Vec<u8>>,
}
impl fmt::Debug for UdpProbeReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpProbeReply")
            .field("vip", &self.vip)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

/// 在调用方已验证 TLS 的自有流上执行单次 UDP 请求。源 IP 用节点分配的 VIP 覆盖。
/// 总时限包括握手、至多三次授权、心跳和数据等待；不重发 UDP，不安装任何路由。
/// 返回或取消释放拥有的流值，实际底层连接释放取决于其 Drop 实现。
///
/// # Errors
/// 输入、握手、授权、I/O、帧格式或总超时失败时返回脱敏错误。
pub async fn probe_udp_ipv4<S>(
    stream: S,
    request: FlowAuthRequest<'_>,
    sign_key: &Secret,
    payload: &[u8],
    budget: Duration,
) -> Result<UdpProbeReply, UdpProbeError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    probe_udp_ipv4_observed(stream, request, sign_key, payload, budget, |_, _| {}).await
}

/// 单次诊断的响应观察入口。观察者收到可能含凭据的原文，必须由宿主保护，不得记录到普通日志。
/// # Errors
/// 与 `probe_udp_ipv4` 相同；观察回调不改变授权判断。
pub async fn probe_udp_ipv4_observed<S, F>(
    stream: S,
    mut request: FlowAuthRequest<'_>,
    sign_key: &Secret,
    payload: &[u8],
    budget: Duration,
    mut observe: F,
) -> Result<UdpProbeReply, UdpProbeError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: FnMut(u8, &[u8]),
{
    if request.flow.protocol != 17 || payload.len() > 65507 || budget.is_zero() {
        return Err(UdpProbeError::InvalidInput);
    }
    timeout(budget, async move {
        let mut connection = L3Connection::open(
            stream,
            request.sid,
            SessionFactory::new().open(),
            ConnectionOptions {
                handshake: IpRequestOptions {
                    timeout: budget,
                    ..IpRequestOptions::default()
                },
                ..ConnectionOptions::default()
            },
        )
        .await
        .map_err(|error| match error {
            ConnectionError::Handshake(IpRequestError::Timeout) => UdpProbeError::Timeout,
            ConnectionError::Handshake(other) => UdpProbeError::Handshake(other),
            other => UdpProbeError::Connection(other),
        })?;
        let vip = connection.vip();
        let sender = connection.sender();
        request.flow.source = vip;
        let signed = sign_request(&request, sign_key)?;
        let mut attempts = 0;
        let token = loop {
            attempts += 1;
            sender
                .send(L3Frame::AuthRequest {
                    payload: signed.to_vec(),
                })
                .await?;
            let decision = loop {
                match connection.recv().await? {
                    L3Frame::AuthResponse { status, payload } => {
                        observe(status, &payload);
                        break parse_response(
                            status,
                            &payload,
                            request.conntrack_hash,
                            request.flow,
                        )?;
                    }
                    L3Frame::HeartbeatResponse { .. } => {}
                    _ => return Err(UdpProbeError::UnexpectedFrame),
                }
            };
            match decision {
                FlowAuthorization::Granted(token) => break token,
                FlowAuthorization::RetryAfter(delay) => {
                    if attempts >= 3 {
                        return Err(UdpProbeError::RetryLimit);
                    }
                    sleep(delay).await;
                }
            }
        };
        let packet = make_udp(request.flow, payload)?;
        sender
            .send(L3Frame::DataRequest(DataFrame {
                token: token.expose().as_bytes().to_vec(),
                packets: vec![packet],
            }))
            .await?;
        let mut tail = Zeroizing::new(Vec::new());
        loop {
            match connection.recv().await? {
                L3Frame::DataResponse { payload } => {
                    // 旧尾部不足一个 IPv4 包，新帧载荷至多 65535，总量有界。
                    tail.extend_from_slice(&payload);
                    let (packets, rest) = split_ipv4_packets(&tail)?;
                    for packet in packets {
                        if let Some(payload) = matching_udp(packet, request.flow) {
                            // 单次匹配往返已成立；之后的 EOF 不撤销这条诊断证据。
                            // 仍等待关闭，保证本次拥有的连接释放。
                            let _ = connection.close().await;
                            return Ok(UdpProbeReply {
                                vip,
                                payload: Zeroizing::new(payload.to_vec()),
                            });
                        }
                    }
                    let consumed = tail.len() - rest.len();
                    tail.drain(..consumed);
                }
                L3Frame::HeartbeatResponse { .. } => {}
                _ => return Err(UdpProbeError::UnexpectedFrame),
            }
        }
    })
    .await
    .map_err(|_| UdpProbeError::Timeout)?
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in bytes.chunks(2) {
        sum += u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
    }
    while sum > 65535 {
        sum = (sum & 65535) + (sum >> 16);
    }
    !u16::try_from(sum).expect("folded checksum")
}
/// 构造无分片 IPv4 UDP 包，供无网卡诊断向原始包入口注入流量。
/// # Errors
/// 非 UDP 流或总长度超过 IPv4/UDP 长度字段时返回 `InvalidInput`。
pub fn make_udp(flow: Ipv4Flow, payload: &[u8]) -> Result<Vec<u8>, UdpProbeError> {
    if flow.protocol != 17 || payload.len() > 65507 {
        return Err(UdpProbeError::InvalidInput);
    }
    let total = u16::try_from(28 + payload.len()).map_err(|_| UdpProbeError::InvalidInput)?;
    let mut packet = vec![0; usize::from(total)];
    packet[0] = 0x45;
    packet[2..4].copy_from_slice(&total.to_be_bytes());
    packet[8] = 64;
    packet[9] = 17;
    packet[12..16].copy_from_slice(&flow.source.octets());
    packet[16..20].copy_from_slice(&flow.destination.octets());
    let check = checksum(&packet[..20]);
    packet[10..12].copy_from_slice(&check.to_be_bytes());
    packet[20..22].copy_from_slice(&flow.source_port.to_be_bytes());
    packet[22..24].copy_from_slice(&flow.destination_port.to_be_bytes());
    packet[24..26].copy_from_slice(&(total - 20).to_be_bytes());
    packet[28..].copy_from_slice(payload);
    // IPv4 UDP 允许零校验和；探针不产生 IP 分片。
    Ok(packet)
}
fn matching_udp(packet: &[u8], flow: Ipv4Flow) -> Option<&[u8]> {
    if packet.len() < 28
        || packet[9] != 17
        || packet[12..16] != flow.destination.octets()
        || packet[16..20] != flow.source.octets()
    {
        return None;
    }
    let header = usize::from(packet[0] & 15) * 4;
    if header < 20
        || packet.len() < header + 8
        || checksum(&packet[..header]) != 0
        || u16::from_be_bytes([packet[6], packet[7]]) & 0x3fff != 0
    {
        return None;
    }
    let udp = &packet[header..];
    let len = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
    if len < 8
        || len != udp.len()
        || udp[..2] != flow.destination_port.to_be_bytes()
        || udp[2..4] != flow.source_port.to_be_bytes()
    {
        return None;
    }
    if udp[6..8] != [0, 0] {
        let mut checked = Zeroizing::new(Vec::with_capacity(12 + udp.len()));
        checked.extend_from_slice(&packet[12..20]);
        checked.extend_from_slice(&[0, 17]);
        checked.extend_from_slice(&udp[4..6]);
        checked.extend_from_slice(udp);
        if checksum(&checked) != 0 {
            return None;
        }
    }
    Some(&udp[8..])
}
