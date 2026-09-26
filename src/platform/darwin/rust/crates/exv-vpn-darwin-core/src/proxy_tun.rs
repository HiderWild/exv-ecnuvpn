//! 只读上游代理 TUN 检测（MAC-PROXY-16 S1；PRD G-⑥ detect-and-report）。
//!
//! 检测归属普通用户 Core（`exv-vpn-darwin-core`）：Engine 按需拉起、Idle 态不存在，
//! 而检测必须覆盖连接前后控制面；`getifaddrs` 与 PF_ROUTE 路由表 dump 均为无特权
//! 只读系统调用，Core（非 root）可做。本模块**只读取系统事实，绝不修改路由、接口、
//! DNS 或任何网络状态**（PRD O1）；探测结果由 `kernel_control_service` 在快照出向
//! 边界统一附加（mirror stats 方案 A）。
//!
//! 判据（系统事实，不是名字 token——macOS utun 无 friendly name/description，
//! Windows 的 GetAdaptersAddresses token 匹配不可移植也不移植）：
//!   1. **fake-ip 接口地址**：候选 utun 接口持有 `198.18.0.0/15`（RFC 2544 基准段，
//!      代理 TUN fake-ip 惯用段，与 Engine `egress/resolver.rs` `is_fake_ip` 同一判据）
//!      内的 IPv4 地址——本机观察事实：Clash Verge 的 utun1024 持 198.18.0.1；
//!   2. **接管段路由**：路由表中 `0.0.0.0/1` 或 `128.0.0.0/1`（1/8…128.0/1 接管段族
//!      的规范对）条目的出口接口为候选 utun（11a 记录的 utun1024 接管段形态）。
//!
//!   满足任一即 `detected`。
//!   3. **默认路由（0.0.0.0/0）被候选 utun 持有**：明确的外部全隧道路由事实。
//!
//! 判据以 IPv4 事实为准（与 egress/offer 的 IPv4 面一致）；IPv6 接口地址
//! （fdfe:dcba:9876::1 等）不作判据、不形成门禁。
//!
//! 自排除：Engine 在状态事件中上报实际持有的接口索引，Core 在输出前排除该接口。
//! 生产快照最长每两秒刷新一次，因此连接期间切换外部 TUN 也会更新；不靠接口新旧猜归属。
//!
//! `name`/`description` 如实填接口名（macOS 无 friendly name，不伪造）；`description`
//! 以 `;` 连接该接口命中的事实摘要（fake-ip 地址、接管段/默认路由 CIDR）作为证据
//! 字符串。探测系统调用失败 → `None`（proto「None = 未探测（或探测失败）」语义），
//! 不重试、不伪造 detected=false。

// getifaddrs/PF_ROUTE 的原始解析在逐字段边界检查后做窄化转换；中文文档中的技术
// 术语不逐个加反引号。
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::cast_ptr_alignment)]

use std::ffi::CStr;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use exv_vpn_wire::generated as wire;

/// `route_policy`：检测到上游代理 TUN（对齐 proto/C++ `to_json` 契约字符串）。
pub(crate) const ROUTE_POLICY_EXV_BEFORE_PROXY_TUN: &str = "exv-before-proxy-tun";

/// `route_policy`：未检测到上游代理 TUN。
pub(crate) const ROUTE_POLICY_NORMAL: &str = "normal";

/// wire `ProxyTunAdapter.kind` 恒定值（对齐 Windows 侧 `KIND_PROXY_TUN`）。
const KIND_PROXY_TUN: &str = "proxy_tun";

/// macOS TUN 接口名前缀：候选接口必须是系统 utun。
const UTUN_PREFIX: &str = "utun";

/// 一次探测的接口事实（`getifaddrs`，IPv4 面，UP|RUNNING）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InterfaceFact {
    /// IPv4 接口索引（`if_nametoindex`）。
    pub(crate) if_index: u32,
    /// 接口名（如 `utun1024` / `en0`）。
    pub(crate) name: String,
    /// 该接口的全部 IPv4 地址。
    pub(crate) ipv4_addresses: Vec<Ipv4Addr>,
}

/// 一次探测的路由事实（PF_ROUTE dump 的 IPv4 条目）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RouteFact {
    /// 目的地址（RTA_DST）。
    pub(crate) destination: Ipv4Addr,
    /// 前缀长度（RTA_NETMASK 前导 1 位数；非连续掩码的条目在收集期被跳过）。
    pub(crate) prefix_len: u8,
    /// 出口接口索引（`rt_msghdr.rtm_index`）。
    pub(crate) if_index: u32,
}

/// 纯判据产物：命中接口的 wire 适配器事实与 detected 位。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DetectionOutcome {
    /// 是否至少命中一个上游代理 TUN 适配器。
    pub(crate) detected: bool,
    /// 命中的适配器事实（未命中为空）。
    pub(crate) adapters: Vec<wire::ProxyTunAdapter>,
}

impl DetectionOutcome {
    /// 共存路由策略字符串（对齐 proto `ProxyTunDetection.route_policy` 契约）。
    #[must_use]
    pub(crate) fn route_policy(&self) -> &'static str {
        if self.detected {
            ROUTE_POLICY_EXV_BEFORE_PROXY_TUN
        } else {
            ROUTE_POLICY_NORMAL
        }
    }
}

/// fake-ip 判据：`198.18.0.0/15`（与 Engine `egress/resolver.rs` 同一常量判据）。
pub(crate) fn is_fake_ip_address(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

/// 候选接口判定：系统 utun 且不在排除集（自排除接缝）。
fn is_candidate(fact: &InterfaceFact, excluded_if_indexes: &[u32]) -> bool {
    fact.name.starts_with(UTUN_PREFIX) && !excluded_if_indexes.contains(&fact.if_index)
}

/// 纯判据：从注入的接口/路由事实判定上游代理 TUN（无任何系统调用）。
///
/// 判据见模块文档；`excluded_if_indexes` 可供事实层直接排除己方接口，生产缓存
/// 另按 Engine 状态事件的实际索引统一过滤。输入只读，不修改网络。
#[must_use]
pub(crate) fn detect_from_facts(
    interfaces: &[InterfaceFact],
    routes: &[RouteFact],
    excluded_if_indexes: &[u32],
) -> DetectionOutcome {
    // 判据 2/3 的路由按出口接口索引分桶（只关注候选 utun 持有的条目）。
    let takeover_routes = |if_index: u32| -> Vec<String> {
        routes
            .iter()
            .filter(|route| {
                route.if_index == if_index
                    && route.prefix_len == 1
                    && (route.destination == Ipv4Addr::UNSPECIFIED
                        || route.destination == Ipv4Addr::new(128, 0, 0, 0))
            })
            .map(|route| format!("{}/{}", route.destination, route.prefix_len))
            .collect()
    };
    let holds_default_route = |if_index: u32| -> bool {
        routes.iter().any(|route| {
            route.if_index == if_index
                && route.prefix_len == 0
                && route.destination == Ipv4Addr::UNSPECIFIED
        })
    };

    let mut adapters = Vec::new();
    for fact in interfaces {
        if !is_candidate(fact, excluded_if_indexes) {
            continue;
        }
        // 判据 1：fake-ip 接口地址。
        let fake_ips: Vec<String> = fact
            .ipv4_addresses
            .iter()
            .filter(|address| is_fake_ip_address(**address))
            .map(ToString::to_string)
            .collect();
        // 判据 2：接管段路由。
        let takeovers = takeover_routes(fact.if_index);
        // 判据 3：默认路由经外部 utun，覆盖采用单默认路由的全隧道。
        let default_route = holds_default_route(fact.if_index);
        if fake_ips.is_empty() && takeovers.is_empty() && !default_route {
            continue;
        }
        let mut evidence: Vec<String> = fake_ips;
        evidence.extend(takeovers);
        if default_route {
            evidence.push("0.0.0.0/0".to_owned());
        }
        adapters.push(wire::ProxyTunAdapter {
            name: fact.name.clone(),
            description: evidence.join(";"),
            if_index: fact.if_index,
            kind: KIND_PROXY_TUN.to_owned(),
        });
    }
    let detected = !adapters.is_empty();
    DetectionOutcome { detected, adapters }
}

/// 生产探针：一次无特权只读探测（getifaddrs + PF_ROUTE dump）；任一系统调用失败
/// 返回 `None`（proto「未探测/探测失败」语义），绝不伪造检测结果。
pub(crate) type ProxyTunProbe = Arc<dyn Fn() -> Option<wire::ProxyTunDetection> + Send + Sync>;

/// 生产探针构造。
#[must_use]
pub(crate) fn production_proxy_tun_probe() -> ProxyTunProbe {
    Arc::new(detect_upstream_proxy_tun)
}

/// 一次真实探测：系统事实 → 纯判据 → wire 形状。
fn detect_upstream_proxy_tun() -> Option<wire::ProxyTunDetection> {
    let interfaces = collect_interface_facts().ok()?;
    let routes = collect_route_facts().ok()?;
    // 先收集原始命中结果，再由缓存按 Engine 实际接口索引排除自身。
    let outcome = detect_from_facts(&interfaces, &routes, &[]);
    Some(to_wire_detection(&outcome))
}

/// 判据产物 → wire `ProxyTunDetection`（字段一一对应）。
#[must_use]
pub(crate) fn to_wire_detection(outcome: &DetectionOutcome) -> wire::ProxyTunDetection {
    wire::ProxyTunDetection {
        detected: outcome.detected,
        adapters: outcome.adapters.clone(),
        route_policy: outcome.route_policy().to_owned(),
    }
}

/// 收集接口 IPv4 事实（`getifaddrs`；**保留 utun**——与 Engine egress 的「排除一切
/// utun」变体不同，本收集保留全部接口，候选判定交给纯判据）。UP|RUNNING 且持有
/// AF_INET 地址的接口按名字聚合。
///
/// # Errors
///
/// `getifaddrs` 系统调用失败时返回 OS 错误。
pub(crate) fn collect_interface_facts() -> Result<Vec<InterfaceFact>, std::io::Error> {
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: addrs is the out pointer for a kernel-allocated list.
    if unsafe { libc::getifaddrs(std::ptr::addr_of_mut!(addrs)) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut facts: Vec<InterfaceFact> = Vec::new();
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
                    .to_string_lossy()
                    .into_owned();
                // SAFETY: AF_INET sockaddr is sockaddr_in; s_addr is the IPv4 octets.
                let octets = unsafe {
                    (*entry.ifa_addr.cast::<libc::sockaddr_in>())
                        .sin_addr
                        .s_addr
                };
                let address = Ipv4Addr::from(octets.to_ne_bytes());
                let flags = i64::from(entry.ifa_flags);
                if flags & i64::from(libc::IFF_UP | libc::IFF_RUNNING) != 0 {
                    if let Some(fact) = facts.iter_mut().find(|fact| fact.name == name) {
                        fact.ipv4_addresses.push(address);
                    } else {
                        // SAFETY: name is a NUL-terminated C string snapshot.
                        let c_name = unsafe { CStr::from_ptr(entry.ifa_name) };
                        // SAFETY: if_nametoindex only reads the name.
                        let if_index = unsafe { libc::if_nametoindex(c_name.as_ptr().cast()) };
                        if if_index != 0 {
                            facts.push(InterfaceFact {
                                if_index,
                                name,
                                ipv4_addresses: vec![address],
                            });
                        }
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

/// 从 PF_ROUTE dump 收集 IPv4 路由事实（与 Engine `egress/physical_route.rs` 同一
/// 数字 MIB `{CTL_NET, PF_ROUTE, 0, AF_UNSPEC, NET_RT_DUMP, 0}`；只读取，无修改）。
///
/// # Errors
///
/// sysctl 系统调用失败时返回 OS 错误。
fn collect_route_facts() -> Result<Vec<RouteFact>, std::io::Error> {
    collect_route_facts_bounded(usize::MAX)
}

pub(crate) fn collect_route_facts_bounded(max_bytes: usize) -> Result<Vec<RouteFact>, std::io::Error> {
    let dump = sysctl_net_route_dump(max_bytes)?;
    let header_size = std::mem::size_of::<libc::rt_msghdr>();
    let mut facts = Vec::new();
    let mut cursor = 0usize;
    while cursor + header_size <= dump.len() {
        // SAFETY: cursor is bounds-checked against header_size above.
        let header = unsafe {
            dump.as_ptr()
                .add(cursor)
                .cast::<libc::rt_msghdr>()
                .read_unaligned()
        };
        let message_len = usize::from(header.rtm_msglen);
        if message_len < header_size || cursor + message_len > dump.len() {
            break;
        }
        if header.rtm_version == libc::RTM_VERSION as u8
            && header.rtm_type == libc::RTM_GET as u8
            && header.rtm_addrs & libc::RTA_DST != 0
        {
            let body = &dump[cursor + header_size..cursor + message_len];
            if let Some((destination, prefix_len)) =
                parse_destination_and_prefix(body, header.rtm_addrs)
            {
                facts.push(RouteFact {
                    destination,
                    prefix_len,
                    if_index: u32::from(header.rtm_index),
                });
            }
        }
        cursor += message_len;
    }
    Ok(facts)
}

/// 依 `rtm_addrs` 位序遍历 sockaddr 序列，返回 `(RTA_DST, RTA_NETMASK 前缀长度)`。
///
/// 仅接受 AF_INET 的 DST 与 NETMASK；NETMASK 缺失、非 AF_INET 或掩码非连续
/// （无法表达为 CIDR 前缀）的条目返回 `None` 跳过。
fn parse_destination_and_prefix(body: &[u8], addrs: i32) -> Option<(Ipv4Addr, u8)> {
    const RTA_NETMASK_BIT: i32 = 0x4;
    let mut offset = 0usize;
    let mut destination = None;
    let mut prefix_len = None;
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
            destination = Some(Ipv4Addr::new(slot[4], slot[5], slot[6], slot[7]));
        }
        if 1 << bit == RTA_NETMASK_BIT && family == libc::AF_INET as u8 && slot.len() >= 8 {
            prefix_len = prefix_from_netmask([slot[4], slot[5], slot[6], slot[7]]);
        }
        offset += slot_len;
    }
    match (destination, prefix_len) {
        (Some(destination), Some(prefix_len)) => Some((destination, prefix_len)),
        _ => None,
    }
}

/// 连续掩码 → CIDR 前缀长度；非连续掩码返回 `None`。
fn prefix_from_netmask(octets: [u8; 4]) -> Option<u8> {
    let bits = u32::from_be_bytes(octets);
    let leading = bits.leading_ones();
    // 掩码必须形如前导 1 后接全 0（ CIDR 可表达）；全 0（默认路由）合法。
    if bits == 0 || bits == (u32::MAX << (32 - leading)) {
        Some(u8::try_from(leading).ok()?)
    } else {
        None
    }
}

/// PF_ROUTE dump 的数字 MIB 读取（与 Engine egress 相同；无特权只读）。
fn sysctl_net_route_dump(max_bytes: usize) -> Result<Vec<u8>, std::io::Error> {
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
        return Err(std::io::Error::last_os_error());
    }
    if size > max_bytes {
        return Err(std::io::Error::other("route diagnostic buffer limit"));
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
        return Err(std::io::Error::last_os_error());
    }
    buffer.truncate(size);
    Ok(buffer)
}

/// 最近一次检测结果的共享缓存（mirror win32 EventBus 的 proxy_tun 缓存；stats 方案 A）。
///
/// `refresh` 在连接边界调用，`current` 在生产快照中限频刷新。连接建立过渡期等待
/// Engine 的接口事实，再排除自身并恢复动态读取。探测失败把缓存置 `None`
/// (proto「未探测/探测失败」语义)，不延续可能过期的旧事实。
pub(crate) struct ProxyTunCache {
    probe: ProxyTunProbe,
    latest: Mutex<Option<wire::ProxyTunDetection>>,
    own_if_index: AtomicU32,
    dynamic: bool,
    refresh_allowed: AtomicBool,
    refreshed_at: Mutex<Option<Instant>>,
    system_proxy: Mutex<Option<wire::SystemProxyDetection>>,
}

impl ProxyTunCache {
    /// 以给定探针创建缓存；初始为「未探测」（`None`）。
    #[must_use]
    pub(crate) fn new(probe: ProxyTunProbe) -> Self {
        Self {
            probe,
            latest: Mutex::new(None),
            own_if_index: AtomicU32::new(0),
            dynamic: false,
            refresh_allowed: AtomicBool::new(true),
            refreshed_at: Mutex::new(None),
            system_proxy: Mutex::new(None),
        }
    }

    /// 真实宿主启用周期刷新；注入探针的局部测试保持确定性。
    pub(crate) fn production() -> Self {
        Self {
            dynamic: true,
            ..Self::new(production_proxy_tun_probe())
        }
    }

    pub(crate) fn set_owned_interface(&self, index: u32, refresh_allowed: bool) {
        self.refresh_allowed
            .store(refresh_allowed, Ordering::Release);
        if self.own_if_index.swap(index, Ordering::AcqRel) != index {
            *lock_recover(&self.refreshed_at) = None;
        }
    }

    pub(crate) fn is_production(&self) -> bool { self.dynamic }

    pub(crate) fn system_proxy(&self) -> Option<wire::SystemProxyDetection> {
        lock_recover(&self.system_proxy).clone()
    }

    /// 探测一次并刷新缓存；探测失败 → `None`（不重试、不阻塞调用方——有界只读系统
    /// 调用，fail open 到 None）。
    pub(crate) fn refresh(&self) {
        self.refresh_allowed.store(true, Ordering::Release);
        let detection = (self.probe)();
        let detection = exclude_owned(detection, self.own_if_index.load(Ordering::Acquire));
        if self.dynamic {
            let mut system_proxy =
                crate::system_proxy::detect(detection.as_ref().is_some_and(|value| value.detected));
            if detection.is_none()
                && let Some(value) = system_proxy.as_mut()
            {
                // 系统代理读取成功不等于 TUN 读取成功，不能为失败的另一项填拓扑结论。
                value.topology.clear();
            }
            *lock_recover(&self.system_proxy) = system_proxy;
        }
        *lock_recover(&self.latest) = detection;
        *lock_recover(&self.refreshed_at) = Some(Instant::now());
    }

    /// 最近一次检测结果（`None` = 尚未探测或探测失败）。
    #[must_use]
    pub(crate) fn current(&self) -> Option<wire::ProxyTunDetection> {
        let due = lock_recover(&self.refreshed_at)
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(2));
        if self.dynamic && self.refresh_allowed.load(Ordering::Acquire) && due {
            self.refresh();
        }
        exclude_owned(
            lock_recover(&self.latest).clone(),
            self.own_if_index.load(Ordering::Acquire),
        )
    }
}

/// 已知的 EXV 设备只能被排除；排除后同步更新布尔位和路由策略。
fn exclude_owned(
    mut detection: Option<wire::ProxyTunDetection>,
    index: u32,
) -> Option<wire::ProxyTunDetection> {
    if let Some(value) = detection.as_mut()
        && index != 0
    {
        value.adapters.retain(|adapter| adapter.if_index != index);
        value.detected = !value.adapters.is_empty();
        value.route_policy = if value.detected {
            ROUTE_POLICY_EXV_BEFORE_PROXY_TUN
        } else {
            ROUTE_POLICY_NORMAL
        }
        .to_owned();
    }
    detection
}

/// 与 `kernel_control_service::lock_recover` 同语义：锁中毒时取回内部值继续
/// （检测缓存只是状态上报，不因并发 panic 拒绝服务）。
fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
