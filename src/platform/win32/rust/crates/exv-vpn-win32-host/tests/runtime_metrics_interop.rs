
//! runtime-metrics wire 互操作 + 诊断码可见性回归（RT-INTEROP-06；L2）。
//!
//! 扩展 `grpc_interop.rs` 模式：真实 engine `HelperControlService` ⇄ 真实 named
//! pipe（DACL + 双向 peer 认证）⇄ 真实 core `EngineControlGrpcClient`，并接入真实
//! `KernelControlService`（GetSnapshot 快照形状 + 同进程 LogAggregator 诊断码扫码）。
//! 断言清单 I1–I4 见执行计划 §5.3（冻结；白名单对 §4.1 表驱动）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use exv_core::grpc_control::{EngineControlGrpcClient, KernelEngineControl, StatsStreamItem};
use exv_core::kernel_control_service::KernelControlService;
use exv_core::log_aggregator::LogAggregator;
use exv_vpn_wire::generated::kernel_control_server::KernelControl;
use exv_vpn_wire::generated::SnapshotRequest;
use exv_vpn_wire::generated::StatsPhase;
use tokio_stream::StreamExt;

mod runtime_metrics_support;

use runtime_metrics_support::{
    EngineServe, FROZEN_DIAG_CODES, FROZEN_WARN_CODES, assert_diag_whitelist,
    connected_snapshot_fixture, hermetic_service, scan_kernel_diag, tempdir_logs, unique_pipe_name,
};

type Service = KernelControlService;

/// 拼装单进程拓扑（与 transport 共用 support harness）。
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

/// 从 seam 流取下一条样本（3s 有界；非 Sample 项视为契约破损）。
async fn next_wire_sample(stream: &mut exv_core::grpc_control::StatsEventStream) -> exv_vpn_wire::generated::StatsEvent {
    let item = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("wire sample within 3s");
    match item {
        Some(StatsStreamItem::Sample(ev)) => ev,
        other => panic!("wire stream 意外终结/出错：{other:?}"),
    }
}

/// I1 wire 保真：真实 pipe 上 `EngineControlGrpcClient::stream_stats`（trait seam
/// 路径）首样本字段保真——`sequence>=1` 且后续严格递增、`timestamp_ms>0`、`phase`
/// i32 与 `StatsPhase` 往返一致、`rx_bytes`/`tx_bytes` 精确等于注册表注入值、首样本
/// `rx_rate == 0`。
#[tokio::test]
async fn i1_seam_stream_first_sample_wire_fidelity() {
    let name = unique_pipe_name("i1");
    let mut engine = EngineServe::start(&name);

    // 订阅前注入：精确累计 + Connected phase（首样本即携带注入值；engine 每条流
    // 首样本速率恒 0）。
    engine.registry().record_rx(1234);
    engine.registry().record_tx(567);
    engine.registry().set_phase(StatsPhase::Connected);

    let mut client = EngineControlGrpcClient::connect(&name, engine.pid(), engine.sid())
        .await
        .expect("core connect");
    // trait seam 路径（显式 trait 调用——inherent passthrough 遮蔽 trait 方法）。
    let mut stream = KernelEngineControl::stream_stats(&mut client, 0)
        .await
        .expect("seam stream_stats opens");

    let first = next_wire_sample(&mut stream).await;
    assert!(first.sequence >= 1, "engine sample sequence >= 1");
    assert!(first.timestamp_ms > 0, "engine 样本时间戳 > 0");
    assert_eq!(
        first.phase, StatsPhase::Connected as i32,
        "phase i32 与注入 StatsPhase 往返一致"
    );
    assert_eq!(first.rx_bytes, 1234, "rx_bytes 精确等于注册表注入值");
    assert_eq!(first.tx_bytes, 567, "tx_bytes 精确等于注册表注入值");
    assert_eq!(first.rx_rate, 0, "首样本 rx_rate == 0");
    assert_eq!(first.tx_rate, 0, "首样本 tx_rate == 0");

    // 后续样本严格递增（engine per-boot 单调序列）。
    let second = next_wire_sample(&mut stream).await;
    let third = next_wire_sample(&mut stream).await;
    assert!(second.sequence > first.sequence, "sequence 严格递增");
    assert!(third.sequence > second.sequence, "sequence 严格递增");

    engine.kill();
}

/// I2 快照 wire 形状：经 `KernelControlService::get_snapshot` 返回的
/// `RuntimeSnapshot`——非 Connected 快照的 `ConnectedState.session_established_at_ms`
/// 不可达（oneof 分支不含 connected）；stats 为 Some 时 8 字段与 common.proto:520-530
/// 一一对应（重点 `sample_tick > 0`，由 `publish_stats` 铸造）。
#[tokio::test]
async fn i2_snapshot_wire_shape_and_stats_fields() {
    let (service, mut engine, _logs_dir, _logs) = setup("i2").await;
    let bus = service.events();

    // 非 Connected 快照：状态为 Idle 占位（engine 无真实数据 → composition 派生），
    // oneof 分支不含 connected → 会话起点不可达。
    let initial = service
        .get_snapshot(tonic::Request::new(SnapshotRequest::default()))
        .await
        .expect("get_snapshot")
        .into_inner();
    assert!(
        !matches!(initial.state, Some(exv_vpn_wire::generated::runtime_snapshot::State::Connected(_))),
        "连接前快照不含 connected 分支"
    );
    assert!(initial.stats.is_none(), "无样本时快照无统计");

    // 发布 Connected fixture（publish_stats 铸造递增 tick 的前提）并拉转发器。
    bus.publish(
        exv_vpn_wire::generated::RuntimeEventKind::Snapshot,
        connected_snapshot_fixture(),
    );
    engine.registry().set_phase(StatsPhase::Connected);
    engine.registry().record_rx(77);
    engine.registry().record_tx(33);
    let handle = service.spawn_stats_forwarder();

    // 轮询 GetSnapshot 等首个样本附着。
    let deadline = Instant::now() + Duration::from_secs(5);
    let stats = loop {
        let snapshot = service
            .get_snapshot(tonic::Request::new(SnapshotRequest::default()))
            .await
            .expect("get_snapshot")
            .into_inner();
        if let Some(stats) = snapshot.stats {
            break stats;
        }
        assert!(Instant::now() < deadline, "I2：5s 内快照未出现统计");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // 8 字段一一对应（common.proto:520-530）：rx_bytes=1, tx_bytes=2, rx_rate_bps=3,
    // tx_rate_bps=4, latency_ms=5, phase=6, engine_sequence=7, sample_tick=8。
    assert_eq!(stats.rx_bytes, 77, "rx_bytes 透传（精确等于注入值）");
    assert_eq!(stats.tx_bytes, 33, "tx_bytes 透传（精确等于注入值）");
    assert_eq!(stats.rx_rate_bps, 0, "首样本归一化速率 0");
    assert_eq!(stats.tx_rate_bps, 0, "首样本归一化速率 0");
    assert_eq!(stats.phase, StatsPhase::Connected as i32, "phase i32 对应");
    assert!(stats.engine_sequence >= 1, "engine_sequence 对应（>=1）");
    assert!(stats.sample_tick > 0, "sample_tick 由 publish_stats 铸造（>0）");
    assert_eq!(stats.latency_ms, 0, "latency_ms 透传（无 ping 注入恒 0）");
    // GetSnapshot 的状态来自 engine 观察 + composition 相态机（不经 bus fixture）：
    // 无 Connected 状态事件时快照保持非 Connected——`ConnectedState
    // .session_established_at_ms` 持续不可达（oneof 分支不含 connected）。
    let snapshot = service
        .get_snapshot(tonic::Request::new(SnapshotRequest::default()))
        .await
        .expect("get_snapshot")
        .into_inner();
    assert!(
        !matches!(snapshot.state, Some(exv_vpn_wire::generated::runtime_snapshot::State::Connected(_))),
        "无 Connected 状态事件时快照保持非 Connected（起点不可达）"
    );

    handle.abort();
    engine.kill();
}

/// I3 诊断码经 logs.list：正常订阅+首样本后，同进程 LogAggregator（tempdir）扫描
/// `source=="core" && component=="kernel"` 条目——码集合 ⊆ 冻结表，至少含
/// `kernel.stats.subscribed` 与 `kernel.stats.first_sample`；每条 `code` 非空、
/// `fields` 键 ⊆ 白名单（表驱动，§4.1）。
#[tokio::test]
async fn i3_diagnostic_codes_visible_via_logs_list() {
    let (service, mut engine, _logs_dir, logs) = setup("i3").await;
    engine.registry().set_phase(StatsPhase::Connected);
    let handle = service.spawn_stats_forwarder();

    // 等首样本（码 1/码 2 的触发事实）。
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.events().current_stats().is_none() {
        assert!(Instant::now() < deadline, "I3：5s 内未收到首样本");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let entries = scan_kernel_diag(&logs);
    assert_diag_whitelist(&entries);
    let codes: Vec<&str> = entries.iter().map(|e| e.code.as_str()).collect();
    assert!(
        codes.contains(&"kernel.stats.subscribed"),
        "至少含 subscribed：{codes:?}"
    );
    assert!(
        codes.contains(&"kernel.stats.first_sample"),
        "至少含 first_sample：{codes:?}"
    );
    for entry in &entries {
        assert!(!entry.code.is_empty(), "code 非空");
        assert!(
            FROZEN_DIAG_CODES.contains(&entry.code.as_str()),
            "码 {} ⊆ 冻结表",
            entry.code
        );
    }

    handle.abort();
    engine.kill();
}

/// I4 零流量诚实性：不注入任何流量（引擎样本照常每秒到达），logs.list 扫描满足——
/// 不得出现任何 warn 级码（码 3-6、8-10）；info 级码仅允许 `kernel.stats.subscribed`
/// 与 `kernel.stats.first_sample`。GetSnapshot `stats == Some` 且累计值为 0（无流量
/// 不是故障，不得产生诊断噪声）。
#[tokio::test]
async fn i4_zero_traffic_produces_no_diagnostic_noise() {
    let (service, mut engine, _logs_dir, logs) = setup("i4").await;
    let handle = service.spawn_stats_forwarder();

    // 等首样本 + 至少再一条（确认订阅生命周期稳定，而非撞巧）。
    let baseline = poll_snapshot_stats_zero(&service).await;
    assert_eq!(baseline.rx_bytes, 0, "零流量累计 rx 精确 0（诚实零）");
    assert_eq!(baseline.tx_bytes, 0, "零流量累计 tx 精确 0");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = service
            .get_snapshot(tonic::Request::new(SnapshotRequest::default()))
            .await
            .expect("get_snapshot")
            .into_inner();
        if let Some(stats) = &snapshot.stats
            && stats.engine_sequence > baseline.engine_sequence
        {
            break;
        }
        assert!(Instant::now() < deadline, "I4：样本未持续到达");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // 留出诊断落盘窗口（若有噪声必已发生）。
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let entries = scan_kernel_diag(&logs);
    assert_diag_whitelist(&entries);
    for entry in &entries {
        assert!(
            !FROZEN_WARN_CODES.contains(&entry.code.as_str()),
            "零流量不得出现 warn 级码：{}",
            entry.code
        );
        assert!(
            entry.code == "kernel.stats.subscribed" || entry.code == "kernel.stats.first_sample",
            "info 级码仅允许 subscribed/first_sample，实得 {}",
            entry.code
        );
    }
    assert!(!entries.is_empty(), "订阅成功与首样本照常触发（info 码存在）");

    handle.abort();
    engine.kill();
}

/// 轮询 GetSnapshot 等首个 stats（I4 专用；谓词 = Some）。
async fn poll_snapshot_stats_zero(service: &Service) -> exv_vpn_wire::generated::RuntimeStats {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = service
            .get_snapshot(tonic::Request::new(SnapshotRequest::default()))
            .await
            .expect("get_snapshot")
            .into_inner();
        if let Some(stats) = snapshot.stats {
            return stats;
        }
        assert!(Instant::now() < deadline, "I4：5s 内快照未出现统计");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
