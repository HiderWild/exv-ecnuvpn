//! `MAC-LIFECYCLE-15` S1 真机生命周期恢复验收（opt-in）：经**真实 Core 进程**的
//! 生产 `LiveConnectionProjection` 链路验证「死会话后下一次 Connect 走完整新拉起」：
//!
//! 1. 与 Tauri UI 同形启动真实 Core（普通用户），经认证 UDS 挂 `WatchEvents`；
//! 2. 产品 `Connect` 链路（Core → 服务代理 → root Engine → Apply）到 Connected；
//!    Engine pid 取自 Core 生产路径自己入库的 `ENGINE_SESSION_STARTED` 日志事实
//!    （Core 拉起接缝每次执行都记录真实 pid），并以 `kill(pid, 0)` 验证存活；
//! 3. 对 root Engine 发真实 SIGKILL（内核回收 utun，进程无法逆序 teardown）；
//! 4. 硬时间界内断言 UI 事件流收到 typed 失联终态 `FailedClean`（`EffectUnknown` +
//!    `ProtocolSession` + `certainty=Unknown` + `retry=UseNewOperation`——Core 不伪造
//!    Engine 错误码，恢复路径=用新 operation 重试），且 Engine pid 已消失；
//! 5. SIGKILL 后做 S2 残留盘点（`/private/tmp` 下 exv runtime 目录计数与归属，
//!    事实落盘不清理）；
//! 6. 第二次 `Connect`（新 operation）必须产生**新的** `ENGINE_SESSION_STARTED`
//!    记录（Core 拉起接缝再次执行）且 pid 与被杀 Engine 不同，到 Connected；
//! 7. 显式 `Stop` 回 Idle，隧道地址 readback 归零。
//!
//! 路由级三方快照（连接前/连接期/终止后零残留）由 `kill_matrix_real.rs` 既有场景
//! 覆盖（同一 teardown 路径与判据）；本文件聚焦 Core 投影的生命周期语义，不重复
//! `PF_ROUTE` dump 解析。
//!
//! 运行条件与 `kill_matrix_real`/`mac_ui_stats_10c_real_core_e2e` 相同：已安装
//! 服务代理、共享 `target/debug` 下存在 `exv-vpn-darwin-core` 与
//! `exv-vpn-darwin-engine`、`~/.exv/` 已保存完整凭据。
//!
//! 运行方式：
//!
//! ```bash
//! EXV_DARWIN_LIFECYCLE_REAL=1 cargo test \
//!   --manifest-path src/platform/darwin/rust/Cargo.toml \
//!   -p exv-vpn-darwin-core \
//!   --test lifecycle_recovery_real -- --nocapture --test-threads=1
//! ```
//!
//! 未设环境变量时本测试直接通过并打印 skip 说明（默认 `cargo test` 不需要 root）。
//! 本测试绝不打印用户名、服务器地址或密码；信号与 readback 全部使用纯 libc。

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::net::UnixStream;

use exv_vpn_darwin_core::{DarwinUiConfig, build_saved_connect_envelope};
use exv_vpn_darwin_ipc::{
    authenticate_client_to_tonic_channel,
    path::{RuntimeDir, RuntimeOwner},
    peer::{ExpectedPeer, SystemPeerLookup},
    ui_core_bootstrap::UiCoreBootstrapV1,
};
use exv_vpn_wire::generated::{
    ConnectIntent, ConnectRequest, EffectCertainty, ErrorCode, ErrorStage, LogsListRequest,
    OperationLookupKey, OperationMethod, RetryAdvice, RuntimeEvent, StopIntent, StopRequest,
    WatchEventsRequest, kernel_control_client::KernelControlClient, runtime_snapshot,
};

/// opt-in 总闸（独立于 `kill_matrix_real` 的 `EXV_DARWIN_SERVICE_AGENT_E2E`，避免误触发
/// 其三场景矩阵）。
const LIFECYCLE_REAL_ENV: &str = "EXV_DARWIN_LIFECYCLE_REAL";
/// 完整连接链路（登录 → CSTP → utun → 数据面 → Connected）的等待上限。
const CONNECT_DEADLINE: Duration = Duration::from_mins(1);
/// SIGKILL 后 typed 失联终态必须到达的硬时间界（状态流 EOF 即投影发布）。
const TERMINAL_BOUND: Duration = Duration::from_secs(20);
/// Engine 进程消失 / Core 日志事实到达的轮询上界。
const POLL_BOUND: Duration = Duration::from_secs(15);
/// Stop 后隧道地址从接口表消失的轮询上界。
const CLEAN_POLL_BOUND: Duration = Duration::from_secs(15);
/// Core 拉起接缝入库的日志事实码（携带真实 Engine pid）。
const ENGINE_LAUNCH_LOG_CODE: &str = "ENGINE_SESSION_STARTED";

/// 两个真实连接场景互斥：避免并行测试同时操作系统网络状态。
static LIFECYCLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 经真实 Core 生产投影链路验证：Engine SIGKILL → typed 失联终态（硬时界）→
/// 死 session 后下一次 Connect 拉起新 pid Engine 到 Connected → Stop 回 Idle 干净。
#[allow(
    clippy::too_many_lines,
    reason = "真机场景按时间顺序线性展开（启动→连接→杀→终态→重连→Stop），拆分会切断证据链"
)]
#[tokio::test(flavor = "current_thread")]
async fn engine_sigkill_yields_typed_terminal_then_reconnect_launches_new_pid() {
    if std::env::var_os(LIFECYCLE_REAL_ENV).is_none() {
        println!(
            "[lifecycle] skip：未设 {LIFECYCLE_REAL_ENV}=1（需要已安装服务代理与已保存凭据的\
             真实宿主）；本测试 opt-in，默认不运行"
        );
        return;
    }
    let _guard = LIFECYCLE_LOCK.lock().await;

    // ── 已保存凭据（只校验存在性，绝不打印）──────────────────────────────
    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    assert!(!config.username().is_empty(), "缺少已保存用户名");
    let envelope =
        build_saved_connect_envelope(&config).expect("saved credentials must form the V1 envelope");
    drop(envelope);

    // ── 与 Tauri 同形启动真实 Core 进程（生产 LiveConnectionProjection 载体）──
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
    println!("[lifecycle] core pid={core_pid} binary={}", core_binary.display());

    let (bootstrap, client_key) = UiCoreBootstrapV1::random_pair().expect("bootstrap key pair");
    child
        .stdin
        .as_mut()
        .expect("core stdin piped")
        .write_all(bootstrap.encode().as_bytes())
        .expect("write bootstrap record");
    drop(child.stdin.take());

    // ── 等 socket 出现并建立认证 channel ────────────────────────────────
    let socket_ready = {
        let started = Instant::now();
        loop {
            if socket.as_path().exists() {
                break true;
            }
            if started.elapsed() > Duration::from_secs(10) {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
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
    let initial = next_snapshot(&mut watch).await;
    assert!(
        matches!(
            initial.snapshot.as_ref().and_then(|s| s.state.as_ref()),
            Some(runtime_snapshot::State::Idle(_))
        ),
        "initial snapshot must be Idle"
    );

    // ── 第一次 Connect：到 Connected，Engine pid 取自 Core 生产日志事实 ──
    let baseline_cursor = logs_cursor(&mut client).await;
    connect(&mut client, vec![0x11; 16]).await;
    let connected_started = Instant::now();
    wait_state(&mut watch, StateFilter::Connected, CONNECT_DEADLINE).await;
    let first_engine_pid = wait_new_engine_launch(&mut client, baseline_cursor, POLL_BOUND).await;
    assert!(
        process_alive(first_engine_pid),
        "connect#1 后 Engine pid {first_engine_pid} 必须存活"
    );
    println!(
        "[lifecycle] connect#1 reached Connected in {:?}, engine_pid={first_engine_pid}",
        connected_started.elapsed()
    );

    // ── SIGKILL root Engine（内核回收 utun；Engine real uid 与本进程一致）──
    // SAFETY: kill(2) 只读取参数。
    let signal_result = unsafe { libc::kill(first_engine_pid as libc::pid_t, libc::SIGKILL) };
    assert_eq!(signal_result, 0, "SIGKILL must be delivered to the engine");
    let kill_started = Instant::now();

    // ── typed 失联终态：硬时间界内收到 FailedClean，错误语义不伪造 ────────
    let terminal = wait_state(&mut watch, StateFilter::FailedClean, TERMINAL_BOUND).await;
    let terminal_elapsed = kill_started.elapsed();
    println!(
        "[lifecycle] EVIDENCE: typed FailedClean terminal {terminal_elapsed:?} after SIGKILL \
         (bound {TERMINAL_BOUND:?})"
    );
    assert!(
        terminal_elapsed <= TERMINAL_BOUND,
        "SIGKILL 后 typed 失联终态必须在硬时间界内到达，实测 {terminal_elapsed:?}"
    );
    let failed = match terminal.snapshot.as_ref().and_then(|s| s.state.as_ref()) {
        Some(runtime_snapshot::State::FailedClean(failed)) => failed,
        other => panic!("wait_state 保证 FailedClean，实测 {other:?}"),
    };
    let error = failed
        .last_error
        .as_ref()
        .expect("engine-lost terminal must carry last_error");
    assert_eq!(error.code, ErrorCode::EffectUnknown as i32, "code=EffectUnknown");
    assert_eq!(
        error.stage,
        ErrorStage::ProtocolSession as i32,
        "stage=ProtocolSession"
    );
    assert_eq!(
        error.certainty,
        EffectCertainty::Unknown as i32,
        "certainty=Unknown"
    );
    assert_eq!(
        error.retry,
        RetryAdvice::UseNewOperation as i32,
        "retry=UseNewOperation（用新 operation 重试，不自动重连）"
    );

    // ── Engine 进程已消失（kill(pid,0) → ESRCH；有界轮询）────────────────
    let gone = wait_process_gone(first_engine_pid, POLL_BOUND);
    println!("[lifecycle] EVIDENCE: engine pid {first_engine_pid} gone in {gone:?}");

    // ── S2 残留盘点：SIGKILL 后 /private/tmp 下 exv runtime 目录事实落盘 ──
    survey_runtime_dirs("after-sigkill");

    // ── 第二次 Connect（新 operation）：必须走完整新拉起（新 pid）───────
    let reconnect_cursor = logs_cursor(&mut client).await;
    let reconnect_started = Instant::now();
    connect(&mut client, vec![0x22; 16]).await;
    wait_state(&mut watch, StateFilter::Connected, CONNECT_DEADLINE).await;
    let second_engine_pid =
        wait_new_engine_launch(&mut client, reconnect_cursor, POLL_BOUND).await;
    assert_ne!(
        second_engine_pid, first_engine_pid,
        "死 session 后下一次 Connect 必须拉起新 pid 的 Engine（完整新拉起，非复用）"
    );
    assert!(
        process_alive(second_engine_pid),
        "connect#2 后新 Engine pid {second_engine_pid} 必须存活"
    );
    println!(
        "[lifecycle] EVIDENCE: reconnect launched new engine_pid={second_engine_pid} \
         (old={first_engine_pid}) in {:?}",
        reconnect_started.elapsed()
    );

    // ── 显式 Stop：回 Idle，隧道地址 readback 归零 ───────────────────────
    client
        .stop(StopRequest {
            intent: Some(StopIntent {
                lookup_key: Some(OperationLookupKey {
                    principal_digest: vec![0; 32],
                    method: OperationMethod::StopTunnel as i32,
                    runtime_epoch: vec![0; 16],
                    operation_id: vec![0x22; 16],
                }),
                request_digest: vec![0; 32],
            }),
        })
        .await
        .expect("Stop must be accepted");
    wait_state(&mut watch, StateFilter::Idle, CONNECT_DEADLINE).await;
    let clean_started = Instant::now();
    loop {
        if !tunnel_address_present() {
            println!(
                "[lifecycle] clean readback after {:?}: no 172.20/16 tunnel address",
                clean_started.elapsed()
            );
            break;
        }
        assert!(
            clean_started.elapsed() <= CLEAN_POLL_BOUND,
            "Stop 后隧道地址必须在轮询上界内消失"
        );
        std::thread::sleep(Duration::from_millis(300));
    }

    // ── 清理 Core 进程与 runtime ────────────────────────────────────────
    drop(watch);
    drop(client);
    child.kill().expect("core child must terminate on request");
    let _status = child.wait().expect("reap core child");
    let _ = runtime.cleanup_empty();
    println!("[lifecycle] core exited, runtime cleanup attempted");
}

/// 发起一次产品 Connect（操作身份由 `operation_id` 区分；凭据由 Core 从已保存
/// 配置构造，测试不接触 secret）。
async fn connect(
    client: &mut KernelControlClient<tonic::transport::Channel>,
    operation_id: Vec<u8>,
) {
    let config = DarwinUiConfig::load().expect("read the current user's saved settings");
    let digest = build_saved_connect_envelope(&config)
        .expect("saved credentials must form the V1 envelope")
        .profile_digest()
        .to_vec();
    let request = ConnectRequest {
        intent: Some(ConnectIntent {
            lookup_key: Some(OperationLookupKey {
                // 与 Windows 同形：Darwin 引擎链路接受零 principal（见 service agent e2e 先例）。
                principal_digest: vec![0; 32],
                method: OperationMethod::Connect as i32,
                runtime_epoch: vec![0; 16],
                operation_id,
            }),
            request_digest: digest,
            profile: None,
        }),
        secret_payload: Vec::new(),
    };
    let reply = client
        .connect(request)
        .await
        .expect("Connect must be accepted");
    drop(reply);
}

/// 顺序消费事件流直到快照状态命中过滤器；超时 panic（硬时间界防卡死）。
async fn wait_state(
    watch: &mut tonic::Streaming<RuntimeEvent>,
    filter: StateFilter,
    bound: Duration,
) -> RuntimeEvent {
    let started = Instant::now();
    loop {
        let event = tokio::time::timeout(bound, next_snapshot(watch))
            .await
            .unwrap_or_else(|_| {
                panic!("等待 {filter:?} 超过硬时间界 {bound:?}（已等 {:?}）", started.elapsed())
            });
        let hit = event
            .snapshot
            .as_ref()
            .and_then(|s| s.state.as_ref())
            .is_some_and(|state| match filter {
                StateFilter::Connected => {
                    matches!(state, runtime_snapshot::State::Connected(_))
                }
                StateFilter::FailedClean => {
                    matches!(state, runtime_snapshot::State::FailedClean(_))
                }
                StateFilter::Idle => matches!(state, runtime_snapshot::State::Idle(_)),
            });
        if hit {
            return event;
        }
    }
}

/// 读下一条携带快照载荷的事件；每条留痕（kind/tick/state）。
async fn next_snapshot(watch: &mut tonic::Streaming<RuntimeEvent>) -> RuntimeEvent {
    loop {
        let event = watch
            .message()
            .await
            .expect("watch stream healthy")
            .expect("event present");
        let Some(snapshot) = event.snapshot.as_ref() else {
            println!(
                "[lifecycle][event] kind={} tick={} (no snapshot)",
                event.kind, event.monotonic_tick
            );
            continue;
        };
        let state_text = snapshot
            .state
            .as_ref()
            .map_or_else(|| "none".to_owned(), |state| format!("{state:?}"));
        println!(
            "[lifecycle][event] kind={} tick={} state={state_text} stats={}",
            event.kind,
            event.monotonic_tick,
            snapshot.stats.is_some(),
        );
        return event;
    }
}

/// 快照状态过滤器。
#[derive(Debug, Clone, Copy)]
enum StateFilter {
    Connected,
    FailedClean,
    Idle,
}

/// 读取 Core 聚合日志的当前游标（尾部拉取后返回「已消费到的 seq」）。
///
/// 契约已钉死 last-seen（W1-B/P9）：增量过滤为严格 `seq > after_seq`，续拉必须
/// 传最后已见条目的 seq（= `next_seq - 1`）；回传 `next_seq` 会恰漏 seq 恰等的
/// 那一条。proto `LogsListReply` 注释已同步改为 last-seen 口径并点名该陷阱
/// （见 `proto/exv/v1/common.proto`），两侧前端桥也已按此翻译——此处
/// `next_seq - 1` 不再是对错误文档的规避，而是契约本体。
async fn logs_cursor(client: &mut KernelControlClient<tonic::transport::Channel>) -> u64 {
    let reply = client
        .logs_list(LogsListRequest {
            after_seq: 0,
            limit: 1,
            filter: String::new(),
        })
        .await
        .expect("logs list must answer")
        .into_inner();
    reply.next_seq.saturating_sub(1)
}

/// 增量拉取游标之后追加的 Engine 拉起记录（`ENGINE_SESSION_STARTED`，Core 拉起
/// 接缝每次执行都入库一条，携带真实 pid），返回最新一条的 pid（无则 `None`）。
async fn new_engine_launch_since(
    client: &mut KernelControlClient<tonic::transport::Channel>,
    cursor: u64,
) -> Option<u32> {
    let reply = client
        .logs_list(LogsListRequest {
            after_seq: cursor,
            limit: 500,
            filter: String::new(),
        })
        .await
        .expect("logs list must answer")
        .into_inner();
    reply
        .entries
        .iter()
        .filter(|entry| entry.code == ENGINE_LAUNCH_LOG_CODE)
        .filter_map(|entry| entry.fields.get("pid").and_then(|value| value.parse().ok()))
        .next_back()
}

/// 有界轮询直到游标之后出现新的拉起记录，返回其 pid。
async fn wait_new_engine_launch(
    client: &mut KernelControlClient<tonic::transport::Channel>,
    cursor: u64,
    bound: Duration,
) -> u32 {
    let started = Instant::now();
    loop {
        if let Some(pid) = new_engine_launch_since(client, cursor).await {
            println!("[lifecycle] engine launch logged after cursor={cursor}: pid={pid}");
            return pid;
        }
        assert!(
            started.elapsed() <= bound,
            "新的 {ENGINE_LAUNCH_LOG_CODE} 日志事实未在 {bound:?} 内到达"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// `kill(pid, 0)` 存活探测（纯 libc；Engine real uid 与本进程一致，权限同
/// `kill_matrix_real` 的信号先例）。
fn process_alive(pid: u32) -> bool {
    // SAFETY: kill(2) 只读取参数；signal 0 只做存在性/权限检查，不投递信号。
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0
}

/// 有界等待进程退出（kqueue `EVFILT_PROC` + `NOTE_EXIT`）；返回实测耗时。
///
/// Engine 的父进程是服务代理（不及时 reap），已退出的 Engine 会停留为 `<defunct>`；
/// `kill(pid, 0)` 对僵尸仍返回成功，无法区分真活。kqueue 的进程退出事件对同
/// real uid 目标可监视（Engine real uid 与本进程一致），在目标进入僵尸态时即触发，
/// 是纯 libc 的真实退出判据（与 `kill_matrix_real.rs` 同判据）。
fn wait_process_gone(pid: u32, bound: Duration) -> Duration {
    let started = Instant::now();
    // 目标已被 reap（pid 不复存在）时直接判定退出，避免对死 pid 注册事件。
    // SAFETY: kill(2) 只读取参数；signal 0 只做存在性/权限检查。
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return started.elapsed();
    }
    // SAFETY: kqueue() 返回新描述符，无副作用。
    let queue = unsafe { libc::kqueue() };
    assert!(queue >= 0, "kqueue() must succeed");
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
        "kevent(EVFILT_PROC/NOTE_EXIT) failed: {}",
        std::io::Error::last_os_error()
    );
    assert!(
        count == 1,
        "pid {pid} still alive after {bound:?} (no NOTE_EXIT)"
    );
    started.elapsed()
}

/// 任一接口仍持有 172.20/16 隧道地址（`getifaddrs`；纯 libc readback）。
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
                let raw = unsafe { (*entry.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr };
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

/// S2 残留盘点：`/private/tmp` 下 exv runtime 目录（`exv-vpn-*`）的计数、归属与
/// socket 存在性，事实落盘（只记录不清理；root-owned 孤儿目录普通用户不可删，
/// 若出现真实重复故障属 MAC-REPAIR-18 候选事实）。
fn survey_runtime_dirs(context: &str) {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(entries) = std::fs::read_dir("/private/tmp") else {
        println!("[lifecycle][{context}] survey: /private/tmp unreadable");
        return;
    };
    let mut count = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("exv-vpn-") {
            continue;
        }
        count += 1;
        let uid = entry.metadata().map_or(0, |meta| meta.uid());
        let has_socket = entry.path().join("engine.sock").exists();
        println!(
            "[lifecycle][{context}] FACT: runtime dir {name} uid={uid} engine_sock={has_socket}"
        );
    }
    println!(
        "[lifecycle][{context}] survey: {count} exv-vpn-* runtime dirs under /private/tmp \
         （成功重连即证明残留不阻断新会话）"
    );
}

/// 解析唯一开发期 Core binary：优先跟随测试可执行文件实际所在的 `target/debug`
/// （与 10c 真机验收一致），可用环境变量覆盖。
fn resolve_core_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("EXV_LIFECYCLE_CORE_BIN") {
        return PathBuf::from(path);
    }
    let executable = std::env::current_exe().expect("test executable path");
    let debug_dir = executable
        .parent()
        .and_then(|deps| deps.parent())
        .expect("target/debug directory");
    debug_dir.join("exv-vpn-darwin-core")
}
