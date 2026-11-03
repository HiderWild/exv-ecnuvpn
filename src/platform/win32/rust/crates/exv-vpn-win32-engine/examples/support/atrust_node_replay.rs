//! Windows Engine 连接对象的真实 DNS 回放；不安装网卡或路由。
use exv_engine::{
    atrust_auth::AuthenticatedAtrustSession,
    atrust_tunnel::{AtrustPacketTunnel, NodeIdentity},
};
use exv_vpn_atrust::transport::l3::{flow_auth::Ipv4Flow, udp_probe::make_udp};
use exv_vpn_atrust::{ConnectPurpose, Dispatch, OutboundTarget, Target, Underlay};
use serde::Deserialize;
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

#[derive(Deserialize)]
pub struct NodeOptions {
    node: SocketAddr,
    server_name: String,
    trusted_node_certificate_der: Option<Vec<u8>>,
}

pub async fn run(
    options: NodeOptions,
    session: &AuthenticatedAtrustSession,
    underlay: &dyn Underlay,
) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(45),
        run_inner(options, session, underlay),
    )
    .await
    .map_err(|_| "engine-node-replay-timeout".to_owned())?
}
async fn run_inner(
    options: NodeOptions,
    session: &AuthenticatedAtrustSession,
    underlay: &dyn Underlay,
) -> Result<(), String> {
    let Dispatch::L3(resource) = session
        .catalog()
        .dispatch(Target::udp(Ipv4Addr::new(202, 120, 80, 2), 53))
    else {
        return Err("dns-resource-missing".into());
    };
    let group = resource.node_group_id.ok_or("dns-node-group-missing")?;
    let target = OutboundTarget::new(
        options.node.ip().to_string(),
        options.node.port(),
        ConnectPurpose::DataNode,
    );
    if !session.catalog().node_groups().iter().any(|g| {
        g.id == group
            && g.addresses
                .iter()
                .any(|a| a.host == target.host && a.port == target.port)
    }) {
        return Err("dns-node-not-in-resource-group".into());
    }
    let stream = exv_engine::atrust_node_tls::connect_node(
        underlay,
        &target,
        &options.server_name,
        options.trusted_node_certificate_der.as_deref(),
    )
    .await?;
    println!("{{\"stage\":\"node-underlay\",\"connected\":true,\"credentials_sent\":false}}");
    println!(
        "{{\"stage\":\"node-tls\",\"verified\":true,\"credentials_sent\":false,\"observed_root\":{}}}",
        options.trusted_node_certificate_der.is_some()
    );
    let (mut owner, mut channels) = AtrustPacketTunnel::open(
        stream,
        session,
        &target,
        group,
        NodeIdentity::for_current_process()?,
    )
    .await?;
    let seed = u16::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..2].try_into().unwrap());
    let mut last_length = 0;
    for round in 0..2u16 {
        let mut queries = Vec::new();
        for flow in 0..2u16 {
            let id = seed.wrapping_add(round * 2 + flow).to_be_bytes();
            let mut query = vec![id[0], id[1], 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
            query.extend_from_slice(b"\x03www\x04ecnu\x03edu\x02cn\x00\x00\x01\x00\x01");
            let packet = make_udp(
                Ipv4Flow {
                    source: owner.vip(),
                    destination: Ipv4Addr::new(202, 120, 80, 2),
                    source_port: 49152 + flow,
                    destination_port: 53,
                    protocol: 17,
                },
                &query,
            )
            .map_err(|e| e.to_string())?;
            channels
                .outgoing
                .send(packet)
                .await
                .map_err(|_| "engine-packet-input-closed")?;
            queries.push(query);
        }
        let mut received = [false; 2];
        while !received.iter().all(|v| *v) {
            let packet = channels
                .incoming
                .recv()
                .await
                .ok_or("engine-packet-output-closed")?;
            if packet.len() < 28 || packet[0] >> 4 != 4 || packet[9] != 17 {
                return Err("dns-packet-shape".into());
            }
            let header = usize::from(packet[0] & 15) * 4;
            if header < 20
                || packet.len() < header + 8
                || packet[12..16] != [202, 120, 80, 2]
                || packet[16..20] != owner.vip().octets()
                || packet[header..header + 2] != 53u16.to_be_bytes()
            {
                return Err("dns-packet-endpoints".into());
            }
            let port = u16::from_be_bytes([packet[header + 2], packet[header + 3]]);
            let index = usize::from(port.checked_sub(49152).ok_or("dns-port")?);
            let query = queries.get(index).ok_or("dns-port")?;
            let reply = &packet[header + 8..];
            if reply.len() < query.len()
                || reply[..2] != query[..2]
                || reply[2] & 0x80 == 0
                || reply[2] & 0x7a != 0
                || reply[4..6] != [0, 1]
                || !reply[12..query.len()].eq_ignore_ascii_case(&query[12..])
            {
                continue;
            }
            if reply[3] & 15 != 0 {
                return Err("dns-rcode".into());
            }
            received[index] = true;
            last_length = reply.len();
        }
    }
    owner.begin_host_shutdown();
    drop(channels);
    // 四次往返已成立；关闭结果单独报告，避免后置 EOF 抹去已验证的数据证据。
    let connection_closed = owner.close().await.is_ok();
    println!(
        "{{\"stage\":\"campus-dns-reply\",\"udp_matched\":true,\"question_matched\":true,\"rcode\":0,\"bytes\":{last_length},\"flows\":2,\"roundtrips\":4,\"connection_closed\":{connection_closed}}}"
    );
    Ok(())
}
