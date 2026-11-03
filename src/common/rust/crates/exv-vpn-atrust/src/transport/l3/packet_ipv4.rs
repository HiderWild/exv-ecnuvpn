//! 当前包分派器的 IPv4/TCP/UDP/ICMP echo 解析，不执行地址改写。
use super::{flow_auth::Ipv4Flow, packet_engine::PacketError};
use std::net::Ipv4Addr;

pub(super) fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in bytes.chunks(2) {
        sum += u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
    }
    while sum > 65535 {
        sum = (sum & 65535) + (sum >> 16);
    }
    !u16::try_from(sum).expect("folded checksum")
}
pub(super) fn parse(packet: &[u8]) -> Result<Ipv4Flow, PacketError> {
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return Err(PacketError::InvalidPacket);
    }
    let header = usize::from(packet[0] & 15) * 4;
    if header < 20
        || header > packet.len()
        || usize::from(u16::from_be_bytes([packet[2], packet[3]])) != packet.len()
    {
        return Err(PacketError::InvalidPacket);
    }
    if u16::from_be_bytes([packet[6], packet[7]]) & 0x3fff != 0 {
        return Err(PacketError::FragmentUnsupported);
    }
    if checksum(&packet[..header]) != 0 {
        return Err(PacketError::InvalidPacket);
    }
    let segment = &packet[header..];
    let protocol = packet[9];
    let (source_port, destination_port) = match protocol {
        6 => {
            if segment.len() < 20 {
                return Err(PacketError::InvalidPacket);
            }
            let offset = usize::from(segment[12] >> 4) * 4;
            if offset < 20 || offset > segment.len() {
                return Err(PacketError::InvalidPacket);
            }
            ports(segment)
        }
        17 => {
            if segment.len() < 8
                || usize::from(u16::from_be_bytes([segment[4], segment[5]])) != segment.len()
            {
                return Err(PacketError::InvalidPacket);
            }
            ports(segment)
        }
        1 => {
            if segment.len() < 8 || checksum(segment) != 0 {
                return Err(PacketError::InvalidPacket);
            }
            if !matches!(segment[0], 0 | 8) {
                return Err(PacketError::IcmpTypeUnsupported(segment[0]));
            }
            (0, 0)
        }
        other => return Err(PacketError::UnsupportedProtocol(other)),
    };
    if protocol == 6 || (protocol == 17 && segment[6..8] != [0, 0]) {
        let mut pseudo = Vec::with_capacity(12 + segment.len());
        pseudo.extend_from_slice(&packet[12..20]);
        pseudo.extend_from_slice(&[0, protocol]);
        pseudo.extend_from_slice(
            &u16::try_from(segment.len())
                .map_err(|_| PacketError::InvalidPacket)?
                .to_be_bytes(),
        );
        pseudo.extend_from_slice(segment);
        if checksum(&pseudo) != 0 {
            return Err(PacketError::InvalidPacket);
        }
    }
    Ok(Ipv4Flow {
        protocol,
        source: Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]),
        destination: Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]),
        source_port,
        destination_port,
    })
}
fn ports(segment: &[u8]) -> (u16, u16) {
    (
        u16::from_be_bytes([segment[0], segment[1]]),
        u16::from_be_bytes([segment[2], segment[3]]),
    )
}

pub(super) fn reverse(flow: Ipv4Flow) -> Ipv4Flow {
    Ipv4Flow {
        source: flow.destination,
        destination: flow.source,
        source_port: flow.destination_port,
        destination_port: flow.source_port,
        ..flow
    }
}
