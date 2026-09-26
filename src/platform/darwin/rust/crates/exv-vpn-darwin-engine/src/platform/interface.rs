//! 接口地址（含点对点 dstaddr=自身）、UP、MTU 的 apply 与 readback（AF_INET ioctl）。
//!
//! 只操作 Engine 自建 utun 的 ifindex/名字；不改其他接口。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

use std::io;
use std::net::Ipv4Addr;
use std::os::fd::RawFd;

use super::PlatformError;
use crate::platform::utun::Utun;

const AF_INET: u8 = 2;
/// `_IOW('i', 12, struct ifreq)`。addr/dstaddr/netmask 单步 ioctl 路线已真机验证
/// 可用（历史上的「SIOCAIFADDR EOPNOTSUPP」结论同样出自错误的常量 era，未复核）。
const SIOCSIFADDR: u64 = 0x8020_690c;
/// `_IOW('i', 25, struct ifreq)`；仅删除本 Engine 先前给 utun 设置的 IPv4 地址。
const SIOCDIFADDR: u64 = 0x8020_6919;
/// `_IOW('i', 14, struct ifreq)`（sys/sockio.h）。**勘误（2026-09-08）**：历史值
/// 误写为 `'i',15`（0x8020_690f）——对错误 ioctl 号得到的 EOPNOTSUPP 被误判为
/// 「utun 不支持设置 dstaddr」，导致接口残缺为 `--> 0.0.0.0`，macOS 源地址选择
/// 拒绝出接口 → ping `sendto: Can't assign requested address`（真机实锤：Clash 的
/// utun 配 `dstaddr=自身` 即正常）。
const SIOCSIFDSTADDR: u64 = 0x8020_690e;
/// `_IOWR('i', 34, struct ifreq)`（sys/sockio.h）：dstaddr 回读。
const SIOCGIFDSTADDR: u64 = 0xC020_6922;
/// `_IOW('i', 22, struct ifreq)`（sys/sockio.h）：设置 IPv4 掩码。
const SIOCSIFNETMASK: u64 = 0x8020_6916;
/// `_IOWR('i', 37, struct ifreq)`（sys/sockio.h）：掩码回读。
const SIOCGIFNETMASK: u64 = 0xC020_6925;
/// `_IOW('i', 52, struct ifreq)`（sys/sockio.h）。
const SIOCSIFMTU: u64 = 0x8020_6934;
/// `_IOWR('i', 51, struct ifreq)`（sys/sockio.h）。
const SIOCGIFMTU: u64 = 0xC020_6933;
/// `_IOWR('i', 17, struct ifreq)`。
const SIOCGIFFLAGS: u64 = 0xC020_6911;
/// `_IOW('i', 16, struct ifreq)`。
const SIOCSIFFLAGS: u64 = 0x8020_6910;
const IFF_UP: i16 = 0x1;

/// `struct sockaddr` 通用 16 字节形态（AF_INET）。
#[repr(C)]
#[derive(Clone, Copy)]
struct RawSockaddrIn {
    bytes: [u8; 16],
}

impl RawSockaddrIn {
    fn ipv4(address: Ipv4Addr) -> Self {
        let mut bytes = [0_u8; 16];
        bytes[0] = 16;
        bytes[1] = AF_INET;
        bytes[4..8].copy_from_slice(&address.octets());
        Self { bytes }
    }
}

/// `struct ifreq`：name + union（flags i16 / mtu i32 / sockaddr 16 字节）。
#[repr(C)]
struct IfReq {
    name: [u8; 16],
    value: [u8; 16],
}

fn interface_req(name: &str) -> IfReq {
    let mut req = IfReq {
        name: [0; 16],
        value: [0; 16],
    };
    req.name[..name.len()].copy_from_slice(name.as_bytes());
    req
}

fn interface_req_with_addr(name: &str, address: RawSockaddrIn) -> IfReq {
    let mut req = interface_req(name);
    req.value = address.bytes;
    req
}

/// 一次 AF_INET ioctl 会话（fd 只在本模块使用）。
struct IoctlSocket(RawFd);

impl IoctlSocket {
    /// # Errors
    ///
    /// socket(2) 失败。
    fn open() -> Result<Self, PlatformError> {
        // SAFETY: socket(2)。
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            return Err(PlatformError::Interface(
                "SOCKET",
                io::Error::last_os_error(),
            ));
        }
        Ok(Self(fd))
    }

    /// # Errors
    ///
    /// ioctl 失败；`name` 是失败的请求常量名（诊断用）。
    fn ioctl_named(
        &self,
        request: u64,
        name: &'static str,
        argument: *mut libc::c_void,
    ) -> Result<(), PlatformError> {
        // SAFETY: fd 有效，argument 指向完整请求结构。
        if unsafe { libc::ioctl(self.0, request, argument) } != 0 {
            return Err(PlatformError::Interface(name, io::Error::last_os_error()));
        }
        Ok(())
    }
}

impl Drop for IoctlSocket {
    fn drop(&mut self) {
        // SAFETY: close(2) on the socket fd we opened.
        unsafe { libc::close(self.0) };
    }
}

/// 对本次创建的 utun 应用 IPv4 地址（点对点 dstaddr = 自身地址）、UP 与 MTU。
///
/// # Errors
///
/// 任一 ioctl 失败返回 typed 错误；失败前已应用的项不回滚（整个连接失败路径
/// 会关闭 utun，接口随之消失）。
pub fn apply_ipv4(
    utun: &Utun,
    address: Ipv4Addr,
    prefix: u8,
    mtu: u16,
) -> Result<(), PlatformError> {
    let socket = IoctlSocket::open()?;

    let _ = prefix; // 点对点接口无 netmask；网段由显式路由表达。
    let mut req = interface_req_with_addr(utun.name(), RawSockaddrIn::ipv4(address));
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCSIFADDR,
        "SIOCSIFADDR",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;
    // 点对点 dstaddr = 自身（Clash 同款形态）：macOS 内核源地址选择要求 p2p 接口的
    // 对端地址有效（非 0.0.0.0），否则 sendto 报 EADDRNOTAVAIL。失败必须让整个
    // apply 失败——残缺接口是连接态不可用的隐性故障。
    let mut dst = interface_req_with_addr(utun.name(), RawSockaddrIn::ipv4(address));
    socket.ioctl_named(
        SIOCSIFDSTADDR,
        "SIOCSIFDSTADDR",
        std::ptr::addr_of_mut!(dst).cast::<libc::c_void>(),
    )?;
    // 掩码显式 /32：不设时内核按类别默认（172.20.x.x → /16），宽掩码会让隐式源
    // 地址选择为「不在该子网」的目的地跳过本接口地址（真机实锤：显式 -S 可通、
    // 隐式 sendto EADDRNOTAVAIL）。窄掩码是 p2p utun 的通用先例（/30~/32）。
    // offer.prefix 只用于探测目标派生（packet::probe），不进接口掩码。
    let mut mask = interface_req_with_addr(
        utun.name(),
        RawSockaddrIn::ipv4(Ipv4Addr::new(255, 255, 255, 255)),
    );
    socket.ioctl_named(
        SIOCSIFNETMASK,
        "SIOCSIFNETMASK",
        std::ptr::addr_of_mut!(mask).cast::<libc::c_void>(),
    )?;

    let mut req = interface_req(utun.name());
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCGIFFLAGS,
        "SIOCGIFFLAGS",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;
    let mut flags = i16::from_ne_bytes([req.value[0], req.value[1]]);
    flags |= IFF_UP;
    req.value[0..2].copy_from_slice(&flags.to_ne_bytes());
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCSIFFLAGS,
        "SIOCSIFFLAGS",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;

    let mut req = interface_req(utun.name());
    req.value[0..4].copy_from_slice(&i32::from(mtu).to_ne_bytes());
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCSIFMTU,
        "SIOCSIFMTU",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;

    // readback barrier：地址、点对点对端与 MTU 必须与期望一致。
    let readback_address = read_ipv4(&socket, utun.name())?;
    if readback_address != address {
        return Err(PlatformError::Readback("ADDRESS"));
    }
    let readback_dst = read_dstaddr(&socket, utun.name())?;
    if readback_dst != address {
        return Err(PlatformError::Readback("DSTADDR"));
    }
    let readback_mask = read_netmask(&socket, utun.name())?;
    if readback_mask != Ipv4Addr::new(255, 255, 255, 255) {
        return Err(PlatformError::Readback("NETMASK"));
    }
    let mut req = interface_req(utun.name());
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCGIFMTU,
        "SIOCGIFMTU",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;
    let readback_mtu = i32::from_ne_bytes([req.value[0], req.value[1], req.value[2], req.value[3]]);
    if readback_mtu != i32::from(mtu) {
        return Err(PlatformError::Readback("MTU"));
    }
    Ok(())
}

/// 移除本连接协商得到的 utun IPv4 地址。
///
/// 自动重连等待期间会保留 utun FD，但不保留地址或业务路由，避免空闲设备继续承接流量。
/// 地址已因设备关闭而消失时由调用方按完成处理；其它失败仍返回以保留可重试事实。
pub fn clear_ipv4(utun: &Utun, address: Ipv4Addr) -> Result<(), PlatformError> {
    let socket = IoctlSocket::open()?;
    let mut request = interface_req_with_addr(utun.name(), RawSockaddrIn::ipv4(address));
    socket.ioctl_named(
        SIOCDIFADDR,
        "SIOCDIFADDR",
        std::ptr::addr_of_mut!(request).cast::<libc::c_void>(),
    )
}

fn read_netmask(socket: &IoctlSocket, name: &str) -> Result<Ipv4Addr, PlatformError> {
    let mut req = interface_req(name);
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCGIFNETMASK,
        "SIOCGIFNETMASK",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;
    Ok(Ipv4Addr::new(
        req.value[4],
        req.value[5],
        req.value[6],
        req.value[7],
    ))
}

fn read_dstaddr(socket: &IoctlSocket, name: &str) -> Result<Ipv4Addr, PlatformError> {
    let mut req = interface_req(name);
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCGIFDSTADDR,
        "SIOCGIFDSTADDR",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;
    Ok(Ipv4Addr::new(
        req.value[4],
        req.value[5],
        req.value[6],
        req.value[7],
    ))
}

fn read_ipv4(socket: &IoctlSocket, name: &str) -> Result<Ipv4Addr, PlatformError> {
    /// `_IOWR('i', 33, struct ifreq)`。
    const SIOCGIFADDR: u64 = 0xC020_6921;
    let mut req = interface_req(name);
    // SAFETY: req is a complete ifreq.
    socket.ioctl_named(
        SIOCGIFADDR,
        "SIOCGIFADDR",
        std::ptr::addr_of_mut!(req).cast::<libc::c_void>(),
    )?;
    Ok(Ipv4Addr::new(
        req.value[4],
        req.value[5],
        req.value[6],
        req.value[7],
    ))
}
