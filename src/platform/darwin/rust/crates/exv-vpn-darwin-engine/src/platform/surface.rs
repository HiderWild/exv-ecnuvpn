//! M4：管线平台步骤的可注入操作面（utun/地址/路由/resolver）。
//!
//! 生产唯一实现 [`RealSurface`] 逐字段转发既有
//! [`utun`]/[`interface`]/[`route`]/[`crate::campus_resolver`] 的生产函数——
//! 引入本 trait 只为给管线级密闭测试（E-2）一个假实现接缝（假设备/内存路由表/
//! 临时 resolver 目录），不改变任何生产行为。注入纪律与
//! [`crate::underlay`] 的接口选择注入同源：生产构造固定，测试经
//! [`crate::bootstrap_runtime::EngineRuntime`] 的测试注入口替换。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use std::net::Ipv4Addr;
use std::path::Path;

use super::PlatformError;
use super::route::AppliedRoute;
use super::utun::Utun;
use crate::campus_resolver::{self, InstalledResolver, ResolverError};

/// 平台操作面（管线与账本的全部平台副作用经此发出）。
pub(crate) trait PlatformSurface: Send + Sync {
    /// 假平台默认无宿主副作用；生产仅由 RealSurface 接线。
    fn apply_proxy_policy(&self, _routes: &[String], _domains: &[String]) -> (Option<crate::system_proxy::Owner>, crate::system_proxy::Result<crate::system_proxy::ApplyStatus>) {
        (None, Ok(crate::system_proxy::ApplyStatus::Disabled))
    }
    fn cleanup_proxy_policy(&self, owner: &mut crate::system_proxy::Owner) -> crate::system_proxy::Result<crate::system_proxy::CleanupStatus> {
        owner.cleanup(&mut crate::system_proxy::NativeStore)
    }
    fn proxy_policy_effective(&self, owner: &mut crate::system_proxy::Owner) -> crate::system_proxy::Result<Option<&'static str>> {
        owner.effective_mode(&mut crate::system_proxy::NativeStore)
    }
    /// 创建 utun 设备（生产=`Utun::acquire`）。
    fn acquire_device(&self) -> Result<Utun, PlatformError>;
    /// 接口地址/MTU 应用（生产=`interface::apply_ipv4`）。
    fn apply_ipv4(
        &self,
        device: &Utun,
        address: Ipv4Addr,
        prefix: u8,
        mtu: u16,
    ) -> Result<(), PlatformError>;
    /// 移除接口地址（生产=`interface::clear_ipv4`）。
    fn clear_ipv4(&self, device: &Utun, address: Ipv4Addr) -> Result<(), PlatformError>;
    /// 新增精确路由（生产=`route::add`）。
    fn route_add(&self, route: &AppliedRoute) -> Result<(), PlatformError>;
    /// 变更已存在路由（生产=`route::change`）。
    fn route_change(&self, route: &AppliedRoute) -> Result<(), PlatformError>;
    /// 删除路由（生产=`route::delete`）。
    fn route_delete(&self, route: &AppliedRoute) -> Result<(), PlatformError>;
    /// 回读精确路由的内核事实（生产=`route::read_exact`）。
    fn route_read_exact(
        &self,
        destination: Ipv4Addr,
        prefix: u8,
    ) -> Result<Option<AppliedRoute>, PlatformError>;
    /// 安装校园域 scoped resolver（M4：使用 Core 信封显式携带的
    /// 完整的 `campus_dns_domains` 列表。
    /// 并经 `campus_dns_servers` 写 `/etc/resolver/<domain>`——以写入和 HUP 原返回处理）。
    ///
    /// # Errors
    ///
    /// 返回成功安装项；失败时返回尚未清除的实际条目与分层错误，由runtime继续持有。
    fn install_resolvers(
        &self,
        domains: &[String],
        nameservers: &[String],
        logs: &crate::log_sink::LogSink,
    ) -> (Vec<InstalledResolver>, Option<ResolverError>);
    /// teardown 删除自有 resolver 文件 + HUP（逐条 best-effort；返回失败条数供
    /// 调用方计 teardown 完整性）。
    fn remove_resolvers(
        &self,
        installed: &[InstalledResolver],
        logs: &crate::log_sink::LogSink,
    ) -> usize;
}

/// 生产实现：逐字段转发既有平台模块（行为与本 trait 引入前逐字节一致）。
pub(crate) struct RealSurface;

impl PlatformSurface for RealSurface {
    fn apply_proxy_policy(&self, routes: &[String], domains: &[String]) -> (Option<crate::system_proxy::Owner>, crate::system_proxy::Result<crate::system_proxy::ApplyStatus>) {
        let additions = match crate::system_proxy::additions(routes, domains) { Ok(value) => value, Err(error) => return (None, Err(error)) };
        let mut owner = match crate::system_proxy::Owner::open(Path::new(crate::system_proxy::OWNER_PATH)) { Ok(value) => value, Err(error) => return (None, Err(error)) };
        let result = owner.apply(&mut crate::system_proxy::NativeStore, &additions);
        // commit/apply 的失败也可能留下真实副作用，owner 必须保留到清理。
        match result {
            Ok(crate::system_proxy::ApplyStatus::Disabled | crate::system_proxy::ApplyStatus::AutomaticUnsupported | crate::system_proxy::ApplyStatus::Unchanged | crate::system_proxy::ApplyStatus::ChangedByOther) => (None, result),
            _ => (Some(owner), result),
        }
    }
    fn acquire_device(&self) -> Result<Utun, PlatformError> {
        Utun::acquire()
    }

    fn apply_ipv4(
        &self,
        device: &Utun,
        address: Ipv4Addr,
        prefix: u8,
        mtu: u16,
    ) -> Result<(), PlatformError> {
        super::interface::apply_ipv4(device, address, prefix, mtu)
    }

    fn clear_ipv4(&self, device: &Utun, address: Ipv4Addr) -> Result<(), PlatformError> {
        super::interface::clear_ipv4(device, address)
    }

    fn route_add(&self, route: &AppliedRoute) -> Result<(), PlatformError> {
        super::route::add(route)
    }

    fn route_change(&self, route: &AppliedRoute) -> Result<(), PlatformError> {
        super::route::change(route)
    }

    fn route_delete(&self, route: &AppliedRoute) -> Result<(), PlatformError> {
        super::route::delete(route)
    }

    fn route_read_exact(
        &self,
        destination: Ipv4Addr,
        prefix: u8,
    ) -> Result<Option<AppliedRoute>, PlatformError> {
        super::route::read_exact(destination, prefix)
    }

    fn install_resolvers(
        &self,
        domains: &[String],
        nameservers: &[String],
        logs: &crate::log_sink::LogSink,
    ) -> (Vec<InstalledResolver>, Option<ResolverError>) {
        if domains.is_empty() {
            // 用户清空=不写 resolver（隧道 DNS /32 路由仍生效）。
            logs.publish(
                "info",
                "platform",
                "RESOLVER_DOMAINS_EMPTY",
                "campus_dns_domains cleared; no resolver files written",
                &[],
            );
            return (Vec::new(), None);
        }
        let (installed, error) = campus_resolver::install_all(
            Path::new(campus_resolver::RESOLVER_DIR),
            domains,
            nameservers,
            campus_resolver::INCLUDE_DOMAIN_KEY,
            &campus_resolver::process_command_runner,
            logs,
        );
        (installed, error)
    }

    fn remove_resolvers(
        &self,
        installed: &[InstalledResolver],
        logs: &crate::log_sink::LogSink,
    ) -> usize {
        let mut failures = 0;
        for entry in installed {
            if let Err(error) = campus_resolver::remove(
                Path::new(campus_resolver::RESOLVER_DIR),
                entry,
                &campus_resolver::process_command_runner,
                logs,
            ) {
                failures += 1;
                logs.publish(
                    "warn",
                    "platform",
                    "RESOLVER_REMOVE_FAILED",
                    format!("resolver file removal failed: {error}"),
                    &[("domain", entry.domain.clone())],
                );
            }
        }
        failures
    }
}
