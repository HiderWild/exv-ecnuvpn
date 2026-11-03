//! 原始包与持续 L3 连接的活动循环。宿主只需提供有界包队列，不在此操作网卡。
use super::{
    L3Frame,
    connection::{ConnectionError, L3Connection},
    packet_engine::{FlowRejection, PacketActions, PacketEngine, PacketError},
};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
#[path = "packet_session_pending.rs"]
mod pending;

pub enum PacketEvent {
    Packet(Vec<u8>),
    Rejected(FlowRejection),
    Dropped(PacketError),
}
impl fmt::Debug for PacketEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Packet(packet) => f
                .debug_struct("Packet")
                .field("length", &packet.len())
                .finish(),
            Self::Rejected(reason) => f.debug_tuple("Rejected").field(reason).finish(),
            Self::Dropped(reason) => f.debug_tuple("Dropped").field(reason).finish(),
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum PacketSessionError {
    #[error("packet connection: {0}")]
    Connection(#[from] ConnectionError),
    #[error("packet processing: {0}")]
    Packet(#[from] PacketError),
    #[error("host packet consumer closed")]
    HostClosed,
}

/// 多流收发入口。宿主输入/输出均为有界 mpsc；每包不超过 IPv4 线格式最大长度，
/// 这些外部队列及调用者持有的包不计入引擎的待授权预算。
/// 调用方关闭输入或连接 Session 时停止。丢弃 future 也会释放拥有的连接和引擎。
/// # Errors
/// 节点传输、无法继续的协议响应、VIP 更新或宿主消费者退出时返回错误；
/// 单流授权拒绝和单个出站包不可处理通过事件报告，不关闭其他流。
pub async fn run_packets(
    mut connection: L3Connection,
    mut engine: PacketEngine,
    mut packets: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Sender<PacketEvent>,
) -> Result<(), PacketSessionError> {
    let session = connection.session();
    let result = tokio::select! {
        biased;
        ()=session.cancelled()=>Ok(()),
        result=run_inner(&mut connection,&mut engine,&mut packets,&events)=>result,
    };
    engine.close();
    let closing = connection.close().await;
    result.and(closing.map_err(PacketSessionError::Connection))
}
async fn run_inner(
    connection: &mut L3Connection,
    engine: &mut PacketEngine,
    packets: &mut mpsc::Receiver<Vec<u8>>,
    events: &mpsc::Sender<PacketEvent>,
) -> Result<(), PacketSessionError> {
    let mut next_tick = tokio::time::Instant::now();
    let mut pending = pending::Pending::new(connection.session().budget());
    let sender = connection.sender();
    type SendFuture = Pin<Box<dyn Future<Output = Result<(), ConnectionError>> + Send>>;
    let mut sending: Option<(SendFuture, usize)> = None;
    let mut held_frame = None;
    loop {
        // A recv releases its original L3 permit. This one-frame lookahead has
        // independent ownership and never scans past a blocked data frame.
        if held_frame.as_ref().is_some_and(|frame| {
            !pending.over()
                && (!matches!(frame, L3Frame::DataResponse { .. }) || pending.host_room())
        }) {
            pending.append(engine.receive(held_frame.take().expect("held frame"), Instant::now())?);
        }
        if sending.is_none() {
            if let Some((frame, cost)) = pending.take_node() {
                let sender = sender.clone();
                sending = Some((Box::pin(async move { sender.send(frame).await }), cost));
            }
        }
        let tick_due = tokio::time::Instant::now() >= next_tick;
        // Stop admitting fresh host work while a due tick waits for egress to
        // drain. A missed tick is a single current-time action, never a backlog.
        let admit_host = !tick_due && pending.node_room();
        let run_tick = pending.node_empty() && !pending.over();
        let has_send = sending.is_some();
        tokio::select! {
            () = events.closed() => return Err(PacketSessionError::HostClosed),
            permit = events.reserve(), if pending.host_pending() => {
                permit.map_err(|_| PacketSessionError::HostClosed)?.send(pending.take_event());
            }
            result = async { sending.as_mut().expect("in-flight send").0.as_mut().await }, if has_send => {
                result?;
                let (_, cost) = sending.take().expect("completed send");
                pending.node_completed(cost);
            }
            frame=connection.recv(), if held_frame.is_none() => held_frame=Some(frame?),
            packet=packets.recv(), if admit_host => {
                let Some(packet)=packet else {return Ok(());};
                let actions = match engine.submit(packet,Instant::now()) {
                    Ok(frames)=>PacketActions {to_node:frames,..PacketActions::default()},
                    Err(error)=>PacketActions {dropped:vec![error],..PacketActions::default()},
                };
                pending.append(actions);
            }
            _=tokio::time::sleep_until(next_tick), if run_tick => {
                let tick_started = tokio::time::Instant::now();
                pending.append(engine.tick(Instant::now()));
                next_tick = tick_started + Duration::from_millis(250);
            }
        }
    }
}
