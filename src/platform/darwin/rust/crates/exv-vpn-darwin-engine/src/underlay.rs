//! Darwin 物理出口的 Common `Underlay` 实现（M2 darwin aTrust：设计 §5.1）。
//!
//! 单实现三用途共用（AuthGateway/ResourceGateway/DataNode）：
//! - **接口选择注入化**：生产入口 [`DarwinPhysicalUnderlay::discover_default`]
//!   经既有 [`crate::egress::physical_route`] 探测当前默认物理接口；测试注入假
//!   接口选择器/假解析器/假 binder（[`Self::from_parts`]，私有字段仅测试构造）。
//! - **解析与拨号同一接口**：注入的解析闭包按 `IP_BOUND_IF` 绑定物理网卡查公共
//!   DNS（既有 [`crate::egress::resolver`] 主线，内置 fake-ip 拒绝线——M0 已实证
//!   本机明文 53 全被 Clash TUN 劫持为 `198.18.0.0/15` 假址）；TCP connect 前对
//!   socket 设置 `IP_BOUND_IF`（既有 [`crate::egress::binder`]）。
//! - **15s 上限**（解析+拨号合计）；**无隐式重试、无默认出口回退**（Common 库纪律
//!   + Windows 实证：绑定失败即失败，绝不静默改走系统默认路由）。
//! - [`ConnectPurpose`] 三用途差异体现在目标端口：拨号端口一律用
//!   `OutboundTarget.port`（DataNode 节点端口如 441 不被改写成网关端口——踩坑 5）。
//!
//! 本模块不操作系统路由/DNS/代理；错误映射到 Common
//! [`UnderlayErrorKind`](exv_vpn_atrust::UnderlayErrorKind) 封闭词汇，不传递
//! 原始 OS 错误字符串或主机名。

// 中文文档中的技术术语不逐个加反引号；socket 选项宽度转换由常量保证。
#![allow(clippy::doc_markdown)]

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use exv_vpn_atrust::transport::{
    ConnectFuture, OutboundTarget, ResolveFuture, Underlay, UnderlayStream,
};
use exv_vpn_atrust::{UnderlayError, UnderlayErrorKind};
use exv_vpn_cstp::connector::SocketBinder;

use crate::egress::EgressError;
use crate::egress::binder::bound_socket_binder;
use crate::egress::physical_route::find_physical_egress;
use crate::egress::resolver::resolve_gateway_ipv4;

/// 单次解析+拨号合计的硬时间上界（设计 §5.1：15s 上限）。
const CONNECT_DEADLINE: Duration = Duration::from_secs(15);

/// 注入式解析线：`(host, port) -> 该接口上可用的 IPv4 socket 地址`。
///
/// 生产闭包绑定 `ifindex`（同口解析）；测试注入假实现以钉死接口选择与
/// fake-ip 拒绝语义。
pub(crate) type InterfaceResolver =
    Arc<dyn Fn(String, u16) -> InterfaceResolveFuture + Send + Sync>;

/// [`InterfaceResolver`] 返回的 future。
pub(crate) type InterfaceResolveFuture =
    Pin<Box<dyn Future<Output = Result<SocketAddr, EgressError>> + Send>>;

/// 绑定单一物理接口的 Common `Underlay` 实现。
///
/// 构造后 `ifindex` 固定：一次连接内解析与拨号共用同一物理出口（探测→构造→
/// 认证的顺序由调用方决定；接口变化属新连接）。调用方仍负责安装节点旁路路由
/// （M4 账本），本类型不做任何网络修改。
pub struct DarwinPhysicalUnderlay {
    ifindex: u32,
    resolver: InterfaceResolver,
    binder: Arc<SocketBinder>,
}

impl DarwinPhysicalUnderlay {
    /// 生产入口：探测当前默认物理接口（既有 `physical_route` 探测逻辑）并在其上
    /// 构造解析线与 `IP_BOUND_IF` binder。
    ///
    /// # Errors
    ///
    /// 物理出口探测失败（无默认网关/无可用物理接口/系统调用失败）时返回
    /// [`EgressError`]；失败前无任何网络修改。
    pub fn discover_default() -> Result<Self, EgressError> {
        let egress = find_physical_egress()?;
        Self::bound_to_interface(egress.ifindex).map_err(|_| EgressError::NoPhysicalEgress)
    }

    /// 以已选定的物理接口索引构造（接口选择注入点）。`ifindex` 必须来自本次连接
    /// 的物理接口选择；解析线与 binder 都钉在该接口上。
    ///
    /// # Errors
    ///
    /// `ifindex == 0`（未选择接口）时返回错误；镜像 win32 `PhysicalUnderlay::new`
    /// 的 fail-closed 语义。
    pub fn bound_to_interface(ifindex: u32) -> Result<Self, &'static str> {
        if ifindex == 0 {
            return Err("aTrust physical interface missing");
        }
        let resolver: InterfaceResolver = Arc::new(move |host, port| {
            Box::pin(async move { resolve_gateway_ipv4(&host, port, Some(ifindex)).await })
        });
        Ok(Self {
            ifindex,
            resolver,
            binder: bound_socket_binder(ifindex),
        })
    }

    /// 已绑定的物理接口索引（诊断用，如日志的 physical_ifindex 字段）。
    #[must_use]
    pub const fn ifindex(&self) -> u32 {
        self.ifindex
    }
}

/// 解析答案可用性：仅接受非未指定、非 fake-ip 的 IPv4（纵深防线——即使注入的
/// 解析线出错，`198.18.0.0/15` 假址也绝不进入拨号）。回环地址放行：密闭夹具的
/// 回环监听依赖它；生产 DNS 答案不会为校园网关返回回环。
fn usable_resolved_address(ip: IpAddr) -> bool {
    let IpAddr::V4(address) = ip else {
        return false; // 物理出口线 IPv4-only：IPv6 一律 fail closed。
    };
    address != Ipv4Addr::UNSPECIFIED && !is_fake_ip(address)
}

/// RFC 2544 基准段 `198.18.0.0/15`：代理 TUN fake-ip 的惯用地址范围（M0 实证
/// 本机被劫持段）。与 `egress::resolver` 的拒绝线同值（此处是拨号前的第二道）。
fn is_fake_ip(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

// Resolve 尚拥有最终 lookup_host 原 I/O；不把已回退的早期DNS尝试冒充最终原因。
fn resolution_failure(
    error: EgressError,
    purpose: exv_vpn_atrust::ConnectPurpose,
) -> UnderlayError {
    match error {
        EgressError::Resolve(error) => {
            UnderlayError::from_io(UnderlayErrorKind::ResolveFailed, purpose, "resolve", &error)
        }
        other => UnderlayError::with_detail(
            UnderlayErrorKind::ResolveFailed,
            purpose,
            "resolve",
            &other.to_string(),
        ),
    }
}

impl Underlay for DarwinPhysicalUnderlay {
    // trait 的 resolve 不携带用途，独立解析的错误暂归认证网关（镜像 win32 语义：
    // 认证和节点建连应使用 connect，该路径保留 target.purpose）。
    fn resolve(&self, host: &str, port: u16) -> ResolveFuture<'_> {
        let resolution = (self.resolver)(host.to_owned(), port);
        Box::pin(async move {
            let purpose = exv_vpn_atrust::ConnectPurpose::AuthGateway;
            let address = tokio::time::timeout(CONNECT_DEADLINE, resolution)
                .await
                .map_err(|error| {
                    UnderlayError::with_detail(
                        UnderlayErrorKind::ResolveFailed,
                        purpose,
                        "resolve.deadline",
                        &error.to_string(),
                    )
                })?
                .map_err(|error| resolution_failure(error, purpose))?;
            if !usable_resolved_address(address.ip()) {
                return Err(UnderlayError::with_detail(
                    UnderlayErrorKind::ResolveFailed,
                    purpose,
                    "resolve",
                    "no usable IPv4 answer",
                ));
            }
            Ok(vec![address.ip()])
        })
    }

    fn connect(&self, target: &OutboundTarget) -> ConnectFuture<'_> {
        // 解析与拨号同接口由构造保证（resolver/binder 同 ifindex）。
        let resolution = (self.resolver)(target.host.clone(), target.port);
        let binder = Arc::clone(&self.binder);
        // 三用途端口差异：拨号端口一律用目标端口（DataNode 节点端口如 441）。
        let port = target.port;
        let purpose = target.purpose;
        Box::pin(async move {
            tokio::time::timeout(CONNECT_DEADLINE, async {
                let mut address = resolution
                    .await
                    .map_err(|error| resolution_failure(error, purpose))?;
                if !usable_resolved_address(address.ip()) {
                    return Err(UnderlayError::with_detail(
                        UnderlayErrorKind::ResolveFailed,
                        purpose,
                        "resolve",
                        "no usable IPv4 answer",
                    ));
                }
                address.set_port(port);
                let socket = tokio::net::TcpSocket::new_v4().map_err(|error| {
                    UnderlayError::from_io(
                        UnderlayErrorKind::ConnectFailed,
                        purpose,
                        "socket",
                        &error,
                    )
                })?;
                // 绑定失败必须失败，不得无声降级为系统默认路由（无回退纪律）。
                binder(&socket).map_err(|error| {
                    UnderlayError::from_io(
                        UnderlayErrorKind::ConnectFailed,
                        purpose,
                        "bind",
                        &error,
                    )
                })?;
                let stream = socket.connect(address).await.map_err(|error| {
                    let kind = match error.kind() {
                        std::io::ErrorKind::ConnectionRefused => UnderlayErrorKind::ConnectRefused,
                        std::io::ErrorKind::TimedOut => UnderlayErrorKind::ConnectTimedOut,
                        _ => UnderlayErrorKind::ConnectFailed,
                    };
                    UnderlayError::from_io(kind, purpose, "connect", &error)
                })?;
                Ok(Box::new(stream) as Box<dyn UnderlayStream>)
            })
            .await
            .map_err(|error| {
                UnderlayError::with_detail(
                    UnderlayErrorKind::ConnectTimedOut,
                    purpose,
                    "deadline",
                    &error.to_string(),
                )
            })?
        })
    }
}
