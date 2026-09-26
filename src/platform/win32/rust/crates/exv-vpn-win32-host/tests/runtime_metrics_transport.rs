
//! runtime-metrics 传输链路量化回归（RT-TRANSPORT-05；L1）。
//!
//! 拓扑与判据见执行计划 §5.2（冻结）：真实 engine `HelperControlService`（真实
//! `StatsPublisher`）⇄ 真实 named pipe（DACL + 双向 peer 认证）⇄ 真实
//! `EngineControlGrpcClient` → 真实 `KernelControlService::spawn_stats_forwarder`
//! → 真实 `EventBus`。注入只走同进程持有的 `registry()` 引用。判据刚性：累计精确
//! 相等、速率同公式重算（±1）、断流以"恢复后收到新样本"为成功条件、诊断码白名单。
//!
//! **契约缺陷记录（不放宽、不伪造，待设计裁决）：**
//! - T2 的冻结重算公式输入 `s2.timestamp_ms - s1.timestamp_ms` 在冻结读点元素
//!   `RuntimeStats` 上**不存在**（wire frozen：common.proto:520-530 无样本时间戳；
//!   每个 gRPC 客户端各得一条独立采样流，forwarder 所消费流的 engine 时间戳对测试
//!   不可观测）。本文件按可表达的最强**精确**判据落地 bus 读点断言（零增量⇒零速率、
//!   首样本速率 0、累计单调且终值精确），±1 同公式重算改在**真实 engine wire 流**
//!   （元素 `StatsEvent` 携带 `timestamp_ms`）上以 host 同源归一化函数
//!   [`exv_core::stats::normalize_stats`] 执行（t2_wire_rate_recompute）——真实
//!   wall-clock 节奏下的数值精确验证。
//! - T5（计数/时间戳回退注入）在本文件缺席：真实 `StatsRegistry` 只能单调递增、
//!   时间戳恒为 wall-clock，冻结的回退注入序列 (rx=1000→100→1100) 在"engine 零改动
//!   + 仅 registry() 注入"约束下不可构造；回退语义由 L0 锁定（`stats.rs` 单测 +
//!   `kernel_control_service` U6/U7）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use exv_core::grpc_control::{EngineControlGrpcClient, KernelEngineControl, StatsStreamItem};
use exv_core::kernel_control_service::KernelControlService;
use exv_core::log_aggregator::LogAggregator;
use exv_core::stats::{TrafficSample, normalize_stats};
use exv_vpn_wire::generated::StatsPhase;
use exv_core::stats::RuntimeStats;
use exv_vpn_wire::generated::kernel_control_server::KernelControl;
use exv_vpn_wire::generated::SnapshotRequest;

/// GetSnapshot 快照携带的 wire 统计（common.proto:520-530；phase 为 i32）。
type SnapshotStats = exv_vpn_wire::generated::RuntimeStats;
use tokio_stream::StreamExt;

mod runtime_metrics_support;

use runtime_metrics_support::{
    EngineServe, FROZEN_DIAG_CODES, assert_diag_whitelist, connected_snapshot_fixture,
    hermetic_service, scan_kernel_diag, tempdir_logs, unique_pipe_name,
};

type Service = KernelControlService;

/// 拼装单进程拓扑：engine serve + core client + hermetic KernelControlService。
async fn setup(tag: &str) -> (Service, EngineServe, tempfile::TempDir, Arc<LogAggregator>) {
    let name = unique_pipe_name(tag);
    let mut engine = EngineServe::start(&name);
    let (logs, logs_dir) = tempdir_logs();
    let client = EngineControlGrpcClient::connect(&name, engine.pid(), engine.sid())
        .await
        .expect("core connect (bounded retry dial)");
    let engine_dyn: Arc<tokio::sync::Mutex<dyn KernelEngineControl>> =
        Arc::new(tokio::sync::Mutex::new(client));
    let service = hermetic_service(engine_dyn, logs.clone());
    let _ = &mut engine;
    (service, engine, logs_dir, logs)
}

/// `delta*1000/elapsed_ms`（饱和；与 host `rate_bytes_per_sec` 同式，±1 重算用）。
fn rate_recompute(delta_bytes: u64, elapsed_ms: i64) -> u64 {
    if elapsed_ms <= 0 {
        return 0;
    }
    let elapsed = match u128::try_from(elapsed_ms) {
        Ok(v) if v > 0 => v,
        _ => return 0,
    };
    u64::try_from(u128::from(delta_bytes) * 1000 / elapsed).unwrap_or(u64::MAX)
}

/// 轮询 GetSnapshot（100ms 间隔）直到谓词成立；失败时打印诊断对象稳定枚举
///（§5.2 尾注：不打印 pipe 名以外的环境身份信息）。
async fn poll_snapshot_stats(
    service: &Service,
    budget: Duration,
    what: &str,
    pred: impl Fn(&SnapshotStats) -> bool,
) -> SnapshotStats {
    let deadline = Instant::now() + budget;
    loop {
        let snapshot = service
            .get_snapshot(tonic::Request::new(SnapshotRequest::default()))
            .await
            .expect("get_snapshot")
            .into_inner();
        if let Some(stats) = snapshot.stats {
            if pred(&stats) {
                return stats;
            }
        }
        assert!(
            Instant::now() < deadline,
            "超时（{what}）；诊断对象 = {:?}",
            service.events().stats_diagnostic()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// T1 首样本零流量：先钉 `stats == None`（确定性，冻结顺序），set_phase(Connected)
/// + record(0) 之后才拉转发器；首个 stats=Some 样本零值精确、phase=Connected、
/// engine_sequence >= 1。禁止在转发器已订阅后靠轮询"撞见"None。
#[tokio::test]
async fn t1_first_sample_zero_traffic_with_none_pinned_before_forwarder() {
    let (service, mut engine, _logs_dir, _logs) = setup("t1").await;

    // 冻结顺序第一步：转发器未启动 → GetSnapshot 钉住 stats == None（确定性）。
    let snapshot = service
        .get_snapshot(tonic::Request::new(SnapshotRequest::default()))
        .await
        .expect("get_snapshot")
        .into_inner();
    assert!(
        snapshot.stats.is_none(),
        "转发器未启动前快照必须无统计（None 阶段确定性观察，禁止订阅后靠轮询撞见）"
    );

    // 注入零流量模式 + Connected phase，然后才 spawn 转发器（engine interval 首 tick
    // 立即完成，首样本毫秒级到达）。
    engine.registry().set_phase(StatsPhase::Connected);
    engine.registry().record_tx(0);
    engine.registry().record_rx(0);
    let handle = service.spawn_stats_forwarder();

    // 轮询 GetSnapshot（100ms 间隔，5s 上限）等首个样本。
    let stats =
        poll_snapshot_stats(&service, Duration::from_secs(5), "T1 首样本", |_| true).await;
    assert_eq!(stats.phase, StatsPhase::Connected as i32, "首样本 phase = Connected");
    assert_eq!(stats.rx_bytes, 0, "累计 rx 精确 = 0（零流量注入）");
    assert_eq!(stats.tx_bytes, 0, "累计 tx 精确 = 0（零流量注入）");
    assert_eq!(stats.rx_rate_bps, 0, "首样本速率 0");
    assert_eq!(stats.tx_rate_bps, 0, "首样本速率 0");
    assert!(stats.engine_sequence >= 1, "engine_sequence >= 1");
    assert!(!handle.is_finished(), "转发器全程未退出（未 panic）");

    handle.abort();
    engine.kill();
}

/// T2 速率归一化。
///
/// 主读点 = `events().subscribe_stats(0)`（无损保序——引擎每次发布的归一化样本
/// 不重不漏，相邻对即 host feed 的真实相邻对；**不**用 GetSnapshot 轮询——其相邻对
/// 非 feed 相邻对，冻结公式必失配）。bus 读点上的精确判据：累计单调不减、终值精确
/// 等于注入值、首样本速率 (0,0)、每对相邻样本"零增量 ⇒ 零速率"（重算公式在
/// delta=0 处与 Δt 无关的精确特例）、正增量 ⇒ 正速率。
///
/// ±1 同公式重算（输入取样本自身 `timestamp_ms` 差）在真实 engine wire 流上执行
///（`StatsEvent` 携带 `timestamp_ms`）：同进程第二客户端订阅 `StreamStats`，以 host
/// 同源 [`normalize_stats`] 归一化真实样本序列，对每对相邻样本断言
/// `rate == delta*1000/Δt`（±1）。契约缺陷（bus 读点元素无 timestamp_ms）见文件头。
#[tokio::test]
async fn t2_rate_recomputed_from_cumulative_delta() {
    let name = unique_pipe_name("t2");
    let mut engine = EngineServe::start(&name);
    let (logs, _logs_dir) = tempdir_logs();
    let client = EngineControlGrpcClient::connect(&name, engine.pid(), engine.sid())
        .await
        .expect("core connect");
    let engine_dyn: Arc<tokio::sync::Mutex<dyn KernelEngineControl>> =
        Arc::new(tokio::sync::Mutex::new(client));

    // 观察流先开（同一 client/channel——pipe 单实例，但 gRPC 多流复用同一 HTTP/2
    // 连接；engine 对每条流各起独立 sampler，携带各自 sequence/timestamp_ms）。
    let mut wire_stream = {
        let mut guard = engine_dyn.lock().await;
        KernelEngineControl::stream_stats(&mut *guard, 0)
            .await
            .expect("wire stream opens")
    };

    let service = hermetic_service(engine_dyn, logs.clone());
    let bus = service.events();

    // Connected 快照 fixture：使 publish_stats 为每个样本铸造递增 tick
    //（subscribe_stats 的 live 过滤以 sample_tick 判定；生产中该角色由状态转发器的
    // Connected 快照承担）。
    bus.publish(
        exv_vpn_wire::generated::RuntimeEventKind::Snapshot,
        connected_snapshot_fixture(),
    );

    // 先订阅 bus 统计流（转发器之前 → 无损），再拉转发器。
    let mut stats_stream = bus.subscribe_stats(0);
    let seen: Arc<std::sync::Mutex<Vec<RuntimeStats>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_collector = seen.clone();
    let collector = tokio::spawn(async move {
        while let Some(stats) = stats_stream.next().await {
            seen_collector.lock().unwrap().push(stats);
        }
    });

    let handle = service.spawn_stats_forwarder();

    // 注入第一段累计 (rx=1000, tx=400)，等到样本可见。
    engine.registry().record_rx(1000);
    engine.registry().record_tx(400);
    let deadline = Instant::now() + Duration::from_secs(8);
    while !seen
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.rx_bytes == 1000 && s.tx_bytes == 400)
    {
        assert!(Instant::now() < deadline, "T2：第一段注入未在样本流出现");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 注入第二段：累计恰达 (rx=3000, tx=1000)。
    engine.registry().record_rx(2000);
    engine.registry().record_tx(600);
    let deadline = Instant::now() + Duration::from_secs(8);
    while !seen
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.rx_bytes == 3000 && s.tx_bytes == 1000)
    {
        assert!(Instant::now() < deadline, "T2：终值注入未在样本流出现");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // 让最后一条样本完成 collector 搬运。
    tokio::time::sleep(Duration::from_millis(1500)).await;
    collector.abort();

    // ---- bus 读点：无损序列的精确判据 ----
    let samples = seen.lock().unwrap().clone();
    assert!(samples.len() >= 2, "至少一对相邻样本");
    let first = &samples[0];
    assert_eq!(
        (first.rx_rate_bps, first.tx_rate_bps),
        (0, 0),
        "本订阅首样本速率 (0,0)"
    );
    for pair in samples.windows(2) {
        let (s1, s2) = (&pair[0], &pair[1]);
        assert!(
            s2.rx_bytes >= s1.rx_bytes && s2.tx_bytes >= s1.tx_bytes,
            "累计单调不减"
        );
        if s2.rx_bytes == s1.rx_bytes {
            assert_eq!(s2.rx_rate_bps, 0, "零增量 ⇒ 零速率（重算公式的 Δt 无关精确特例）");
        } else {
            assert!(s2.rx_rate_bps > 0, "正增量 ⇒ 正速率（host feed 权威归一化）");
        }
        if s2.tx_bytes == s1.tx_bytes {
            assert_eq!(s2.tx_rate_bps, 0, "零增量 ⇒ 零速率");
        } else {
            assert!(s2.tx_rate_bps > 0, "正增量 ⇒ 正速率");
        }
    }
    let last = samples.last().expect("非空");
    assert_eq!(last.rx_bytes, 3000, "终值精确等于注入值");
    assert_eq!(last.tx_bytes, 1000, "终值精确等于注入值");

    handle.abort();

    // ---- ±1 同公式重算：真实 engine wire 流（携带 timestamp_ms，已在转发器之前
    // 于同一 channel 上打开）+ host 同源归一化函数。
    engine.registry().record_rx(5000);
    // (sequence, timestamp_ms, rx_bytes, tx_bytes, rx_bps, tx_bps)。
    let mut outcomes: Vec<(u64, i64, u64, u64, u64, u64)> = Vec::new();
    let mut sample = TrafficSample::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let item = tokio::time::timeout(Duration::from_secs(3), wire_stream.next())
            .await
            .expect("wire sample within 3s");
        let ev = match item {
            Some(StatsStreamItem::Sample(ev)) => ev,
            other => panic!("wire stream 意外终结：{other:?}"),
        };
        let (stats, _outcome) = normalize_stats(&ev, &mut sample);
        outcomes.push((
            ev.sequence,
            ev.timestamp_ms,
            ev.rx_bytes,
            ev.tx_bytes,
            stats.rx_rate_bps,
            stats.tx_rate_bps,
        ));
        if ev.rx_bytes >= 5000 && outcomes.len() >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "T2：wire 观察流未达注入终值");
    }
    // 对该真实流内每对相邻样本：s2 速率 == delta*1000/Δt（±1；Δt 取样本自身
    // timestamp_ms 差）——host 同源归一化函数在真实 wall-clock 节奏下的数值精确验证。
    for pair in outcomes.windows(2) {
        let (_, ts1, rx1, tx1, _, _) = pair[0];
        let (seq2, ts2, rx2c, tx2c, rx_rate2, tx_rate2) = pair[1];
        let dt = ts2 - ts1;
        let rx_delta = rx2c.saturating_sub(rx1);
        let tx_delta = tx2c.saturating_sub(tx1);
        assert!(
            (rx_rate2 as i64 - rate_recompute(rx_delta, dt) as i64).abs() <= 1,
            "rx 速率同公式重算（±1，样本 {seq2}）：rate={rx_rate2}，recompute={}，Δt={dt}ms",
            rate_recompute(rx_delta, dt)
        );
        assert!(
            (tx_rate2 as i64 - rate_recompute(tx_delta, dt) as i64).abs() <= 1,
            "tx 速率同公式重算（±1，样本 {seq2}）：rate={tx_rate2}，recompute={}，Δt={dt}ms",
            rate_recompute(tx_delta, dt)
        );
    }

    engine.kill();
}

/// T3 断流→诊断→恢复：关停 engine 专用 runtime（真实管道断裂；abort serve future
/// 不关闭 pipe——tonic 分离连接 task，故以 runtime 关停回收全部 engine 任务）→
/// 断流诊断恰一条（真实 pipe 断裂在 wire 上表现为传输错误项或干净 EOF，两者都是
/// 冻结表的断流码；干净 EOF 的 sample_count 精确对账）→ 以**同一 pipe 名**重新
/// serve → 转发器退避后重订阅（attempt ≥ 2），5s 内 GetSnapshot 出现新样本；全程
/// JoinHandle 未结束；日志码 ⊆ 冻结表。
#[tokio::test]
async fn t3_stream_break_diagnostic_then_recover_same_pipe() {
    let (service, mut engine, _logs_dir, logs) = setup("t3").await;
    let bus = service.events();
    bus.publish(
        exv_vpn_wire::generated::RuntimeEventKind::Snapshot,
        connected_snapshot_fixture(),
    );

    // 无损收集器：数转发器已发布的样本（码 5 sample_count 精确对账面）。
    let mut stats_stream = bus.subscribe_stats(0);
    let received = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let received_collector = received.clone();
    let collector = tokio::spawn(async move {
        while (stats_stream.next().await).is_some() {
            received_collector.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });

    let handle = service.spawn_stats_forwarder();

    // 等首个样本（保证断流时 sample_count >= 1 可对账），并记录断流前最新 tick。
    let deadline = Instant::now() + Duration::from_secs(8);
    while bus.current_stats().is_none() {
        assert!(Instant::now() < deadline, "T3：断流前未收到任何样本");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let tick_before_break = bus.current_stats().expect("stats").sample_tick;

    // ---- 断流：真实管道断裂。
    engine.kill();

    // 断流诊断恰一条：stream_error 或 stream_ended（真实断裂的表现形态二选一，
    // 两者皆冻结表断流码；干净 EOF 路径做 sample_count 精确对账）。
    let deadline = Instant::now() + Duration::from_secs(5);
    let (code, fields) = loop {
        let breaks: Vec<_> = scan_kernel_diag(&logs)
            .into_iter()
            .filter(|e| {
                e.code == "kernel.stats.stream_error" || e.code == "kernel.stats.stream_ended"
            })
            .collect();
        if let Some(entry) = breaks.first() {
            assert_eq!(breaks.len(), 1, "断流诊断恰一条，实得 {}", breaks.len());
            assert_diag_whitelist(&breaks);
            break (entry.code.clone(), entry.fields.clone());
        }
        assert!(Instant::now() < deadline, "T3：5s 内未出现断流诊断码");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let expected_keys = if code == "kernel.stats.stream_error" { 2 } else { 1 };
    assert_eq!(fields.len(), expected_keys, "断流码 fields 白名单精确");
    if code == "kernel.stats.stream_ended" {
        // 让 collector 追平后精确对账 sample_count。
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            fields["sample_count"],
            received.load(std::sync::atomic::Ordering::SeqCst).to_string(),
            "干净 EOF 的 sample_count 精确"
        );
    }

    // ---- 恢复：以同一 pipe 名重新 serve；转发器退避后重订阅。
    engine.restart();
    poll_snapshot_stats(
        &service,
        Duration::from_secs(5),
        "T3 恢复新样本",
        |s| s.sample_tick > tick_before_break,
    )
    .await;

    // 码 1 的第二次 subscribed 记录（attempt >= 2——同一转发器生命周期第二次订阅）。
    let subscribed: Vec<u32> = scan_kernel_diag(&logs)
        .into_iter()
        .filter(|e| e.code == "kernel.stats.subscribed")
        .map(|e| e.fields["attempt"].parse::<u32>().expect("attempt u32"))
        .collect();
    assert!(
        subscribed.iter().any(|&attempt| attempt >= 2),
        "恢复必须出现第二次 subscribed 记录（attempt ≥ 2）：{subscribed:?}"
    );

    // 全程 JoinHandle 未结束（未 panic）；全部诊断码 ⊆ 冻结表。
    assert!(!handle.is_finished(), "转发器全程未退出（未 panic）");
    let entries = scan_kernel_diag(&logs);
    assert_diag_whitelist(&entries);

    collector.abort();
    handle.abort();
    engine.kill();
}

/// T4 订阅失败诊断：先 kill engine 再 spawn 转发器（订阅必然失败）→ 3s 内出现码 3
///（error_kind ∈ 冻结枚举），state=Retrying（经 `stats_diagnostic()`）。
#[tokio::test]
async fn t4_subscribe_failure_diagnostic_when_server_absent() {
    let (service, mut engine, _logs_dir, logs) = setup("t4").await;
    // 先 abort server（真实管道消失）再拉转发器 → 订阅失败路径。
    engine.kill();

    let handle = service.spawn_stats_forwarder();

    use exv_core::kernel_control_service::{StatsForwarderError, StatsForwarderState};
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let diagnostic = service.events().stats_diagnostic();
        if diagnostic.state == StatsForwarderState::Retrying {
            assert_eq!(
                diagnostic.last_error,
                Some(StatsForwarderError::SubscribeFailed),
                "诊断对象 last_error = SubscribeFailed"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "T4：3s 内未进入 Retrying；diagnostic = {diagnostic:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let failed: Vec<_> = scan_kernel_diag(&logs)
        .into_iter()
        .filter(|e| e.code == "kernel.stats.subscribe_failed")
        .collect();
    assert!(!failed.is_empty(), "3s 内出现码 3");
    assert_diag_whitelist(&failed);

    handle.abort();
    engine.kill();
}

/// T6 phase 反映：set_phase(Failed) 后新样本 `phase == Failed`；样本继续透传
///（phase 变化不断流——后续样本仍到达）。最新值断言走 GetSnapshot。
#[tokio::test]
async fn t6_phase_change_reflected_and_stream_continues() {
    let (service, mut engine, _logs_dir, _logs) = setup("t6").await;
    let handle = service.spawn_stats_forwarder();

    // 等首个样本出现（基线 sequence）。
    let baseline =
        poll_snapshot_stats(&service, Duration::from_secs(5), "T6 基线样本", |_| true).await;

    // set_phase(Failed) → 后续样本携带 Failed。
    engine.registry().set_phase(StatsPhase::Failed);
    let failed_stats = poll_snapshot_stats(
        &service,
        Duration::from_secs(5),
        "T6 phase=Failed",
        |s| s.phase == StatsPhase::Failed as i32 && s.engine_sequence > baseline.engine_sequence,
    )
    .await;
    assert!(
        failed_stats.engine_sequence > baseline.engine_sequence,
        "phase 变化后样本继续透传"
    );

    // 再等一条更新的样本（不断流）。
    poll_snapshot_stats(
        &service,
        Duration::from_secs(5),
        "T6 样本继续",
        |s| s.engine_sequence > failed_stats.engine_sequence,
    )
    .await;

    assert!(!handle.is_finished(), "转发器全程未退出");
    handle.abort();
    engine.kill();
}

/// 冻结表兜底：诊断码宇宙恰为 §4.1 的 11 码（防将来加码不走契约）。
#[test]
fn frozen_table_is_the_only_universe() {
    assert_eq!(FROZEN_DIAG_CODES.len(), 11, "冻结表 11 码");
}
