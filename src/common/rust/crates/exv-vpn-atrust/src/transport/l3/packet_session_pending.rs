//! run_packets 私有动作所有权；两个方向额度独立，超额动作批全局唯一。
use super::{
    super::{L3Frame, packet_engine::PacketActions},
    PacketEvent,
};
use crate::session::QueueBudget;
use std::{collections::VecDeque, mem::size_of};

#[derive(Default)]
struct Usage {
    units: usize,
    bytes: usize,
}
impl Usage {
    fn add(&mut self, bytes: usize) {
        self.units += 1;
        self.bytes += bytes;
    }
    fn remove(&mut self, bytes: usize) {
        self.units -= 1;
        self.bytes -= bytes;
    }
    fn below(&self, limit: QueueBudget) -> bool {
        self.units < limit.max_units() && self.bytes < limit.max_bytes()
    }
    fn over(&self, limit: QueueBudget) -> bool {
        self.units > limit.max_units() || self.bytes > limit.max_bytes()
    }
}

pub(super) struct Pending {
    limit: QueueBudget,
    node: VecDeque<L3Frame>,
    host: VecDeque<PacketEvent>,
    node_usage: Usage,
    host_usage: Usage,
}
impl Pending {
    pub fn new(limit: QueueBudget) -> Self {
        Self {
            limit,
            node: VecDeque::new(),
            host: VecDeque::new(),
            node_usage: Usage::default(),
            host_usage: Usage::default(),
        }
    }
    pub fn over(&self) -> bool {
        self.node_usage.over(self.limit) || self.host_usage.over(self.limit)
    }
    pub fn host_room(&self) -> bool {
        !self.over() && self.host_usage.below(self.limit)
    }
    pub fn node_room(&self) -> bool {
        !self.over() && self.node_usage.below(self.limit)
    }
    pub fn node_empty(&self) -> bool {
        self.node_usage.units == 0
    }
    pub fn host_pending(&self) -> bool {
        !self.host.is_empty()
    }
    pub fn append(&mut self, actions: PacketActions) {
        debug_assert!(
            !self.over(),
            "only one complete batch can exceed local quotas"
        );
        for frame in actions.to_node {
            self.node_usage.add(frame_cost(&frame));
            self.node.push_back(frame);
        }
        for packet in actions.to_host {
            self.push_event(PacketEvent::Packet(packet));
        }
        for rejection in actions.rejected {
            self.push_event(PacketEvent::Rejected(rejection));
        }
        for error in actions.dropped {
            self.push_event(PacketEvent::Dropped(error));
        }
    }
    fn push_event(&mut self, event: PacketEvent) {
        self.host_usage.add(event_cost(&event));
        self.host.push_back(event);
    }
    /// 在途帧继续计费；仅 send future 完成后调用 node_completed。
    pub fn take_node(&mut self) -> Option<(L3Frame, usize)> {
        self.node.pop_front().map(|frame| {
            let cost = frame_cost(&frame);
            (frame, cost)
        })
    }
    pub fn node_completed(&mut self, bytes: usize) {
        self.node_usage.remove(bytes);
    }
    /// 只允许在外部 reserve 成功之后调用。
    pub fn take_event(&mut self) -> PacketEvent {
        let event = self.host.pop_front().expect("pending event");
        self.host_usage.remove(event_cost(&event));
        event
    }
}
fn event_cost(event: &PacketEvent) -> usize {
    size_of::<PacketEvent>()
        + match event {
            PacketEvent::Packet(packet) => packet.capacity(),
            _ => 0,
        }
}
fn frame_cost(frame: &L3Frame) -> usize {
    size_of::<L3Frame>()
        + match frame {
            L3Frame::AuthRequest { payload }
            | L3Frame::AuthResponse { payload, .. }
            | L3Frame::DataResponse { payload }
            | L3Frame::HeartbeatResponse { payload }
            | L3Frame::SecondVipResponse { payload, .. } => payload.capacity(),
            L3Frame::DataRequest(data) => {
                data.token.capacity()
                    + data.packets.capacity() * size_of::<Vec<u8>>()
                    + data.packets.iter().map(Vec::capacity).sum::<usize>()
            }
            L3Frame::HeartbeatRequest => 0,
        }
}
