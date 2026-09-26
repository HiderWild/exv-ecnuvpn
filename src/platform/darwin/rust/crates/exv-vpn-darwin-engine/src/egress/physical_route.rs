//! 读取真实物理出口：接口 IPv4 事实（`getifaddrs`）+ 默认网关（PF_ROUTE dump）。
//!
//! 物理出口 = 持有默认路由的接口；排除 utun、loopback 等虚拟接口；找不到持有
//! 默认路由且 IPv4 可用的物理接口时 fail closed。本模块只读取事实，不做任何
//! 网络修改。

// PF_ROUTE/getifaddrs 的原始解析在逐字段边界检查后做窄化转换；中文文档中的
// 技术术语不逐个加反引号。
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::cast_ptr_alignment)]
#![allow(clippy::needless_borrows_for_generic_args)]

use std::ffi::CStr;
use std::net::Ipv4Addr;

use crate::egress::EgressError;

/// 一次连接内固定的物理出口事实。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalEgress {
    /// 物理 IPv4 出口的接口索引（`IP_BOUND_IF` 输入）。
    pub ifindex: u32,
    /// 接口名（诊断用，如 `en0`）。
    pub interface: &'static str,
    /// 该出口的 IPv4 地址。
    pub address: Ipv4Addr,
    /// 默认网关 IPv4（来自 PF_ROUTE dump）。
    pub gateway: Ipv4Addr,
}

/// 名称或路径明确属于非物理出口的 macOS 接口前缀。
fn is_excluded_interface(name: &[u8]) -> bool {
    const EXCLUDED: [&[u8]; 10] = [
        b"utun", b"lo", b"awdl", b"llw", b"bridge", b"ap", b"vlan", b"gif", b"stf", b"tap",
    ];
    EXCLUDED.iter().any(|prefix| name.starts_with(prefix))
}

/// 收集持有 IPv4、UP 且 RUNNING 的非虚拟接口 `(ifindex, name, address)`。
///
/// # Errors
///
/// `getifaddrs` 系统调用失败时返回 [`EgressError::InterfaceProbe`]。
pub fn collect_interface_facts() -> Result<Vec<(u32, &'static str, Ipv4Addr)>, EgressError> {
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: addrs is the out pointer for a kernel-allocated list.
    if unsafe { libc::getifaddrs(std::ptr::addr_of_mut!(addrs)) } != 0 {
        return Err(EgressError::InterfaceProbe(std::io::Error::last_os_error()));
    }
    let mut facts: Vec<(u32, &'static str, Ipv4Addr)> = Vec::new();
    let mut cursor = addrs;
    while !cursor.is_null() {
        // SAFETY: cursor points at a valid chain node until freeifaddrs.
        let entry = unsafe { &*cursor };
        let next = entry.ifa_next;
        if !entry.ifa_addr.is_null() {
            // SAFETY: ifa_addr is a valid sockaddr when non-null.
            let family = unsafe { (*entry.ifa_addr).sa_family };
            if family == libc::AF_INET as u8 {
                // SAFETY: ifa_name is a valid NUL-terminated C string.
                let name = unsafe { CStr::from_ptr(entry.ifa_name) }
                    .to_bytes()
                    .to_vec();
                // SAFETY: AF_INET sockaddr is sockaddr_in; s_addr is the IPv4 octets.
                let octets = unsafe {
                    (*entry.ifa_addr.cast::<libc::sockaddr_in>())
                        .sin_addr
                        .s_addr
                };
                let address = Ipv4Addr::from(octets.to_ne_bytes());
                let flags = i64::from(entry.ifa_flags);
                if !is_excluded_interface(&name)
                    && flags & i64::from(libc::IFF_UP | libc::IFF_RUNNING) != 0
                {
                    let c_name = std::ffi::CString::new(name.clone()).map_err(|_| {
                        EgressError::InterfaceProbe(std::io::Error::from_raw_os_error(libc::EINVAL))
                    })?;
                    // SAFETY: if_nametoindex only reads the name.
                    let ifindex = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
                    if ifindex != 0 {
                        let leaked: &'static str =
                            Box::leak(String::from_utf8_lossy(&name).into_owned().into_boxed_str());
                        facts.push((ifindex, leaked, address));
                    }
                }
            }
        }
        cursor = next;
    }
    // SAFETY: the list was allocated by getifaddrs.
    unsafe { libc::freeifaddrs(addrs) };
    Ok(facts)
}

/// 从 PF_ROUTE dump（`net.route.0.0.dump`）读取默认路由的 `(ifindex, gateway)`。
///
/// # Errors
///
/// sysctl 失败或 dump 中没有 IPv4 默认路由时返回 typed 错误。
pub fn default_gateway() -> Result<(u32, Ipv4Addr), EgressError> {
    let route_dump = sysctl_net_route_dump()?;
    let header_size = std::mem::size_of::<libc::rt_msghdr>();
    let mut cursor = 0usize;
    while cursor + header_size <= route_dump.len() {
        // SAFETY: cursor is bounds-checked against header_size above.
        let header = unsafe {
            route_dump
                .as_ptr()
                .add(cursor)
                .cast::<libc::rt_msghdr>()
                .read_unaligned()
        };
        let message_len = usize::from(header.rtm_msglen);
        if message_len < header_size || cursor + message_len > route_dump.len() {
            break;
        }
        if header.rtm_version == libc::RTM_VERSION as u8
            && header.rtm_type == libc::RTM_GET as u8
            && header.rtm_addrs & libc::RTA_DST != 0
        {
            let body = &route_dump[cursor + header_size..cursor + message_len];
            if let Some((gateway, is_default)) = parse_gateway(body, header.rtm_addrs)
                && is_default
            {
                return Ok((u32::from(header.rtm_index), gateway));
            }
        }
        cursor += message_len;
    }
    Err(EgressError::NoDefaultGateway)
}

/// 依 `rtm_addrs` 位序遍历 sockaddr 序列，返回 `RTA_GATEWAY` 的 IPv4 值与
/// 默认路由判定（`RTA_NETMASK` 缺失或为 0.0.0.0，且 DST 为 0.0.0.0）。
fn parse_gateway(body: &[u8], addrs: i32) -> Option<(Ipv4Addr, bool)> {
    const RTA_NETMASK_BIT: i32 = 0x4;
    let mut offset = 0usize;
    let mut gateway = None;
    let mut destination_zero = false;
    let mut netmask_zero = true;
    for bit in 0..12 {
        if addrs & (1 << bit) == 0 {
            continue;
        }
        if offset >= body.len() {
            return None;
        }
        let length = usize::from(body[offset]);
        let family = body[offset + 1];
        let slot_len = if length == 0 {
            4
        } else {
            length.div_ceil(4) * 4
        };
        if offset + slot_len > body.len() {
            return None;
        }
        let slot = &body[offset..offset + slot_len];
        if 1 << bit == libc::RTA_DST && family == libc::AF_INET as u8 && slot.len() >= 8 {
            destination_zero = slot[4..8] == [0, 0, 0, 0];
        }
        if 1 << bit == libc::RTA_GATEWAY && family == libc::AF_INET as u8 && slot.len() >= 8 {
            gateway = Some(Ipv4Addr::new(slot[4], slot[5], slot[6], slot[7]));
        }
        if 1 << bit == RTA_NETMASK_BIT && family == libc::AF_INET as u8 && slot.len() >= 8 {
            netmask_zero = slot[4..8] == [0, 0, 0, 0];
        }
        offset += slot_len;
    }
    gateway.map(|gateway| (gateway, destination_zero && netmask_zero))
}

fn sysctl_net_route_dump() -> Result<Vec<u8>, EgressError> {
    // macOS 的 PF_ROUTE dump 没有可按名查询的 OID（sysctlbyname 返回 ENOENT），
    // 必须使用数字 MIB：{CTL_NET, PF_ROUTE, 0, AF_UNSPEC, NET_RT_DUMP, 0}。
    let mut mib: [libc::c_int; 6] = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        libc::AF_UNSPEC,
        libc::NET_RT_DUMP,
        0,
    ];
    let mut size: usize = 0;
    // SAFETY: NULL buffer queries the required size.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(size),
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || size == 0 {
        return Err(EgressError::RouteDump(std::io::Error::last_os_error()));
    }
    let mut buffer = vec![0_u8; size];
    // SAFETY: buffer was sized by the probe call above.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            std::ptr::addr_of_mut!(size),
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return Err(EgressError::RouteDump(std::io::Error::last_os_error()));
    }
    buffer.truncate(size);
    Ok(buffer)
}

/// 探测物理 IPv4 出口：持有默认路由且 IPv4 可用的非虚拟接口。
///
/// # Errors
///
/// 无默认网关、无可用物理 IPv4 接口，或默认路由不在任何已启用物理接口上时
/// 返回 typed 错误；失败前没有任何网络修改。
pub fn find_physical_egress() -> Result<PhysicalEgress, EgressError> {
    let (default_ifindex, gateway) = default_gateway()?;
    select_default_route_egress(default_ifindex, gateway, collect_interface_facts()?)
}

/// 以已读取的默认路由和接口事实构造物理出口。
///
/// 保持为独立纯函数，使本地测试能注入默认路由事实，证明不会因枚举顺序误选另一张
/// IPv4 网卡。生产入口仍在本函数外先读取 PF_ROUTE 与 `getifaddrs` 的真实事实。
fn select_default_route_egress(
    default_ifindex: u32,
    gateway: Ipv4Addr,
    facts: Vec<(u32, &'static str, Ipv4Addr)>,
) -> Result<PhysicalEgress, EgressError> {
    let interface = facts
        .into_iter()
        .find(|(ifindex, _, address)| {
            *ifindex == default_ifindex
                && *address != Ipv4Addr::UNSPECIFIED
                && *address != Ipv4Addr::LOCALHOST
        })
        .ok_or(EgressError::NoPhysicalEgress)?;
    let (ifindex, name, address) = interface;
    Ok(PhysicalEgress {
        ifindex,
        interface: name,
        address,
        gateway,
    })
}
