//! `MAC-PROXY-16 S2` 共存场景化真机验收（opt-in）：Mihomo（用户 Clash Verge 的
//! TUN，root，用户资产）**运行中**，经产品链路（真实 Core 进程 → 服务代理 →
//! root Engine，与 10c/11a/11b 同形）执行两轮「连接 → 校园 IP 目标经隧道直访 →
//! 公网域名走 fake-ip 代理路径 → Stop → Mihomo 完好」，证明真实共存。
//!
//! 红线（只读观察）：
//! - 本宿主持 fake-ip 地址的 utun（utun1024）是用户业务资产，全程**只读观察**，
//!   绝不 kill/篡改/重配；「Mihomo 完好」只以 readback 证明，不做任何主动操作；
//! - 不读 Mihomo 配置文件与内部 API（external-controller 等），共存判据只用
//!   任何进程都可读的系统事实：`PF_ROUTE` 路由查询/路由表 dump、`getifaddrs`
//!   接口地址、DNS 解析行为、本机代理端口连通性；
//! - exv 侧承诺的是「校园 routes 优先 + 不截走公网」；用户 fake-ip-filter 与
//!   订阅规则导致的域名分流差异如实记录，不算失败。
//!
//! 路由事实的取得方式（本宿主实测校准，2026-08-30）：
//! - 逐目标路由决策用 PF_ROUTE `RTM_GET` 原始查询（`route get` 的只读等价物，
//!   查询不改任何路由状态）；`rt_msghdr` 采用本宿主 SDK 真实布局
//!   （flags i32@8、addrs i32@12、pid@16、seq@20、sockaddrs@92）；
//! - 路由表 dump 只用于三方快照残留判定与接管段盘点：条目以
//!   （ifindex, DST 槽, GATEWAY 槽, NETMASK 槽原始字节）为等价键，NETMASK 槽
//!   在 Darwin 上是裁剪字节串（无独立 family），**不做前缀解释**，只做原始等价
//!   比较，避免布局臆断。
//!
//! 运行条件：
//! - 已安装服务代理（标签 `com.exv.vpn.service-agent`，root 守护进程）；
//! - 共享 `target/debug` 下存在 `exv-vpn-darwin-core` 与 `exv-vpn-darwin-engine`；
//! - `~/.exv/` 已保存完整可连接配置（server、username 与已加密密码）；
//! - **用户 Clash Verge 处于运行态**（存在持 `198.18.0.0/15` 地址的 utun 接口）。
//!
//! 运行方式：
//!
//! ```bash
//! EXV_DARWIN_PROXY16_S2=1 cargo test \
//!   --manifest-path src/platform/darwin/rust/Cargo.toml \
//!   -p exv-vpn-darwin-core \
//!   --test proxy_coexistence_real_e2e -- --ignored --nocapture --test-threads=1
//! ```
//!
//! 凭据保护（P0）：本测试绝不打印用户名、服务器地址或密码明文；`~/.exv/`
//! 已保存凭据只做存在性校验，连接由 Core 产品路径自行消费，测试后保持原状。
//! 每轮使用独立随机 operation id，两轮不串 operation。

// PF_ROUTE/getifaddrs 原始解析在逐字段边界检查后做窄化转换；中文文档中的技术
// 术语不逐个加反引号；真机主场景函数长属预期（两轮业务流）。
#![allow(clippy::doc_markdown)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::too_many_lines)]

use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use tokio::{net::UnixStream, time};

use exv_vpn_darwin_core::{DarwinUiConfig, build_saved_connect_envelope};
use exv_vpn_darwin_ipc::{
    authenticate_client_to_tonic_channel,
    path::{RuntimeDir, RuntimeOwner},
    peer::{ExpectedPeer, SystemPeerLookup},
    ui_core_bootstrap::UiCoreBootstrapV1,
};
use exv_vpn_wire::generated::{
    ConnectIntent, ConnectRequest, OperationLookupKey, OperationMethod, RuntimeEvent,
    SnapshotRequest, StopIntent, StopRequest, WatchEventsRequest,
    kernel_control_client::KernelControlClient, runtime_snapshot,
};

/// 与现有 service agent e2e 相同的 opt-in 总闸之外的本场景专属开关。
const S2_ENV: &str = "EXV_DARWIN_PROXY16_S2";
/// 兼容既有 service agent e2e 总闸。
const COMPANION_E2E_ENV: &str = "EXV_DARWIN_SERVICE_AGENT_E2E";

/// 校园 IP 目标：校内 DNS `202.120.80.2`（11a/11b 已验证目标集；属配置路由
/// `202.120.80.0/20`，S2 任务书指定的校园目标示例）。
const CAMPUS_DNS: Ipv4Addr = Ipv4Addr::new(202, 120, 80, 2);
/// 校园边缘目标：`222.66.117.109:443`（11b/10c 隧道数据面探测既用目标）。
const CAMPUS_EDGE: Ipv4Addr = Ipv4Addr::new(222, 66, 117, 109);
/// 公网一般目标（路由归属核对用，不发起业务）。
const PUBLIC_GENERAL: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
/// 公网域名目标：经系统解析器解析，开态下应被 Mihomo fake-ip 接管。
const PUBLIC_DOMAIN: &str = "example.com";
/// 本机代理端口（系统 `netstat` 观察到 127.0.0.1:7890 监听，Clash Verge 惯用
/// mixed port；基线连通才启用差分断言）。
const PROXY_PORT: u16 = 7890;

/// 完整链路（登录 → CSTP → utun → 数据面）事件等待上限。
const EVENT_DEADLINE: Duration = Duration::from_secs(90);
/// 内核拆除接口/路由的有界轮询上界（11a 同界）。
const CLEAN_POLL_BOUND: Duration = Duration::from_secs(15);
/// 网络探测超时。
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// PF_ROUTE 查询应答超时。
const ROUTE_REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// S1 契约：detected 时的 `route_policy` 字符串（proto `ProxyTunDetection`）。
const ROUTE_POLICY_EXV_BEFORE_PROXY_TUN: &str = "exv-before-proxy-tun";

/// 本宿主 Mihomo 接管段的 DST 锚点（1/8…128.0/1 八块；用户 Clash Verge 实测形态）。
const TAKEOVER_BLOCK_DSTS: [Ipv4Addr; 8] = [
    Ipv4Addr::new(1, 0, 0, 0),
    Ipv4Addr::new(2, 0, 0, 0),
    Ipv4Addr::new(4, 0, 0, 0),
    Ipv4Addr::new(8, 0, 0, 0),
    Ipv4Addr::new(16, 0, 0, 0),
    Ipv4Addr::new(32, 0, 0, 0),
    Ipv4Addr::new(64, 0, 0, 0),
    Ipv4Addr::new(128, 0, 0, 0),
];

// ---------------------------------------------------------------------------
// 系统 readback（纯 libc，全部只读）
// ---------------------------------------------------------------------------

/// 一次 `RTM_GET` 查询得到的真实路由决策（`route get` 的只读等价物）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteDecision {
    /// 路由出接口索引（`rtm_index`）。
    ifindex: u32,
    /// 路由出接口名。
    ifname: String,
    /// 命中路由的 DST 槽地址（内核返回的匹配路由键）。
    destination: Option<Ipv4Addr>,
    /// `RTA_GATEWAY` 为 `AF_INET` 时的网关地址（接口路由形态为 `None`）。
    gateway: Option<Ipv4Addr>,
}

/// 路由表 dump 的一条 readback（等价键 = 五元组；NETMASK 槽只存原始字节）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DumpEntry {
    destination: Option<Ipv4Addr>,
    gateway: Option<Ipv4Addr>,
    ifindex: u32,
    /// NETMASK 槽原始字节（Darwin 裁剪字节串，不做前缀解释）。
    mask_raw: Option<Vec<u8>>,
    /// `rtm_flags`（exv 安装特征 = `0x1|0x2|0x10`，见 Engine `platform/route.rs`）。
    flags: i32,
}

impl DumpEntry {
    /// exv 安装特征：Engine `route::add` 恒以 `RTF_UP|RTF_GATEWAY|RTF_STATIC(0x10)`
    /// 与 NETMASK 槽安装路由；系统瞬态条目（邻居/克隆路由）不具备该组合。
    fn has_exv_installation_signature(&self) -> bool {
        self.flags & 0x13 == 0x13 && self.mask_raw.is_some()
    }
}

/// 解析一个 sockaddr 槽（RTA bit 序游标式）；返回（family, 数据起点, 槽长）。
/// 返回 `None` 表示缓冲不足。
fn sockaddr_slot(body: &[u8], offset: usize) -> Option<(u8, usize, usize)> {
    if offset >= body.len() {
        return None;
    }
    let sa_len = usize::from(body[offset]);
    let family = if offset + 1 < body.len() {
        body[offset + 1]
    } else {
        0
    };
    let slot_len = if sa_len == 0 { 4 } else { (sa_len + 3) & !3 };
    if offset + slot_len > body.len() {
        return None;
    }
    Some((family, offset, slot_len))
}

/// 槽内 `AF_INET` 地址（数据起点 = 槽起点 + 4）。
fn slot_ipv4(body: &[u8], data: usize) -> Option<Ipv4Addr> {
    if data + 8 <= body.len() {
        Some(Ipv4Addr::new(
            body[data + 4],
            body[data + 5],
            body[data + 6],
            body[data + 7],
        ))
    } else {
        None
    }
}

/// 槽内 `AF_INET` 地址（数据起点 = 槽起点 + 4）；family 非 `AF_INET` 返回 `None`。
fn af_inet_address(family: u8, body: &[u8], data: usize) -> Option<Ipv4Addr> {
    if family == libc::AF_INET as u8 {
        slot_ipv4(body, data)
    } else {
        None
    }
}

/// 依 `addrs` 位序遍历 sockaddr 槽并回调 `(bit, family, data, slot_len)`。
fn for_each_sockaddr_slot(body: &[u8], addrs: i32, mut visit: impl FnMut(i32, u8, usize, usize)) {
    let mut offset = 0usize;
    for bit in 0..16i32 {
        if addrs & (1 << bit) == 0 {
            continue;
        }
        let Some((family, data, slot_len)) = sockaddr_slot(body, offset) else {
            return;
        };
        visit(bit, family, data, slot_len);
        offset += slot_len;
    }
}

/// dump 内核 `PF_ROUTE` 路由表中的 IPv4 条目
/// （`{CTL_NET, PF_ROUTE, 0, AF_UNSPEC, NET_RT_DUMP, 0}`；无特权只读）。
fn route_dump() -> Vec<DumpEntry> {
    let mut mib: [libc::c_int; 6] = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        libc::AF_UNSPEC,
        libc::NET_RT_DUMP,
        0,
    ];
    let mut size: usize = 0;
    // SAFETY: NULL 缓冲探测所需大小。
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Vec::new();
    }
    let mut buffer = vec![0_u8; size];
    // SAFETY: 缓冲按探测大小分配。
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Vec::new();
    }
    buffer.truncate(size);
    parse_route_dump(&buffer)
}

/// 解析一次 PF_ROUTE dump 缓冲（纯函数；DST/GATEWAY 槽解析，NETMASK 槽存原始字节）。
fn parse_route_dump(buffer: &[u8]) -> Vec<DumpEntry> {
    const HEADER: usize = std::mem::size_of::<libc::rt_msghdr>();
    let mut entries = Vec::new();
    let mut cursor = 0usize;
    while cursor + HEADER <= buffer.len() {
        // SAFETY: cursor 已按头大小边界检查；标量为宿主字节序。
        let rtm = unsafe {
            buffer
                .as_ptr()
                .add(cursor)
                .cast::<libc::rt_msghdr>()
                .read_unaligned()
        };
        let message_len = usize::from(rtm.rtm_msglen);
        if rtm.rtm_version != libc::RTM_VERSION as u8
            || rtm.rtm_type != libc::RTM_GET as u8
            || message_len < HEADER
            || cursor + message_len > buffer.len()
        {
            break;
        }
        let body = &buffer[cursor + HEADER..cursor + message_len];
        let mut entry = DumpEntry {
            destination: None,
            gateway: None,
            ifindex: u32::from(rtm.rtm_index),
            mask_raw: None,
            flags: rtm.rtm_flags,
        };
        for_each_sockaddr_slot(body, rtm.rtm_addrs, |bit, family, data, slot_len| {
            match 1 << bit {
                a if a == libc::RTA_DST => {
                    if let Some(address) = af_inet_address(family, body, data) {
                        entry.destination = Some(address);
                    }
                }
                a if a == libc::RTA_GATEWAY => {
                    if let Some(address) = af_inet_address(family, body, data) {
                        entry.gateway = Some(address);
                    }
                }
                a if a == libc::RTA_NETMASK => {
                    entry.mask_raw = Some(body[data..data + slot_len].to_vec());
                }
                _ => {}
            }
        });
        if entry.destination.is_some() {
            entries.push(entry);
        }
        cursor += message_len;
    }
    entries
}

/// 向内核发起一次 PF_ROUTE `RTM_GET` 查询并解析真实路由决策（只读；`route get`
/// 的等价物）。查询失败（含超时）返回 `None`，不重试、不阻塞调用方。
fn route_get(target: Ipv4Addr) -> Option<RouteDecision> {
    static SEQ: AtomicU32 = AtomicU32::new(0x5211);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed).max(1) as i32;
    let pid = unsafe { libc::getpid() };

    // SAFETY: socket(2) 创建只读用途的 PF_ROUTE 原始套接字。
    let sock = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, 0) };
    if sock < 0 {
        return None;
    }
    let timeout = libc::timeval {
        tv_sec: ROUTE_REPLY_TIMEOUT.as_secs() as libc::time_t,
        tv_usec: 0,
    };
    // SAFETY: sock 有效；timeout 为栈上合法 timeval。
    let _ = unsafe {
        libc::setsockopt(
            sock,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            std::ptr::addr_of!(timeout).cast(),
            u32::try_from(std::mem::size_of::<libc::timeval>()).ok()?,
        )
    };

    let header = std::mem::size_of::<libc::rt_msghdr>();
    let mut message = vec![0_u8; header + 16];
    let message_len = u16::try_from(message.len()).ok()?;
    message[0..2].copy_from_slice(&message_len.to_ne_bytes());
    message[2] = libc::RTM_VERSION as u8;
    message[3] = libc::RTM_GET as u8;
    message[12..16].copy_from_slice(&libc::RTA_DST.to_ne_bytes());
    message[16..20].copy_from_slice(&pid.to_ne_bytes());
    message[20..24].copy_from_slice(&seq.to_ne_bytes());
    // sockaddr_in（sin_len=16, family=AF_INET, port=0, addr=target）。
    message[header] = 16;
    message[header + 1] = libc::AF_INET as u8;
    message[header + 4..header + 8].copy_from_slice(&target.octets());

    // SAFETY: message 缓冲与长度匹配；只读查询消息。
    let sent = unsafe { libc::write(sock, message.as_ptr().cast(), message.len()) };
    if sent != message.len() as isize {
        // SAFETY: 关闭本调用创建的套接字。
        unsafe { libc::close(sock) };
        return None;
    }

    let mut reply = [0_u8; 512];
    let decision = loop {
        // SAFETY: reply 缓冲与长度匹配。
        let received = unsafe { libc::read(sock, reply.as_mut_ptr().cast(), reply.len()) };
        if received <= 0 {
            break None;
        }
        let received = received as usize;
        if received < header {
            continue;
        }
        // SAFETY: received >= header，标量为宿主字节序。
        let rtm = unsafe { reply.as_ptr().cast::<libc::rt_msghdr>().read_unaligned() };
        // 过滤广播（RTM_MISS 等）：只接受本 pid、本 seq 的 RTM_GET 应答。
        if rtm.rtm_type != libc::RTM_GET as u8 || rtm.rtm_pid != pid || rtm.rtm_seq != seq {
            continue;
        }
        let body = &reply[header..received];
        let mut decision = RouteDecision {
            ifindex: u32::from(rtm.rtm_index),
            ifname: if_name(u32::from(rtm.rtm_index)),
            destination: None,
            gateway: None,
        };
        for_each_sockaddr_slot(body, rtm.rtm_addrs, |bit, family, data, _| match 1 << bit {
            a if a == libc::RTA_DST => {
                if let Some(address) = af_inet_address(family, body, data) {
                    decision.destination = Some(address);
                }
            }
            a if a == libc::RTA_GATEWAY => {
                if let Some(address) = af_inet_address(family, body, data) {
                    decision.gateway = Some(address);
                }
            }
            _ => {}
        });
        break Some(decision);
    };
    // SAFETY: 关闭本调用创建的套接字。
    unsafe { libc::close(sock) };
    decision
}

/// 接口索引 → 接口名（`if_indextoname`；失败时以 `if#N` 占位）。
fn if_name(ifindex: u32) -> String {
    let mut buffer = [0_u8; 64];
    // SAFETY: buffer 为合法可写缓冲，指针仅在本调用内使用。
    let pointer = unsafe { libc::if_indextoname(ifindex, buffer.as_mut_ptr().cast()) };
    if pointer.is_null() {
        format!("if#{ifindex}")
    } else {
        // SAFETY: pointer 指向 buffer 内 NUL 结尾的名字。
        unsafe { std::ffi::CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

/// 接口名 → 索引（`if_nametoindex`；0 = 不存在）。
fn if_index(name: &str) -> u32 {
    let Ok(c_name) = std::ffi::CString::new(name) else {
        return 0;
    };
    // SAFETY: c_name 为合法 NUL 字符串，只读。
    unsafe { libc::if_nametoindex(c_name.as_ptr()) }
}

/// 全部接口的 IPv4 地址映射（`getifaddrs`，只读）。
fn interface_ipv4_map() -> HashMap<String, Vec<Ipv4Addr>> {
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: addrs 为内核分配列表的出参。
    if unsafe { libc::getifaddrs(std::ptr::addr_of_mut!(addrs)) } != 0 {
        return HashMap::new();
    }
    let mut map: HashMap<String, Vec<Ipv4Addr>> = HashMap::new();
    let mut cursor = addrs;
    while !cursor.is_null() {
        // SAFETY: cursor 指向链表有效节点。
        let entry = unsafe { &*cursor };
        let next = entry.ifa_next;
        if !entry.ifa_addr.is_null() {
            // SAFETY: ifa_addr 非空时为有效 sockaddr。
            let family = unsafe { (*entry.ifa_addr).sa_family };
            if family == libc::AF_INET as u8 {
                // SAFETY: ifa_name 为有效 NUL 字符串；AF_INET 即 sockaddr_in。
                let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
                    .to_string_lossy()
                    .into_owned();
                let octets = unsafe {
                    (*entry.ifa_addr.cast::<libc::sockaddr_in>())
                        .sin_addr
                        .s_addr
                };
                let address = Ipv4Addr::from(octets.to_ne_bytes());
                map.entry(name).or_default().push(address);
            }
        }
        cursor = next;
    }
    // SAFETY: 释放 getifaddrs 列表。
    unsafe { libc::freeifaddrs(addrs) };
    map
}

/// fake-ip 判据：`198.18.0.0/15`（与 S1 检测/Engine resolver 同一判据）。
fn is_fake_ip(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

/// 持 fake-ip 地址的 utun 接口（Mihomo TUN 的最强系统信号）。
fn find_fake_ip_utun(interfaces: &HashMap<String, Vec<Ipv4Addr>>) -> Option<(String, Ipv4Addr)> {
    let mut candidates: Vec<(String, Ipv4Addr)> = interfaces
        .iter()
        .filter(|(name, _)| name.starts_with("utun"))
        .flat_map(|(name, addresses)| {
            addresses
                .iter()
                .filter(|address| is_fake_ip(**address))
                .map(|address| (name.clone(), *address))
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// 当前持有 `172.20/16` 地址的接口集合（exv 隧道地址段；`172.20.10.x` 热点
/// 形态按接口名差分排除，不单独判定）。
fn holders_of_17220(interfaces: &HashMap<String, Vec<Ipv4Addr>>) -> HashSet<String> {
    interfaces
        .iter()
        .filter(|(_, addresses)| {
            addresses
                .iter()
                .any(|address| address.octets()[0] == 172 && address.octets()[1] == 20)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// 系统解析器对公网域名的首个 IPv4 解析结果（`getaddrinfo`，与 dig 同路径）。
fn resolve_first_ipv4(domain: &str) -> Option<Ipv4Addr> {
    (domain, 44_u16)
        .to_socket_addrs()
        .ok()?
        .find_map(|address| match address {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            SocketAddr::V6(_) => None,
        })
}

/// TCP 端口连通性探测（有界超时；只连接，不发业务数据）。
fn tcp_port_open(address: Ipv4Addr, port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((address, port)), Duration::from_secs(3)).is_ok()
}

/// 发送最小 TLS ClientHello 并等待任意 TLS record 回复，返回 record 类型字节。
/// 对端任何 TLS record（alert/handshake/appdata）即证明双向数据面。
#[allow(clippy::similar_names)]
fn tls_record_probe(address: Ipv4Addr, port: u16, context: &str) -> u8 {
    use std::io::{Read as _, Write as _};
    let mut stream = TcpStream::connect_timeout(&SocketAddr::from((address, port)), PROBE_TIMEOUT)
        .unwrap_or_else(|error| panic!("{context}: TCP connect must succeed: {error}"));
    stream
        .write_all(&[
            0x16, 0x03, 0x01, 0x00, 0x2d, // record: handshake, len 45
            0x01, 0x00, 0x00, 0x29, // ClientHello, len 41
            0x03, 0x03, // client version TLS 1.2
        ])
        .expect("{context}: write ClientHello header");
    stream
        .write_all(&[0x11_u8; 32]) // random
        .expect("{context}: write random");
    stream
        .write_all(&[
            0x00, // session id len
            0x00, 0x02, 0x00, 0x2f, // one cipher suite
            0x01, 0x00, // compression: null
            0x00, 0x00, // extensions len 0
        ])
        .expect("{context}: write ClientHello body");
    stream
        .set_read_timeout(Some(PROBE_TIMEOUT))
        .expect("{context}: set read timeout");
    let mut reply = [0_u8; 5];
    let received = stream
        .read(&mut reply)
        .unwrap_or_else(|error| panic!("{context}: must receive a reply: {error}"));
    assert!(
        received >= 1,
        "{context}: peer must reply with a TLS record"
    );
    assert!(
        [0x15, 0x16, 0x17].contains(&reply[0]),
        "{context}: expected a TLS record, got {:#x}",
        reply[0]
    );
    reply[0]
}

/// 公网目标的业务级探测降级为「有界重试 + 如实记录」：对端（真实站点/代理链路）
/// 响应差异属用户业务现实（计划 §4.3），探测失败不构成共存失败。每次尝试有硬
/// 时间界；返回 `Some(record)` = 拿到任意 TLS record（含 alert），`None` = 有界
/// 内无响应（路由归属证据已足够，业务响应如实落盘）。
fn tls_record_probe_best_effort(address: Ipv4Addr, port: u16, attempts: u32) -> Option<u8> {
    use std::io::{Read as _, Write as _};
    for attempt in 1..=attempts {
        let Ok(mut stream) =
            TcpStream::connect_timeout(&SocketAddr::from((address, port)), PROBE_TIMEOUT)
        else {
            continue;
        };
        let hello_written = stream
            .write_all(&[
                0x16, 0x03, 0x01, 0x00, 0x2d, // record: handshake, len 45
                0x01, 0x00, 0x00, 0x29, // ClientHello, len 41
                0x03, 0x03, // client version TLS 1.2
            ])
            .and_then(|()| stream.write_all(&[0x11_u8; 32]))
            .and_then(|()| {
                stream.write_all(&[
                    0x00, // session id len
                    0x00, 0x02, 0x00, 0x2f, // one cipher suite
                    0x01, 0x00, // compression: null
                    0x00, 0x00, // extensions len 0
                ])
            });
        if hello_written.is_err() {
            continue;
        }
        stream.set_read_timeout(Some(PROBE_TIMEOUT)).ok()?;
        let mut reply = [0_u8; 5];
        if let Ok(1..) = stream.read(&mut reply)
            && [0x15, 0x16, 0x17].contains(&reply[0])
        {
            return Some(reply[0]);
        }
        let _ = attempt;
    }
    None
}

/// 对校园 DNS 发送一次真实 UDP 查询并等待响应（事务 id 匹配即算业务可达）。
fn dns_probe(server: Ipv4Addr, domain: &str) -> Option<usize> {
    let socket = std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0))).ok()?;
    socket.set_read_timeout(Some(PROBE_TIMEOUT)).ok()?;
    socket.connect(SocketAddr::from((server, 53))).ok()?;
    let mut id = [0_u8; 2];
    getrandom::fill(&mut id).ok()?;
    let mut query = Vec::with_capacity(32);
    query.extend_from_slice(&id);
    // 标准递归查询头。
    query.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for label in domain.split('.') {
        let length = u8::try_from(label.len()).ok()?;
        query.push(length);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // A + IN
    socket.send(&query).ok()?;
    let mut reply = [0_u8; 512];
    let received = socket.recv(&mut reply).ok()?;
    (received >= 4 && reply[0] == id[0] && reply[1] == id[1]).then_some(received)
}

// ---------------------------------------------------------------------------
// 分流事实采集与对照表
// ---------------------------------------------------------------------------

/// 一次采集的分流系统事实（对照表的一行份；全部只读）。
#[derive(Debug, Clone)]
struct SplitFacts {
    mihomo_utun: Option<String>,
    mihomo_ifindex: u32,
    mihomo_fake_ip: Option<Ipv4Addr>,
    campus_dns_decision: Option<RouteDecision>,
    campus_edge_decision: Option<RouteDecision>,
    public_decision: Option<RouteDecision>,
    takeover_blocks: Vec<Ipv4Addr>,
    default_via: Option<String>,
    public_resolution: Option<Ipv4Addr>,
    proxy_port_open: bool,
}

impl SplitFacts {
    /// Mihomo TUN 是否在场（持 fake-ip 地址的 utun）。
    fn mihomo_present(&self) -> bool {
        self.mihomo_utun.is_some()
    }

    /// 接管段块数（经 Mihomo TUN、网关为 Mihomo fake-ip 地址的接管段 DST）。
    fn takeover_count(&self) -> usize {
        self.takeover_blocks.len()
    }
}

/// 采集当前时刻的分流系统事实（全部只读）。
fn collect_split_facts() -> SplitFacts {
    let interfaces = interface_ipv4_map();
    let dump = route_dump();
    let (mihomo_utun, mihomo_fake_ip) = find_fake_ip_utun(&interfaces)
        .map_or((None, None), |(name, address)| (Some(name), Some(address)));
    let mihomo_ifindex = mihomo_utun.as_ref().map_or(0, |name| if_index(name));
    let public_resolution = resolve_first_ipv4(PUBLIC_DOMAIN);
    let public_decision = public_resolution.and_then(route_get);
    let takeover_blocks = match (mihomo_fake_ip, mihomo_ifindex) {
        (Some(gateway), ifindex) if ifindex != 0 => {
            let mut blocks: Vec<Ipv4Addr> = dump
                .iter()
                .filter(|entry| {
                    entry.ifindex == ifindex
                        && entry.gateway == Some(gateway)
                        && entry
                            .destination
                            .is_some_and(|dst| TAKEOVER_BLOCK_DSTS.contains(&dst))
                })
                .filter_map(|entry| entry.destination)
                .collect();
            blocks.sort();
            blocks
        }
        _ => Vec::new(),
    };
    let default_via = dump
        .iter()
        .find(|entry| {
            entry.destination == Some(Ipv4Addr::UNSPECIFIED)
                && entry.gateway.is_some_and(|gateway| !is_fake_ip(gateway))
        })
        .map(|entry| if_name(entry.ifindex));
    SplitFacts {
        campus_dns_decision: route_get(CAMPUS_DNS),
        campus_edge_decision: route_get(CAMPUS_EDGE),
        public_decision,
        takeover_blocks,
        default_via,
        mihomo_utun,
        mihomo_ifindex,
        mihomo_fake_ip,
        public_resolution,
        proxy_port_open: tcp_port_open(Ipv4Addr::LOCALHOST, PROXY_PORT),
    }
}

fn decision_text(decision: Option<&RouteDecision>) -> String {
    decision.map_or_else(
        || "（查询失败）".to_owned(),
        |decision| {
            format!(
                "命中 {} 经 {} (if {}) gw {}",
                decision
                    .destination
                    .map_or_else(|| "?".to_owned(), |value| value.to_string()),
                decision.ifname,
                decision.ifindex,
                decision
                    .gateway
                    .map_or_else(|| "link".to_owned(), |value| value.to_string())
            )
        },
    )
}

/// 打印一张对照表（每轮：连接前 / 连接期 / Stop 后各一份，落盘证据摘录）。
fn print_split_table(stage: &str, facts: &SplitFacts) {
    println!("[S2] ---- 对照表（{stage}）----");
    println!(
        "[S2] | Mihomo TUN 接口            | {:?} if={} fake-ip={:?}",
        facts.mihomo_utun, facts.mihomo_ifindex, facts.mihomo_fake_ip
    );
    println!(
        "[S2] | route get 校园 IP {}   | {}",
        CAMPUS_DNS,
        decision_text(facts.campus_dns_decision.as_ref())
    );
    println!(
        "[S2] | route get 校园边缘 {}  | {}",
        CAMPUS_EDGE,
        decision_text(facts.campus_edge_decision.as_ref())
    );
    println!(
        "[S2] | 接管段块（dst ∧ gw=Mihomo）| {:?}（{} 块）",
        facts.takeover_blocks,
        facts.takeover_count()
    );
    println!(
        "[S2] | 默认路由出接口             | {:?}",
        facts.default_via
    );
    println!(
        "[S2] | {PUBLIC_DOMAIN} 解析            | {:?}（fake-ip={:?}）",
        facts.public_resolution,
        facts.public_resolution.is_some_and(is_fake_ip)
    );
    println!(
        "[S2] | route get 解析结果         | {}",
        decision_text(facts.public_decision.as_ref())
    );
    println!(
        "[S2] | 代理端口 127.0.0.1:{PROXY_PORT} 连通  | {}",
        facts.proxy_port_open
    );
}

// ---------------------------------------------------------------------------
// 产品链路（真实 Core 进程，10c 同形）
// ---------------------------------------------------------------------------

/// Core 二进制解析（共享 `target/debug`；可用环境变量覆盖）。
fn resolve_core_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("EXV_PROXY16_S2_CORE_BIN") {
        return PathBuf::from(path);
    }
    let executable = std::env::current_exe().expect("test executable path");
    let debug_dir = executable
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target/debug directory");
    debug_dir.join("exv-vpn-darwin-core")
}

/// panic 兜底守卫：任何退出路径都回收 Core 子进程（测试脚手架卫生）。
struct CoreGuard {
    child: std::process::Child,
}

impl Drop for CoreGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn random_bytes() -> Vec<u8> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("random operation id");
    bytes.to_vec()
}

fn lookup_key(method: i32, operation_id: Vec<u8>) -> OperationLookupKey {
    OperationLookupKey {
        // 与 Windows 同形：Darwin 引擎链路接受零 principal（service agent e2e 先例）。
        principal_digest: vec![0; 32],
        method,
        runtime_epoch: vec![0; 16],
        operation_id,
    }
}

/// 顺序读事件直到拿到快照载荷事件；每个事件留痕。
async fn next_snapshot_event(watch: &mut tonic::Streaming<RuntimeEvent>) -> RuntimeEvent {
    loop {
        let event = watch
            .message()
            .await
            .expect("watch stream healthy")
            .expect("event present");
        let state_text = event.snapshot.as_ref().map_or_else(
            || "none".to_owned(),
            |snapshot| {
                snapshot
                    .state
                    .as_ref()
                    .map_or_else(|| "empty".to_owned(), |state| format!("{state:?}"))
            },
        );
        println!(
            "[S2] event tick={} state={} proxy_tun={}",
            event.monotonic_tick,
            state_text,
            event
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.proxy_tun.as_ref())
                .map_or_else(
                    || "None".to_owned(),
                    |detection| format!(
                        "detected={} policy={} adapters={:?}",
                        detection.detected, detection.route_policy, detection.adapters
                    )
                )
        );
        if event.snapshot.is_some() {
            return event;
        }
    }
}

/// 断言检测事实 = 系统事实（detected + utun 适配器 + route_policy 契约）。
fn assert_detection_matches_mihomo(
    context: &str,
    detection: Option<&exv_vpn_wire::generated::ProxyTunDetection>,
    mihomo_utun: &str,
) {
    let detection = detection.unwrap_or_else(|| panic!("{context}: proxy_tun 必须已探测（开态）"));
    assert!(
        detection.detected,
        "{context}: Mihomo TUN（{mihomo_utun}）运行中必须 detected"
    );
    assert_eq!(
        detection.route_policy, ROUTE_POLICY_EXV_BEFORE_PROXY_TUN,
        "{context}: route_policy 契约"
    );
    let adapter = detection
        .adapters
        .iter()
        .find(|adapter| adapter.name == mihomo_utun)
        .unwrap_or_else(|| panic!("{context}: adapters 必须含 {mihomo_utun}"));
    println!(
        "[S2] {context}: proxy_tun detected adapter={mihomo_utun} if_index={} evidence={:?}",
        adapter.if_index, adapter.description
    );
}

// ---------------------------------------------------------------------------
// 主场景
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要已安装服务代理、真实学校网关、已保存凭据，且用户 Clash Verge 处于运行态；\
           必须以 EXV_DARWIN_PROXY16_S2=1（或 EXV_DARWIN_SERVICE_AGENT_E2E=1）显式启用"]
async fn open_state_two_rounds_coexistence_business_flow_leaves_mihomo_intact() {
    assert!(
        std::env::var_os(S2_ENV).is_some() || std::env::var_os(COMPANION_E2E_ENV).is_some(),
        "该真机验收必须以 EXV_DARWIN_PROXY16_S2=1（或 EXV_DARWIN_SERVICE_AGENT_E2E=1）显式启用"
    );

    // ── 前置：已保存凭据存在性（不读明文、不打印）────────────────────────
    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    assert!(
        !config.username().is_empty(),
        "缺少已保存用户名，请先在设置页保存"
    );
    let envelope =
        build_saved_connect_envelope(&config).expect("saved credentials must form the V1 envelope");
    let request_digest = envelope.profile_digest().to_vec();
    drop(envelope);

    // ── 前置：Mihomo 开态判定（持 fake-ip 地址的 utun 存在）──────────────
    let baseline = collect_split_facts();
    assert!(
        baseline.mihomo_present(),
        "用户 Clash Verge 必须处于运行态（存在持 198.18.0.0/15 地址的 utun）；请开启后重试"
    );
    let mihomo_utun = baseline.mihomo_utun.clone().expect("checked above");
    let mihomo_ifindex = baseline.mihomo_ifindex;
    let mihomo_fake_ip = baseline.mihomo_fake_ip.expect("fake-ip 接口地址必须可读");
    let baseline_takeover = baseline.takeover_count();
    assert!(
        baseline_takeover >= 6,
        "开态前置：Mihomo 接管段（1/8…128.0/1 至少 6 块）必须可观察，实得 {baseline_takeover}"
    );
    println!(
        "[S2] 开态前置确认：Mihomo TUN {mihomo_utun}(if {mihomo_ifindex}) 持 {mihomo_fake_ip}，接管段 {} 块；两轮业务流开始（每轮 连接→分流断言→Stop→Mihomo 完好 readback）",
        baseline.takeover_count()
    );
    print_split_table("第 0 份：连接前基线", &baseline);

    // ── 启动真实 Core 进程（与 Tauri UI 同形 spawn 参数与认证协议）────────
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create runtime dir");
    let socket = runtime.engine_socket_path().expect("derive socket path");
    // SAFETY: `geteuid` 只读当前普通用户凭证。
    let ui_uid = unsafe { libc::geteuid() };
    let core_child = std::process::Command::new(resolve_core_binary())
        .arg("--ui-pid")
        .arg(std::process::id().to_string())
        .arg("--ui-uid")
        .arg(ui_uid.to_string())
        .arg("--ui-socket")
        .arg(socket.as_path())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the real dev Core process");
    let core_pid = core_child.id();
    let mut guard = CoreGuard { child: core_child };
    println!("[S2] core pid={core_pid}");

    let (bootstrap, client_key) = UiCoreBootstrapV1::random_pair().expect("bootstrap key pair");
    std::io::Write::write_all(
        guard.child.stdin.as_mut().expect("core stdin piped"),
        bootstrap.encode().as_bytes(),
    )
    .expect("write bootstrap record");
    drop(guard.child.stdin.take());

    let mut socket_ready = false;
    for _ in 0..100 {
        if socket.as_path().exists() {
            socket_ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(socket_ready, "core must create the authenticated socket");
    let stream = UnixStream::connect(socket.as_path())
        .await
        .expect("connect the core UDS");
    let expected_core = ExpectedPeer::new(ui_uid, core_pid);
    let ui_identity = ExpectedPeer::new(ui_uid, std::process::id());
    let (channel, _info) = authenticate_client_to_tonic_channel(
        stream,
        expected_core,
        ui_identity,
        &SystemPeerLookup,
        &client_key,
    )
    .await
    .expect("authenticate the core channel");
    let mut client = KernelControlClient::new(channel);

    // ── 控制面：WatchEvents initial Idle 快照必须携带开态检测事实 ────────
    let mut watch = client
        .watch_events(WatchEventsRequest { resume_tick: 0 })
        .await
        .expect("watch events")
        .into_inner();
    let initial = next_snapshot_event(&mut watch).await;
    let initial_snapshot = initial.snapshot.clone().expect("snapshot present");
    assert!(
        matches!(
            initial_snapshot.state.as_ref(),
            Some(runtime_snapshot::State::Idle(_))
        ),
        "initial snapshot must be Idle"
    );
    assert_detection_matches_mihomo(
        "WatchEvents initial Idle",
        initial_snapshot.proxy_tun.as_ref(),
        &mihomo_utun,
    );

    // 基线集合（172.20/16 持有者差分基线与基线路由等价键集合）。
    let baseline_17220 = holders_of_17220(&interface_ipv4_map());
    let baseline_dump = route_dump();
    let baseline_keys: HashSet<DumpEntry> = baseline_dump.iter().cloned().collect();

    for round in 1..=2 {
        println!("[S2] ======== 第 {round} 轮开始（operation 随机独立）========");

        // ── 连接前：GetSnapshot 检测事实 + 对照表 ────────────────────────
        let before = collect_split_facts();
        assert!(
            before.mihomo_present() && before.mihomo_utun.as_deref() == Some(mihomo_utun.as_str()),
            "第 {round} 轮连接前：Mihomo TUN 必须仍在（用户资产存续）"
        );
        assert_eq!(
            before.takeover_count(),
            baseline_takeover,
            "第 {round} 轮连接前：接管段必须与基线一致"
        );
        assert_eq!(
            before
                .campus_dns_decision
                .as_ref()
                .map(|decision| decision.ifindex),
            Some(mihomo_ifindex),
            "第 {round} 轮连接前：校园 IP 必须经 Mihomo 接管段（exv 未连接），实得 {:?}",
            before.campus_dns_decision
        );
        let idle_snapshot = client
            .get_snapshot(SnapshotRequest {
                runtime_epoch: Vec::new(),
            })
            .await
            .expect("GetSnapshot must answer")
            .into_inner();
        assert!(
            matches!(
                idle_snapshot.state.as_ref(),
                Some(runtime_snapshot::State::Idle(_))
            ),
            "第 {round} 轮连接前 GetSnapshot 必须 Idle"
        );
        assert_detection_matches_mihomo(
            &format!("GetSnapshot（第 {round} 轮连接前 Idle）"),
            idle_snapshot.proxy_tun.as_ref(),
            &mihomo_utun,
        );
        print_split_table(&format!("第 {round} 轮 · 连接前"), &before);

        // ── 连接（产品链路 Connect，独立随机 operation）──────────────────
        let connect_operation = random_bytes();
        let reply = client
            .connect(ConnectRequest {
                intent: Some(ConnectIntent {
                    lookup_key: Some(lookup_key(
                        OperationMethod::Connect as i32,
                        connect_operation.clone(),
                    )),
                    request_digest: request_digest.clone(),
                    profile: None,
                }),
                secret_payload: Vec::new(),
            })
            .await
            .expect("Connect must be accepted");
        drop(reply);

        let deadline = time::Instant::now() + EVENT_DEADLINE;
        loop {
            assert!(
                time::Instant::now() < deadline,
                "第 {round} 轮必须在时限内到达 Connected"
            );
            let event = next_snapshot_event(&mut watch).await;
            let snapshot = event.snapshot.as_ref().expect("snapshot present");
            let failure = match snapshot.state.as_ref() {
                Some(runtime_snapshot::State::FailedClean(failed)) => failed.last_error.as_ref(),
                Some(runtime_snapshot::State::FailedDirty(failed)) => failed.last_error.as_ref(),
                _ => None,
            };
            if let Some(error) = failure {
                panic!(
                    "第 {round} 轮连接失败：code={} stage={}",
                    error.code, error.stage
                );
            }
            let connected = matches!(
                snapshot.state.as_ref(),
                Some(runtime_snapshot::State::Connected(_))
            ) && snapshot.stats.is_some();
            if connected {
                assert_detection_matches_mihomo(
                    &format!("WatchEvents Connected（第 {round} 轮）"),
                    snapshot.proxy_tun.as_ref(),
                    &mihomo_utun,
                );
                break;
            }
        }
        println!(
            "[S2] 第 {round} 轮：Connected 到达，开始分流断言（目标探测动作：校园 DNS 53 / 校园边缘 TLS / 公网域名 fake-ip TLS）"
        );

        // ── 分流断言（连接期）：校园 IP 经隧道，公网域名走 fake-ip 代理路径 ──
        let during = collect_split_facts();
        let during_dump = route_dump();
        let during_17220 = holders_of_17220(&interface_ipv4_map());
        let exv_interfaces: Vec<String> =
            during_17220.difference(&baseline_17220).cloned().collect();
        assert!(
            !exv_interfaces.is_empty(),
            "第 {round} 轮连接期：必须观察到 exv 隧道接口（新增 172.20/16 持有者）"
        );
        let exv_ifindex = if_index(exv_interfaces.first().expect("checked non-empty above"));

        // ① 校园 IP 目标：真实路由决策必须经 exv 隧道接口（最长前缀胜出，
        //    出接口不再是 Mihomo 接管段），并配真实业务探测（DNS 53 / 边缘 TLS）。
        let campus_dns = during
            .campus_dns_decision
            .clone()
            .expect("校园 IP 路由查询必须成功");
        assert_eq!(
            campus_dns.ifindex,
            exv_ifindex,
            "第 {round} 轮连接期：校园 IP {CAMPUS_DNS} 必须经 exv 隧道接口，实得 {}",
            decision_text(Some(&campus_dns))
        );
        let campus_edge = during
            .campus_edge_decision
            .clone()
            .expect("校园边缘路由查询必须成功");
        assert_eq!(
            campus_edge.ifindex,
            exv_ifindex,
            "第 {round} 轮连接期：校园边缘 {CAMPUS_EDGE} 必须经 exv 隧道接口，实得 {}",
            decision_text(Some(&campus_edge))
        );
        let dns_reply = dns_probe(CAMPUS_DNS, "ecnu.edu.cn").unwrap_or_else(|| {
            panic!("第 {round} 轮连接期：校内 DNS {CAMPUS_DNS} 53 业务探测必须经隧道得到响应")
        });
        let edge_record = tls_record_probe(CAMPUS_EDGE, 443, &format!("campus-edge-round{round}"));
        println!(
            "[S2] 第 {round} 轮：校园 IP 经隧道业务证明——route get {CAMPUS_DNS} → {} (if {exv_ifindex})；DNS 202.120.80.2 响应 {dns_reply} B；边缘 TLS record={edge_record:#x}",
            campus_dns.ifname
        );

        // ② 公网域名目标：fake-ip 解析 + 解析结果路由仍归 Mihomo + 真实代理业务探测。
        let resolution = during
            .public_resolution
            .expect("公网域名必须可解析（连接期）");
        assert!(
            is_fake_ip(resolution),
            "第 {round} 轮连接期：{PUBLIC_DOMAIN} 必须解析为 fake-ip（198.18.0.0/15），实得 {resolution}——如用户 fake-ip-filter 差异请如实回填证据"
        );
        let public_decision = during
            .public_decision
            .clone()
            .expect("fake-ip 路由查询必须成功");
        assert_eq!(
            public_decision.ifindex,
            mihomo_ifindex,
            "第 {round} 轮连接期：fake-ip 目标必须仍经 Mihomo TUN（exv 不得截走公网），实得 {}",
            decision_text(Some(&public_decision))
        );
        assert!(
            during.takeover_count() >= 6,
            "第 {round} 轮连接期：Mihomo 接管段必须仍在，实得 {} 块",
            during.takeover_count()
        );
        let fake_record = tls_record_probe_best_effort(resolution, 443, 2);
        println!(
            "[S2] 第 {round} 轮：公网域名走 fake-ip 代理证明——{PUBLIC_DOMAIN}={resolution}（fake-ip），route get → {} (if {})，业务探测 {}（Mihomo 代理路径；有界重试，无响应如实记录）",
            public_decision.ifname,
            public_decision.ifindex,
            fake_record.map_or_else(
                || "无 TLS 响应".to_owned(),
                |record| format!("TLS record={record:#x}")
            )
        );

        // ③ exv 只装校园 routes：连接期新增路由如实记录（三方快照之「连接期」）。
        let added: Vec<DumpEntry> = during_dump
            .iter()
            .filter(|entry| !baseline_keys.contains(entry))
            .cloned()
            .collect();
        // exv 只装自己的路由（红线：绝不经 Mihomo 接口）。凡出接口为 Mihomo TUN
        // 的条目属于用户代理自身行为（如 fake-ip 主机路由），不计入 exv 残留判定，
        // 但如实记录其数量变化。
        let added_exv: Vec<DumpEntry> = added
            .iter()
            .filter(|entry| entry.ifindex != mihomo_ifindex)
            .cloned()
            .collect();
        let added_via_mihomo = added.len() - added_exv.len();
        println!(
            "[S2] 第 {round} 轮：连接期新增路由 {} 条（其中 exv 侧 {} 条、Mihomo 侧 {} 条；基线 {} 条 → 连接期 {} 条），exv 隧道接口 {exv_interfaces:?}",
            added.len(),
            added_exv.len(),
            added_via_mihomo,
            baseline_dump.len(),
            during_dump.len()
        );
        print_split_table(&format!("第 {round} 轮 · 连接期"), &during);

        // ── Stop（产品链路）→ typed 终态（Idle 快照）────────────────────
        let stop_operation = random_bytes();
        let stop_reply = client
            .stop(StopRequest {
                intent: Some(StopIntent {
                    lookup_key: Some(lookup_key(
                        OperationMethod::StopTunnel as i32,
                        stop_operation,
                    )),
                    request_digest: request_digest.clone(),
                }),
            })
            .await
            .expect("Stop must be accepted");
        drop(stop_reply);

        let deadline = time::Instant::now() + EVENT_DEADLINE;
        loop {
            assert!(
                time::Instant::now() < deadline,
                "第 {round} 轮 Stop 后必须在时限内回到 Idle"
            );
            let event = next_snapshot_event(&mut watch).await;
            let snapshot = event.snapshot.as_ref().expect("snapshot present");
            if matches!(
                snapshot.state.as_ref(),
                Some(runtime_snapshot::State::Idle(_))
            ) {
                assert_detection_matches_mihomo(
                    &format!("WatchEvents Idle（第 {round} 轮 Stop 后）"),
                    snapshot.proxy_tun.as_ref(),
                    &mihomo_utun,
                );
                break;
            }
        }

        // ── exv 清理 + Mihomo 完好 readback（有界轮询，纯只读）────────────
        let cleaned_started = time::Instant::now();
        loop {
            let dump = route_dump();
            let campus_back_to_mihomo =
                route_get(CAMPUS_DNS).is_some_and(|decision| decision.ifindex == mihomo_ifindex);
            let no_new_17220 = holders_of_17220(&interface_ipv4_map()) == baseline_17220;
            // 残留只判「具备 exv 安装特征」的条目（DST+GATEWAY+NETMASK 且 flags
            // 含 0x13）；系统瞬态条目（邻居/协议克隆路由，自然过期）如实计数，
            // 不打印其目的地址（P0：不泄露服务器/路径明文）。
            let transient = dump
                .iter()
                .filter(|entry| {
                    added_exv.contains(entry) && !entry.has_exv_installation_signature()
                })
                .count();
            let residual: Vec<&DumpEntry> = dump
                .iter()
                .filter(|entry| added_exv.contains(entry) && entry.has_exv_installation_signature())
                .collect();
            if campus_back_to_mihomo && no_new_17220 && residual.is_empty() {
                println!(
                    "[S2] 第 {round} 轮：exv 清理 readback 在 {:?} 内完成——校园路由归还 Mihomo 接管段、无 172.20/16 新增残留、零 exv 特征残留路由（系统瞬态条目 {transient} 条，自然过期，不计入）",
                    cleaned_started.elapsed()
                );
                break;
            }
            assert!(
                cleaned_started.elapsed() <= CLEAN_POLL_BOUND,
                "第 {round} 轮 Stop 后清理超界：campus_back_to_mihomo={campus_back_to_mihomo} no_new_17220={no_new_17220} residual_count={} transient_count={transient}",
                residual.len()
            );
            std::thread::sleep(Duration::from_millis(300));
        }

        // Mihomo 完好 readback：接口、地址、接管段、默认路由、fake-ip DNS、代理端口。
        let after = collect_split_facts();
        assert_eq!(
            after.mihomo_utun.as_deref(),
            Some(mihomo_utun.as_str()),
            "第 {round} 轮 Stop 后：Mihomo TUN（用户资产）必须仍在"
        );
        assert_eq!(
            after.mihomo_fake_ip,
            Some(mihomo_fake_ip),
            "第 {round} 轮 Stop 后：fake-ip 接口地址必须不变"
        );
        assert_eq!(
            after.takeover_count(),
            baseline_takeover,
            "第 {round} 轮 Stop 后：接管段路由必须完好（与基线块数一致），实得 {}",
            after.takeover_count()
        );
        assert_eq!(
            after.default_via, baseline.default_via,
            "第 {round} 轮 Stop 后：用户默认路由必须未被破坏"
        );
        assert!(
            route_get(PUBLIC_GENERAL).is_some_and(|decision| decision.ifindex == mihomo_ifindex),
            "第 {round} 轮 Stop 后：公网一般目标必须仍经 Mihomo TUN"
        );
        let after_resolution = after
            .public_resolution
            .expect("Stop 后公网域名仍必须可解析");
        assert!(
            is_fake_ip(after_resolution),
            "第 {round} 轮 Stop 后：fake-ip DNS 行为必须仍在（实得 {after_resolution}）"
        );
        if baseline.proxy_port_open {
            assert!(
                after.proxy_port_open,
                "第 {round} 轮 Stop 后：代理端口 127.0.0.1:{PROXY_PORT} 必须仍连通"
            );
        }
        // fake-ip 代理业务仍通（Stop 只回收 exv 隧道，不动 Mihomo；有界重试，
        // 无响应如实记录——路由归属与 DNS 证据已证明 Mihomo 路径完好）。
        let intact_record = tls_record_probe_best_effort(after_resolution, 443, 2);
        println!(
            "[S2] 第 {round} 轮：Mihomo 完好 readback——{mihomo_utun}@{mihomo_fake_ip} 在、接管段 {} 块、默认路由 {:?}、公网一般目标 route get → Mihomo、fake-ip DNS {after_resolution}、代理端口 {}、业务探测 {}",
            after.takeover_count(),
            after.default_via,
            after.proxy_port_open,
            intact_record.map_or_else(
                || "无 TLS 响应（如实记录）".to_owned(),
                |record| format!("TLS record={record:#x}")
            )
        );
        print_split_table(&format!("第 {round} 轮 · Stop 后（Mihomo 完好）"), &after);
        println!("[S2] ======== 第 {round} 轮完成 ========");

        // 每轮之间不重置第三方组件（Mihomo 全程未触碰）；轮次隔离只靠新 operation。
    }

    // ── 终局：Idle GetSnapshot 检测事实 + 总清理 readback + Core 回收 ────
    let final_snapshot = client
        .get_snapshot(SnapshotRequest {
            runtime_epoch: Vec::new(),
        })
        .await
        .expect("final GetSnapshot must answer")
        .into_inner();
    assert!(
        matches!(
            final_snapshot.state.as_ref(),
            Some(runtime_snapshot::State::Idle(_))
        ),
        "终局状态必须 Idle"
    );
    assert_detection_matches_mihomo(
        "终局 GetSnapshot",
        final_snapshot.proxy_tun.as_ref(),
        &mihomo_utun,
    );
    let final_17220 = holders_of_17220(&interface_ipv4_map());
    assert_eq!(
        final_17220, baseline_17220,
        "终局：不得残留任何连接期新增的 172.20/16 隧道地址接口"
    );
    println!("[S2] 终局总清理 readback 通过（两轮零残留，凭据与 Mihomo 均保持原状）");

    drop(watch);
    drop(client);
    drop(guard);
    runtime
        .cleanup_empty()
        .expect("runtime dir must clean after core exit");
    println!("[S2] core exited, runtime cleaned");
}
