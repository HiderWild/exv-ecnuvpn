//! Darwin 物理出口探测、IPv4 网关解析与 `IP_BOUND_IF` 出口绑定。
//!
//! 本模块只读取网络事实并把出站 socket 钉到物理接口；不创建 utun、不改路由/DNS。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

pub mod binder;
pub mod physical_route;
pub mod resolver;

use std::fmt;

/// egress 各阶段的 typed 错误；文本只含稳定类别与 OS errno，不含主机名。
#[derive(Debug)]
pub enum EgressError {
    /// `getifaddrs` 探测失败。
    InterfaceProbe(std::io::Error),
    /// PF_ROUTE dump sysctl 失败。
    RouteDump(std::io::Error),
    /// 系统路由表没有 IPv4 默认路由。
    NoDefaultGateway,
    /// 没有可用（UP/RUNNING、有 IPv4、非虚拟）的物理出口接口。
    NoPhysicalEgress,
    /// 系统解析器失败。
    Resolve(std::io::Error),
    /// 解析结果中没有可用的 IPv4（含仅 IPv6 或未指定地址）。
    NoIpv4Answer,
}

impl fmt::Display for EgressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InterfaceProbe(_) => "DARWIN_EGRESS_INTERFACE_PROBE_FAILED",
            Self::RouteDump(_) => "DARWIN_EGRESS_ROUTE_DUMP_FAILED",
            Self::NoDefaultGateway => "DARWIN_EGRESS_NO_DEFAULT_GATEWAY",
            Self::NoPhysicalEgress => "DARWIN_EGRESS_NO_PHYSICAL_INTERFACE",
            Self::Resolve(_) => "DARWIN_EGRESS_RESOLVE_FAILED",
            Self::NoIpv4Answer => "DARWIN_EGRESS_NO_IPV4_ANSWER",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for EgressError {}
