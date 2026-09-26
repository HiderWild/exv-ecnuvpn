//! `MAC-UI-STATS-10c` 真机 Core 级验收（opt-in）：真实 Core 进程（与 Tauri UI 同一
//! spawn 参数与认证协议）+ 真实服务代理 root Engine + `~/.exv` 已保存凭据，经产品
//! `Connect` 链路建立数据面后，验证统计随 `WatchEvents` 的 `snapshot.stats` 到达、
//! 随样本刷新，并在 `Stop` 后不再残留。
//!
//! 背景：本真机验证会话中 UI 级 a11y 驱动不可用（宿主无辅助功能授权，合成输入被
//! 系统丢弃），按验证计划降级为 Core 级证据：同一产品链路（UI→Core gRPC 契约）下
//! 读取 UI 渲染统计面板所消费的同一字段（`snapshot.stats`）。
//!
//! 运行条件：
//! - 已安装服务代理（标签 `com.exv.vpn.service-agent`，root 守护进程）；
//! - 共享 `target/debug` 下存在 `exv-vpn-darwin-core` 与 `exv-vpn-darwin-engine`；
//! - `~/.exv/` 已保存完整可连接配置。
//!
//! 运行方式：
//!
//! ```bash
//! EXV_DARWIN_STATS_10C_REAL=1 cargo test \
//!   --manifest-path src/platform/darwin/rust/Cargo.toml \
//!   -p exv-vpn-darwin-core \
//!   --test mac_ui_stats_10c_real_core_e2e -- --ignored --nocapture
//! ```
//!
//! 本测试绝不打印用户名、服务器地址或密码。

use std::path::PathBuf;
use std::time::Duration;

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
    RuntimeStats, SnapshotRequest, StopIntent, StopRequest, WatchEventsRequest,
    kernel_control_client::KernelControlClient, runtime_snapshot,
};

/// 事件等待上限：完整链路（登录 → CSTP → utun → 数据面）40 秒内应见分晓。
const EVENT_DEADLINE: Duration = Duration::from_secs(45);
/// 两次统计读数间隔：大于 Engine 默认 1 秒采样周期，且留出抖动余量。
const REFRESH_GAP: Duration = Duration::from_secs(3);

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要已安装服务代理与已保存凭据的真实宿主；必须以 EXV_DARWIN_STATS_10C_REAL=1 显式启用"]
async fn stats_arrive_with_connected_snapshots_and_refresh_until_stop() {
    assert!(
        std::env::var_os("EXV_DARWIN_STATS_10C_REAL").is_some(),
        "该真机验收必须显式以 EXV_DARWIN_STATS_10C_REAL=1 运行"
    );

    // ── 已保存凭据（只校验存在性，绝不打印）───────────────────────────────
    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    assert!(!config.username().is_empty(), "缺少已保存用户名");
    let envelope =
        build_saved_connect_envelope(&config).expect("saved credentials must form the V1 envelope");
    let connect_request = ConnectRequest {
        intent: Some(ConnectIntent {
            lookup_key: Some(OperationLookupKey {
                // 与 Windows 同形：Darwin 引擎链路接受零 principal（见 service agent e2e 先例）。
                principal_digest: vec![0; 32],
                method: OperationMethod::Connect as i32,
                runtime_epoch: vec![0; 16],
                operation_id: vec![7; 16],
            }),
            request_digest: envelope.profile_digest().to_vec(),
            profile: None,
        }),
        secret_payload: Vec::new(),
    };
    drop(envelope);

    // ── 与 Tauri 同形启动真实 Core 进程 ─────────────────────────────────
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create runtime dir");
    let socket = runtime.engine_socket_path().expect("derive socket path");
    // SAFETY: `geteuid` 只读当前普通用户凭证。
    let ui_uid = unsafe { libc::geteuid() };
    let core_binary = resolve_core_binary();
    let mut child = std::process::Command::new(&core_binary)
        .arg("--ui-pid")
        .arg(std::process::id().to_string())
        .arg("--ui-uid")
        .arg(ui_uid.to_string())
        .arg("--ui-socket")
        .arg(socket.as_path())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the real dev Core process");
    let core_pid = child.id();
    println!("[10c] core pid={core_pid} binary={}", core_binary.display());

    let (bootstrap, client_key) =
        UiCoreBootstrapV1::random_pair().expect("bootstrap key pair");
    use std::io::Write as _;
    child
        .stdin
        .as_mut()
        .expect("core stdin piped")
        .write_all(bootstrap.encode().as_bytes())
        .expect("write bootstrap record");
    drop(child.stdin.take());

    // ── 等 socket 出现并建立认证 channel ────────────────────────────────
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

    // ── WatchEvents：首个 Idle 快照 ─────────────────────────────────────
    let mut watch = client
        .watch_events(WatchEventsRequest { resume_tick: 0 })
        .await
        .expect("watch events")
        .into_inner();
    let initial = next_event(&mut watch).await;
    assert!(
        matches!(
            initial.snapshot.as_ref().and_then(|s| s.state.as_ref()),
            Some(runtime_snapshot::State::Idle(_))
        ),
        "initial snapshot must be Idle"
    );
    assert!(
        initial.snapshot.as_ref().and_then(|s| s.stats).is_none(),
        "Idle snapshot carries no stats"
    );
    println!("[10c] initial Idle snapshot tick={}", initial.monotonic_tick);

    // ── 产品 Connect 链路（Core 内部：service agent → root Engine → Apply）───
    let reply = client
        .connect(connect_request)
        .await
        .expect("Connect must be accepted");
    drop(reply);

    // ── 等待首个带 stats 的 Connected 快照（UI 统计面板的数据源）────────
    let first = time::timeout(EVENT_DEADLINE, wait_connected_with_stats(&mut watch))
        .await
        .expect("Connected with stats within deadline");
    let first_stats = first
        .snapshot
        .as_ref()
        .and_then(|s| s.stats)
        .expect("connected snapshot carries stats");
    println!(
        "[10c] sample#1 tick={} engine_seq={} rx={} B tx={} B rx_rate={} B/s tx_rate={} B/s latency_ms={} phase={:?}",
        first_stats.sample_tick,
        first_stats.engine_sequence,
        first_stats.rx_bytes,
        first_stats.tx_bytes,
        first_stats.rx_rate_bps,
        first_stats.tx_rate_bps,
        first_stats.latency_ms,
        first_stats.phase(),
    );
    assert_eq!(
        first_stats.latency_ms, 0,
        "首个 Connected 样本到达时 180s 探测周期尚未到期，latency_ms 必须=0（0=未知）"
    );

    // ── 刷新验证：经隧道发起真实流量，下一个样本必须反映累计字节增长 ─────
    let probe = std::thread::spawn(probe_traffic_through_tunnel);
    std::thread::sleep(REFRESH_GAP);
    let second = time::timeout(EVENT_DEADLINE, wait_connected_with_stats(&mut watch))
        .await
        .expect("refreshed stats within deadline");
    let second_stats = second
        .snapshot
        .as_ref()
        .and_then(|s| s.stats)
        .expect("refreshed snapshot carries stats");
    probe.join().expect("tunnel traffic probe must succeed");
    println!(
        "[10c] sample#2 tick={} engine_seq={} rx={} B tx={} B rx_rate={} B/s tx_rate={} B/s latency_ms={} phase={:?}",
        second_stats.sample_tick,
        second_stats.engine_sequence,
        second_stats.rx_bytes,
        second_stats.tx_bytes,
        second_stats.rx_rate_bps,
        second_stats.tx_rate_bps,
        second_stats.latency_ms,
        second_stats.phase(),
    );
    assert!(
        second_stats.sample_tick > 0,
        "已发布样本必须铸造事件总线 tick"
    );
    assert_eq!(
        second_stats.phase(),
        exv_vpn_wire::generated::StatsPhase::Connected,
        "连接稳定期样本的 phase 必须为 Connected（前端 guard 的接受条件）"
    );
    assert!(
        second_stats.sample_tick > first_stats.sample_tick,
        "样本 tick 必须随刷新推进（{} → {}）",
        first_stats.sample_tick,
        second_stats.sample_tick,
    );
    assert!(
        second_stats.rx_bytes > first_stats.rx_bytes || second_stats.tx_bytes > first_stats.tx_bytes,
        "真实隧道流量后累计字节必须增长（rx: {} → {}，tx: {} → {}）",
        first_stats.rx_bytes,
        second_stats.rx_bytes,
        first_stats.tx_bytes,
        second_stats.tx_bytes,
    );

    // ── 连接中 GetSnapshot 行为（如实记录，不做断言）────────────────────
    let dial_snapshot = client
        .get_snapshot(SnapshotRequest {
            runtime_epoch: Vec::new(),
        })
        .await
        .expect("GetSnapshot must answer on the authenticated channel")
        .into_inner();
    let dial_state = dial_snapshot
        .state
        .as_ref()
        .map_or_else(|| "none".to_owned(), |state| format!("{state:?}"));
    println!(
        "[10c] GetSnapshot(connected) returns stats={} state={dial_state}（现状记录，非断言）",
        dial_snapshot.stats.is_some(),
    );

    // ── Stop 后统计消失：Idle 快照不再携带 stats ────────────────────────
    client
        .stop(StopRequest {
            intent: Some(StopIntent {
                lookup_key: Some(OperationLookupKey {
                    principal_digest: vec![0; 32],
                    method: OperationMethod::StopTunnel as i32,
                    runtime_epoch: vec![0; 16],
                    operation_id: vec![9; 16],
                }),
                request_digest: vec![0; 32],
            }),
        })
        .await
        .expect("Stop must be accepted");
    let after = time::timeout(EVENT_DEADLINE, wait_idle_snapshot(&mut watch))
        .await
        .expect("Idle snapshot after stop within deadline");
    assert!(
        after.snapshot.as_ref().and_then(|s| s.stats).is_none(),
        "Stop 后快照不得残留统计"
    );
    println!(
        "[10c] after stop: Idle snapshot tick={}, stats gone",
        after.monotonic_tick
    );

    // ── 清理 Core 进程与 runtime ────────────────────────────────────────
    drop(watch);
    drop(client);
    child
        .kill()
        .expect("core child must terminate on request");
    let _status = child.wait().expect("reap core child");
    runtime
        .cleanup_empty()
        .expect("runtime dir must clean after core exit");
    println!("[10c] core exited, runtime cleaned");
}

/// 等待带 stats 的 Connected 快照（UI 统计面板的到达路径：随事件流推送的
/// 快照载荷，无论事件 kind 是 Snapshot 还是 Transition）。
async fn wait_connected_with_stats(watch: &mut tonic::Streaming<RuntimeEvent>) -> RuntimeEvent {
    loop {
        let event = next_event(watch).await;
        let connected = event.snapshot.as_ref().is_some_and(|snapshot| {
            matches!(
                snapshot.state.as_ref(),
                Some(runtime_snapshot::State::Connected(_))
            ) && snapshot.stats.is_some()
        });
        if connected {
            return event;
        }
    }
}

/// 等待 Stop 后的 Idle 快照（同样不筛选事件 kind）。
async fn wait_idle_snapshot(watch: &mut tonic::Streaming<RuntimeEvent>) -> RuntimeEvent {
    loop {
        let event = next_event(watch).await;
        let idle = event.snapshot.as_ref().is_some_and(|snapshot| {
            matches!(snapshot.state.as_ref(), Some(runtime_snapshot::State::Idle(_)))
        });
        if idle {
            return event;
        }
    }
}

/// 顺序读事件直到拿到快照载荷事件；每个事件都留痕。
async fn next_event(watch: &mut tonic::Streaming<RuntimeEvent>) -> RuntimeEvent {
    loop {
        let event = watch
            .message()
            .await
            .expect("watch stream healthy")
            .expect("event present");
        let state_text = event
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.state.as_ref())
            .map_or_else(
                || "none".to_owned(),
                |state| match state {
                    runtime_snapshot::State::Idle(_) => "Idle".to_owned(),
                    runtime_snapshot::State::Connecting(connecting) => {
                        format!("Connecting(phase={})", connecting.phase)
                    }
                    runtime_snapshot::State::AwaitingInteraction(_) => {
                        "AwaitingInteraction".to_owned()
                    }
                    runtime_snapshot::State::Connected(_) => "Connected".to_owned(),
                    runtime_snapshot::State::Stopping(_) => "Stopping".to_owned(),
                    runtime_snapshot::State::Reconciling(_) => "Reconciling".to_owned(),
                    runtime_snapshot::State::FailedClean(_) => "FailedClean".to_owned(),
                    runtime_snapshot::State::FailedDirty(_) => "FailedDirty".to_owned(),
                },
            );
        println!(
            "[10c][event] kind={} tick={} state={state_text} stats={}",
            event.kind,
            event.monotonic_tick,
            event
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.stats)
                .is_some(),
        );
        if let Some(snapshot) = event.snapshot.as_ref() {
            if let Some(runtime_snapshot::State::FailedDirty(failed)) = snapshot.state.as_ref() {
                panic!("pipeline failed (dirty): {failed:?}");
            }
            if let Some(runtime_snapshot::State::FailedClean(failed)) = snapshot.state.as_ref() {
                panic!("pipeline failed (clean): {failed:?}");
            }
        }
        if event.snapshot.is_some() {
            return event;
        }
    }
}

/// 经隧道对校园边缘（222.66.117.109:443，配置路由 222.66.117.0/24）发起真实
/// TLS 探测：产生可计数的上下行流量，供统计刷新断言使用。
fn probe_traffic_through_tunnel() {
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

/// 解析唯一开发期 Core binary：优先跟随测试可执行文件实际所在的 `target/debug`
/// （与本车道 cargo target-dir 解析一致），可用环境变量覆盖。
fn resolve_core_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("EXV_STATS_10C_CORE_BIN") {
        return PathBuf::from(path);
    }
    let executable = std::env::current_exe().expect("test executable path");
    let debug_dir = executable
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target/debug directory");
    debug_dir.join("exv-vpn-darwin-core")
}

/// 让未使用的 `RuntimeStats` 导入在断言文本外保持引用（防 unused 警告）。
#[allow(dead_code)]
fn stats_shape_witness(stats: &RuntimeStats) -> u64 {
    stats.sample_tick
}
