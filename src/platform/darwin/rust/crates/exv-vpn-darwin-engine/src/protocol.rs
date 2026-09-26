//! WebVPN 登录与 CSTP 会话的真实组装（只复用 Common `exv-vpn-cstp`）。
//!
//! 序列与 Windows `tunnel_runtime::assemble` 阶段 1 对齐：物理出口探测 → 网关
//! IPv4 解析 → `ConnectingControl`（WebVPN 登录，凭据登录后即清零）→
//! `NegotiatingTunnel`（CSTP CONNECT + 真实 offer）。本模块不创建 utun、
//! 不应用任何平台网络资源。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use std::fmt;
use std::net::Ipv4Addr;
use std::sync::Arc;

use tokio::sync::mpsc;

use exv_vpn_cstp::connector::{BootstrapConfig, TrustPolicy};
use exv_vpn_cstp::session::{CstpControlEvent, CstpSession, SessionError, TunnelOffer};
use exv_vpn_cstp::webvpn::{LoginError, WebvpnLogin};
use exv_vpn_wire::generated as wire;

use crate::egress::binder::bound_socket_binder;
use crate::egress::physical_route::{PhysicalEgress, find_physical_egress};
use crate::egress::resolver::{CachedGateway, resolve_gateway_ipv4_observed};

const CSTP_GATEWAY_PORT: u16 = 443;

/// 连接序列的 typed 失败；文本只含稳定类别。
#[derive(Debug)]
pub enum ConnectError {
    /// 物理出口或解析失败（ConnectingControl 前）。
    Egress(crate::egress::EgressError),
    /// WebVPN 登录失败（认证拒绝 / SAML / 2FA / TLS / HTTP）。
    Login(LoginError),
    /// CSTP CONNECT 或 offer 失败。
    Session(SessionError),
    /// 登录前的 VPN 服务端专用路由未能读取、更新或回读确认。
    ControlRoute(crate::platform::PlatformError),
    /// 取消栅栏触发。
    Cancelled,
}

impl ConnectError {
    /// 失败发生时已进入的 wire 阶段（供 ConnectStatusEvent.error 关联）。
    #[must_use]
    pub const fn failure_phase(&self) -> wire::ConnectPhase {
        match self {
            Self::Login(LoginError::SamlRequired | LoginError::TwoFactorRequired) => {
                wire::ConnectPhase::AwaitingInteraction
            }
            Self::Session(_) => wire::ConnectPhase::NegotiatingTunnel,
            Self::Egress(_) | Self::Login(_) | Self::ControlRoute(_) | Self::Cancelled => {
                wire::ConnectPhase::ConnectingControl
            }
        }
    }

    /// 映射为 Common `VpnError`（只填 code/stage，凭据与主机名永不入错误）。
    #[must_use]
    pub fn to_vpn_error(&self) -> wire::VpnError {
        use wire::{ErrorCode, ErrorStage};
        let (code, stage) = match self {
            Self::Egress(_) | Self::ControlRoute(_) => {
                (ErrorCode::ObservationFailed, ErrorStage::ConnectingControl)
            }
            Self::Login(LoginError::SamlRequired | LoginError::TwoFactorRequired) => {
                (ErrorCode::Unauthorized, ErrorStage::AwaitingInteraction)
            }
            Self::Login(LoginError::LoginRejected { .. }) => {
                (ErrorCode::Unauthorized, ErrorStage::ConnectingControl)
            }
            Self::Login(_) | Self::Session(_) => {
                (ErrorCode::EffectUnknown, ErrorStage::ProtocolSession)
            }
            Self::Cancelled => (
                ErrorCode::CancelledBeforeStart,
                ErrorStage::ConnectingControl,
            ),
        };
        wire::VpnError {
            code: code as i32,
            stage: stage as i32,
            ..Default::default()
        }
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Egress(error) => write!(formatter, "DARWIN_CONNECT_EGRESS_FAILED: {error}"),
            Self::Login(error) => write!(formatter, "DARWIN_CONNECT_LOGIN_FAILED: {error:?}"),
            Self::Session(error) => write!(formatter, "DARWIN_CONNECT_SESSION_FAILED: {error:?}"),
            Self::ControlRoute(error) => {
                write!(formatter, "DARWIN_CONNECT_CONTROL_ROUTE_FAILED: {error}")
            }
            Self::Cancelled => formatter.write_str("DARWIN_CONNECT_CANCELLED"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// CSTP 会话建立成功的产物：真实 offer + 会话通道。
///
/// `write_channel`/`read_channel`/`control_rx` 由持有者接管：写通道发 keepalive/DPD
/// 应答，读通道必须保持存活（drop 会使读任务退出），控制通道透出网关 DPD 请求。
/// 全部 drop 即关闭 TLS 会话。
pub struct EstablishedSession {
    /// 网关给出的权威 offer。
    pub offer: TunnelOffer,
    /// 本连接固定的物理出口事实（后续 utun/bypass 阶段复用）。
    pub egress: PhysicalEgress,
    /// 上行 CSTP 帧通道。
    pub write_channel: mpsc::UnboundedSender<Vec<u8>>,
    /// 下行数据帧通道（必须保持存活）。
    pub read_channel: mpsc::UnboundedReceiver<Vec<u8>>,
    /// 控制面事件（网关 keepalive/DPD 请求等）。
    pub control_rx: mpsc::UnboundedReceiver<CstpControlEvent>,
}

/// 建立一次真实的 WebVPN 登录 + CSTP 会话。
///
/// `username`/`password` 为本次一次性凭据；登录返回后由调用方清零。每个阶段
/// 边界经 `on_phase` 发布真实 wire 阶段；`is_cancelled` 在各阶段边界检查，
/// 取消时已建立的 TLS 连接随返回路径关闭。
///
/// # Errors
///
/// 任一阶段失败返回 [`ConnectError`]；失败前系统网络零 mutation。
/// 此公开协议测试入口不写登录前控制路由；生产 Engine 必须调用
/// [`establish_observed`] 并提供真实的路由校正闭包。
pub async fn establish(
    hostname: &str,
    username: &[u8],
    password: &[u8],
    user_agent: &str,
    is_cancelled: impl Fn() -> bool,
    on_phase: impl Fn(wire::ConnectPhase) + Send + Sync,
) -> Result<EstablishedSession, ConnectError> {
    establish_observed(
        hostname,
        username,
        password,
        user_agent,
        is_cancelled,
        on_phase,
        None,
        |_, _| Ok(()),
    )
    .await
}

/// 宿主日志观察入口，凭据与请求内容不进入观察字段。
pub(crate) async fn establish_observed(
    hostname: &str,
    username: &[u8],
    password: &[u8],
    user_agent: &str,
    is_cancelled: impl Fn() -> bool,
    on_phase: impl Fn(wire::ConnectPhase) + Send + Sync,
    diagnostics: Option<(&crate::log_sink::LogSink, &[u8])>,
    prepare_control_route: impl Fn(
        crate::egress::physical_route::PhysicalEgress,
        Ipv4Addr,
    ) -> Result<(), crate::platform::PlatformError>,
) -> Result<EstablishedSession, ConnectError> {
    let started = std::time::Instant::now();
    let log = |code, message, mut fields: Vec<(&'static str, String)>| {
        if let Some((sink, operation)) = diagnostics {
            fields.extend(crate::session_diagnostics::identity(operation));
            fields.push(("elapsed_ms", started.elapsed().as_millis().to_string()));
            sink.publish("info", "cstp", code, message, &fields);
        }
    };
    log(
        "CONNECT_BEGIN",
        "开始建立 CSTP 会话",
        crate::session_diagnostics::build_fields(),
    );
    let egress = find_physical_egress().map_err(|error| {
        log("EGRESS_FAILED", "物理出口观察失败", vec![]);
        ConnectError::Egress(error)
    })?;
    let (gateway, source) =
        resolve_gateway_ipv4_observed(hostname, CSTP_GATEWAY_PORT, Some(egress.ifindex))
            .await
            .map_err(|error| {
                log("GATEWAY_RESOLUTION_FAILED", "网关 IPv4 解析失败", vec![]);
                ConnectError::Egress(error)
            })?;
    log(
        "GATEWAY_RESOLVED",
        "已解析本次实际连接地址",
        vec![
            ("resolver_source", source.into()),
            ("gateway_address", gateway.to_string()),
            ("physical_ifindex", egress.ifindex.to_string()),
        ],
    );
    let server_ipv4 = match gateway.ip() {
        std::net::IpAddr::V4(address) => address,
        std::net::IpAddr::V6(_) => {
            return Err(ConnectError::Egress(
                crate::egress::EgressError::NoIpv4Answer,
            ));
        }
    };
    prepare_control_route(egress, server_ipv4).map_err(|error| {
        log("CONTROL_ROUTE_FAILED", "VPN 服务端专用路由校正失败", vec![]);
        ConnectError::ControlRoute(error)
    })?;
    log(
        "CONTROL_ROUTE_READY",
        "VPN 服务端专用路由已按当前物理网关回读确认",
        vec![("server_ipv4", server_ipv4.to_string())],
    );
    let gateway = CachedGateway::new(gateway);
    if is_cancelled() {
        return Err(ConnectError::Cancelled);
    }

    // 阶段 ConnectingControl：WebVPN 登录（login 与 CSTP 控制面共用同一 binder）。
    on_phase(wire::ConnectPhase::ConnectingControl);
    let binder = bound_socket_binder(egress.ifindex);
    let login = WebvpnLogin::perform_login_with_socket_binder(
        hostname,
        gateway.get(),
        TrustPolicy::Production,
        None,
        username,
        password,
        user_agent,
        Some(Arc::clone(&binder)),
    )
    .await
    .map_err(|error| {
        log("LOGIN_FAILED", "WebVPN 登录阶段失败", vec![]);
        ConnectError::Login(error)
    })?;

    log("LOGIN_COMPLETED", "WebVPN 登录完成", vec![]);
    if is_cancelled() {
        return Err(ConnectError::Cancelled);
    }

    // 阶段 NegotiatingTunnel：CSTP CONNECT + 真实 offer。
    on_phase(wire::ConnectPhase::NegotiatingTunnel);
    let config = BootstrapConfig {
        hostname: hostname.to_owned(),
        gateway_addr: gateway.get(),
        trust: TrustPolicy::Production,
        dtls_offered: false,
        deadline: None,
        socket_binder: Some(binder),
        gateway_resolver: None,
    };
    let observe_socket = |socket: &tokio::net::TcpStream| {
        use std::os::fd::AsRawFd;
        let mut bound: libc::c_uint = 0;
        let mut length = std::mem::size_of_val(&bound) as libc::socklen_t;
        // 只读已连接 socket 的真实 IP_BOUND_IF；读取失败不冒充配置意图。
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IP,
                25,
                std::ptr::from_mut(&mut bound).cast(),
                &mut length,
            )
        };
        log(
            "CSTP_SOCKET_ESTABLISHED",
            "已观察实际 CSTP socket",
            vec![
                (
                    "local_address",
                    socket
                        .local_addr()
                        .map_or_else(|_| "unobserved".into(), |a| a.to_string()),
                ),
                (
                    "peer_address",
                    socket
                        .peer_addr()
                        .map_or_else(|_| "unobserved".into(), |a| a.to_string()),
                ),
                (
                    "socket_bound_ifindex",
                    if result == 0 {
                        bound.to_string()
                    } else {
                        "unobserved".into()
                    },
                ),
            ],
        );
    };
    let session = CstpSession::open_with_user_agent_observed(
        config,
        Some(&login),
        Some(user_agent),
        Some(&observe_socket),
    )
    .await
    .map_err(|error| {
        log("CSTP_NEGOTIATION_FAILED", "CSTP 协商阶段失败", vec![]);
        ConnectError::Session(error)
    })?;
    log(
        "CSTP_OFFER_ACCEPTED",
        "已接受网关实际协商参数",
        vec![
            ("offer_mtu", session.offer_plan.mtu.to_string()),
            (
                "tunnel_address",
                session.offer_plan.ipv4_address.to_string(),
            ),
            ("tunnel_prefix", session.offer_plan.prefix.to_string()),
            (
                "dns_server_count",
                session.offer_plan.dns_servers.len().to_string(),
            ),
            ("route_count", session.offer_plan.routes.len().to_string()),
        ],
    );
    let CstpSession {
        offer_plan,
        write_channel,
        read_channel,
        control_rx,
        ..
    } = session;
    Ok(EstablishedSession {
        offer: offer_plan,
        egress,
        write_channel,
        read_channel,
        control_rx,
    })
}
