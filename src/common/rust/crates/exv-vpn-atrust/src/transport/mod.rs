//! 传输层：出口端口与 L3 线格式。
//!
//! # 出口约束（设计 §3.1，强制）
//!
//! 本模块声明的 [`Underlay`] 是协议库**唯一**的出站连接入口：
//!
//! - 协议库内所有出站连接（认证 HTTP/票据兑换、资源获取、控制中心、每个数据节点）都必须
//!   经由注入的 [`Underlay`] 完成，不允许绕开它直接使用默认拨号器或标准库网络调用。
//! - 本 crate 不提供 `Underlay` 的默认实现：宿主必须注入自己的实现（Windows VGDC/binder、
//!   Darwin binder，T07/T10）。
//! - 该约束**不**扩大到外部浏览器：浏览器认证页面的请求沿用浏览器自身网络配置，不能被
//!   宣称为受 EXV socket binder 控制（设计 §5.3）。
//!
//! 解析与选路属于宿主出口决策：宿主负责让解析答案与连接实际使用的接口一致，并在目标落入
//! 校园路由时先完成防回环再建连（设计 §7.2）。协议库只表达“要连哪个 host:port、用于什么
//! 用途”，不自行探测物理出口、不缓存解析结果、不做隐式重试。

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::UnderlayError;

pub mod l3;

/// 出站连接的用途。
///
/// 设计 §3.1 要求显式区分认证入口、资源入口与数据节点：三者可能不是同一 host，宿主的出口
/// 决策与 UI 提示都按用途区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectPurpose {
    /// 认证入口（浏览器登录结果交换、会话建立）。
    AuthGateway,
    /// 资源入口（资源清单与控制中心）。
    ResourceGateway,
    /// 数据节点（L3/TCP 隧道对端）。
    DataNode,
}

impl ConnectPurpose {
    /// 稳定机器码，供宿主日志与 UI 映射。
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::AuthGateway => "auth-gateway",
            Self::ResourceGateway => "resource-gateway",
            Self::DataNode => "data-node",
        }
    }
}

impl std::fmt::Display for ConnectPurpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// 一次出站连接请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundTarget {
    /// 目标主机名或 IP 字面量（与资源/节点数据里出现的形态一致，不做补全）。
    pub host: String,
    /// 目标端口。
    pub port: u16,
    /// 该连接的用途。
    pub purpose: ConnectPurpose,
}

impl OutboundTarget {
    /// 构造。
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16, purpose: ConnectPurpose) -> Self {
        Self {
            host: host.into(),
            port,
            purpose,
        }
    }
}

/// 宿主出口提供的双向字节流（TLS 之下或之上的裸流）。
pub trait UnderlayStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> UnderlayStream for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

/// [`Underlay::resolve`] 返回的 future。
pub type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, UnderlayError>> + Send + 'a>>;

/// [`Underlay::connect`] 返回的 future。
pub type ConnectFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn UnderlayStream>, UnderlayError>> + Send + 'a>>;

/// 宿主出口端口：解析 + 按宿主出口建连。
///
/// 实现者（宿主）负责：
///
/// - 用宿主自己的出口策略解析与建连（Windows VGDC/binder、Darwin binder），保证流量不从
///   EXV 自己的隧道绕回；
/// - 对落入校园路由的目标先安排防回环；无法保证时只让本次连接失败，不静默改道；
/// - 把自身失败映射到 [`crate::error::UnderlayErrorKind`] 的封闭词汇，不传递原始错误字符串
///   （见 [`crate::error`] 的脱敏约定）。
///
/// 实现者不得：使用系统默认解析器后直接默认拨号、修改系统网卡/路由/DNS/代理、替调用方重试
/// 或降级 TLS 验证。
///
/// # 例子
///
/// ```
/// use std::net::{IpAddr, Ipv4Addr};
/// use exv_vpn_atrust::transport::{ConnectFuture, ConnectPurpose, OutboundTarget, ResolveFuture, Underlay};
/// use exv_vpn_atrust::error::UnderlayError;
///
/// /// 一个只回答解析结果的假端口（真实实现属于宿主）。
/// struct FakeUnderlay;
///
/// impl Underlay for FakeUnderlay {
///     fn resolve(&self, _host: &str, _port: u16) -> ResolveFuture<'_> {
///         Box::pin(async { Ok(vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))]) })
///     }
///
///     fn connect(&self, _target: &OutboundTarget) -> ConnectFuture<'_> {
///         Box::pin(async {
///             Err(UnderlayError::new(
///                 exv_vpn_atrust::UnderlayErrorKind::ConnectFailed,
///                 ConnectPurpose::DataNode,
///             ))
///         })
///     }
/// }
/// ```
pub trait Underlay: Send + Sync {
    /// 在宿主出口上解析主机名。
    ///
    /// `host` 可能是主机名或已是 IP 字面量；实现必须返回该出口上可用的地址集合，
    /// 不得回落到系统默认解析器后直接使用其结果。
    fn resolve(&self, host: &str, port: u16) -> ResolveFuture<'_>;

    /// 按宿主出口建立一条到 `target` 的连接。
    fn connect(&self, target: &OutboundTarget) -> ConnectFuture<'_>;
}

