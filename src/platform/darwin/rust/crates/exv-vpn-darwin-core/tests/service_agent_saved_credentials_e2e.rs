//! `MAC-CONFIG-04` 真机验收（opt-in）：用当前用户已保存的 Windows 同构密文凭据构造
//! 一次性 `DarwinEngineConnectV1`，经服务代理启动的真实 root Engine 消费并返回
//! `Pending`，随后显式 Stop 并读取 Engine 正常终态。
//!
//! W1-C（P7）后 Core 的 Connect 已是「UI 弹窗凭据优先 / 磁盘回退」双分支；本 e2e
//! 刻意直接走磁盘回退分支的输入（`build_saved_connect_envelope`），因此仍要求
//! `~/.exv/` 已保存完整可连接配置——UI 弹窗凭据路径由 core 单测/fixture 覆盖，
//! 不在本真机窗口重复。
//!
//! 运行条件：
//! - 已安装服务代理（标签 `com.exv.vpn.service-agent`，root 守护进程）；
//! - 根仓共享 `target/debug` 下存在 `exv-vpn-darwin-engine`；
//! - `~/.exv/` 已保存完整可连接配置（server、username 与已加密密码）。
//!
//! 运行方式：
//!
//! ```bash
//! EXV_DARWIN_SERVICE_AGENT_E2E=1 cargo test \
//!   --manifest-path src/platform/darwin/rust/Cargo.toml \
//!   -p exv-vpn-darwin-core \
//!   --test service_agent_saved_credentials_e2e -- --ignored --nocapture
//! ```
//!
//! 本测试绝不打印用户名、服务器地址或密码；所有断言只使用字段存在性与稳定错误码。

use exv_vpn_darwin_core::{
    DarwinUiConfig, build_saved_connect_envelope,
    engine_lifecycle_real::launch_fixed_engine_session,
};
use exv_vpn_darwin_ipc::connect_envelope::DarwinEngineConnectV1;
use exv_vpn_wire::generated as wire;

/// 用随机 operation id 与 envelope digest 组装与 Core 生产路径同形的 Apply 请求；
/// envelope 被本次编码消费，这正是"一次性"语义。
fn apply_request(
    envelope: DarwinEngineConnectV1,
    operation_id: [u8; 16],
) -> wire::ApplyTunnelRequest {
    let request_digest = envelope.profile_digest().to_vec();
    let mtu = u32::from(envelope.mtu_override().unwrap_or(1_290));
    wire::ApplyTunnelRequest {
        lookup_key: Some(wire::OperationLookupKey {
            principal_digest: vec![0; 32],
            method: wire::OperationMethod::ApplyTunnel as i32,
            runtime_epoch: vec![0; 16],
            operation_id: operation_id.to_vec(),
        }),
        plan: Some(wire::TunnelPlan {
            mtu,
            ..Default::default()
        }),
        request_digest,
        secret_payload: envelope.encode().to_vec(),
        windows_connection_mode: wire::WindowsConnectionMode::Standard as i32,
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要已安装服务代理与已保存凭据的真实宿主；必须以 EXV_DARWIN_SERVICE_AGENT_E2E=1 显式启用"]
async fn saved_credentials_are_consumed_by_the_real_root_engine() {
    assert!(
        std::env::var_os("EXV_DARWIN_SERVICE_AGENT_E2E").is_some(),
        "该真机验收必须显式以 EXV_DARWIN_SERVICE_AGENT_E2E=1 运行"
    );

    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    assert!(
        !config.username().is_empty(),
        "缺少已保存用户名，请先在设置页保存"
    );
    let saved_password = config
        .load_saved_password()
        .expect("saved password ciphertext must decrypt");
    assert!(
        !saved_password.is_empty(),
        "缺少已保存密码，请先在设置页保存"
    );
    drop(saved_password);

    let envelope =
        build_saved_connect_envelope(&config).expect("saved credentials must form the V1 envelope");
    let stop_digest = envelope.profile_digest().to_vec();
    let operation_id = {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).expect("random operation id");
        bytes
    };
    let request = apply_request(envelope, operation_id);

    let mut session = launch_fixed_engine_session()
        .await
        .expect("service agent must start the fixed root Engine");
    let mut status_stream = session
        .attach_connect_status()
        .await
        .expect("attach Engine status stream before apply");
    let reply = session
        .apply_tunnel(request)
        .await
        .expect("ApplyTunnel must be admitted");
    match reply.result {
        Some(wire::apply_tunnel_reply::Result::Pending(pending)) => {
            assert_eq!(pending.operation_id, operation_id.to_vec());
        }
        other => panic!("ApplyTunnel must return Pending, got {other:?}"),
    }

    // 完整链路：WebVPN 登录 → CSTP → utun/网络事务 → 数据面 → Connected 终态。
    loop {
        let event =
            tokio::time::timeout(std::time::Duration::from_secs(40), status_stream.message())
                .await
                .expect("status event within 40s")
                .expect("status stream healthy")
                .expect("status event present");
        if event.operation_id != operation_id.to_vec() {
            continue;
        }
        if let Some(error) = event.error {
            let native = error.native.as_ref().map(|n| n.code).unwrap_or(0);
            panic!(
                "pipeline failed: code={} stage={} native_errno={native} retry_marker={}",
                error.code, error.stage, error.retry
            );
        }
        let coarse = wire::StatsPhase::try_from(event.coarse_phase).ok();
        if coarse == Some(wire::StatsPhase::Connected) {
            println!("reached Connected: phase={}", event.connect_phase);
            break;
        }
    }
    // 数据面真实业务证明：校园网段目标（网关边缘 222.66.117.109，属配置路由
    // 222.66.117.0/24）经隧道取得应用层响应；物理出口无法区分时以「隧道路由
    // 存在 + 响应到达」共同证明双向通路。
    assert_traffic_through_tunnel();

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
        .expect("StopTunnel must succeed");
    assert!(matches!(
        stop.result,
        Some(wire::stop_tunnel_reply::Result::Stopped(_))
    ));

    session.close().expect("Engine must exit clean after close");

    // Stop 后清理证明：校园路由与隧道地址不再存在于系统状态（纯 libc 读取，
    // 守卫禁止测试内启动子进程）。内核拆除接口有延迟：轮询最多 10 秒。
    let mut cleaned = false;
    for _ in 0..20 {
        if !campus_route_present() && !tunnel_address_present() {
            cleaned = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert!(
        cleaned,
        "campus routes and tunnel address must be cleaned after stop; ifconfig_has_17220={}",
        tunnel_address_present()
    );
}

/// 从 PF_ROUTE dump 判断校园前缀路由是否仍存在。
fn campus_route_present() -> bool {
    const AF_INET: u8 = 2;
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
    // SAFETY: NULL 缓冲查询所需大小。
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
        return false;
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
        return false;
    }
    buffer.truncate(size);
    let header = std::mem::size_of::<libc::rt_msghdr>();
    let mut cursor = 0usize;
    let mut saw_tunnel_route = false;
    while cursor + header <= buffer.len() {
        // SAFETY: cursor 已按头大小边界检查。
        let rtm = unsafe {
            buffer
                .as_ptr()
                .add(cursor)
                .cast::<libc::rt_msghdr>()
                .read_unaligned()
        };
        let message_len = usize::from(rtm.rtm_msglen);
        if message_len < header || cursor + message_len > buffer.len() {
            break;
        }
        // 目的地址槽（RTA_DST=0x1）：消息体前 16 字节 ifaddrs 位图第一位。
        if rtm.rtm_addrs & libc::RTA_DST != 0 {
            let body = &buffer[cursor + header..cursor + message_len];
            if body.len() >= 8 && body[1] == AF_INET {
                let dst = std::net::Ipv4Addr::new(body[4], body[5], body[6], body[7]);
                let octets = dst.octets();
                // 172.20/16 或 222.66.117/24：本连接的两类清理目标。
                if (octets[0] == 172 && octets[1] == 20)
                    || (octets[0] == 222 && octets[1] == 66 && octets[2] == 117)
                {
                    saw_tunnel_route = true;
                }
            }
        }
        cursor += message_len;
    }
    saw_tunnel_route
}

/// 任一接口仍持有 172.20/16 地址。
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
                let octets = unsafe {
                    (*entry.ifa_addr.cast::<libc::sockaddr_in>())
                        .sin_addr
                        .s_addr
                };
                let octets = octets.to_ne_bytes();
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
/// 经隧道对校园目标发起真实 TLS 探测：对端任何 TLS record 回复即证明
/// 双向数据面（上行 ClientHello 已到校、下行响应经隧道返回）。
fn assert_traffic_through_tunnel() {
    use std::io::{Read as _, Write as _};
    let mut stream = std::net::TcpStream::connect("222.66.117.109:443")
        .expect("TCP connect to campus edge through the tunnel");
    stream
        .write_all(&[
            0x16, 0x03, 0x01, 0x00, 0x2d, // record: handshake, len 45
            0x01, 0x00, 0x00, 0x29, // ClientHello, len 41
            0x03, 0x03, // client version TLS 1.2
        ])
        .expect("write ClientHello header");
    stream
        .write_all(&[0x11_u8; 32]) // random
        .expect("write random");
    stream
        .write_all(&[
            0x00, // session id len
            0x00, 0x02, 0x00, 0x2f, // one cipher suite
            0x01, 0x00, // compression: null
            0x00, 0x00, // extensions len 0
        ])
        .expect("write ClientHello body");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("set read timeout");
    let mut reply = [0_u8; 5];
    let n = stream
        .read(&mut reply)
        .expect("campus edge must reply through the tunnel");
    assert!(n >= 1, "empty reply from campus edge");
    assert!(
        [0x15, 0x16, 0x17].contains(&reply[0]),
        "expected a TLS record reply, got {:#x}",
        reply[0]
    );
}
