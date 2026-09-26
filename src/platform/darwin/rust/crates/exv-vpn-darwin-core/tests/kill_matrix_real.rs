//! `MAC-SCHOOL-MVP-11a` 连接终止矩阵真机验收（opt-in）：对「Core kill、Engine
//! SIGTERM、Engine SIGKILL」三个终止场景取得真实清理/恢复结果并落盘证据。
//!
//! Windows 对齐行为（执行设计 §3.1 W-MVP-16）：Core 退出/失联后 Engine 必须
//! teardown、不遗留隧道；崩溃/被杀后回 Idle 可重试，不自动重连。
//!
//! 运行条件与 [`service_agent_saved_credentials_e2e`] 相同：已安装服务代理、根仓共享
//! `target/debug` 下存在 `exv-vpn-darwin-engine`、`~/.exv/` 已保存完整凭据。
//!
//! 运行方式：
//!
//! ```bash
//! EXV_DARWIN_SERVICE_AGENT_E2E=1 cargo test \
//!   --manifest-path src/platform/darwin/rust/Cargo.toml \
//!   -p exv-vpn-darwin-core \
//!   --test kill_matrix_real -- --ignored --nocapture --test-threads=1
//! ```
//!
//! 本测试绝不打印用户名、服务器地址或密码；网络 readback 全部使用纯 libc
//! （`sysctl` `PF_ROUTE` dump 与 `getifaddrs`），Engine 进程终止使用真实信号。
//!
//! 场景 A（Core kill）需要 Core 是一个可被 SIGKILL 的独立进程（生产拓扑中 Core
//! 正是独立进程），因此采用 W13-T 子进程模式：重执行本测试二进制进入子进程角色，
//! 以自己的 pid 建立 ticket/Engine 绑定并完成真实连接，由父进程 SIGKILL。这一
//! 子进程是受测行为本身（被杀的 Core），不是测试脚手架的外部命令替换。

use std::{
    net::Ipv4Addr,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use exv_vpn_darwin_core::{
    DarwinUiConfig, build_saved_connect_envelope,
    engine_lifecycle_real::{EngineControlSession, launch_fixed_engine_session},
};
use exv_vpn_darwin_ipc::connect_envelope::DarwinEngineConnectV1;
use exv_vpn_wire::generated as wire;

/// 与现有 service agent e2e 相同的 opt-in 总闸。
const COMPANION_E2E_ENV: &str = "EXV_DARWIN_SERVICE_AGENT_E2E";
/// 子进程角色标记：携带该环境变量的测试进程扮演持有认证连接的 Core。
const CHILD_ENV: &str = "EXV_KILL_MATRIX_CHILD";
/// 子进程就绪文件路径环境变量：Connected 后写入 engine pid 与路由快照。
const READY_ENV: &str = "EXV_KILL_MATRIX_READY";

/// Core kill 场景中 Engine 经 EOF 检测自行退出的硬时间界（Windows 对齐判据）。
const ENGINE_EXIT_BOUND: Duration = Duration::from_secs(20);
/// 内核拆除接口/地址的轮询上界。
const CLEAN_POLL_BOUND: Duration = Duration::from_secs(15);
/// Engine 被杀后 Core 侧状态流必须给出断连（而非悬挂）的上界。
const DISCONNECT_DETECT_BOUND: Duration = Duration::from_secs(10);

/// 三场景真实连接互斥：避免并行场景同时操作系统网络状态。
///
/// 使用 `tokio::sync::Mutex`：互斥区覆盖整个真机连接场景（含大量 await），属确需
/// 跨 await 持锁；异步锁等待不阻塞测试线程，且无毒化语义（测试 panic 即释放，
/// 与原 `into_inner` 行为一致）。
static MATRIX_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// 公共请求构造（与 Core 生产路径同形）
// ---------------------------------------------------------------------------

/// 用已保存凭据构造一次与 Core 生产路径同形的 Apply 请求；
/// 返回 `(operation_id, request, stop_digest)`。
fn connect_request() -> (Vec<u8>, wire::ApplyTunnelRequest, Vec<u8>) {
    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    let envelope: DarwinEngineConnectV1 =
        build_saved_connect_envelope(&config).expect("saved credentials must form the V1 envelope");
    let request_digest = envelope.profile_digest().to_vec();
    let stop_digest = request_digest.clone();
    let operation_id = {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).expect("random operation id");
        bytes.to_vec()
    };
    let mtu = u32::from(envelope.mtu_override().unwrap_or(1_290));
    let request = wire::ApplyTunnelRequest {
        lookup_key: Some(wire::OperationLookupKey {
            principal_digest: vec![0; 32],
            method: wire::OperationMethod::ApplyTunnel as i32,
            runtime_epoch: vec![0; 16],
            operation_id: operation_id.clone(),
        }),
        plan: Some(wire::TunnelPlan {
            mtu,
            ..Default::default()
        }),
        request_digest,
        secret_payload: envelope.encode().to_vec(),
        windows_connection_mode: wire::WindowsConnectionMode::Standard as i32,
    };
    (operation_id, request, stop_digest)
}

/// 消费状态流直到本 operation 到达 Connected；typed 失败直接 panic 并携带错误码。
async fn wait_connected(
    status: &mut tonic::Streaming<wire::ConnectStatusEvent>,
    operation_id: &[u8],
    context: &str,
) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(40), status.message())
            .await
            .unwrap_or_else(|_| panic!("{context}: status event within 40s"))
            .expect("{context}: status stream healthy")
            .expect("{context}: status event present");
        if event.operation_id != operation_id {
            continue;
        }
        if let Some(error) = event.error {
            let native = error.native.as_ref().map_or(0, |n| n.code);
            panic!(
                "{context}: pipeline failed: code={} stage={} native_errno={native} retry={}",
                error.code, error.stage, error.retry
            );
        }
        if wire::StatsPhase::try_from(event.coarse_phase).ok() == Some(wire::StatsPhase::Connected)
        {
            println!(
                "[{context}] reached Connected: phase={}",
                event.connect_phase
            );
            return;
        }
    }
}

/// 发起一次完整连接并保持：返回 `(session, status_stream, operation_id, stop_digest)`。
async fn connect_and_hold(
    context: &str,
) -> (
    EngineControlSession,
    tonic::Streaming<wire::ConnectStatusEvent>,
    Vec<u8>,
    Vec<u8>,
) {
    let (operation_id, request, stop_digest) = connect_request();
    let mut session = launch_fixed_engine_session().await.unwrap_or_else(|error| {
        panic!("{context}: service agent must start the fixed root Engine: {error}")
    });
    let mut status_stream = session
        .attach_connect_status()
        .await
        .unwrap_or_else(|error| panic!("{context}: attach status before apply: {error}"));
    let reply = session
        .apply_tunnel(request)
        .await
        .unwrap_or_else(|error| panic!("{context}: ApplyTunnel must be admitted: {error}"));
    assert!(
        matches!(
            reply.result,
            Some(wire::apply_tunnel_reply::Result::Pending(_))
        ),
        "{context}: ApplyTunnel must return Pending"
    );
    wait_connected(&mut status_stream, &operation_id, context).await;
    (session, status_stream, operation_id, stop_digest)
}

/// 显式 Stop 并确认 Engine 返回 Stopped；返回 stop 前的干净轮询责任交给调用方。
async fn stop_tunnel(
    session: &mut EngineControlSession,
    operation_id: &[u8],
    stop_digest: Vec<u8>,
    context: &str,
) {
    let stop = session
        .stop_tunnel(wire::StopTunnelRequest {
            lookup_key: Some(wire::OperationLookupKey {
                principal_digest: vec![0; 32],
                method: wire::OperationMethod::StopTunnel as i32,
                runtime_epoch: vec![0; 16],
                operation_id: operation_id.to_vec(),
            }),
            request_digest: stop_digest,
        })
        .await
        .unwrap_or_else(|error| panic!("{context}: StopTunnel must succeed: {error}"));
    assert!(
        matches!(
            stop.result,
            Some(wire::stop_tunnel_reply::Result::Stopped(_))
        ),
        "{context}: StopTunnel must return Stopped"
    );
}

// ---------------------------------------------------------------------------
// 网络 readback（纯 libc）
// ---------------------------------------------------------------------------

/// 一条内核路由的 readback 条目。
#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteEntry {
    destination: Ipv4Addr,
    prefix: u8,
    /// `RTF_GATEWAY` 且网关为 `AF_INET` 时的网关地址。
    gateway: Option<Ipv4Addr>,
    /// 网关 sockaddr 为 `AF_LINK` 时的接口索引（接口路由形态）。
    gateway_ifindex: Option<u32>,
    /// 路由出接口（`rtm_index`）。
    ifindex: u32,
    flags: i32,
}

impl RouteEntry {
    /// 忽略 `flags` 的稳定比较键（删除/残留判定使用）。
    fn key(&self) -> (Ipv4Addr, u8, Option<Ipv4Addr>, Option<u32>, u32) {
        (
            self.destination,
            self.prefix,
            self.gateway,
            self.gateway_ifindex,
            self.ifindex,
        )
    }

    /// bypass/host 路由特征：/32 且网关就是自身地址（物理网关 bypass）。
    fn is_physical_bypass(&self) -> bool {
        self.prefix == 32 && self.gateway == Some(self.destination)
    }
}

/// `PF_ROUTE` dump 的 MIB 元素数（`CTL_NET`, `PF_ROUTE`, 0, `AF_UNSPEC`, `NET_RT_DUMP`, 0）。
const ROUTE_DUMP_MIB_LEN: libc::c_uint = 6;

/// dump 内核 `PF_ROUTE` 路由表中的 `IPv4` 条目。
fn route_dump() -> Vec<RouteEntry> {
    // {CTL_NET, PF_ROUTE, 0, AF_UNSPEC, NET_RT_DUMP, 0}
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
            ROUTE_DUMP_MIB_LEN,
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
            ROUTE_DUMP_MIB_LEN,
            buffer.as_ptr().cast_mut().cast(),
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

/// 解析一次 PF_ROUTE dump 缓冲（测试内纯函数，便于对齐 rt_msghdr 布局）。
fn parse_route_dump(buffer: &[u8]) -> Vec<RouteEntry> {
    const AF_INET: u8 = 2;
    const AF_LINK: u8 = 18;
    const RTA_DST: i32 = 0x1;
    const RTA_GATEWAY: i32 = 0x2;
    const RTA_NETMASK: i32 = 0x4;
    const HEADER: usize = std::mem::size_of::<libc::rt_msghdr>();
    // macOS PF_ROUTE sockaddr 槽位按 4 字节对齐向上取整；sa_len 为 0 时占 4 字节。
    let roundup = |length: usize| if length == 0 { 4 } else { (length + 3) & !3 };

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
            || message_len < HEADER
            || cursor + message_len > buffer.len()
        {
            break;
        }
        if rtm.rtm_addrs & RTA_DST != 0 {
            let mut offset = cursor + HEADER;
            let mut slot: i32 = 1;
            let mut destination = None;
            let mut gateway = None;
            let mut gateway_ifindex = None;
            let mut netmask = None;
            while slot <= libc::RTA_AUTHOR && offset <= cursor + message_len {
                if rtm.rtm_addrs & slot != 0 {
                    if offset + 2 <= cursor + message_len {
                        // SAFETY: sa_len/sa_family 两字节已做边界检查。
                        let sa_len = usize::from(buffer[offset]);
                        let sa_family = buffer[offset + 1];
                        let bounded = sa_len.min(cursor + message_len - offset).max(4);
                        match (sa_family, slot) {
                            (AF_INET, RTA_DST) if bounded >= 8 => {
                                // SAFETY: sockaddr_in 的地址字节在 bounded 范围内。
                                destination = Some(Ipv4Addr::new(
                                    buffer[offset + 4],
                                    buffer[offset + 5],
                                    buffer[offset + 6],
                                    buffer[offset + 7],
                                ));
                            }
                            (AF_INET, RTA_GATEWAY) if bounded >= 8 => {
                                gateway = Some(Ipv4Addr::new(
                                    buffer[offset + 4],
                                    buffer[offset + 5],
                                    buffer[offset + 6],
                                    buffer[offset + 7],
                                ));
                            }
                            (AF_LINK, RTA_GATEWAY) if bounded >= 6 => {
                                // sockaddr_dl: len, family, index(u16), ...
                                gateway_ifindex = Some(u32::from(u16::from_ne_bytes([
                                    buffer[offset + 4],
                                    buffer[offset + 5],
                                ])));
                            }
                            (AF_INET, RTA_NETMASK) if bounded >= 8 => {
                                netmask = Some(Ipv4Addr::new(
                                    buffer[offset + 4],
                                    buffer[offset + 5],
                                    buffer[offset + 6],
                                    buffer[offset + 7],
                                ));
                            }
                            // dump 中默认路由等 /0 条目的 netmask 是 sa_len=0 的空槽。
                            (_, RTA_NETMASK) => {
                                netmask = Some(Ipv4Addr::UNSPECIFIED);
                            }
                            _ => {}
                        }
                    }
                    let sa_len = usize::from(buffer[offset]);
                    offset += roundup(sa_len);
                } else {
                    slot <<= 1;
                    continue;
                }
                slot <<= 1;
            }
            if let (Some(destination), Some(netmask)) = (destination, netmask) {
                let mask = u32::from(netmask);
                let prefix = if mask == 0 {
                    0
                } else {
                    32 - mask.trailing_zeros().min(32) as u8
                };
                entries.push(RouteEntry {
                    destination,
                    prefix,
                    gateway,
                    gateway_ifindex,
                    ifindex: u32::from(rtm.rtm_index),
                    flags: rtm.rtm_flags,
                });
            }
        }
        cursor += message_len;
    }
    entries
}

/// 任一接口仍持有 172.20/16 隧道地址。
fn tunnel_address_present() -> bool {
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: addrs 为内核分配列表的出参。
    if unsafe { libc::getifaddrs(&mut addrs) } != 0 {
        return false;
    }
    let mut present = false;
    let mut cursor = addrs;
    while !cursor.is_null() {
        // SAFETY: cursor 指向链表有效节点。
        let entry = unsafe { &*cursor };
        let next = entry.ifa_next;
        if !entry.ifa_addr.is_null() {
            // SAFETY: ifa_addr 非空时为有效 sockaddr。
            let family = unsafe { (*entry.ifa_addr).sa_family };
            if family == libc::AF_INET as u8 {
                // SAFETY: AF_INET sockaddr 即 sockaddr_in。
                let raw = unsafe {
                    (*entry.ifa_addr.cast::<libc::sockaddr_in>())
                        .sin_addr
                        .s_addr
                };
                let octets = raw.to_ne_bytes();
                if octets[0] == 172 && octets[1] == 20 {
                    present = true;
                }
            }
        }
        cursor = next;
    }
    // SAFETY: 释放 getifaddrs 列表。
    unsafe { libc::freeifaddrs(addrs) };
    present
}

/// 校园前缀（172.20/16 或 222.66.117/24）路由是否仍存在。
fn campus_route_present(dump: &[RouteEntry]) -> bool {
    dump.iter().any(|entry| {
        let octets = entry.destination.octets();
        (octets[0] == 172 && octets[1] == 20 && entry.prefix >= 16)
            || (octets[0] == 222 && octets[1] == 66 && octets[2] == 117 && entry.prefix >= 24)
    })
}

/// 有界轮询直到隧道地址与校园路由消失；返回最终事实。
fn poll_clean(context: &str) -> (bool, Vec<RouteEntry>) {
    let started = Instant::now();
    loop {
        let dump = route_dump();
        if !tunnel_address_present() && !campus_route_present(&dump) {
            println!(
                "[{context}] clean readback after {:?}: no 172.20/16 address, no campus route",
                started.elapsed()
            );
            return (true, dump);
        }
        if started.elapsed() > CLEAN_POLL_BOUND {
            println!(
                "[{context}] clean readback TIMEOUT after {:?}: address_17220={} campus_route={}",
                started.elapsed(),
                tunnel_address_present(),
                campus_route_present(&dump)
            );
            return (false, dump);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 三方快照残留判定：连接期新增（相对连接前基线）且终止后仍在的路由。
///
/// 快照对比必须以连接前基线排除宿主既有路由（如 Mihomo TUN 的 0/1 接管段），
/// 否则与隧道无关的既有条目会被误判为残留。
fn residual_routes(
    before: &[RouteEntry],
    connected: &[RouteEntry],
    after: &[RouteEntry],
) -> Vec<RouteEntry> {
    let before_keys: std::collections::HashSet<_> = before.iter().map(RouteEntry::key).collect();
    let after_keys: std::collections::HashSet<_> = after.iter().map(RouteEntry::key).collect();
    connected
        .iter()
        .filter(|entry| !before_keys.contains(&entry.key()) && after_keys.contains(&entry.key()))
        .cloned()
        .collect()
}

/// 发送真实信号。
fn send_signal(pid: u32, signal: libc::c_int) {
    // SAFETY: kill(2) 只读取参数。
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    assert_eq!(result, 0, "signal {signal} to pid {pid} must be delivered");
}

/// 有界等待进程消失（kqueue `EVFILT_PROC` + `NOTE_EXIT`）；返回实测耗时。
///
/// Engine 的父进程是服务代理（不及时 reap），已退出的 Engine 会短暂停留为
/// `<defunct>`；`kill(pid, 0)` 对僵尸仍返回成功，无法区分真活。kqueue 的进程
/// 退出事件对同 real uid 目标可监视（Engine real uid 与本进程一致），在目标
/// 进入僵尸态时即触发，是纯 libc 的真实退出判据。
fn wait_process_gone(pid: u32, bound: Duration, context: &str) -> Duration {
    let started = Instant::now();
    // 目标已被 reap（pid 不复存在）时直接判定退出，避免对死 pid 注册事件。
    // SAFETY: kill(2) 只读取参数。
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return started.elapsed();
    }
    // SAFETY: kqueue() 返回新描述符，无副作用。
    let queue = unsafe { libc::kqueue() };
    assert!(queue >= 0, "{context}: kqueue() must succeed");
    let registration = libc::kevent {
        ident: pid as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut fired: libc::kevent = unsafe { std::mem::zeroed() };
    let timeout = libc::timespec {
        tv_sec: bound.as_secs() as libc::time_t,
        tv_nsec: libc::c_long::from(bound.subsec_nanos()),
    };
    // SAFETY: changelist 读取栈上注册事件，eventlist 写入栈上缓冲。
    let count = unsafe {
        libc::kevent(
            queue,
            std::ptr::from_ref(&registration),
            1,
            &mut fired,
            1,
            &timeout,
        )
    };
    // SAFETY: 关闭本调用创建的 kqueue 描述符。
    unsafe { libc::close(queue) };
    assert!(
        count >= 0,
        "{context}: kevent(EVFILT_PROC/NOTE_EXIT) failed: {}",
        std::io::Error::last_os_error()
    );
    assert!(
        count == 1,
        "{context}: pid {pid} still alive after {bound:?} (no NOTE_EXIT)"
    );
    started.elapsed()
}

// ---------------------------------------------------------------------------
// readback 断言与重试验证
// ---------------------------------------------------------------------------

/// 断言终止后网络状态：无隧道地址、无校园路由、连接期新增路由零残留；打印事实。
fn assert_no_residual(
    context: &str,
    before: &[RouteEntry],
    connected: &[RouteEntry],
    after: &[RouteEntry],
) {
    let residual = residual_routes(before, connected, after);
    for entry in &residual {
        let kind = if entry.is_physical_bypass() {
            "physical-bypass"
        } else {
            "other"
        };
        println!(
            "[{context}] RESIDUAL route ({kind}) dst={}/{} gw={:?} gw_if={:?} if={}",
            entry.destination, entry.prefix, entry.gateway, entry.gateway_ifindex, entry.ifindex
        );
    }
    assert!(
        residual.is_empty(),
        "{context}: 连接期应用的路由在终止后必须零残留（含物理网关 bypass /32），残留 {} 条",
        residual.len()
    );
}

/// 终止后的重试验证：重新连接到 Connected，Stop 后网络资源干净。
async fn assert_retry_succeeds_and_stops_clean(context: &str) {
    let (mut session, _status, operation_id, stop_digest) = connect_and_hold(context).await;
    stop_tunnel(&mut session, &operation_id, stop_digest, context).await;
    session
        .close()
        .expect("{context}: Engine must exit clean after close");
    let (clean, final_dump) = poll_clean(context);
    assert!(clean, "{context}: Stop 后网络资源必须干净");
    // Stop 后同样不允许任何校园/隧道路由残留（含 bypass 自愈后的形态）。
    assert!(
        !final_dump.iter().any(|entry| entry.is_physical_bypass()),
        "{context}: Stop 后物理网关 bypass /32 必须已被 teardown 删除"
    );
}

// ---------------------------------------------------------------------------
// 场景 A：Core kill（SIGKILL Core 子进程）
// ---------------------------------------------------------------------------

/// 场景 A 子进程角色：以本进程 pid 建立 ticket/Engine 绑定，完成真实连接到
/// Connected，写就绪文件（engine pid + 连接前基线 + 连接期路由快照），随后
/// 阻塞等待被 SIGKILL。
async fn child_role_connect_and_block(ready_path: PathBuf) {
    let context = "core-kill-child";
    let before = route_dump();
    let (session, _status, _operation_id, _stop_digest) = connect_and_hold(context).await;
    let connected = route_dump();
    let mut snapshot = format!("engine_pid={}\n", session.engine_pid());
    snapshot.push_str("BEFORE\n");
    for entry in &before {
        snapshot.push_str(&format!(
            "route dst={}/{} gw={:?} gw_if={:?} if={}\n",
            entry.destination, entry.prefix, entry.gateway, entry.gateway_ifindex, entry.ifindex
        ));
    }
    snapshot.push_str("CONNECTED\n");
    for entry in &connected {
        snapshot.push_str(&format!(
            "route dst={}/{} gw={:?} gw_if={:?} if={}\n",
            entry.destination, entry.prefix, entry.gateway, entry.gateway_ifindex, entry.ifindex
        ));
    }
    std::fs::write(ready_path, snapshot).expect("write ready marker");
    std::future::pending::<()>().await;
}

/// 解析就绪文件：第一行 `engine_pid=`，随后 `BEFORE`/`CONNECTED` 两段路由快照。
fn parse_ready(ready: &str) -> (u32, Vec<RouteEntry>, Vec<RouteEntry>) {
    let mut lines = ready.lines();
    let engine_pid = lines
        .next()
        .and_then(|line| line.strip_prefix("engine_pid="))
        .and_then(|value| value.parse().ok())
        .expect("ready file must carry engine pid");
    let mut section = "";
    let mut before = Vec::new();
    let mut connected = Vec::new();
    for line in lines {
        match line {
            "BEFORE" => {
                section = "before";
                continue;
            }
            "CONNECTED" => {
                section = "connected";
                continue;
            }
            _ => {}
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let mut destination = None;
        let mut prefix = 0_u8;
        let mut gateway = None;
        let mut gateway_ifindex = None;
        let mut ifindex = 0_u32;
        for field in fields {
            if let Some(value) = field.strip_prefix("dst=") {
                if let Some((address, bits)) = value.split_once('/') {
                    destination = address.parse().ok();
                    prefix = bits.parse().unwrap_or(0);
                }
            } else if let Some(value) = field.strip_prefix("gw=") {
                gateway = value.parse().ok();
            } else if let Some(value) = field.strip_prefix("gw_if=") {
                gateway_ifindex = value.parse().ok();
            } else if let Some(value) = field.strip_prefix("if=") {
                ifindex = value.parse().unwrap_or(0);
            }
        }
        if let Some(destination) = destination {
            let entry = RouteEntry {
                destination,
                prefix,
                gateway,
                gateway_ifindex,
                ifindex,
                flags: 0,
            };
            if section == "before" {
                before.push(entry);
            } else if section == "connected" {
                connected.push(entry);
            }
        }
    }
    (engine_pid, before, connected)
}

// ---------------------------------------------------------------------------
// 三个终止场景
// ---------------------------------------------------------------------------

/// **场景 A（Core kill）**：SIGKILL 持有认证连接的 Core 子进程后，Engine 必须经
/// EOF/失联检测在硬时间界内自行退出（Windows 对齐 W-MVP-16），utun 地址/校园
/// 路由/物理网关 bypass 全部清理，且之后可以重新连接（Idle 可重试）。
#[tokio::test(flavor = "current_thread")]
#[ignore = "需要已安装服务代理与已保存凭据的真实宿主；必须以 EXV_DARWIN_SERVICE_AGENT_E2E=1 显式启用"]
async fn core_sigkill_engine_teardowns_and_retry_succeeds() {
    let _guard = MATRIX_LOCK.lock().await;
    assert!(
        std::env::var_os(COMPANION_E2E_ENV).is_some(),
        "该真机验收必须显式以 EXV_DARWIN_SERVICE_AGENT_E2E=1 运行"
    );

    // 子进程角色：连接成功后阻塞，由父进程 SIGKILL（本进程即受测 Core）。
    if std::env::var_os(CHILD_ENV).is_some() {
        let ready =
            PathBuf::from(std::env::var_os(READY_ENV).expect("child must receive the ready path"));
        child_role_connect_and_block(ready).await;
        std::process::exit(0);
    }

    let ready = std::env::temp_dir().join(format!(
        "exv-kill-matrix-core-sigkill-{}.ready",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&ready);
    let mut child = Command::new(std::env::current_exe().expect("current test exe"))
        .args([
            "--exact",
            "core_sigkill_engine_teardowns_and_retry_succeeds",
            "--ignored",
            "--nocapture",
        ])
        .env(COMPANION_E2E_ENV, "1")
        .env(CHILD_ENV, "1")
        .env(READY_ENV, &ready)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn core child (W13-T pattern)");

    // 就绪 = 子进程已带真实隧道到达 Connected。
    let started = Instant::now();
    while !ready.exists() {
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("core child did not reach Connected within 120s");
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("core child exited early with {status}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let snapshot = std::fs::read_to_string(&ready).expect("read ready marker");
    let (child_engine_pid, before, connected) = parse_ready(&snapshot);
    println!(
        "[core-kill] child connected: engine_pid={child_engine_pid} before_routes={} snapshot_routes={}",
        before.len(),
        connected.len()
    );

    // SIGKILL Core：内核关闭其全部 fd，Engine 侧认证连接收到 EOF。
    let core_pid = child.id();
    send_signal(core_pid, libc::SIGKILL);
    let wait = child.wait().expect("reap core child");
    println!("[core-kill] core pid {core_pid} SIGKILLed, wait status={wait:?}");

    // Engine 必须经失联检测在硬时间界内自行退出。
    let elapsed = wait_process_gone(child_engine_pid, ENGINE_EXIT_BOUND, "core-kill");
    println!(
        "[core-kill] EVIDENCE: engine exited by itself in {elapsed:?} (bound {ENGINE_EXIT_BOUND:?})"
    );
    assert!(
        elapsed <= ENGINE_EXIT_BOUND,
        "core kill → engine 必须在硬时间界内自退（Windows 对齐），实测 {elapsed:?}"
    );

    // 网络 readback：隧道地址/校园路由消失 + 连接期路由零残留。
    let (clean, after) = poll_clean("core-kill");
    assert!(clean, "core kill 后 utun 地址与校园路由必须被清理");
    assert_no_residual("core-kill", &before, &connected, &after);
    let _ = std::fs::remove_file(&ready);

    // Idle 可重试：重新走完整连接 → Stop → 干净。
    assert_retry_succeeds_and_stops_clean("core-kill-retry").await;
}

/// **场景 B（Engine SIGTERM）**：连接保持中向 root Engine（real uid 与本进程一致）
/// 发送真实 SIGTERM，Engine 必须走信号处理清理路径（逆序 teardown），Core 侧状态
/// 流在有限时间内得到断连而非悬挂，之后可以重新连接。
#[tokio::test(flavor = "current_thread")]
#[ignore = "需要已安装服务代理与已保存凭据的真实宿主；必须以 EXV_DARWIN_SERVICE_AGENT_E2E=1 显式启用"]
async fn engine_sigterm_signal_teardown_and_retry_succeeds() {
    let _guard = MATRIX_LOCK.lock().await;
    assert!(
        std::env::var_os(COMPANION_E2E_ENV).is_some(),
        "该真机验收必须显式以 EXV_DARWIN_SERVICE_AGENT_E2E=1 运行"
    );

    let context = "engine-sigterm";
    let before = route_dump();
    let (session, mut status_stream, _operation_id, _stop_digest) = connect_and_hold(context).await;
    let engine_pid = session.engine_pid();
    let connected = route_dump();
    let bypass_before: Vec<_> = connected
        .iter()
        .filter(|entry| entry.is_physical_bypass())
        .collect();
    println!(
        "[{context}] connected: engine_pid={engine_pid} routes={} bypass_entries={}",
        connected.len(),
        bypass_before.len()
    );

    // 真实 SIGTERM 到 root Engine（Engine real uid = 本进程 uid，信号被允许）。
    send_signal(engine_pid, libc::SIGTERM);

    // Core 侧：状态流必须在有限时间内给出断连，不允许悬挂。
    let disconnect = tokio::time::timeout(DISCONNECT_DETECT_BOUND, status_stream.message()).await;
    match disconnect {
        Ok(Ok(None)) => {
            println!("[{context}] EVIDENCE: status stream ended (None) = typed disconnect")
        }
        Ok(Err(status)) => {
            println!("[{context}] EVIDENCE: status stream returned transport error: {status}")
        }
        Ok(Ok(Some(event))) => {
            panic!("[{context}] status stream stayed alive after SIGTERM, got {event:?}")
        }
        Err(_) => panic!(
            "[{context}] status stream hung longer than {DISCONNECT_DETECT_BOUND:?} after SIGTERM"
        ),
    }

    // Engine 必须在硬时间界内退出（信号处理 teardown 后干净退出）。
    let elapsed = wait_process_gone(engine_pid, ENGINE_EXIT_BOUND, context);
    println!("[{context}] EVIDENCE: engine exited after SIGTERM in {elapsed:?}");

    // 网络 readback：隧道地址/校园路由消失 + 连接期路由零残留（含 bypass）。
    let (clean, after) = poll_clean(context);
    assert!(clean, "SIGTERM 后 utun 地址与校园路由必须被清理");
    assert_no_residual(context, &before, &connected, &after);

    // Idle 可重试：重新走完整连接 → Stop → 干净。
    assert_retry_succeeds_and_stops_clean("engine-sigterm-retry").await;
}

/// **场景 C（Engine SIGKILL）**：内核立即回收 Engine 的 utun fd（接口消失、接口
/// 路由被内核移除），Core 侧必须检测断连；**显式检查**不绑定 utun 的物理网关
/// bypass /32 是否残留（已知风险点，事实如实落盘）；被杀后必须仍可重试。
#[tokio::test(flavor = "current_thread")]
#[ignore = "需要已安装服务代理与已保存凭据的真实宿主；必须以 EXV_DARWIN_SERVICE_AGENT_E2E=1 显式启用"]
async fn engine_sigkill_facts_recorded_and_retry_succeeds() {
    let _guard = MATRIX_LOCK.lock().await;
    assert!(
        std::env::var_os(COMPANION_E2E_ENV).is_some(),
        "该真机验收必须显式以 EXV_DARWIN_SERVICE_AGENT_E2E=1 运行"
    );

    let context = "engine-sigkill";
    let before = route_dump();
    let (session, mut status_stream, _operation_id, _stop_digest) = connect_and_hold(context).await;
    let engine_pid = session.engine_pid();
    let connected = route_dump();
    let bypass_before: Vec<_> = connected
        .iter()
        .filter(|entry| entry.is_physical_bypass())
        .collect();
    println!(
        "[{context}] connected: engine_pid={engine_pid} routes={} bypass_entries={}",
        connected.len(),
        bypass_before.len()
    );

    // SIGKILL 无法被进程处理：立即死亡，内核回收全部 fd。
    send_signal(engine_pid, libc::SIGKILL);
    let elapsed = wait_process_gone(engine_pid, ENGINE_EXIT_BOUND, context);
    println!("[{context}] EVIDENCE: engine gone after SIGKILL in {elapsed:?}");

    // Core 侧：状态流必须在有限时间内给出断连，不允许悬挂。
    let disconnect = tokio::time::timeout(DISCONNECT_DETECT_BOUND, status_stream.message()).await;
    match disconnect {
        Ok(Ok(None)) => {
            println!("[{context}] EVIDENCE: status stream ended (None) = typed disconnect")
        }
        Ok(Err(status)) => {
            println!("[{context}] EVIDENCE: status stream returned transport error: {status}")
        }
        Ok(Ok(Some(event))) => {
            panic!("[{context}] status stream stayed alive after SIGKILL, got {event:?}")
        }
        Err(_) => panic!(
            "[{context}] status stream hung longer than {DISCONNECT_DETECT_BOUND:?} after SIGKILL"
        ),
    }

    // 网络 readback：接口路由由内核回收；bypass 残留作为事实显式记录。
    let (clean, after) = poll_clean(context);
    assert!(clean, "SIGKILL 后 utun 地址与校园路由必须被内核回收");
    let residual = residual_routes(&before, &connected, &after);
    let bypass_residual: Vec<_> = residual
        .iter()
        .filter(|entry| entry.is_physical_bypass())
        .collect();
    println!(
        "[{context}] EVIDENCE: residual routes after SIGKILL = {} (physical-bypass={})",
        residual.len(),
        bypass_residual.len()
    );
    for entry in &residual {
        println!(
            "[{context}] FACT: RESIDUAL route dst={}/{} gw={:?} gw_if={:?} if={}",
            entry.destination, entry.prefix, entry.gateway, entry.gateway_ifindex, entry.ifindex
        );
    }
    // SIGKILL 属于「崩溃/被杀」场景：接口路由的回收由内核完成，进程无法执行
    // 逆序 teardown，因此本场景不断言零残留；被杀后果必须由「可重试」验收
    // 覆盖——残留不得阻断下一次连接。

    // 被杀后可重试：重新走完整连接（不得被残留 bypass 阻断），Stop 后干净。
    assert_retry_succeeds_and_stops_clean("engine-sigkill-retry").await;
}
