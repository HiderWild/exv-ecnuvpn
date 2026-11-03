//! aTrust 认证、节点选择与协议连接；平台网卡和路由由运行时随后应用。

use crate::atrust_tunnel::{AtrustPacketTunnel, NodeIdentity, RawPacketChannels};
use exv_vpn_atrust::auth::origin::GatewayOrigin;
use exv_vpn_atrust::{ConnectPurpose, OutboundTarget, Underlay};
use exv_vpn_cstp::session::TunnelOffer;
use exv_vpn_win32_config::ExvConfig;
use exv_vpn_win32_ipc::browser_session::BrowserSession;
use exv_vpn_win32_resource::routes::RouteRow;
use std::sync::Arc;

pub(crate) struct PreparedAtrust {
    pub offer: TunnelOffer,
    /// 目录下发的解析器只用于校园域名 NRPT，不写到接口 DNS。
    pub campus_dns_servers: Vec<String>,
    pub bypasses: Vec<RouteRow>,
    pub owner: AtrustPacketTunnel,
    pub channels: RawPacketChannels,
}

fn choose_node<'a>(
    nodes: &'a [exv_vpn_atrust::resource::NodeAddress],
    observed_certificate: Option<&[u8]>,
) -> Result<&'a exv_vpn_atrust::resource::NodeAddress, String> {
    if let Some(der) = observed_certificate {
        return nodes
            .iter()
            .find(|node| {
                node.class == exv_vpn_atrust::resource::NodeClass::Wan
                    && crate::atrust_node_tls::observed_node_matches(&node.host, node.port, der)
            })
            .ok_or_else(|| {
                "aTrust observed-certificate node absent from authorized catalog".into()
            });
    }
    nodes
        .iter()
        .find(|node| node.class == exv_vpn_atrust::resource::NodeClass::Wan)
        .or_else(|| nodes.first())
        .ok_or_else(|| "aTrust node unavailable".into())
}

pub(crate) async fn prepare(
    browser: BrowserSession,
    config: &ExvConfig,
    observed_certificate: Option<&[u8]>,
) -> Result<PreparedAtrust, String> {
    let gateway = if config.server.contains("://") {
        config.server.clone()
    } else {
        format!("https://{}", config.server)
    };
    let gateway_origin = GatewayOrigin::parse(&gateway)
        .map_err(|_| crate::atrust_auth::invalid_https_origin().to_string())?;
    let gateway_host = gateway_origin.host().to_owned();
    let gateway_port = gateway_origin.port();
    // 显式诊断材料保持原作用域；正常学校预设由安装内置证书提供相同精确信任。
    let observed_certificate = observed_certificate
        .or_else(|| crate::atrust_node_tls::bundled_certificate_for_gateway(&gateway));
    let browser_origin = GatewayOrigin::parse(browser.gateway())
        .map_err(|_| crate::atrust_auth::invalid_https_origin().to_string())?;
    if !browser_origin.same_origin(&gateway_origin) {
        // 两个 origin 已通过校验；只记录主机和端口，不记录会话或认证 URL。
        return Err(format!(
            "aTrust browser gateway differs from selected gateway: browser={}:{}, selected={gateway_host}:{gateway_port}",
            browser_origin.host(),
            browser_origin.port()
        ));
    }
    let mtu = u16::try_from(config.mtu)
        .ok()
        .filter(|mtu| *mtu >= 68)
        .ok_or("aTrust configured MTU is invalid")?;
    let nics = exv_vpn_win32_resource::vgdc_dns::find_physical_nics()
        .map_err(|e| format!("aTrust physical interface: {e}"))?;
    let nic = nics
        .first()
        .ok_or("aTrust physical interface unavailable")?;
    let underlay = Arc::new(crate::atrust_underlay::PhysicalUnderlay::new(nic.ifindex)?);
    let session =
        crate::atrust_auth::authenticate_browser_session(browser, &gateway, underlay.clone())
            .await
            .map_err(|e| e.to_string())?;
    let group = session
        .catalog()
        .node_groups()
        .iter()
        .find(|group| {
            !group.addresses.is_empty()
                && session.catalog().apps().iter().any(|app| {
                    app.node_group_id.as_deref() == Some(group.id.as_str())
                        && !app.entries.is_empty()
                })
        })
        .ok_or("aTrust authorized resource node group unavailable")?;
    let node = choose_node(&group.addresses, observed_certificate)?;
    let target = OutboundTarget::new(node.host.clone(), node.port, ConnectPurpose::DataNode);
    let mut bypasses = Vec::new();
    for (host, port) in [(&gateway_host, gateway_port), (&target.host, target.port)] {
        for address in underlay
            .resolve(host, port)
            .await
            .map_err(|e| e.to_string())?
        {
            let std::net::IpAddr::V4(address) = address else {
                continue;
            };
            let row = exv_vpn_win32_resource::routes::gateway_bypass_row(nic, address)
                .map_err(|e| format!("aTrust bypass: {e:?}"))?;
            if !bypasses.contains(&row) {
                bypasses.push(row);
            }
        }
    }
    let stream = crate::atrust_node_tls::connect_node(
        underlay.as_ref(),
        &target,
        "sdp.ssl.com",
        observed_certificate,
    )
    .await?;
    let (owner, channels) = AtrustPacketTunnel::open(
        stream,
        &session,
        &target,
        group.id.clone(),
        NodeIdentity::for_current_process()?,
    )
    .await?;
    // VIP 来自节点；/32 与 MTU 是本机接口配置，不能宣称为服务端协商参数。
    // 不把目录中的全网资源变成全局路由，校园范围仍由 EXV 用户配置提供。
    let offer = TunnelOffer {
        ipv4_address: owner.vip(),
        prefix: 32,
        mtu,
        dns_servers: Vec::new(),
        routes: Vec::new(),
    };
    Ok(PreparedAtrust {
        campus_dns_servers: session.catalog().dns_servers().to_vec(),
        offer,
        bypasses,
        owner,
        channels,
    })
}
