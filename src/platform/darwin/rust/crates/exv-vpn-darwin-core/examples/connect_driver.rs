//! 无人值守连接驱动（dev 工具；不进生产调用路径——执行红线）。
//!
//! 以"UI"身份拉起一个 Core 子进程（argv/bootstrap/UDS 认证对齐
//! `tauri/app/src/kernel/core_process.rs` 的生产链路），驱动真实连接/断开/
//! 崩溃场景，并以 JSONL 观测行输出到 stdout：
//!   {"event":"state",...}  WatchEvents 投影出的快照状态
//!   {"event":"probe",...}  穿隧道 TCP 探测结果
//!   {"event":"pump",...}   泵诊断文件中的 teardown 时间线（EXV 隧道权威证据）
//!   {"event":"sys",...}    core 死亡后的系统面观察（utun/引擎存活）
//!   {"event":"summary",...} 收尾摘要（退出码与之对应）
//!
//! 用法（示例）：
//!   connect_driver --core /Applications/EXV.app/Contents/MacOS/exv-vpn-darwin-core \
//!     --probe 202.120.80.2:53                    # 连接→探测→Stop→干净退出
//!   connect_driver --core <core> --kill-core-after 5 --observe-secs 25 \
//!     --probe 202.120.80.2:53                    # Connected 后杀 core，观察隧道存续
//!   connect_driver --core <core> --no-stop --stay-secs 600   # 留连接（外部杀引擎场景）
//!   connect_driver --core <core> --reconnect-on-engine-loss 1  # 引擎失联终态后重发连接
//!
//! 凭据：Connect 不带 secret_payload → core 回落 `~/.exv/config.json` 已保存密文
//! （与 UI「密码框留空」同一语义）；本驱动绝不读取或打印秘密材料。
//! 注意：探测目标若同时被本机其他 TUN（如 Clash）路由覆盖，断开后探测可能经其
//! 代答——EXV 隧道的存续判定以泵诊断文件的 teardown 时间线为准。
//! 与生产链路的已知有意偏差（dev 工具量级）：不校验 bootstrap record 定长与首
//! 事件 tick==1/Idle；ready 期限 10s（生产 5s）。杀 core 场景（SIGKILL）后
//! runtime 目录因残留 socket 无法 rmdir（by design，`runtime_cleanup_refused`
//! 如实上报；残留目录即 T6 预检域①的自有资源）。

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use exv_vpn_darwin_ipc::{
    authenticate_client_to_tonic_channel,
    path::{RuntimeDir, RuntimeOwner},
    peer::{ExpectedPeer, SystemPeerLookup},
    ui_core_bootstrap::UiCoreBootstrapV1,
};
use exv_vpn_wire::generated::{
    ConnectIntent, ConnectRequest, ErrorStage, OperationLookupKey, OperationMethod,
    RuntimeEvent, RuntimeSnapshot, RetryAdvice, StopIntent, StopRequest, WatchEventsRequest,
    kernel_control_client::KernelControlClient, runtime_snapshot,
};
use tokio::net::UnixStream;
use tokio::time::{Instant, sleep, timeout};
use tonic::transport::Channel;
use tonic::Streaming;

const PUMP_DIAG: &str = "/private/tmp/exv-engine-pump.log";

#[derive(Debug, Default)]
struct Args {
    core: Option<PathBuf>,
    probe: Option<String>,
    kill_core_after: Option<u64>,
    observe_secs: u64,
    reconnect_on_engine_loss: u32,
    /// T5 真机验证：引擎失联终态后不退出也不手动重发——观察 core 自身的 R2
    /// 自动重连（期望 N 次重新 Connected）。
    expect_core_r2: u32,
    connected_timeout: u64,
    stay_secs: u64,
    no_stop: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        observe_secs: 30,
        connected_timeout: 90,
        stay_secs: 3,
        ..Args::default()
    };
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("missing value for {flag}"));
        match flag.as_str() {
            "--core" => args.core = Some(PathBuf::from(value()?)),
            "--probe" => args.probe = Some(value()?),
            "--kill-core-after" => args.kill_core_after = value()?.parse().ok(),
            "--observe-secs" => args.observe_secs = value()?.parse().unwrap_or(30),
            "--reconnect-on-engine-loss" => {
                args.reconnect_on_engine_loss = value()?.parse().unwrap_or(0)
            }
            "--expect-core-r2" => args.expect_core_r2 = value()?.parse().unwrap_or(0),
            "--connected-timeout" => {
                args.connected_timeout = value()?.parse().unwrap_or(90)
            }
            "--stay-secs" => args.stay_secs = value()?.parse().unwrap_or(3),
            "--no-stop" => args.no_stop = true,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if args.core.is_none() {
        return Err("--core <path-to-exv-vpn-darwin-core> is required".to_string());
    }
    Ok(args)
}

fn emit(event: &str, fields: &[(&str, String)]) {
    let mut map = serde_json::Map::new();
    map.insert("ts".into(), serde_json::json!(now_ms()));
    map.insert("event".into(), serde_json::json!(event));
    for (key, value) in fields {
        map.insert((*key).to_string(), serde_json::json!(value));
    }
    println!("{}", serde_json::Value::Object(map));
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn state_name(snapshot: &RuntimeSnapshot) -> &'static str {
    match &snapshot.state {
        Some(runtime_snapshot::State::Idle(_)) => "Idle",
        Some(runtime_snapshot::State::Connecting(_)) => "Connecting",
        Some(runtime_snapshot::State::AwaitingInteraction(_)) => "AwaitingInteraction",
        Some(runtime_snapshot::State::Connected(_)) => "Connected",
        Some(runtime_snapshot::State::Stopping(_)) => "Stopping",
        Some(runtime_snapshot::State::Reconciling(_)) => "Reconciling",
        Some(runtime_snapshot::State::FailedClean(_)) => "FailedClean",
        Some(runtime_snapshot::State::FailedDirty(_)) => "FailedDirty",
        None => "Unknown",
    }
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut buffer = vec![0_u8; len];
    let _ = getrandom::fill(&mut buffer);
    buffer
}

fn lookup_key(method: OperationMethod) -> Option<OperationLookupKey> {
    Some(OperationLookupKey {
        principal_digest: vec![0_u8; 32],
        method: method as i32,
        runtime_epoch: random_bytes(16),
        operation_id: random_bytes(16),
    })
}

fn connect_request() -> ConnectRequest {
    ConnectRequest {
        auth_binding: None,
        intent: Some(ConnectIntent {
            lookup_key: lookup_key(OperationMethod::Connect),
            request_digest: random_bytes(32),
            profile: None,
        }),
        // 空 payload：core 回落已保存密文（UI「密码框留空」同一语义）。
        secret_payload: Vec::new(),
    }
}

fn spawn_core(binary: &std::path::Path, socket: &std::path::Path, ui_uid: u32) -> Child {
    // SAFETY: geteuid 只读取当前进程凭证。
    let ui_pid = std::process::id();
    Command::new(binary)
        .arg("--ui-pid")
        .arg(ui_pid.to_string())
        .arg("--ui-uid")
        .arg(ui_uid.to_string())
        .arg("--ui-socket")
        .arg(socket)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn core")
}

fn utun_interfaces() -> Vec<String> {
    let output = Command::new("ifconfig")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    output
        .lines()
        .filter_map(|line| line.split(':').next())
        .filter(|name| name.starts_with("utun"))
        .map(str::to_string)
        .collect()
}

fn engine_alive() -> bool {
    Command::new("pgrep")
        .arg("-f")
        .arg("exv-vpn-darwin-engine --service")
        .output()
        .map(|out| !out.stdout.is_empty())
        .unwrap_or(false)
}

/// 泵诊断文件末尾的 teardown/pump 时间线（EXV 隧道存续的权威证据）。
fn pump_tail() -> Vec<String> {
    std::fs::read_to_string(PUMP_DIAG)
        .map(|content| {
            content
                .lines()
                .filter(|line| line.contains("teardown") || line.contains("pump started"))
                .rev()
                .take(4)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn tcp_probe(target: &str) -> Result<Duration, String> {
    let started = Instant::now();
    match timeout(Duration::from_secs(6), tokio::net::TcpStream::connect(target)).await {
        Ok(Ok(_)) => Ok(started.elapsed()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("timeout after 6s".to_string()),
    }
}

async fn dial_core(
    child: &mut Child,
    runtime: &RuntimeDir,
    socket: &std::path::Path,
    ui_uid: u32,
    client_key: &exv_vpn_darwin_ipc::auth::AuthKey,
) -> Result<Channel, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("core exited during bootstrap: {status}"));
        }
        if Instant::now() >= deadline {
            return Err("core socket did not appear within 10s".to_string());
        }
        sleep(Duration::from_millis(50)).await;
    }
    runtime
        .revalidate_path()
        .map_err(|error| format!("runtime revalidate: {error}"))?;
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|error| format!("connect core socket: {error}"))?;
    let expected_core = ExpectedPeer::new(ui_uid, child.id());
    let ui_identity = ExpectedPeer::new(ui_uid, std::process::id());
    authenticate_client_to_tonic_channel(
        stream,
        expected_core,
        ui_identity,
        &SystemPeerLookup,
        client_key,
    )
    .await
    .map(|(channel, _)| channel)
    .map_err(|error| format!("authenticate to core: {error}"))
}

enum DriveOutcome {
    ConnectedThenStopped,
    /// `--no-stop`：刻意保留连接退出（隧道随后随 core EOF 收尾），未发送 Stop。
    ConnectedLeftUp,
    /// T5：观察到 core 侧 R2 自动重连到预期次数。
    CoreR2Reconnected,
    ConnectedThenCoreKilled,
    EngineLossThenGaveUp,
    EngineLossThenReconnected,
    Failed(String),
}

enum Step {
    Message(Result<Option<RuntimeEvent>, tonic::Status>),
    Timer,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("connect_driver: {error}");
            std::process::exit(2);
        }
    };
    let core_path = args.core.clone().expect("checked");
    // SAFETY: geteuid 只读取当前进程凭证。
    let ui_uid = unsafe { libc::geteuid() };

    let runtime = match RuntimeDir::create(RuntimeOwner::current()) {
        Ok(runtime) => runtime,
        Err(error) => {
            emit("summary", &[("error", format!("runtime dir: {error}"))]);
            std::process::exit(1);
        }
    };
    let socket = match runtime.engine_socket_path() {
        Ok(socket) => socket,
        Err(error) => {
            emit("summary", &[("error", format!("socket path: {error}"))]);
            let _ = runtime.cleanup_empty();
            std::process::exit(1);
        }
    };
    let mut child = spawn_core(&core_path, socket.as_path(), ui_uid);
    emit("core_spawned", &[("pid", child.id().to_string())]);

    let (bootstrap, client_key) = UiCoreBootstrapV1::random_pair().expect("bootstrap pair");
    {
        let record = bootstrap.encode();
        let mut stdin = child.stdin.take().expect("core stdin");
        use std::io::Write;
        if stdin.write_all(record.as_bytes()).is_err() {
            emit("summary", &[("error", "bootstrap stdin write failed".into())]);
            let _ = child.kill();
            let _ = child.wait();
            let _ = runtime.cleanup_empty();
            std::process::exit(1);
        }
    }

    let channel = match dial_core(&mut child, &runtime, socket.as_path(), ui_uid, &client_key).await
    {
        Ok(channel) => channel,
        Err(error) => {
            emit("summary", &[("error", error)]);
            let _ = child.kill();
            let _ = child.wait();
            let _ = runtime.cleanup_empty();
            std::process::exit(1);
        }
    };
    drop(client_key);
    emit("core_authenticated", &[("pid", child.id().to_string())]);

    let mut kernel = KernelControlClient::new(channel.clone());
    let mut watch = kernel
        .watch_events(WatchEventsRequest { resume_tick: 0 })
        .await
        .expect("watch events")
        .into_inner();

    // 消费首个快照确认就绪（对齐 UI 的 open_initial_watch 语义）。
    match watch.message().await {
        Ok(Some(event)) => {
            if let Some(snapshot) = &event.snapshot {
                emit(
                    "state",
                    &[
                        ("state", state_name(snapshot).to_string()),
                        ("tick", event.monotonic_tick.to_string()),
                    ],
                );
            }
        }
        other => {
            emit(
                "summary",
                &[("error", format!("first watch event unexpected: {other:?}"))],
            );
            let _ = child.kill();
            let _ = child.wait();
            let _ = runtime.cleanup_empty();
            std::process::exit(1);
        }
    }

    let outcome = drive(&args, &mut kernel, &mut watch, &mut child).await;

    // 收尾：EOF 释放 channel 让 core 有序退出，再回收子进程与目录。
    drop(kernel);
    drop(watch);
    drop(channel);
    let exit_deadline = Instant::now() + Duration::from_secs(8);
    let mut exit_status = None;
    while Instant::now() < exit_deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status);
                break;
            }
            Ok(None) => sleep(Duration::from_millis(100)).await,
            Err(error) => {
                emit("core_exit_error", &[("error", error.to_string())]);
                break;
            }
        }
    }
    if let Some(status) = exit_status {
        emit("core_exited", &[("status", status.to_string())]);
    } else {
        let _ = child.kill();
        let _ = child.wait();
        emit("core_exit_timeout", &[]);
    }
    match runtime.cleanup_empty() {
        Ok(()) => emit("runtime_cleaned", &[]),
        Err(error) => emit("runtime_cleanup_refused", &[("error", error.to_string())]),
    }

    let ok = !matches!(outcome, DriveOutcome::Failed(_) | DriveOutcome::EngineLossThenGaveUp);
    let label = match &outcome {
        DriveOutcome::ConnectedThenStopped => "connected_then_stopped",
        DriveOutcome::ConnectedLeftUp => "connected_left_up",
        DriveOutcome::ConnectedThenCoreKilled => "connected_then_core_killed",
        DriveOutcome::EngineLossThenGaveUp => "engine_loss_then_gave_up",
        DriveOutcome::EngineLossThenReconnected => "engine_loss_then_reconnected",
        DriveOutcome::CoreR2Reconnected => "core_r2_reconnected",
        DriveOutcome::Failed(reason) => {
            emit("failure", &[("reason", reason.clone())]);
            "failed"
        }
    };
    for line in pump_tail() {
        emit("pump", &[("line", line)]);
    }
    emit(
        "summary",
        &[
            ("outcome", label.to_string()),
            ("engine_alive", engine_alive().to_string()),
        ],
    );
    if ok {
        std::process::exit(0);
    }
    std::process::exit(1);
}

#[allow(clippy::too_many_lines)]
async fn drive(
    args: &Args,
    kernel: &mut KernelControlClient<Channel>,
    watch: &mut Streaming<RuntimeEvent>,
    child: &mut Child,
) -> DriveOutcome {
    let mut engine_loss_reconnects_left = args.reconnect_on_engine_loss;
    let mut core_r2_expected = args.expect_core_r2;
    let mut engine_loss_seen = false;
    let mut reconnected_after_loss = false;
    let mut connected_deadline = Instant::now() + Duration::from_secs(args.connected_timeout);
    let mut connected_reached = false;
    let mut last_state = String::new();
    // Connected 后的定时动作（互斥：kill-core 场景优先于 stay/stop）。
    let mut action_deadline: Option<Instant> = None;

    let connect = timeout(Duration::from_secs(15), kernel.connect(connect_request())).await;
    if let Err(status) = connect.map_err(|_| tonic::Status::cancelled("connect rpc timeout"))
        .and_then(|reply| reply.map(|r| r).map_err(|status| status))
    {
        return DriveOutcome::Failed(format!("connect rpc: {status}"));
    }
    emit("connect_sent", &[]);

    loop {
        // 复审整改：定时器恒挂——Connected 前挂 connected_deadline（防 core 卡死
        // 时事件循环无限阻塞烧掉无人值守轮），Connected 后挂 action_deadline。
        let step = if let Some(deadline) = action_deadline {
            tokio::select! {
                message = watch.message() => Step::Message(message),
                _ = tokio::time::sleep_until(deadline) => Step::Timer,
            }
        } else {
            tokio::select! {
                message = watch.message() => Step::Message(message),
                _ = tokio::time::sleep_until(connected_deadline) => {
                    return DriveOutcome::Failed("connected timeout (no events)".to_string());
                }
            }
        };

        let event = match step {
            Step::Timer => {
                if args.kill_core_after.is_some() {
                    if child.kill().is_ok() {
                        emit("core_killed", &[("pid", child.id().to_string())]);
                    }
                    action_deadline = None;
                    continue;
                }
                if args.no_stop {
                    return DriveOutcome::ConnectedLeftUp;
                }
                let stop = StopRequest {
                    intent: Some(StopIntent {
                        lookup_key: lookup_key(OperationMethod::Stop),
                        request_digest: random_bytes(32),
                    }),
                };
                match timeout(Duration::from_secs(15), kernel.stop(stop)).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(status)) => return DriveOutcome::Failed(format!("stop rpc: {status}")),
                    Err(_) => return DriveOutcome::Failed("stop rpc timeout".to_string()),
                }
                emit("stop_sent", &[]);
                action_deadline = None;
                continue;
            }
            Step::Message(Ok(None)) => {
                // Watch 流结束 = core 已死（EOF 主路径或被杀）。
                emit("watch_eof", &[("last_state", last_state.clone())]);
                if connected_reached {
                    return observe_after_core_death(args).await;
                }
                return DriveOutcome::Failed("watch ended before Connected".to_string());
            }
            Step::Message(Err(status)) => {
                emit("watch_error", &[("status", status.to_string())]);
                if connected_reached {
                    return observe_after_core_death(args).await;
                }
                return DriveOutcome::Failed(format!("watch transport: {status}"));
            }
            Step::Message(Ok(Some(event))) => event,
        };

        let Some(snapshot) = &event.snapshot else {
            continue;
        };
        let state = state_name(snapshot).to_string();
        if state != last_state {
            let mut fields = vec![
                ("state", state.clone()),
                ("tick", event.monotonic_tick.to_string()),
                ("mode", snapshot.mode.clone()),
            ];
            if let Some(stats) = &snapshot.stats {
                fields.push(("rx_bytes", stats.rx_bytes.to_string()));
                fields.push(("tx_bytes", stats.tx_bytes.to_string()));
            }
            emit("state", &fields);
            last_state = state.clone();
        }

        match &snapshot.state {
            Some(runtime_snapshot::State::Connected(_)) if !connected_reached => {
                connected_reached = true;
                if engine_loss_seen && args.expect_core_r2 > 0 {
                    emit(
                        "core_r2_reconnected",
                        &[("remaining", core_r2_expected.to_string())],
                    );
                    if core_r2_expected == 0 {
                        return DriveOutcome::CoreR2Reconnected;
                    }
                }
                if let Some(target) = &args.probe {
                    match tcp_probe(target).await {
                        Ok(elapsed) => emit(
                            "probe",
                            &[
                                ("target", target.clone()),
                                ("ok", "true".into()),
                                ("elapsed_ms", elapsed.as_millis().to_string()),
                            ],
                        ),
                        Err(error) => emit(
                            "probe",
                            &[
                                ("target", target.clone()),
                                ("ok", "false".into()),
                                ("error", error),
                            ],
                        ),
                    }
                }
                let wait = args.kill_core_after.unwrap_or(args.stay_secs);
                action_deadline = Some(Instant::now() + Duration::from_secs(wait));
            }
            Some(runtime_snapshot::State::Idle(_)) if connected_reached => {
                if engine_loss_seen && reconnected_after_loss {
                    return DriveOutcome::EngineLossThenReconnected;
                }
                return DriveOutcome::ConnectedThenStopped;
            }
            Some(runtime_snapshot::State::FailedClean(failed)) => {
                let engine_loss = failed.last_error.as_ref().is_some_and(|error| {
                    error.stage == ErrorStage::ProtocolSession as i32
                        && error.retry == RetryAdvice::UseNewOperation as i32
                });
                if !connected_reached {
                    let detail = failed
                        .last_error
                        .as_ref()
                        .map(|error| format!("stage={} retry={}", error.stage, error.retry))
                        .unwrap_or_else(|| "no error detail".into());
                    return DriveOutcome::Failed(format!("connect failed: {detail}"));
                }
                if engine_loss && args.expect_core_r2 > 0 {
                    engine_loss_seen = true;
                    core_r2_expected -= 1;
                    emit(
                        "engine_loss_awaiting_core_r2",
                        &[("remaining", core_r2_expected.to_string())],
                    );
                    connected_reached = false;
                    connected_deadline = Instant::now() + Duration::from_secs(60);
                    action_deadline = None;
                    continue;
                }
                if engine_loss {
                    engine_loss_seen = true;
                    if engine_loss_reconnects_left > 0 {
                        engine_loss_reconnects_left -= 1;
                        emit(
                            "engine_loss_reconnect",
                            &[("left", engine_loss_reconnects_left.to_string())],
                        );
                        // 引擎经 launchd 自启需要数秒，退避后再重发。
                        sleep(Duration::from_secs(6)).await;
                        match timeout(Duration::from_secs(15), kernel.connect(connect_request())).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(status)) => {
                                return DriveOutcome::Failed(format!("reconnect rpc: {status}"))
                            }
                            Err(_) => {
                                return DriveOutcome::Failed("reconnect rpc timeout".to_string())
                            }
                        }
                        reconnected_after_loss = true;
                        connected_reached = false;
                        connected_deadline = Instant::now() + Duration::from_secs(90);
                        action_deadline = None;
                        continue;
                    }
                    return DriveOutcome::EngineLossThenGaveUp;
                }
                return DriveOutcome::ConnectedThenStopped;
            }
            Some(runtime_snapshot::State::FailedDirty(_)) => {
                return DriveOutcome::Failed("terminal FailedDirty".to_string());
            }
            _ => {}
        }

        if !connected_reached && Instant::now() >= connected_deadline {
            return DriveOutcome::Failed("connected timeout".to_string());
        }
    }
}

/// core 死亡后的系统面观察：utun 集合变化 + 泵文件 teardown 时间线（权威）。
async fn observe_after_core_death(args: &Args) -> DriveOutcome {
    let deadline = Instant::now() + Duration::from_secs(args.observe_secs);
    let mut last_utuns = utun_interfaces();
    let mut pump_seen = std::collections::HashSet::new();
    while Instant::now() < deadline {
        sleep(Duration::from_secs(2)).await;
        let utuns = utun_interfaces();
        if utuns != last_utuns {
            emit(
                "sys",
                &[
                    ("utuns", format!("{utuns:?}")),
                    ("engine_alive", engine_alive().to_string()),
                ],
            );
            last_utuns = utuns;
        }
        for line in pump_tail() {
            if pump_seen.insert(line.clone()) {
                emit("pump", &[("line", line)]);
            }
        }
    }
    emit(
        "sys_final",
        &[
            ("utuns", format!("{:?}", utun_interfaces())),
            ("engine_alive", engine_alive().to_string()),
        ],
    );
    DriveOutcome::ConnectedThenCoreKilled
}
