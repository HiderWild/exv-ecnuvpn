//! 单节点组的原始 IPv4 包分派与多流授权缓存。宿主选择校园路由和节点；本模块不联网。
//! 新流只查一次目录，授权期间按 Session 同型预算排队，授权 token 只用于对应五元组。
//! 调用方须及时提交返回的帧、交付返回的包，并定期调用 tick；关闭后不得重用此对象。
use super::{
    DataFrame, L3Frame,
    codec::split_ipv4_packets,
    flow_auth::{
        FlowAuthError, FlowAuthRequest, FlowAuthorization, Ipv4Flow, decode_response, sign_request,
    },
    packet_ipv4::{parse, reverse},
};
use crate::{
    Secret,
    error::L3CodecError,
    resource::{DispatchReason, DispatchRef, FlowProtocol, ResourceCatalog, Target},
    session::QueueBudget,
};
use std::{
    collections::HashMap,
    fmt,
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub struct FlowIdentity {
    pub sid: Secret,
    pub sign_key: Secret,
    pub device_id: String,
    pub connection_id: String,
    pub process_hash: String,
    pub environment: Option<serde_json::Value>,
}
impl fmt::Debug for FlowIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FlowIdentity(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FlowLimits {
    pub pending: QueueBudget,
    pub max_flows: usize,
    pub auth_timeout: Duration,
    pub idle_timeout: Duration,
}
impl Default for FlowLimits {
    fn default() -> Self {
        Self {
            pending: QueueBudget::DEFAULT,
            max_flows: 4096,
            auth_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_mins(5),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PacketError {
    #[error("packet engine closed")]
    Closed,
    #[error("invalid IPv4 packet or checksum")]
    InvalidPacket,
    #[error("IPv4 fragment handling not implemented")]
    FragmentUnsupported,
    #[error("IP protocol {0} not implemented")]
    UnsupportedProtocol(u8),
    #[error("ICMP type {0} not implemented")]
    IcmpTypeUnsupported(u8),
    #[error("packet source does not match assigned VIP")]
    SourceMismatch,
    #[error("resource dispatch: {0}")]
    Resource(DispatchReason),
    #[error("resource belongs to another node group")]
    NodeGroupMismatch,
    #[error("pending packet budget exhausted")]
    PendingBudget,
    #[error("flow table capacity exhausted")]
    FlowCapacity,
    #[error("flow authorization: {0}")]
    Authorization(#[from] FlowAuthError),
    #[error("IPv4 framing: {0}")]
    Codec(#[from] L3CodecError),
    #[error("unexpected packet engine frame")]
    UnexpectedFrame,
    #[error("node VIP update requires host reconfiguration")]
    VipUpdate,
}

#[derive(Debug)]
pub enum FlowFailure {
    Authorization(FlowAuthError),
    AuthorizationTimeout,
    RetryLimit,
}
#[derive(Debug)]
pub struct FlowRejection {
    pub flow: Ipv4Flow,
    pub reason: FlowFailure,
}

/// 返回值是所有权移交，不再占引擎内部排队预算；调用方收发队列须保持有界。
#[derive(Default)]
pub struct PacketActions {
    pub to_node: Vec<L3Frame>,
    pub to_host: Vec<Vec<u8>>,
    pub rejected: Vec<FlowRejection>,
    pub dropped: Vec<PacketError>,
}
struct Pending {
    signed: Zeroizing<Vec<u8>>,
    packets: Vec<Vec<u8>>,
    attempts: u8,
    retry_at: Option<Instant>,
    deadline: Instant,
}
enum State {
    Pending(Pending),
    Ready(Secret),
}
struct Entry {
    id: u64,
    last_seen: Instant,
    state: State,
}

pub struct PacketEngine {
    vip: Ipv4Addr,
    node_group: String,
    catalog: ResourceCatalog,
    identity: FlowIdentity,
    limits: FlowLimits,
    entries: HashMap<Ipv4Flow, Entry>,
    next_id: u64,
    pending_units: usize,
    pending_bytes: usize,
    tail: Zeroizing<Vec<u8>>,
    closed: bool,
}
impl PacketEngine {
    #[must_use]
    pub fn new(
        vip: Ipv4Addr,
        node_group: String,
        catalog: ResourceCatalog,
        identity: FlowIdentity,
        limits: FlowLimits,
    ) -> Self {
        Self {
            vip,
            node_group,
            catalog,
            identity,
            limits,
            entries: HashMap::new(),
            next_id: 1,
            pending_units: 0,
            pending_bytes: 0,
            tail: Zeroizing::new(Vec::new()),
            closed: false,
        }
    }
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }
    /// 接收网卡原始包。相同五元组复用已授权 token，待授权包不会提前发出。
    /// # Errors
    /// 包格式、源地址、资源/节点组、预算、签名或关闭错误。
    pub fn submit(&mut self, packet: Vec<u8>, now: Instant) -> Result<Vec<L3Frame>, PacketError> {
        self.ensure_open()?;
        let flow = parse(&packet)?;
        if flow.source != self.vip {
            return Err(PacketError::SourceMismatch);
        }
        if let Some(entry) = self.entries.get_mut(&flow)
            && let State::Ready(token) = &entry.state
        {
            entry.last_seen = now;
            return Ok(vec![data(token, packet)]);
        }
        self.check_budget(packet.len())?;
        if let Some(entry) = self.entries.get_mut(&flow) {
            let State::Pending(pending) = &mut entry.state else {
                unreachable!();
            };
            self.pending_bytes += packet.len();
            self.pending_units += 1;
            pending.packets.push(packet);
            return Ok(Vec::new());
        }
        if self.entries.len() >= self.limits.max_flows {
            return Err(PacketError::FlowCapacity);
        }
        let protocol = match flow.protocol {
            6 => FlowProtocol::Tcp,
            17 => FlowProtocol::Udp,
            _ => FlowProtocol::Icmp,
        };
        let target = Target {
            ip: flow.destination,
            protocol,
            port: if flow.protocol == 1 {
                None
            } else {
                Some(flow.destination_port)
            },
        };
        let dispatch = self.catalog.dispatch_ref(target);
        let DispatchRef::L3(resource) = dispatch else {
            return Err(PacketError::Resource(dispatch.reason()));
        };
        if resource.node_group_id().or(self.catalog.major_node_group())
            != Some(self.node_group.as_str())
        {
            return Err(PacketError::NodeGroupMismatch);
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(PacketError::FlowCapacity)?;
        let signed = sign_request(
            &FlowAuthRequest {
                sid: &self.identity.sid,
                app_id: resource.app_id(),
                process_hash: &self.identity.process_hash,
                app_token: None,
                device_id: &self.identity.device_id,
                connection_id: &self.identity.connection_id,
                lang: "en-US",
                environment: self.identity.environment.as_ref(),
                conntrack_hash: id,
                flow,
                domain: None,
            },
            &self.identity.sign_key,
        )?;
        let frame = L3Frame::AuthRequest {
            payload: signed.to_vec(),
        };
        self.pending_bytes += packet.len();
        self.pending_units += 1;
        self.entries.insert(
            flow,
            Entry {
                id,
                last_seen: now,
                state: State::Pending(Pending {
                    signed,
                    packets: vec![packet],
                    attempts: 1,
                    retry_at: None,
                    deadline: now + self.limits.auth_timeout,
                }),
            },
        );
        Ok(vec![frame])
    }
    /// 处理节点帧：授权结果只影响匹配流；跨帧 IPv4 边界在这里保留。
    /// # Errors
    /// 格式错误、需宿主处理的 VIP 更新、意外方向或关闭时返回错误。
    pub fn receive(&mut self, frame: L3Frame, now: Instant) -> Result<PacketActions, PacketError> {
        self.ensure_open()?;
        match frame {
            L3Frame::AuthResponse { status, payload } => self.authorize(status, &payload, now),
            L3Frame::DataResponse { payload } => self.receive_data(&payload, now),
            L3Frame::HeartbeatResponse { .. } => Ok(PacketActions::default()),
            L3Frame::SecondVipResponse { .. } => Err(PacketError::VipUpdate),
            _ => Err(PacketError::UnexpectedFrame),
        }
    }
    /// 定期驱动重试和回收。重试沿用固定上游至多三次，延迟来自服务端响应。
    pub fn tick(&mut self, now: Instant) -> PacketActions {
        let mut result = PacketActions::default();
        if self.closed {
            return result;
        }
        let mut remove = Vec::new();
        for (flow, entry) in &mut self.entries {
            match &mut entry.state {
                State::Ready(_)
                    if now.saturating_duration_since(entry.last_seen)
                        >= self.limits.idle_timeout =>
                {
                    remove.push(*flow);
                }
                State::Pending(pending)
                    if pending.retry_at.is_some_and(|at| now >= at) || now >= pending.deadline =>
                {
                    if pending.attempts >= 3 {
                        result.rejected.push(FlowRejection {
                            flow: *flow,
                            reason: FlowFailure::AuthorizationTimeout,
                        });
                        remove.push(*flow);
                    } else {
                        pending.attempts += 1;
                        pending.retry_at = None;
                        pending.deadline = now + self.limits.auth_timeout;
                        result.to_node.push(L3Frame::AuthRequest {
                            payload: pending.signed.to_vec(),
                        });
                    }
                }
                _ => {}
            }
        }
        for flow in remove {
            self.remove(flow);
        }
        result
    }
    /// 本次节点连接关闭后清除全部 token、排队包和尾部，不自动重放至新会话。
    pub fn close(&mut self) {
        self.closed = true;
        self.entries.clear();
        self.pending_units = 0;
        self.pending_bytes = 0;
        self.tail.clear();
    }
    fn ensure_open(&self) -> Result<(), PacketError> {
        if self.closed {
            Err(PacketError::Closed)
        } else {
            Ok(())
        }
    }
    fn check_budget(&self, bytes: usize) -> Result<(), PacketError> {
        if self.pending_units >= self.limits.pending.max_units()
            || bytes
                > self
                    .limits
                    .pending
                    .max_bytes()
                    .saturating_sub(self.pending_bytes)
        {
            Err(PacketError::PendingBudget)
        } else {
            Ok(())
        }
    }
    fn release_pending(&mut self, pending: &Pending) {
        self.pending_units -= pending.packets.len();
        self.pending_bytes -= pending.packets.iter().map(Vec::len).sum::<usize>();
    }
    fn remove(&mut self, flow: Ipv4Flow) {
        if let Some(Entry {
            state: State::Pending(pending),
            ..
        }) = self.entries.remove(&flow)
        {
            self.release_pending(&pending);
        }
    }
    fn authorize(
        &mut self,
        status: u8,
        payload: &[u8],
        now: Instant,
    ) -> Result<PacketActions, PacketError> {
        let mut pending = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.state, State::Pending(_)));
        let Some(first) = pending.next() else {
            return Ok(PacketActions::default());
        };
        // 无 Pending 时沿用空结果；否则仅解码一次，再按原遍历顺序匹配。
        let response = decode_response(payload)?;
        let Some((flow, _)) = std::iter::once(first)
            .chain(pending)
            .find(|(flow, entry)| response.matches_flow(entry.id, **flow))
        else {
            return Ok(PacketActions::default());
        };
        let flow = *flow;
        let decision = response.into_authorization(status);
        let mut entry = self.entries.remove(&flow).expect("matched entry");
        let State::Pending(mut pending) = entry.state else {
            unreachable!();
        };
        let mut result = PacketActions::default();
        match decision {
            Ok(FlowAuthorization::Granted(token)) => {
                self.release_pending(&pending);
                result.to_node = pending
                    .packets
                    .into_iter()
                    .map(|p| data(&token, p))
                    .collect();
                entry.state = State::Ready(token);
                entry.last_seen = now;
                self.entries.insert(flow, entry);
            }
            Ok(FlowAuthorization::RetryAfter(delay)) if pending.attempts < 3 => {
                if delay.is_zero() {
                    pending.attempts += 1;
                    pending.retry_at = None;
                    result.to_node.push(L3Frame::AuthRequest {
                        payload: pending.signed.to_vec(),
                    });
                } else {
                    pending.retry_at = Some(now + delay);
                }
                pending.deadline = now + delay + self.limits.auth_timeout;
                entry.state = State::Pending(pending);
                self.entries.insert(flow, entry);
            }
            other => {
                self.release_pending(&pending);
                result.rejected.push(FlowRejection {
                    flow,
                    reason: match other {
                        Err(error) => FlowFailure::Authorization(error),
                        _ => FlowFailure::RetryLimit,
                    },
                });
            }
        }
        Ok(result)
    }
    fn receive_data(&mut self, payload: &[u8], now: Instant) -> Result<PacketActions, PacketError> {
        self.tail.extend_from_slice(payload);
        let (packets, rest) = match split_ipv4_packets(&self.tail) {
            Ok(parts) => parts,
            Err(error) => {
                self.tail.clear();
                return Err(error.into());
            }
        };
        let mut result = PacketActions::default();
        for packet in packets {
            let flow = match parse(packet) {
                Ok(flow) => reverse(flow),
                Err(error) => {
                    result.dropped.push(error);
                    continue;
                }
            };
            if let Some(entry) = self.entries.get_mut(&flow)
                && matches!(entry.state, State::Ready(_))
            {
                entry.last_seen = now;
                result.to_host.push(packet.to_vec());
            }
        }
        let consumed = self.tail.len() - rest.len();
        self.tail.drain(..consumed);
        Ok(result)
    }
}
fn data(token: &Secret, packet: Vec<u8>) -> L3Frame {
    L3Frame::DataRequest(DataFrame {
        token: token.expose().as_bytes().to_vec(),
        packets: vec![packet],
    })
}
