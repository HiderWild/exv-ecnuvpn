//! `MAC-SCHOOL-MVP-11b` 真机验收（opt-in）：错凭据 → typed 认证失败 → 用户经
//! 产品链路更新密码 → 重试 → 真实 Connected → Stop → 资源清理 readback。
//!
//! 执行设计 §8.3 学校流第 8 项的完整产品闭环：本测试覆盖 Core/服务代理级主验收——
//! 错密码经产品链路（`ConfigSet` 同路径的 `apply_and_save`）写入，真实 root
//! Engine（服务代理启动）消费后必须返回 typed 认证失败（`Unauthorized` /
//! `ConnectingControl`，不是 panic、悬挂或 UNIMPLEMENTED）；随后同一 Engine
//! 会话内经产品链路恢复真实密码并重试，必须到达真实 `Connected`；显式
//! `StopTunnel` 后轮询系统状态确认校园路由与隧道地址清理干净。
//!
//! 运行条件：
//! - 已安装服务代理（标签 `com.exv.vpn.service-agent`，root 守护进程）；
//! - `~/.exv/` 已保存完整可连接配置（server、username 与已加密的真实密码）；
//! - 当前宿主可达真实学校网关。
//!
//! 运行方式：
//!
//! ```bash
//! EXV_DARWIN_REAL_SCHOOL_CRED_RETRY=1 cargo test \
//!   --manifest-path src/platform/darwin/rust/Cargo.toml \
//!   -p exv-vpn-darwin-core \
//!   --test credential_retry_real_e2e -- --ignored --nocapture
//! ```
//!
//! 凭据保护（P0）：错凭据环节使用显式占位假密码；真实密码仅在内存
//! [`Zeroizing`] 备份，任何退出路径（含断言失败展开）都经守卫走产品链路恢复。
//! 本测试绝不打印用户名、服务器地址或密码明文。

use std::sync::{Arc, Mutex};

use exv_vpn_darwin_core::{
    DarwinConfigItem, DarwinUiConfig, build_saved_connect_envelope,
    engine_lifecycle_real::launch_fixed_engine_session,
};
use exv_vpn_darwin_ipc::connect_envelope::DarwinEngineConnectV1;
use exv_vpn_wire::generated as wire;
use zeroize::Zeroizing;

/// 显式非真实的占位假密码：仅用于触发网关认证拒绝。
const WRONG_PASSWORD: &str = "exv-11b-definitely-not-a-real-password";

/// 单个状态事件的等待上限；网关认证拒绝应在秒级返回。
const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);

/// 一次连接状态等待循环的总预算，防止异常事件流导致悬挂。
const CONNECT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// 真实密码的内存备份；`Drop` 兜底恢复，保证任何退出路径都不残留假密码。
struct RealPasswordBackup {
    password: Mutex<Option<Zeroizing<String>>>,
}

impl RealPasswordBackup {
    fn take(&self) -> Option<Zeroizing<String>> {
        self.password
            .lock()
            .expect("password backup lock")
            .take()
    }
}

impl Drop for RealPasswordBackup {
    fn drop(&mut self) {
        if let Some(real) = self.password.lock().expect("password backup lock").take() {
            let restored = restore_password_via_product_path(&real);
            if restored.is_err() {
                eprintln!(
                    "[11b] FATAL: product-path password restore failed on drop; \
                     restore from the offline backup before connecting again"
                );
            }
        }
    }
}

/// 经产品链路（`apply_and_save`，即设置页 `ConfigSet` 的同一路径）写入密码并
/// 解密回读验证。密文写入与回读均不落任何明文日志。
fn restore_password_via_product_path(plaintext: &str) -> Result<(), exv_vpn_darwin_core::DarwinConfigError> {
    let mut config = DarwinUiConfig::load()?;
    config.apply_and_save(vec![DarwinConfigItem::new("password", plaintext)])?;
    let readback = config.load_saved_password()?;
    assert_eq!(readback, plaintext, "password restore readback must round-trip");
    Ok(())
}

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

fn random_operation_id() -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("random operation id");
    bytes
}

/// 等待指定 operation 的下一事件；超时或流断开直接失败（不允许悬挂）。
async fn next_event_for(
    status_stream: &mut tonic::Streaming<wire::ConnectStatusEvent>,
    operation_id: &[u8],
) -> wire::ConnectStatusEvent {
    let event = tokio::time::timeout(EVENT_TIMEOUT, status_stream.message())
        .await
        .expect("status event within timeout")
        .expect("status stream healthy")
        .expect("status event present");
    assert_eq!(
        event.operation_id,
        operation_id,
        "events for other operations must not be observed here"
    );
    event
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要已安装服务代理、真实学校网关与已保存真实凭据；必须以 \
           EXV_DARWIN_REAL_SCHOOL_CRED_RETRY=1（或 EXV_DARWIN_SERVICE_AGENT_E2E=1）显式启用"]
async fn wrong_password_fails_typed_then_update_and_retry_connects() {
    assert!(
        std::env::var_os("EXV_DARWIN_REAL_SCHOOL_CRED_RETRY").is_some()
            || std::env::var_os("EXV_DARWIN_SERVICE_AGENT_E2E").is_some(),
        "该真机验收必须以 EXV_DARWIN_REAL_SCHOOL_CRED_RETRY=1 显式启用"
    );

    // ---- 准备：读取已保存真实凭据，仅内存备份（Zeroizing），绝不打印。 ----
    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    assert!(
        !config.username().is_empty(),
        "缺少已保存用户名，请先在设置页保存"
    );
    let real_password = Zeroizing::new(
        config
            .load_saved_password()
            .expect("saved password ciphertext must decrypt"),
    );
    assert!(
        !real_password.is_empty(),
        "缺少已保存真实密码，请先在设置页保存"
    );
    let backup = Arc::new(RealPasswordBackup {
        password: Mutex::new(Some(Zeroizing::clone(&real_password))),
    });
    let _guard = Arc::clone(&backup);

    // ---- 阶段 A：错凭据经产品链路写入并加密落盘。 ----
    let mut config = config;
    config
        .apply_and_save(vec![DarwinConfigItem::new("password", WRONG_PASSWORD)])
        .expect("product-path write of the placeholder password must succeed");
    let wrong_readback = config.load_saved_password().expect("wrong password decrypts");
    assert_eq!(
        wrong_readback,
        WRONG_PASSWORD,
        "ConfigSet-equivalent write must round-trip the placeholder password"
    );
    drop(wrong_readback);
    println!("[11b] phase A: placeholder password written via product path");

    // ---- 阶段 B：错凭据连接 → typed 认证失败。 ----
    let wrong_envelope =
        build_saved_connect_envelope(&config).expect("wrong-credential envelope must form");
    let wrong_operation = random_operation_id();
    let mut session = launch_fixed_engine_session()
        .await
        .expect("service agent must start the fixed root Engine");
    println!("[11b] phase B: root Engine pid={} launched", session.engine_pid());
    let mut status_stream = session
        .attach_connect_status()
        .await
        .expect("attach Engine status stream before apply");
    let wrong_reply = session
        .apply_tunnel(apply_request(wrong_envelope, wrong_operation))
        .await
        .expect("ApplyTunnel with wrong credentials must be admitted");
    assert!(
        matches!(
            wrong_reply.result,
            Some(wire::apply_tunnel_reply::Result::Pending(_))
        ),
        "ApplyTunnel must return Pending, not an immediate error"
    );

    let deadline = tokio::time::Instant::now() + CONNECT_DEADLINE;
    let failure_event = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "wrong-credential failure must arrive within the deadline"
        );
        let event = next_event_for(&mut status_stream, &wrong_operation).await;
        if event.error.is_some() {
            break event;
        }
        assert_ne!(
            wire::StatsPhase::try_from(event.coarse_phase).ok(),
            Some(wire::StatsPhase::Connected),
            "wrong credentials must never reach Connected"
        );
    };
    let error = failure_event
        .error
        .expect("loop only breaks on events carrying an error");
    // typed 失败判定：认证拒绝必须是 Unauthorized / ConnectingControl，且不是
    // panic、悬挂或 gRPC UNIMPLEMENTED（事件流已健康返回，本身就是反证）。
    assert_eq!(
        error.code,
        wire::ErrorCode::Unauthorized as i32,
        "wrong password must surface the typed Unauthorized code"
    );
    assert_eq!(
        error.stage,
        wire::ErrorStage::ConnectingControl as i32,
        "authentication rejection must be attributed to the ConnectingControl stage"
    );
    assert_eq!(
        wire::StatsPhase::try_from(failure_event.coarse_phase).ok(),
        Some(wire::StatsPhase::Failed),
        "the failed operation must report the Failed coarse phase"
    );
    println!(
        "[11b] phase B: typed auth failure confirmed (code=Unauthorized stage=ConnectingControl)"
    );

    // ---- 阶段 C：用户经产品链路更新为真实密码（同一 Engine 会话内重试）。 ----
    let real = backup
        .take()
        .expect("real password backup must still be held before restore");
    restore_password_via_product_path(real.as_str())
        .expect("product-path restore of the real password must succeed");
    drop(real);
    println!("[11b] phase C: real password restored via product path");

    let config = DarwinUiConfig::load().expect("reload config after restore");
    let retry_envelope =
        build_saved_connect_envelope(&config).expect("restored-credential envelope must form");
    let stop_digest = retry_envelope.profile_digest().to_vec();
    let retry_operation = random_operation_id();
    let retry_reply = session
        .apply_tunnel(apply_request(retry_envelope, retry_operation))
        .await
        .expect("retry ApplyTunnel on the same Engine session must be admitted");
    assert!(
        matches!(
            retry_reply.result,
            Some(wire::apply_tunnel_reply::Result::Pending(_))
        ),
        "retry ApplyTunnel must return Pending (state machine must not be stuck after failure)"
    );

    // ---- 阶段 D：重试必须到达真实 Connected。 ----
    let deadline = tokio::time::Instant::now() + CONNECT_DEADLINE;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "retry must reach Connected within the deadline"
        );
        let event = next_event_for(&mut status_stream, &retry_operation).await;
        if let Some(error) = event.error {
            let native = error.native.as_ref().map_or(0, |n| n.code);
            panic!(
                "retry with restored credentials must connect, got failure: \
                 code={} stage={} native_errno={native} retry_marker={}",
                error.code, error.stage, error.retry
            );
        }
        if wire::StatsPhase::try_from(event.coarse_phase).ok() == Some(wire::StatsPhase::Connected)
        {
            println!(
                "[11b] phase D: retry reached Connected (connect_phase={})",
                event.connect_phase
            );
            break;
        }
    }

    // 数据面真实业务证明（复用 MAC-CONFIG-04 先例的校园边缘 TLS 探测）。
    assert_traffic_through_tunnel();
    println!("[11b] phase D: application-layer traffic verified through the tunnel");

    // ---- 阶段 E：Stop → 干净清理 readback。 ----
    let stop = session
        .stop_tunnel(wire::StopTunnelRequest {
            lookup_key: Some(wire::OperationLookupKey {
                principal_digest: vec![0; 32],
                method: wire::OperationMethod::StopTunnel as i32,
                runtime_epoch: vec![0; 16],
                operation_id: retry_operation.to_vec(),
            }),
            request_digest: stop_digest,
        })
        .await
        .expect("StopTunnel must succeed");
    assert!(
        matches!(
            stop.result,
            Some(wire::stop_tunnel_reply::Result::Stopped(_))
        ),
        "StopTunnel must return Stopped"
    );
    drop(status_stream);
    session
        .close()
        .expect("Engine must exit clean after close");

    // Stop 后清理证明：校园路由与隧道地址不再存在于系统状态（纯 libc 读取）。
    // 内核拆除接口有延迟：轮询最多 10 秒。
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
        "campus routes and tunnel address must be cleaned after stop; \
         ifconfig_has_17220={}",
        tunnel_address_present()
    );
    println!("[11b] phase E: stop acknowledged, routes and tunnel address cleaned");

    // ---- 终局：确认磁盘上的密码仍是真实密码（readback），备份被消费完毕。 ----
    let final_config = DarwinUiConfig::load().expect("final config read");
    let final_password = final_config.load_saved_password().expect("final decrypt");
    assert_eq!(
        final_password,
        real_password.as_str(),
        "the saved password at test end must be the real password"
    );
    drop(final_password);
    assert!(
        backup.take().is_none(),
        "the drop guard must hold nothing after an explicit restore"
    );
    println!("[11b] done: saved password verified as the real one; nothing left to restore");
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
                let octets =
                    unsafe { (*entry.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr };
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
