//! 原始包与持续 L3 连接的活动循环。宿主只需提供有界包队列，不在此操作网卡。
use super::{
    connection::{ConnectionError, L3Connection},
    packet_engine::{FlowRejection, PacketActions, PacketEngine, PacketError},
};
use std::{
    fmt,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

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
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let actions = tokio::select! {
            frame=connection.recv()=>engine.receive(frame?,Instant::now())?,
            packet=packets.recv()=>{
                let Some(packet)=packet else {return Ok(());};
                match engine.submit(packet,Instant::now()) {
                    Ok(frames)=>PacketActions {to_node:frames,..PacketActions::default()},
                    Err(error)=>{
                        events.send(PacketEvent::Dropped(error)).await.map_err(|_|PacketSessionError::HostClosed)?;
                        continue;
                    }
                }
            }
            _=tick.tick()=>engine.tick(Instant::now()),
        };
        let sender = connection.sender();
        for frame in actions.to_node {
            sender.send(frame).await?;
        }
        for packet in actions.to_host {
            events
                .send(PacketEvent::Packet(packet))
                .await
                .map_err(|_| PacketSessionError::HostClosed)?;
        }
        for rejected in actions.rejected {
            events
                .send(PacketEvent::Rejected(rejected))
                .await
                .map_err(|_| PacketSessionError::HostClosed)?;
        }
        for error in actions.dropped {
            events
                .send(PacketEvent::Dropped(error))
                .await
                .map_err(|_| PacketSessionError::HostClosed)?;
        }
    }
}
