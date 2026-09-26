//! Engine 独占的平台网络资源实现（utun、接口、路由）。
//!
//! Engine 是唯一持有这些 FD 与系统修改的进程；Core/UI 永不接触。
//!
//! DNS：MVP 管线不改系统 DNS（对齐 Windows——Windows 只把 offer DNS 应用到
//! 自己的 Wintun 适配器 GUID，从不碰物理网卡 DNS；见
//! win32-engine/src/platform_tunnel.rs `luid_to_guid` → `DnsApplier::apply`）。
//! macOS 的适配器级 DNS（scoped resolver / 为 utun 建 SC service）待独立设计；
//! 隧道侧 DNS 服务器经 /32 路由可达。

pub mod interface;
pub mod route;
pub mod utun;

use std::fmt;

/// 平台网络资源的 typed 错误；文本只含稳定类别与 OS errno。
#[derive(Debug)]
pub enum PlatformError {
    /// utun 创建失败。
    Utun(&'static str, std::io::Error),
    /// 接口地址/MTU/标志 apply 失败。
    Interface(&'static str, std::io::Error),
    /// 路由 add/delete 失败（内核 ack 非零或写失败）。
    Route(&'static str, std::io::Error),
    /// readback 与期望不一致。
    Readback(&'static str),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utun(step, error) => write!(formatter, "DARWIN_PLATFORM_UTUN_{step}: {error}"),
            Self::Interface(step, error) => {
                write!(formatter, "DARWIN_PLATFORM_IF_{step}: {error}")
            }
            Self::Route(step, error) => {
                write!(formatter, "DARWIN_PLATFORM_ROUTE_{step}: {error}")
            }
            Self::Readback(step) => write!(formatter, "DARWIN_PLATFORM_READBACK_{step}"),
        }
    }
}

impl std::error::Error for PlatformError {}
