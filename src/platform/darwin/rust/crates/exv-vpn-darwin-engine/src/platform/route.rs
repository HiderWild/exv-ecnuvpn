//! 精确 IPv4 路由的 add/delete（PF_ROUTE 原始消息；不调用 shell/route 命令）。
//!
//! 每条路由都记录 `(dst, prefix, gateway, ifindex)`，删除必须匹配同一条目。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_possible_truncation)]

use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use super::PlatformError;

const AF_INET: u8 = 2;
const RTM_ADD: u8 = 0x1;
const RTM_DELETE: u8 = 0x2;
const RTM_CHANGE: u8 = 0x3;
const RTF_UP: i32 = 0x1;
const RTF_GATEWAY: i32 = 0x2;
const RTF_STATIC: i32 = 0x10;
const RTA_DST: i32 = 0x1;
const RTA_GATEWAY: i32 = 0x2;
const RTA_NETMASK: i32 = 0x4;

/// 一条已应用的路由；删除时按原值匹配。
///
/// 网关一律 INET 地址形态：隧道路由的网关是本隧道地址，物理 bypass 的网关是
/// 出口物理网关。p2p utun 上不得改用 AF_LINK 接口路由形态——见 route_message
/// 注释中的 rt_ifa 解析约束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedRoute {
    pub destination: Ipv4Addr,
    pub prefix: u8,
    pub gateway: Ipv4Addr,
    pub ifindex: u32,
}

/// 登录前控制连接专用路由的实际处置结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlRouteUpdate {
    /// 已有精确路由与本次物理网关一致，未写 PF_ROUTE。
    Reused,
    /// 原条目不存在，已创建并回读确认。
    Added,
    /// 原条目存在但下一跳或接口不同，已变更并回读确认。
    Changed,
}

/// PF_ROUTE 会话 socket。
struct RouteSocket(OwnedFd);

impl RouteSocket {
    /// # Errors
    ///
    /// socket(2) 失败。
    fn open() -> Result<Self, PlatformError> {
        // SAFETY: socket(2)。
        let fd = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, 0) };
        if fd < 0 {
            return Err(PlatformError::Route("SOCKET", io::Error::last_os_error()));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // 内核对本 socket 的 RTM 回执必须可达；2 秒兜底避免异常时永久挂起。
        let timeout = libc::timeval {
            tv_sec: 2,
            tv_usec: 0,
        };
        // SAFETY: fd 有效，timeout 是完整 timeval。
        unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                std::ptr::from_ref(&timeout).cast::<libc::c_void>(),
                u32::try_from(std::mem::size_of::<libc::timeval>()).expect("size fits u32"),
            )
        };
        Ok(Self(fd))
    }
}

/// 16 字节 AF_INET sockaddr。
fn sockaddr_in(address: Ipv4Addr) -> Vec<u8> {
    let mut bytes = vec![0_u8; 16];
    bytes[0] = 16;
    bytes[1] = AF_INET;
    bytes[4..8].copy_from_slice(&address.octets());
    bytes
}

/// 构造 `rt_msghdr + DST|GATEWAY|NETMASK` 的路由消息。
fn route_message(kind: u8, seq: i32, route: &AppliedRoute) -> Vec<u8> {
    // rt_msghdr 标量字段一律宿主字节序；仅 sockaddr 内容本身按网络序摆放。
    // 三个 sockaddr 槽均为 16 字节 AF_INET（DST/GATEWAY/NETMASK）。
    let mut message = vec![0_u8; 92 + 16 + 16 + 16];
    let message_len = u16::try_from(message.len())
        .expect("msglen fits u16")
        .to_ne_bytes();
    message[0..2].copy_from_slice(&message_len);
    message[2] = libc::RTM_VERSION as u8;
    message[3] = kind;
    let index = u16::try_from(route.ifindex)
        .expect("ifindex fits u16")
        .to_ne_bytes();
    message[4..6].copy_from_slice(&index);
    // GATEWAY 槽一律 INET 地址（隧道路由 = 本隧道地址，Clash 同机同款形态）。
    // AF_LINK 接口路由形态（`route add -interface utunN`）在 p2p utun 上建路由时
    // rt_ifa 解析不到接口地址：dstaddr=自身不等于任意目标地址，窄掩码也不包含
    // 目标——未绑定源的 sendto 因无源可选报 EADDRNOTAVAIL（真机实锤：显式 -S
    // 可通、隐式失败）；INET 网关让内核按网关解析 ifa，命中 dstaddr=自身。
    let flags = RTF_UP | RTF_GATEWAY | RTF_STATIC;
    let addrs = RTA_DST | RTA_GATEWAY | RTA_NETMASK;
    message[8..12].copy_from_slice(&flags.to_ne_bytes());
    message[12..16].copy_from_slice(&addrs.to_ne_bytes());
    message[20..24].copy_from_slice(&seq.to_ne_bytes());

    let mut offset = 92;
    let mut slot = RTA_DST;
    while slot <= RTA_NETMASK {
        let value: Vec<u8> = match slot {
            RTA_DST => sockaddr_in(route.destination),
            RTA_GATEWAY => sockaddr_in(route.gateway),
            _ => sockaddr_in(prefix_to_mask(route.prefix)),
        };
        message[offset..offset + value.len()].copy_from_slice(&value);
        offset += value.len();
        slot <<= 1;
    }
    message
}

/// `ack_timeout_ms` 供删除路径快速失败（批量删除时不逐条长等）。
fn write_and_ack_inner(
    socket: &RouteSocket,
    message: &[u8],
    ack_timeout_ms: u64,
) -> Result<(), PlatformError> {
    // SAFETY: fd 有效，message 是完整路由消息。
    let written = unsafe {
        libc::write(
            socket.0.as_raw_fd(),
            message.as_ptr().cast::<libc::c_void>(),
            message.len(),
        )
    };
    if written < 0 {
        return Err(PlatformError::Route("WRITE", io::Error::last_os_error()));
    }
    // 收包超时：add 需要确认；delete 批量场景用短超时。
    let timeout = libc::timeval {
        tv_sec: libc::time_t::try_from(ack_timeout_ms / 1000).unwrap_or(0),
        tv_usec: libc::suseconds_t::try_from((ack_timeout_ms % 1000) * 1000).unwrap_or(0),
    };
    // SAFETY: fd 有效，timeout 是完整 timeval。
    unsafe {
        libc::setsockopt(
            socket.0.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            std::ptr::from_ref(&timeout).cast::<libc::c_void>(),
            u32::try_from(std::mem::size_of::<libc::timeval>()).expect("size fits u32"),
        )
    };
    let mut reply = [0_u8; 512];
    // SAFETY: read(2) into a valid buffer.
    let read = unsafe {
        libc::read(
            socket.0.as_raw_fd(),
            std::ptr::from_mut(&mut reply).cast::<libc::c_void>(),
            reply.len(),
        )
    };
    if read < 92 {
        return Err(PlatformError::Route(
            "ACK",
            io::Error::from_raw_os_error(libc::EIO),
        ));
    }
    // ack 里的标量同样是宿主字节序。
    let errno = i32::from_ne_bytes([reply[24], reply[25], reply[26], reply[27]]);
    if errno != 0 {
        return Err(PlatformError::Route(
            "ACK",
            io::Error::from_raw_os_error(errno),
        ));
    }
    Ok(())
}

fn prefix_to_mask(prefix: u8) -> Ipv4Addr {
    let mask = u32::MAX << (32 - u32::from(prefix));
    Ipv4Addr::from(mask)
}

/// 添加一条路由并等待内核 ack。
///
/// # Errors
///
/// 写入或内核 ack 失败返回 typed 错误（RTM_ADD 对已存在条目返回 EEXIST；
/// 与 delete 的 ESRCH 同理，目标状态已在，视为成功——崩溃后重试的幂等性
/// 依赖此语义，残留路由不得阻断下一次连接）。
pub fn add(route: &AppliedRoute) -> Result<(), PlatformError> {
    let socket = RouteSocket::open()?;
    write_and_ack_inner(&socket, &route_message(RTM_ADD, 1, route), 300)
}

/// 修改一条已存在的精确路由并等待内核 ack。
///
/// 仅由已经读取并比较过的同一目的/前缀条目调用；不能把 `EEXIST` 当作成功。
pub fn change(route: &AppliedRoute) -> Result<(), PlatformError> {
    let socket = RouteSocket::open()?;
    write_and_ack_inner(&socket, &route_message(RTM_CHANGE, 1, route), 300)
}

/// 删除一条与原值匹配的路由并等待内核 ack。
///
/// # Errors
///
/// 写入或内核 ack 失败返回 typed 错误（RTM_DELETE 对不存在条目返回 ESRCH，
/// 调用方在清理路径可把 ESRCH 视为已达目的状态）。
pub fn delete(route: &AppliedRoute) -> Result<(), PlatformError> {
    let socket = RouteSocket::open()?;
    match write_and_ack_inner(&socket, &route_message(RTM_DELETE, 1, route), 300) {
        Ok(()) => Ok(()),
        Err(PlatformError::Route("ACK", error)) if error.raw_os_error() == Some(libc::ESRCH) => {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// 读取精确 IPv4 路由的实际 `(destination, prefix, gateway, ifindex)`。
///
/// 这里读的是 PF_ROUTE dump 的内核事实，不从此前调用的缓存、添加参数或 `EEXIST`
/// 推断结果。网关为非 IPv4 形式的条目不是本 Engine 可管理的控制路由，因而不返回。
pub fn read_exact(
    destination: Ipv4Addr,
    prefix: u8,
) -> Result<Option<AppliedRoute>, PlatformError> {
    let dump = route_dump()?;
    let header_size = std::mem::size_of::<libc::rt_msghdr>();
    let mut cursor = 0usize;
    while cursor + header_size <= dump.len() {
        // SAFETY: cursor has been bounds checked for a complete header.
        let header = unsafe {
            dump.as_ptr()
                .add(cursor)
                .cast::<libc::rt_msghdr>()
                .read_unaligned()
        };
        let length = usize::from(header.rtm_msglen);
        if length < header_size || cursor + length > dump.len() {
            break;
        }
        if header.rtm_version == libc::RTM_VERSION as u8
            && header.rtm_type == libc::RTM_GET as u8
            && let Some((actual_destination, actual_prefix, gateway)) = parse_route_addresses(
                &dump[cursor + header_size..cursor + length],
                header.rtm_addrs,
                header.rtm_flags,
            )
            && actual_destination == destination
            && actual_prefix == prefix
        {
            return Ok(Some(AppliedRoute {
                destination: actual_destination,
                prefix: actual_prefix,
                gateway,
                ifindex: u32::from(header.rtm_index),
            }));
        }
        cursor += length;
    }
    Ok(None)
}

fn route_dump() -> Result<Vec<u8>, PlatformError> {
    let mut mib: [libc::c_int; 6] = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        libc::AF_UNSPEC,
        libc::NET_RT_DUMP,
        0,
    ];
    let mut size = 0usize;
    // SAFETY: NULL buffer requests the required route dump size.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(size),
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size == 0
    {
        return Err(PlatformError::Route("DUMP", io::Error::last_os_error()));
    }
    let mut dump = vec![0_u8; size];
    // SAFETY: dump is allocated to the size returned by the previous sysctl call.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            dump.as_mut_ptr().cast(),
            std::ptr::addr_of_mut!(size),
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(PlatformError::Route("DUMP", io::Error::last_os_error()));
    }
    dump.truncate(size);
    Ok(dump)
}

fn parse_route_addresses(body: &[u8], addrs: i32, flags: i32) -> Option<(Ipv4Addr, u8, Ipv4Addr)> {
    let mut offset = 0usize;
    let mut destination = None;
    let mut gateway = None;
    let mut mask = None;
    for bit in 0..12 {
        let flag = 1_i32 << bit;
        if addrs & flag == 0 {
            continue;
        }
        let length = usize::from(*body.get(offset)?);
        let family = *body.get(offset + 1)?;
        let slot_len = if length == 0 {
            4
        } else {
            length.div_ceil(4) * 4
        };
        let slot = body.get(offset..offset + slot_len)?;
        if flag == RTA_NETMASK {
            // Darwin 的压缩 NETMASK 不是 sockaddr 语义：真机 /32 为
            // `08 ff ff ff ff ff ff ff`（family 字节恰为 255），不能按 family
            // 筛选。掩码总是槽位 offset 4 起的至多四字节；更短的槽以 0 补齐。
            mask = Some(Ipv4Addr::new(
                *slot.get(4).unwrap_or(&0),
                *slot.get(5).unwrap_or(&0),
                *slot.get(6).unwrap_or(&0),
                *slot.get(7).unwrap_or(&0),
            ));
            offset += slot_len;
            continue;
        }
        if family == AF_INET && slot.len() >= 8 {
            let address = Ipv4Addr::new(slot[4], slot[5], slot[6], slot[7]);
            match flag {
                RTA_DST => destination = Some(address),
                RTA_GATEWAY => gateway = Some(address),
                _ => {}
            }
        }
        offset += slot_len;
    }
    let prefix = if flags & libc::RTF_HOST != 0 {
        32
    } else {
        mask_to_prefix(mask?)?
    };
    Some((destination?, prefix, gateway?))
}

fn mask_to_prefix(mask: Ipv4Addr) -> Option<u8> {
    let value = u32::from(mask);
    let prefix = value.leading_ones() as u8;
    let canonical = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    (value == canonical).then_some(prefix)
}
