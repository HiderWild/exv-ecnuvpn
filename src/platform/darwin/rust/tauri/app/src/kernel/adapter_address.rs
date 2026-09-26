//! 读取 EXV 隧道（engine 创建的 utun 接口）当前的 IPv4 地址（W2-A/P5；win32
//! `exv-vpn-win32-resource/src/adapter_address.rs` 的 darwin 对应实装）。
//!
//! 这是只读观测接口：不缓存、不创建接口、不修改地址。连接页的「校内地址」使用它
//! 读取实际 utun 接口的当前地址；未连接、地址尚未完成分配或候选歧义时返回
//! `Ok(None)`，而不是用配置占位值冒充真实地址。两侧同为壳侧本地 seam（win32 读
//! Wintun 适配器，darwin 枚举系统 utun），不经 wire、无 proto 变更；语义契约由
//! 两侧模块文档互指冻结（darwin↔win32 顶层对齐计划 §2 P5）。
//!
//! 判据（macOS utun 无 friendly name，不基于名字 token，只基于地址事实；与 core
//! `exv-vpn-darwin-core` proxy_tun 上游检测共享同一事实面与 fake-ip 语义，方向相反
//! ——proxy_tun 找上游代理 TUN，本模块找 EXV 自己的 utun 并排除上游）：
//!   * 候选 = `utun` 前缀接口（engine 的 TUN 走系统 utun 家族；getifaddrs 无特权
//!     只读，普通用户壳可观测 root engine 创建的接口）；
//!   * E1 排除持有 `198.18.0.0/15`（RFC 2544 基准段，代理 fake-ip 惯用段）地址的
//!     接口——那是 Mihomo/Clash 等上游代理 TUN，不是 EXV 的隧道；
//!   * E2 排除仅持 `169.254.0.0/16` 链路本地地址的接口（无已分配的真实单播地址）；
//!   * E3 排除无 IPv4 地址的接口（IPv6 不作判据，与 proxy_tun 的 IPv4 面一致）。
//!
//! 剩余候选恰 1 个 → 返回其首个 IPv4；0 个（未连接/未分配）或 >1 个（歧义）→
//! `None`。歧义时诚实返回 `None`，不按枚举序猜。

// getifaddrs 的原始解析做窄化转换（sockaddr 族比对、标志位聚合）；中文文档中的
// 技术术语不逐个加反引号（照 core proxy_tun 同款模块级豁免）。
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::doc_markdown)]

use std::ffi::CStr;
use std::net::Ipv4Addr;

/// macOS TUN 接口名前缀：候选接口必须是系统 utun（与 core proxy_tun 同一口径）。
const UTUN_PREFIX: &str = "utun";

/// fake-ip 判据：`198.18.0.0/15`（照 core `proxy_tun.rs` 的 `is_fake_ip` 与 Engine
/// `egress/resolver.rs` 同一常量判据）。
fn is_fake_ip_address(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

/// IPv4 链路本地判据：`169.254.0.0/16`（RFC 3927）。
fn is_link_local_address(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 169 && octets[1] == 254
}

/// 纯判据：从注入的接口 IPv4 事实（接口名 → 该接口全部 IPv4 地址）判定 EXV 隧道
/// 地址（无任何系统调用；生产线事实来自 [`collect_interface_ipv4_facts`]）。
///
/// 排除与命中规则见模块文档；剩余候选 0 个或 >1 个返回 `None`（歧义诚实）。
#[must_use]
pub(crate) fn first_ipv4_from_facts(interfaces: &[(String, Vec<Ipv4Addr>)]) -> Option<Ipv4Addr> {
    let mut hit: Option<Ipv4Addr> = None;
    for (name, addresses) in interfaces {
        if !name.starts_with(UTUN_PREFIX) || addresses.is_empty() {
            continue; // 非 utun 接口，或 E3（无 IPv4）。
        }
        if addresses.iter().any(|address| is_fake_ip_address(*address)) {
            continue; // E1：上游代理 TUN。
        }
        if addresses.iter().all(|address| is_link_local_address(*address)) {
            continue; // E2：仅持链路本地。
        }
        if hit.is_some() {
            return None; // 双候选歧义：不按枚举序猜。
        }
        hit = addresses.first().copied();
    }
    hit
}

/// 生产观测：一次 getifaddrs 只读枚举 → 纯判据。每次调用重新枚举（无缓存、无
/// State、可重入）；`getifaddrs` 是无特权只读系统调用，无任何副作用。
///
/// # Errors
///
/// `getifaddrs` 系统调用失败时返回 OS 错误；判得 `None`（未连接/未分配/歧义）不是
/// 错误。
pub(crate) fn first_ipv4_for_exv_utun() -> Result<Option<Ipv4Addr>, std::io::Error> {
    Ok(first_ipv4_from_facts(&collect_interface_ipv4_facts()?))
}

/// 收集接口 IPv4 事实（`getifaddrs`；AF_INET、IFF_UP|RUNNING，按接口名聚合——与
/// core `proxy_tun.rs` 的收集同款；不调 `if_nametoindex`，判据无需接口索引）。
///
/// # Errors
///
/// `getifaddrs` 系统调用失败时返回 OS 错误。
fn collect_interface_ipv4_facts() -> Result<Vec<(String, Vec<Ipv4Addr>)>, std::io::Error> {
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: addrs 是 getifaddrs 的内核分配链表出参。
    if unsafe { libc::getifaddrs(std::ptr::addr_of_mut!(addrs)) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut facts: Vec<(String, Vec<Ipv4Addr>)> = Vec::new();
    let mut cursor = addrs;
    while !cursor.is_null() {
        // SAFETY: cursor 在 freeifaddrs 之前指向链表中的有效节点。
        let entry = unsafe { &*cursor };
        let next = entry.ifa_next;
        if !entry.ifa_addr.is_null() {
            // SAFETY: ifa_addr 非空时是有效 sockaddr。
            let family = unsafe { (*entry.ifa_addr).sa_family };
            let flags = i64::from(entry.ifa_flags);
            if family == libc::AF_INET as u8
                && flags & i64::from(libc::IFF_UP | libc::IFF_RUNNING) != 0
            {
                // SAFETY: ifa_name 是 NUL 结尾 C 字符串。
                let name = unsafe { CStr::from_ptr(entry.ifa_name) }
                    .to_string_lossy()
                    .into_owned();
                // SAFETY: AF_INET 的 sockaddr 即 sockaddr_in；s_addr 是 IPv4 八位组。
                let octets =
                    unsafe { (*entry.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr };
                let address = Ipv4Addr::from(octets.to_ne_bytes());
                if let Some((_, addresses)) = facts.iter_mut().find(|(known, _)| *known == name) {
                    addresses.push(address);
                } else {
                    facts.push((name, vec![address]));
                }
            }
        }
        cursor = next;
    }
    // SAFETY: 链表由 getifaddrs 分配，交还 freeifaddrs。
    unsafe { libc::freeifaddrs(addrs) };
    Ok(facts)
}
