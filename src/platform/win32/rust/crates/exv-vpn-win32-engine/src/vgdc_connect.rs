
//! 生产装配点：VGDC 双线直连 DNS + CSTP 控制面 socket 出口绑定（C4）。
//!
//! 本模块是 engine（本 helper 进程）在**拉起后、connect 前**装配控制面直连原语的
//! 接缝——engine 解析 VPN 服务器域名时走 VGDC 双线（DoH 直连 + 绑物理网卡 UDP/53
//! 兜底），不经 Mihomo fake-ip 污染；控制面连接以 `IP_UNICAST_IF` 钉在物理出口。
//!
//! - [`production_gateway_resolver`]：由物理出口 ifindex（`ifindex == 0` 时经
//!   `find_physical_nics` 自动发现首个物理网卡）构造 `BootstrapConfig.gateway_resolver`
//!   闭包——内部捕获 `DualLineConfig::production`，async 解析 hostname → `(ip, port)`。
//! - [`production_socket_binder`]：`BootstrapConfig.socket_binder` 的物理出口绑定闭包
//!   （`exv-vpn-win32-resource::vgdc_dns::socket_binder_for_ifindex` 重导出）。
//!
//! 装配顺序（C4 与 C3 衔接）：解析结果（物理出口 IP）供 CSTP connector 连接；解析与
//! 绑定共用同一物理网卡 ifindex（`find_physical_nics` 一次发现）。P5 接线点：engine
//! 的 connect 路径构造 `BootstrapConfig` 时注入两个闭包。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::os::windows::io::AsRawSocket;
use std::sync::Arc;
use std::time::{Duration, Instant};

use exv_vpn_cstp::connector::{GatewayResolver, ResolverFuture, SocketBinder};
use exv_vpn_win32_resource::vgdc_dns::{
    DualLineConfig, ResolutionSource, ResolveLayerError, find_physical_nics,
    socket_binder_for_ifindex,
};

/// CSTP 控制面默认端口（标准 HTTPS/CSTP 端口；协议帧未携带端口字段）。
pub const CSTP_GATEWAY_PORT: u16 = 443;

/// 由物理出口 ifindex 构造 VGDC 双线 DNS 解析器闭包（`BootstrapConfig.gateway_resolver`）。
///
/// `ifindex == 0`（未探测）时经 [`find_physical_nics`] 自动发现首个物理网卡作为出口；
/// 发现失败或无物理网卡 → `Err(String)`（fail closed，不静默回退系统解析——直连保证
/// 是 C4 的承诺）。`port` 是 CSTP 控制面端口（默认 [`CSTP_GATEWAY_PORT`]）。
///
/// 返回的闭包是 async 形态（`ResolverFuture`）：DoH 线路在调用方 tokio 上下文内直接
/// await，不 block_on 外部 runtime。
///
/// # Errors
///
/// `ifindex == 0` 且物理网卡发现失败/为空 → `String` 描述。
pub fn production_gateway_resolver(
    ifindex: u32,
    port: u16,
) -> Result<Arc<GatewayResolver>, String> {
    production_gateway_resolver_observed(ifindex, port, None)
}

/// 实际一次解析的结果；不保存原始错误文本或连接凭据。
#[derive(Debug, Clone)]
pub struct GatewayResolutionObservation {
    pub address: Option<SocketAddr>,
    pub source: Option<ResolutionSource>,
    pub elapsed: Duration,
    pub failed_layers: Option<usize>,
}

pub type GatewayResolutionObserver = dyn Fn(&GatewayResolutionObservation) + Send + Sync;

/// 原解析闭包的小观测端口；保留解析选择、返回值与错误字符串行为。
pub fn production_gateway_resolver_observed(
    ifindex: u32,
    port: u16,
    observer: Option<Arc<GatewayResolutionObserver>>,
) -> Result<Arc<GatewayResolver>, String> {
    let resolved_ifindex = if ifindex == 0 {
        let nics = find_physical_nics().map_err(|e| format!("vgdc-nics:{e}"))?;
        let nic = nics
            .first()
            .ok_or_else(|| "vgdc-nics:no-physical-nic".to_string())?;
        nic.ifindex
    } else {
        ifindex
    };
    let cfg = DualLineConfig::production(Some(resolved_ifindex))
        .map_err(|e| format!("vgdc-config:{e}"))?;
    Ok(resolver_with_config(cfg, port, observer))
}

fn resolver_with_config(
    cfg: DualLineConfig,
    port: u16,
    observer: Option<Arc<GatewayResolutionObserver>>,
) -> Arc<GatewayResolver> {
    Arc::new(move |host: &str| -> ResolverFuture {
        let cfg = cfg.clone();
        let observer = observer.clone();
        let host = host.to_string();
        Box::pin(async move {
            let started = Instant::now();
            let result =
                exv_vpn_win32_resource::vgdc_dns::resolve_gateway_dual_line(&cfg, &host).await;
            if let Some(observe) = observer {
                let (address, source, failed_layers) = match &result {
                    Ok((ip, source)) => (Some(SocketAddr::from((*ip, port))), Some(*source), None),
                    Err(errors) => (None, None, Some(errors.len())),
                };
                observe(&GatewayResolutionObservation {
                    address,
                    source,
                    elapsed: started.elapsed(),
                    failed_layers,
                });
            }
            result
                .map(|(ip, _source)| SocketAddr::from((ip, port)))
                .map_err(|errors| {
                    let detail: Vec<String> =
                        errors.iter().map(ResolveLayerError::describe).collect();
                    format!("vgdc-resolve:{}", detail.join("; "))
                })
        })
    })
}

/// 只读取这条已连接 socket 的端点和已生效 IP_UNICAST_IF；不会二次解析或重选路由。
pub(crate) fn connected_socket_fields(socket: &tokio::net::TcpStream) -> BTreeMap<String, String> {
    use windows::Win32::NetworkManagement::IpHelper::ConvertInterfaceIndexToLuid;
    use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
    use windows::Win32::Networking::WinSock::{
        IP_UNICAST_IF, IPPROTO_IP, SOCKET, WSAGetLastError, getsockopt,
    };
    let mut fields = BTreeMap::new();
    for (name, result) in [
        ("local_addr", socket.local_addr()),
        ("peer_addr", socket.peer_addr()),
    ] {
        match result {
            Ok(address) => {
                fields.insert(name.to_string(), address.to_string());
            }
            Err(error) => {
                fields.insert(name.to_string(), "unavailable".into());
                fields.insert(
                    format!("{name}_error"),
                    error
                        .raw_os_error()
                        .map_or_else(|| "none".into(), |code| code.to_string()),
                );
            }
        }
    }
    let mut index_bytes = [0u8; 4];
    let mut length = 4i32;
    // SAFETY: socket 存活，缓冲区为 4 字节 DWORD，optlen 与其长度一致。
    let result = unsafe {
        getsockopt(
            SOCKET(socket.as_raw_socket() as usize),
            IPPROTO_IP.0,
            IP_UNICAST_IF,
            windows::core::PSTR(index_bytes.as_mut_ptr()),
            &raw mut length,
        )
    };
    if result == 0 && length == 4 {
        // getsockopt 返回主机字节序；仅 setsockopt 的输入要求网络字节序。
        // https://learn.microsoft.com/en-us/windows/win32/winsock/ipproto-ip-socket-options
        let ifindex = u32::from_ne_bytes(index_bytes);
        fields.insert("socket_bound_ifindex".into(), ifindex.to_string());
        if ifindex != 0 {
            let mut luid = NET_LUID_LH::default();
            // SAFETY: 查询已绑定索引对应的标识，不重新运行出口选择。
            let code = unsafe { ConvertInterfaceIndexToLuid(ifindex, &raw mut luid) };
            if code.0 == 0 {
                fields.insert(
                    "socket_bound_luid".into(),
                    unsafe { luid.Value }.to_string(),
                );
            } else {
                fields.insert("socket_luid_query_error".into(), code.0.to_string());
            }
        }
    } else if result != 0 {
        // SAFETY: 紧随失败的 WinSock 调用读取同线程错误码。
        fields.insert(
            "socket_binding_query_error".into(),
            unsafe { WSAGetLastError() }.0.to_string(),
        );
    } else {
        fields.insert(
            "socket_binding_query_error".into(),
            "unexpected_option_length".into(),
        );
        fields.insert("socket_option_length".into(), length.to_string());
    }
    fields
}

/// 由物理出口 ifindex 构造 CSTP/TLS 控制面 socket 出口绑定闭包
/// （`BootstrapConfig.socket_binder`；`IP_UNICAST_IF`）。
///
/// `ifindex == 0`（未探测/无效）返回 `None`——调用方可回退到不绑定（默认路由）。
#[must_use]
pub fn production_socket_binder(ifindex: u32) -> Option<Arc<SocketBinder>> {
    socket_binder_for_ifindex(ifindex)
}

/// 模式在 Core 受理手动连接时冻结，登录和 CSTP 必须共用这一选择。
pub(crate) fn socket_binder_for_mode(
    mode: exv_vpn_win32_config::ConnectionMode,
    physical_ifindex: u32,
) -> Option<Arc<SocketBinder>> {
    match mode {
        exv_vpn_win32_config::ConnectionMode::Standard => {
            production_socket_binder(physical_ifindex)
        }
        exv_vpn_win32_config::ConnectionMode::Compatibility => None,
    }
}

// ---------------------------------------------------------------------------
// 单元测试（非网络）：闭包形态、IP 字面量路径、ifindex==0 自动发现与 fail-closed。
// 真实双线解析机制测试在 `exv-vpn-win32-resource::vgdc_dns`。
// ---------------------------------------------------------------------------
