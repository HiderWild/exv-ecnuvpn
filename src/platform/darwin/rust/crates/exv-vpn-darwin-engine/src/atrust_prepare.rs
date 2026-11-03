//! darwin aTrust 数据面组装（M3：win32 `atrust_prepare.rs` 的等价实现）。
//!
//! 从认证产物（M2 移交点 `Ok(authenticated)`）到 [`PreparedAtrust`]：
//! 节点组选择 → [`choose_node`]（证书匹配 Wan 优先→首个 Wan→组首，**不因无
//! WAN 拒绝**）→ 网关+节点 /32 旁路目的地解析（M4 起 [`PreparedAtrust::bypasses`]
//! 由 `bootstrap_runtime` 的**多行旁路账本**安装——先于校园路由、逐行记账、
//! 失败交既有回滚、逆序清理）→ 节点 TLS（[`crate::atrust_node_tls`] 三重作用域）
//! → L3 握手 + PacketEngine（[`crate::atrust_tunnel`]）→ 本机接口参数 offer。
//!
//! offer 语义与 win32 对齐：VIP 来自节点；`/32` 与 MTU 是本机接口配置，不宣称为
//! 服务端协商参数（prefix 恒 32）；**不把目录中的全网资源变成全局路由**——校园
//! 范围由 EXV 用户配置提供（经 core 计划 `ipv4_routes` 注入，Cisco envelope
//! routes 的 aTrust 等价通道）。`campus_dns_servers` 只用于校园域分域解析（M4
//! /etc/resolver——经 [`crate::campus_resolver`] 写入与生效门），不写到接口 DNS。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use std::net::Ipv4Addr;

use exv_vpn_atrust::auth::browser_handoff::AuthenticatedAtrustSession;
use exv_vpn_atrust::auth::origin::GatewayOrigin;
use exv_vpn_atrust::resource::{NodeAddress, NodeClass, ResourceCatalog};
use exv_vpn_atrust::{ConnectPurpose, OutboundTarget, Secret, Underlay};
use exv_vpn_cstp::session::TunnelOffer;

use crate::atrust_node_tls;
use crate::atrust_tunnel::{AtrustOpenError, AtrustPacketTunnel, NodeIdentity, RawPacketChannels};

/// 组装完成的 aTrust 数据面（管线输入；win32 `PreparedAtrust` 等价形状）。
pub(crate) struct PreparedAtrust {
    /// 本机接口参数（VIP=节点分配；prefix 恒 32；mtu=配置值；无服务端路由/DNS）。
    pub offer: TunnelOffer,
    /// 目录下发的解析器（仅 M4 校园域分域解析用，不写接口 DNS）。
    pub campus_dns_servers: Vec<String>,
    /// 网关+节点端点的 /32 旁路目的地（去重保序）。**M4 起由
    /// `bootstrap_runtime::EngineRuntime::reconcile_bypass_routes` 安装**（多行
    /// 旁路账本：先于校园路由、逐行记账、失败交既有回滚、逆序清理）。
    pub bypasses: Vec<Ipv4Addr>,
    pub owner: Option<AtrustPacketTunnel>,
    pub channels: RawPacketChannels,
}

/// 数据面组装的 typed 失败；`Display` 只含稳定 reason 码（L3 细节经 Common
/// 已脱敏的 Display 附着，不含 SID/节点载荷/证书材料）。
#[derive(Debug)]
pub(crate) enum AtrustDataError {
    /// 授权资源节点组不可用（无含地址且含可用资源的组）。
    GroupUnavailable,
    /// 节点不可用（observed 证书节点不在授权目录内，或组内无任何地址）。
    NodeUnavailable,
    /// 配置 MTU 非法（win32 同语义：`u16` 且 ≥68）。
    MtuInvalid,
    /// 网关/节点端点解析失败（Underlay 封闭词汇错误，无主机名）。
    BypassResolve(String),
    /// 节点 TLS 失败（win32 稳定类别词表）。
    NodeTls(String),
    /// 每流身份材料构造失败。
    Identity(&'static str),
    /// L3 握手/连接建立失败（含节点组不匹配与 SID 缺失的前置契约失败）。
    L3(AtrustOpenError),
}

impl AtrustDataError {
    /// 稳定 reason 码（进日志/trace；全部非秘密）。
    #[must_use]
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::GroupUnavailable => "DARWIN_ATRUST_NODE_GROUP_UNAVAILABLE",
            Self::NodeUnavailable => "DARWIN_ATRUST_NODE_UNAVAILABLE",
            Self::MtuInvalid => "DARWIN_ATRUST_MTU_INVALID",
            Self::BypassResolve(_) => "DARWIN_ATRUST_BYPASS_RESOLVE_FAILED",
            Self::NodeTls(_) => "DARWIN_ATRUST_NODE_TLS_FAILED",
            Self::Identity(_) => "DARWIN_ATRUST_NODE_IDENTITY_FAILED",
            Self::L3(_) => "DARWIN_ATRUST_L3_HANDSHAKE_FAILED",
        }
    }

    /// 数据面失败发生在隧道协商阶段（认证已完成；对齐 Cisco Session 失败的
    /// code 语义）。MTU 非法属配置输入错误。
    #[must_use]
    pub(crate) fn vpn_error(&self) -> exv_vpn_wire::generated::VpnError {
        use exv_vpn_wire::generated::{ErrorCode, ErrorStage, VpnError};
        let code = match self {
            Self::MtuInvalid => ErrorCode::InvalidInput,
            _ => ErrorCode::EffectUnknown,
        };
        VpnError {
            code: code as i32,
            stage: ErrorStage::NegotiatingTunnel as i32,
            ..Default::default()
        }
    }
}

impl std::fmt::Display for AtrustDataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BypassResolve(detail) | Self::NodeTls(detail) => {
                write!(formatter, "{}: {detail}", self.reason())
            }
            Self::Identity(detail) => write!(formatter, "{}: {detail}", self.reason()),
            Self::L3(error) => write!(formatter, "{}: {error}", self.reason()),
            _ => formatter.write_str(self.reason()),
        }
    }
}

impl std::error::Error for AtrustDataError {}

/// 节点选择（win32 `choose_node` 等价）：observed 证书 → 目录中证书匹配的 Wan
/// 节点（无则拒绝——证书授权的节点必须真实存在）；无 observed → 首个 Wan；
/// **无 Wan 时取组首（不因无 WAN 拒绝）**。
fn choose_node<'a>(
    nodes: &'a [NodeAddress],
    observed_certificate: Option<&[u8]>,
) -> Result<&'a NodeAddress, AtrustDataError> {
    if let Some(der) = observed_certificate {
        return nodes
            .iter()
            .find(|node| {
                node.class == NodeClass::Wan
                    && atrust_node_tls::observed_node_matches(&node.host, node.port, der)
            })
            .ok_or(AtrustDataError::NodeUnavailable);
    }
    nodes
        .iter()
        .find(|node| node.class == NodeClass::Wan)
        .or_else(|| nodes.first())
        .ok_or(AtrustDataError::NodeUnavailable)
}

/// 从认证产物组装数据面（生产入口：M2 移交点 `Ok(authenticated)` 的消费者）。
///
/// `platform` 来自信封字段（与协议 platform 同源——M0 证据值；FlowIdentity
/// 的 environment.platform 取值链，绝不写 "Windows"）。
///
/// # Errors
///
/// 见 [`AtrustDataError`]；失败前不创建任何平台资源（utun/路由/DNS 属管线阶段）。
pub(crate) async fn prepare_data_plane(
    authenticated: &AuthenticatedAtrustSession,
    gateway: &str,
    platform: &str,
    underlay: &dyn Underlay,
    mtu: u32,
) -> Result<PreparedAtrust, AtrustDataError> {
    let catalog = authenticated.catalog();
    let sid = authenticated.sid();
    prepare_from_catalog(catalog, sid, gateway, platform, underlay, mtu).await
}

/// 与 [`prepare_data_plane`] 同语义，直接消费目录与 SID（密闭夹具与生产共用
/// 单一实现——认证产物类型不可在测试中构造）。
///
/// # Errors
///
/// 同 [`prepare_data_plane`]。
pub(crate) async fn prepare_from_catalog(
    catalog: &ResourceCatalog,
    sid: Option<&Secret>,
    gateway: &str,
    platform: &str,
    underlay: &dyn Underlay,
    mtu: u32,
) -> Result<PreparedAtrust, AtrustDataError> {
    let mtu = u16::try_from(mtu)
        .ok()
        .filter(|mtu| *mtu >= 68)
        .ok_or(AtrustDataError::MtuInvalid)?;
    // 学校预设内置证书（仅 vpn.ecnu.edu.cn:443 网关；其余走平台验证器）。
    let observed_certificate = atrust_node_tls::bundled_certificate_for_gateway(gateway);
    let group = catalog
        .node_groups()
        .iter()
        .find(|group| {
            !group.addresses.is_empty()
                && catalog.apps().iter().any(|app| {
                    app.node_group_id.as_deref() == Some(group.id.as_str())
                        && !app.entries.is_empty()
                })
        })
        .ok_or(AtrustDataError::GroupUnavailable)?;
    let node = choose_node(&group.addresses, observed_certificate)?;
    let target = OutboundTarget::new(node.host.clone(), node.port, ConnectPurpose::DataNode);
    // 网关+节点 /32 旁路目的地（M4 由旁路账本安装）。主机名端点经 Underlay
    // 解析后并入（Common 纪律：node_endpoint_ips 不是地址全集，主机名必须解析）。
    let gateway_origin = GatewayOrigin::parse(gateway)
        .map_err(|_| AtrustDataError::NodeTls("gateway-origin-invalid".to_owned()))?;
    let mut bypasses = Vec::new();
    for (host, port) in [
        (gateway_origin.host().to_owned(), gateway_origin.port()),
        (target.host.clone(), target.port),
    ] {
        for address in underlay
            .resolve(&host, port)
            .await
            .map_err(|error| AtrustDataError::BypassResolve(error.to_string()))?
        {
            let std::net::IpAddr::V4(address) = address else {
                continue;
            };
            if !bypasses.contains(&address) {
                bypasses.push(address);
            }
        }
    }
    // 节点 TLS：observed 证书路径对 (222.66.117.110:441, sdp.ssl.com) 生效，
    // 其余节点/网关走平台验证器；SNI 恒 sdp.ssl.com。
    let stream =
        atrust_node_tls::connect_node(underlay, &target, "sdp.ssl.com", observed_certificate)
            .await
            .map_err(AtrustDataError::NodeTls)?;
    let identity =
        NodeIdentity::for_current_process(platform).map_err(AtrustDataError::Identity)?;
    let (owner, channels) = AtrustPacketTunnel::open_with_catalog(
        stream,
        catalog,
        sid,
        &target,
        group.id.clone(),
        identity,
    )
    .await
    .map_err(AtrustDataError::L3)?;
    let offer = TunnelOffer {
        ipv4_address: owner.vip(),
        prefix: 32,
        mtu,
        dns_servers: Vec::new(),
        routes: Vec::new(),
    };
    Ok(PreparedAtrust {
        campus_dns_servers: catalog.dns_servers().to_vec(),
        offer,
        bypasses,
        owner: Some(owner),
        channels,
    })
}
