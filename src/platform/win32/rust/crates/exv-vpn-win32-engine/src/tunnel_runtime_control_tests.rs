//! 控制段的隔离原语验收：实际取得接口 → 实际组装入口 → 原协议释放。
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

thread_local! {
    static BEFORE_RESOURCES_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn observe_before_resources_lock() {
    BEFORE_RESOURCES_LOCK.with(|hook| {
        if let Some(observe) = hook.borrow_mut().take() {
            observe();
        }
    });
}

fn context() -> ApplyContext {
    ApplyContext {
        connection_mode: ConnectionMode::Standard,
        plan: super::tests::test_plan(),
        credentials: None,
        operation_id: vec![61; 16],
        status: Arc::new(StatusPublisher::new()),
        stats: Arc::new(crate::stats::StatsPublisher::new()),
        log: Arc::new(LogSink::null()),
    }
}

const TRACE_SID: &str = "S-1-5-21-3623811015-3361044348-30300820-1013";

fn trace_runtime() -> (
    Arc<RealTunnelRuntime>,
    tempfile::TempDir,
    crate::connect_gate::ConnectEnvelope,
) {
    trace_runtime_with_runner(Arc::new(|_| {
        Box::pin(async {
            crate::preflight::PreflightReport {
                started_at_ms: 0,
                elapsed: Duration::ZERO,
                domains: vec![],
                cleanup: vec![],
                dns_cache_flushed: false,
            }
        })
    }))
}

fn trace_runtime_with_runner(
    runner: crate::aftermath::AftermathVerifier,
) -> (
    Arc<RealTunnelRuntime>,
    tempfile::TempDir,
    crate::connect_gate::ConnectEnvelope,
) {
    trace_runtime_with_runner_and_kind(runner, crate::connection_trace::TraceKind::Atrust)
}

fn trace_runtime_with_runner_and_kind(
    runner: crate::aftermath::AftermathVerifier,
    kind: crate::connection_trace::TraceKind,
) -> (
    Arc<RealTunnelRuntime>,
    tempfile::TempDir,
    crate::connect_gate::ConnectEnvelope,
) {
    use crate::connection_trace::*;
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new(crate::connect_gate::ConnectGate::with_store(
        Some(ConnectionTraceStore::open(dir.path()).unwrap()),
        TraceEngineMode::Oneshot,
        Some(TRACE_SID.into()),
        dir.path().to_owned(),
        runner,
    ));
    let rt = Arc::new(
        RealTunnelRuntime::new_with_config_dir(
            PathBuf::from("absent.dll"),
            "TraceStage".into(),
            dir.path().to_owned(),
        )
        .with_trace_gate(gate.clone()),
    );
    let envelope = crate::connect_gate::ConnectEnvelope {
        kind,
        on_core_exit: TraceOnCoreExit::Keep,
        reconnect: TraceReconnectPolicy {
            enabled: true,
            on_oneshot_engine_crash: false,
            notify: true,
        },
        core_pid: 4242,
        ui_pid: 4243,
    };
    let receipt = rt
        .inner_runtime()
        .block_on(gate.ensure_connect_trace_with_receipt(&envelope, &LogSink::null()))
        .unwrap();
    rt.attempt_ledger
        .lock()
        .unwrap()
        .begin(&[61; 16], receipt.into_ledger_fact());
    (rt, dir, envelope)
}

fn trace_rollback_outcome(rt: &RealTunnelRuntime) -> crate::connect_ledger::RollbackOutcome {
    rt.attempt_ledger
        .lock()
        .unwrap()
        .last_rollback
        .as_ref()
        .unwrap()
        .clone()
}

#[test]
fn rollback_trace_clean_deletes_only_after_actual_control_close() {
    let (rt, dir, _) = trace_runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    rt.assemble_prepared(
        ctx,
        Default::default(),
        None,
        prepared,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    )
    .unwrap_err();
    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "clean 实际逐项结果：{:?}",
        trace_rollback_outcome(&rt)
    );
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    assert_eq!(
        store.read(TRACE_SID).unwrap(),
        crate::connection_trace::TraceReadOutcome::Absent,
        "clean 实际逐项结果：{:?}",
        trace_rollback_outcome(&rt)
    );
    assert_eq!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .entries[0]
            .state,
        crate::connect_ledger::EntryState::Compensated
    );
    rt.disconnect().unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[test]
fn rollback_trace_mixed_results_keep_only_actual_unresolved_domains_then_w1_recreates() {
    let recovery: Arc<std::sync::Mutex<std::sync::Weak<RealTunnelRuntime>>> =
        Arc::new(std::sync::Mutex::new(std::sync::Weak::new()));
    let reader = recovery.clone();
    let (rt, dir, envelope) = trace_runtime_with_runner(Arc::new(move |hint| {
        let rt = reader.lock().unwrap().upgrade();
        Box::pin(async move {
            let cleanup = if let Some(record) = hint {
                assert_eq!(
                    serde_json::to_value(record).unwrap()["rollback_unresolved"],
                    serde_json::json!(["route_family"])
                );
                // W1 门的隔离端口：真实剩余义务经既有 retry_clear，报告来自实际结果。
                let result = rt.unwrap().retry_pending_routes();
                vec![crate::preflight::CleanupAction {
                    domain: crate::preflight::DomainKind::Routes,
                    finding_kind: "route_dangling",
                    target: "isolated-route".into(),
                    outcome: match result {
                        Ok(()) => crate::preflight::CleanupOutcome::Done,
                        Err(error) => crate::preflight::CleanupOutcome::Failed(error),
                    },
                }]
            } else {
                vec![]
            };
            crate::preflight::PreflightReport {
                started_at_ms: 0,
                elapsed: Duration::ZERO,
                domains: vec![],
                cleanup,
                dns_cache_flushed: false,
            }
        })
    }));
    *recovery.lock().unwrap() = Arc::downgrade(&rt);
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    bind_pending(&mut control, true);
    drop(control);
    drop(packets);
    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "mixed 实际逐项结果：{:?}",
        trace_rollback_outcome(&rt)
    );
    let path = dir.path().join(format!("trace-{TRACE_SID}.json"));
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let old_id = json["connection_id"].as_str().unwrap().to_owned();
    let actual = json["rollback_unresolved"].clone();
    // 先收尾隔离路由义务，断言失败也不留下待清资源。
    crate::platform_tunnel::tests::ledger_allow_pending_removal(
        &mut rt.pending_routes.lock().unwrap()[0],
    );
    assert_eq!(
        actual,
        serde_json::json!(["route_family"]),
        "mixed 实际逐项结果：{:?}",
        trace_rollback_outcome(&rt)
    );
    let next = rt
        .inner_runtime()
        .block_on(
            rt.trace_gate
                .as_ref()
                .unwrap()
                .ensure_connect_trace_with_receipt(&envelope, &LogSink::null()),
        )
        .unwrap();
    assert_ne!(next.connection_id, old_id);
    assert!(rt.pending_routes.lock().unwrap().is_empty());
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(json["rollback_unresolved"], serde_json::json!([]));
}

#[test]
fn rollback_trace_replacement_is_neither_deleted_nor_overwritten() {
    for unresolved in [false, true] {
        let (rt, dir, envelope) = trace_runtime();
        let ctx = context();
        let (prepared, _peer) = atrust(&rt, &ctx, Arc::new(AtomicU64::new(0)));
        let PreparedConnection {
            mut control,
            packets,
            ..
        } = prepared;
        if unresolved {
            bind_pending(&mut control, true);
        }
        let gate = rt.trace_gate.as_ref().unwrap();
        gate.replace_connect_trace_with_receipt(&envelope).unwrap();
        let path = dir.path().join(format!("trace-{TRACE_SID}.json"));
        let before = std::fs::read(&path).unwrap();
        drop(control);
        drop(packets);
        let after = std::fs::read(&path);
        if unresolved {
            crate::platform_tunnel::tests::ledger_allow_pending_removal(
                &mut rt.pending_routes.lock().unwrap()[0],
            );
            rt.retry_pending_routes().unwrap();
        }
        assert_eq!(
            after.unwrap(),
            before,
            "replacement unresolved={unresolved} 实际逐项结果：{:?}",
            trace_rollback_outcome(&rt)
        );
    }
}

#[test]
fn rollback_trace_store_failure_is_actual_unresolved_not_clean() {
    let (rt, dir, _) = trace_runtime();
    let ctx = context();
    let (prepared, _peer) = atrust(&rt, &ctx, Arc::new(AtomicU64::new(0)));
    // 保持真实可读 trace，用同目录临时路径占位使原子回写发生实际 I/O 错误。
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    bind_pending(&mut control, true);
    std::fs::create_dir(dir.path().join(format!("trace-{TRACE_SID}.json.tmp"))).unwrap();
    drop(control);
    drop(packets);
    let outcome = trace_rollback_outcome(&rt);
    let domains = outcome.unresolved.iter().map(|e| e.0).collect::<Vec<_>>();
    crate::platform_tunnel::tests::ledger_allow_pending_removal(
        &mut rt.pending_routes.lock().unwrap()[0],
    );
    rt.retry_pending_routes().unwrap();
    assert_eq!(
        domains,
        vec![LedgerDomain::RouteFamily, LedgerDomain::Trace],
        "实际逐项结果必须保留详情：{outcome:?}"
    );
}

#[test]
fn rollback_trace_existing_and_skipped_some_follow_actual_uuid() {
    for skipped in [false, true] {
        let (rt, dir, _) = trace_runtime();
        let mut slot = rt.attempt_ledger.lock().unwrap();
        let trace = &mut slot.active.as_mut().unwrap().trace;
        let TraceAdmission::Created { connection_id } = trace else {
            panic!("receipt");
        };
        *trace = if skipped {
            TraceAdmission::Skipped {
                connection_id: Some(connection_id.clone()),
            }
        } else {
            TraceAdmission::Existing {
                connection_id: connection_id.clone(),
            }
        };
        drop(slot);
        let (prepared, _peer) = atrust(&rt, &context(), Arc::new(AtomicU64::new(0)));
        drop(prepared);
        let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
        assert_eq!(
            store.read(TRACE_SID).unwrap(),
            crate::connection_trace::TraceReadOutcome::Absent,
            "Existing / Skipped(Some) skipped={skipped} 实际逐项结果：{:?}",
            trace_rollback_outcome(&rt)
        );
    }
}

#[test]
fn rollback_trace_unwired_and_skipped_none_never_claim_current_file() {
    for trace in [
        TraceAdmission::Unwired,
        TraceAdmission::Skipped {
            connection_id: None,
        },
    ] {
        let (rt, dir, _) = trace_runtime();
        let path = dir.path().join(format!("trace-{TRACE_SID}.json"));
        let before = std::fs::read(&path).unwrap();
        let mut slot = rt.attempt_ledger.lock().unwrap();
        slot.active = None;
        slot.begin(&[61; 16], trace);
        drop(slot);
        let (prepared, _peer) = atrust(&rt, &context(), Arc::new(AtomicU64::new(0)));
        drop(prepared);
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(
            !rt.attempt_ledger
                .lock()
                .unwrap()
                .last_rollback
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .any(|e| e.domain == LedgerDomain::Trace)
        );
    }
}

#[test]
fn rollback_trace_delete_io_failure_still_publishes_failed_with_cause() {
    use std::os::windows::fs::OpenOptionsExt;
    let (rt, dir, _) = trace_runtime();
    let path = dir.path().join(format!("trace-{TRACE_SID}.json"));
    // 允许读写、不共享 DELETE；store 真读取能成功，真实删除应发生 sharing violation。
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&path)
        .unwrap();
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    assert!(matches!(
        store.read(TRACE_SID).unwrap(),
        crate::connection_trace::TraceReadOutcome::Present(_)
    ));
    let mut ctx = context();
    ctx.log = Arc::new(LogSink::null());
    let mut logs = ctx.log.open_stream(0);
    let stats = ctx.stats.clone();
    let status = ctx.status.clone();
    RealTunnelRuntime::run_assemble_with_reset(
        &rt,
        ctx,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
        &stats,
        &status,
        &[61; 16],
    );
    drop(held);
    assert_eq!(
        status.latest().unwrap().coarse_phase,
        generated::StatsPhase::Failed
    );
    assert!(!rt.is_assembling());
    assert_eq!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .last_rollback
            .as_ref()
            .unwrap()
            .unresolved
            .iter()
            .map(|e| e.0)
            .collect::<Vec<_>>(),
        vec![LedgerDomain::Trace]
    );
    assert!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .last_rollback
            .as_ref()
            .unwrap()
            .unresolved[0]
            .1
            .contains("code: 32"),
        "临时句柄必须造成实际 Windows sharing violation，不能用读取或其它失败代替删除故障"
    );
    let mut events = vec![];
    while let Ok(event) = logs.try_recv() {
        events.push(event.unwrap());
    }
    assert!(
        events
            .iter()
            .any(|event| event.code == "tunnel.rollback.unresolved"
                && event.fields["unresolved_domains"] == "trace")
    );
    assert!(events.iter().any(|event| {
        event.code == "tunnel.assemble.failed"
            && event
                .fields
                .get("detail")
                .is_some_and(|detail| detail.contains("实际回滚未竟域：trace"))
    }));
}

#[test]
fn fix1_started_receipt_does_not_infer_adopt_from_connected_snapshot() {
    let fake = Arc::new(FakeTunnelRuntime::new());
    assert_eq!(
        fake.clone()
            .start_apply_with_receipt(context(), TraceAdmission::Unwired)
            .unwrap(),
        ApplyAcceptance::Started
    );
    assert!(
        fake.is_connected(),
        "默认适配即使看到 Connected 也不能猜 Adopted"
    );
    let (rt, _dir, _) = trace_runtime();
    // 无凭据新装配在网络操作前失败或响应取消；Started 仅表示同步受理，不冒充 Connected。
    assert_eq!(
        rt.clone()
            .start_apply_with_receipt(context(), TraceAdmission::Unwired)
            .unwrap(),
        ApplyAcceptance::Started
    );
    assert!(rt.disconnect_report().stopped);
    assert!(rt.live.lock().unwrap().is_none());
}

#[test]
fn rollback_trace_committed_duplicate_adopt_and_normal_disconnect_do_not_rollback() {
    // 无新凭据入口真实请求 Cisco；必须建立同协议的局部 TLS owner，不能把
    // aTrust 替换受理的 Ok 当成 adopt。仅连接既有回环 fixture。
    let (rt, dir, _) = trace_runtime_with_runner_and_kind(
        Arc::new(|_| {
            Box::pin(async {
                crate::preflight::PreflightReport {
                    started_at_ms: 0,
                    elapsed: Duration::ZERO,
                    domains: vec![],
                    cleanup: vec![],
                    dns_cache_flushed: false,
                }
            })
        }),
        crate::connection_trace::TraceKind::Cisco,
    );
    let ctx = context();
    let (session, mut ended) = rt.inner_runtime().block_on(cisco_pair());
    let prepared = rt.hold_cisco_control(
        session,
        &ctx.operation_id,
        ctx.connection_mode,
        RuntimeDiagnostics::new(ctx.log.clone(), &ctx.operation_id),
    );
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    bind_threads(
        &mut control,
        EngineDataPlaneThreads::test_gated_worker(move || {
            entered_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
        }),
    );
    rt.install_live(
        control.into_live(),
        &ctx.status,
        &ctx.stats,
        &ctx.operation_id,
    )
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(rt.is_connected(), "采纳断言需要真实 worker 仍存活");
    assert!(!rt.is_assembling());
    {
        let live = rt.live.lock().unwrap();
        let live = live.as_ref().unwrap();
        assert_eq!(live.protocol, Protocol::Cisco);
        assert_eq!(live.connection_mode, ctx.connection_mode);
    }
    assert_eq!(
        ctx.status.latest().unwrap().coarse_phase,
        generated::StatsPhase::Connected
    );
    assert_eq!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .last_closed
            .as_ref()
            .unwrap()
            .ended,
        Some(crate::connect_ledger::AttemptEnd::Committed)
    );
    let path = dir.path().join(format!("trace-{TRACE_SID}.json"));
    let before = std::fs::read(&path).unwrap();
    rt.assembling.store(true, Ordering::SeqCst);
    assert!(
        rt.clone()
            .start_apply_recorded(context(), None, None)
            .is_err()
    );
    rt.assembling.store(false, Ordering::SeqCst);
    let adopted = context();
    let adopted_status = adopted.status.clone();
    assert_eq!(
        rt.clone()
            .start_apply_with_receipt(adopted, TraceAdmission::Unwired)
            .unwrap(),
        ApplyAcceptance::Adopted,
        "真实 Cisco Live rebind 产生采纳回执，不以其后存活快照替代"
    );
    assert!(!rt.is_assembling(), "采纳不得启动新组装线程");
    assert_eq!(
        adopted_status.latest().unwrap().coarse_phase,
        generated::StatsPhase::Connected
    );
    assert!(rt.attempt_ledger.lock().unwrap().active.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(rt.attempt_ledger.lock().unwrap().last_rollback.is_none());
    rt.inner_runtime().block_on(async {
        assert!(
            tokio::time::timeout(Duration::from_millis(60), &mut ended)
                .await
                .is_err(),
            "采纳后真实 TLS owner 仍存活"
        );
    });
    drop(packets);
    release_tx.send(()).unwrap();
    rt.disconnect().unwrap();
    rt.inner_runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(2), ended)
            .await
            .unwrap()
            .unwrap();
    });
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(rt.attempt_ledger.lock().unwrap().last_rollback.is_none());
}

#[test]
fn rollback_trace_previous_cleanup_failure_keeps_actual_r1_trace() {
    let gate_calls = Arc::new(AtomicU64::new(0));
    let calls = gate_calls.clone();
    let (rt, dir, _) = trace_runtime_with_runner(Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            crate::preflight::PreflightReport {
                started_at_ms: 0,
                elapsed: Duration::ZERO,
                domains: vec![],
                cleanup: vec![],
                dns_cache_flushed: false,
            }
        })
    }));
    let initial_gate_calls = gate_calls.load(Ordering::SeqCst);
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    let crate::connection_trace::TraceReadOutcome::Present(before) = store.read(TRACE_SID).unwrap()
    else {
        panic!("实际已建立 trace")
    };
    rt.attempt_ledger.lock().unwrap().active = None;
    rt.pending_routes
        .lock()
        .unwrap()
        .push(crate::platform_tunnel::tests::ledger_isolated_pending_fixture(true));
    let ctx = context();
    let status = ctx.status.clone();
    let mut logs = ctx.log.open_stream(0);
    rt.clone().start_apply(ctx).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while rt.is_assembling() && Instant::now() < until {
        std::thread::yield_now();
    }
    assert!(!rt.is_assembling(), "局部前置清理须有界结束");
    let outcome = trace_rollback_outcome(&rt);
    let trace = store.read(TRACE_SID).unwrap();
    let remaining = rt
        .pending_routes
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.len())
        .sum::<usize>();
    let mut details = vec![];
    while let Ok(event) = logs.try_recv() {
        details.push(event.unwrap());
    }
    // 断言前完成隔离义务；真实 retry_clear 算法仍运行，不操作系统路由。
    for pending in rt.pending_routes.lock().unwrap().iter_mut() {
        crate::platform_tunnel::tests::ledger_allow_pending_removal(pending);
    }
    rt.retry_pending_routes().unwrap();
    assert!(rt.pending_routes.lock().unwrap().is_empty());
    assert_eq!(remaining, 1);
    assert_eq!(
        gate_calls.load(Ordering::SeqCst),
        initial_gate_calls,
        "R1 不重复 gate"
    );
    assert_eq!(outcome.attempt_id, vec![61; 16]);
    assert_eq!(
        status.latest().unwrap().coarse_phase,
        generated::StatsPhase::Failed
    );
    assert_eq!(
        status.latest().unwrap().error.unwrap().reason,
        "platform/previous_cleanup"
    );
    let crate::connection_trace::TraceReadOutcome::Present(after) = trace else {
        panic!("旧路由清理实际失败，不得删除同 UUID 痕：{outcome:?}; logs={details:?}")
    };
    assert_eq!(after.connection_id, before.connection_id);
    assert_eq!(after.rollback_unresolved, vec!["route_family"]);
    assert_eq!(outcome.unresolved_domains(), vec!["route_family"]);
    let route = outcome
        .entries
        .iter()
        .find(|e| e.domain == LedgerDomain::RouteFamily)
        .unwrap();
    assert!(matches!(
        route.state,
        crate::connect_ledger::EntryState::Unresolved(_)
    ));
    assert!(
        route.detail.as_ref().unwrap().contains("code: 5"),
        "{outcome:?}"
    );
    assert!(
        outcome
            .entries
            .iter()
            .any(|e| e.domain == LedgerDomain::Trace
                && e.state == crate::connect_ledger::EntryState::Retained)
    );
    assert!(
        details
            .iter()
            .any(|e| e.code == "tunnel.rollback.unresolved"),
        "{details:?}"
    );
    assert!(
        !details
            .iter()
            .any(|e| e.code == "tunnel.rollback.completed"),
        "{details:?}"
    );
}

#[test]
fn rollback_trace_clean_early_failure_deletes_actual_r1_trace() {
    let gate_calls = Arc::new(AtomicU64::new(0));
    let calls = gate_calls.clone();
    let (rt, dir, _) = trace_runtime_with_runner(Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            crate::preflight::PreflightReport {
                started_at_ms: 0,
                elapsed: Duration::ZERO,
                domains: vec![],
                cleanup: vec![],
                dns_cache_flushed: false,
            }
        })
    }));
    let initial_gate_calls = gate_calls.load(Ordering::SeqCst);
    rt.attempt_ledger.lock().unwrap().active = None;
    rt.pending_routes
        .lock()
        .unwrap()
        .push(crate::platform_tunnel::tests::ledger_isolated_pending_fixture(false));
    let ctx = context();
    let status = ctx.status.clone();
    rt.clone().start_apply(ctx).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while rt.is_assembling() && Instant::now() < until {
        std::thread::yield_now();
    }
    assert!(!rt.is_assembling(), "局部前置失败须有界结束");
    let outcome = trace_rollback_outcome(&rt);
    assert!(rt.pending_routes.lock().unwrap().is_empty());
    assert_eq!(
        gate_calls.load(Ordering::SeqCst),
        initial_gate_calls,
        "R1 不重复 gate"
    );
    assert_eq!(
        status.latest().unwrap().coarse_phase,
        generated::StatsPhase::Failed
    );
    assert_ne!(
        status.latest().unwrap().error.unwrap().reason,
        "platform/previous_cleanup"
    );
    assert!(outcome.resolved(), "{outcome:?}");
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    assert_eq!(
        store.read(TRACE_SID).unwrap(),
        crate::connection_trace::TraceReadOutcome::Absent,
        "干净旧清理后的前置失败仍须收口实际 R1 痕：{outcome:?}"
    );
}

#[test]
fn rollback_trace_previous_cleanup_access_failure_keeps_actual_r1_trace() {
    let (rt, dir, _) = trace_runtime();
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    let crate::connection_trace::TraceReadOutcome::Present(before) = store.read(TRACE_SID).unwrap()
    else {
        panic!("实际已建立 trace")
    };
    rt.attempt_ledger.lock().unwrap().active = None;
    // 只毒化这个临时 runtime 的资源访问锁；没有系统资源或 store I/O 故障。
    let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _resources = rt.resources.lock().unwrap();
        panic!("isolated previous cleanup access failure")
    }));
    let ctx = context();
    let status = ctx.status.clone();
    rt.clone().start_apply(ctx).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while rt.is_assembling() && Instant::now() < until {
        std::thread::yield_now();
    }
    rt.resources.clear_poison();
    assert!(poison.is_err());
    assert!(!rt.is_assembling(), "局部访问失败须有界结束");
    let outcome = trace_rollback_outcome(&rt);
    let trace = store.read(TRACE_SID).unwrap();
    assert_eq!(
        status.latest().unwrap().coarse_phase,
        generated::StatsPhase::Failed
    );
    assert_eq!(
        status.latest().unwrap().error.unwrap().reason,
        "platform/previous_cleanup"
    );
    let crate::connection_trace::TraceReadOutcome::Present(after) = trace else {
        panic!("实际清理访问失败，不得假空 clean 删痕：{outcome:?}")
    };
    assert_eq!(after.connection_id, before.connection_id);
    assert_eq!(after.rollback_unresolved, vec!["trace"]);
    assert_eq!(outcome.unresolved_domains(), vec!["trace"]);
    assert!(
        outcome
            .entries
            .iter()
            .any(|e| e.domain == LedgerDomain::Trace
                && e.state
                    == crate::connect_ledger::EntryState::Unresolved("previous_cleanup_access")
                && e.detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("runtime resources lock"))),
        "{outcome:?}"
    );
    assert!(
        !outcome
            .entries
            .iter()
            .any(|e| e.domain == LedgerDomain::RouteFamily),
        "未访问真实路由，不臆造该域：{outcome:?}"
    );
}

fn runtime() -> Arc<RealTunnelRuntime> {
    let rt = Arc::new(RealTunnelRuntime::new(
        PathBuf::from("absent.dll"),
        "ControlStage".into(),
    ));
    rt.attempt_ledger
        .lock()
        .unwrap()
        .begin(&[61; 16], TraceAdmission::Unwired);
    rt
}

fn offer() -> exv_vpn_cstp::session::TunnelOffer {
    exv_vpn_cstp::session::TunnelOffer {
        ipv4_address: "10.0.0.1".parse().unwrap(),
        prefix: 24,
        mtu: 1400,
        dns_servers: vec![],
        routes: vec![],
    }
}

fn atrust(
    rt: &Arc<RealTunnelRuntime>,
    ctx: &ApplyContext,
    completed: Arc<AtomicU64>,
) -> (PreparedConnection, tokio::io::DuplexStream) {
    let (owner, channels, peer) =
        rt.inner_runtime()
            .block_on(crate::atrust_tunnel::tests::pair_with_join_observer(
                completed,
            ));
    let prepared = rt.hold_atrust_control(
        crate::atrust_prepare::PreparedAtrust {
            offer: offer(),
            campus_dns_servers: vec![],
            bypasses: vec![],
            owner,
            channels,
        },
        &ctx.operation_id,
        ctx.connection_mode,
        RuntimeDiagnostics::new(ctx.log.clone(), &ctx.operation_id),
    );
    (prepared, peer)
}

#[test]
fn connecting_control_atrust_nic_failure_awaits_real_worker_exactly_once() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, mut peer) = atrust(&rt, &ctx, completed.clone());
    // None 使真实 NIC 确保分支观察到 preflight 依赖已失去：不加载 DLL，不建卡。
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert!(error.detail.contains("preflight NIC changed"));
    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "失败返回前必须 await 真正 worker，abort 不算节点释放"
    );
    rt.inner_runtime().block_on(async {
        let mut byte = [0];
        assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
    });
    rt.disconnect().unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

// RED 已观察基线 NIC 前置错误；GREEN 仅此构造点显式 armed。
fn acceptance_fault_runtime() -> Arc<RealTunnelRuntime> {
    let rt = Arc::new(
        RealTunnelRuntime::new(PathBuf::from("absent.dll"), "ControlStage".into())
            .with_acceptance_fault_once(Some(AcceptanceFaultOnce::AfterControlBeforeNic)),
    );
    rt.attempt_ledger
        .lock()
        .unwrap()
        .begin(&[61; 16], TraceAdmission::Unwired);
    rt
}

fn acceptance_fault_assert_control_only_rollback(rt: &RealTunnelRuntime, id: &[u8]) {
    let outcome = trace_rollback_outcome(rt);
    assert_eq!(outcome.attempt_id, id);
    assert!(outcome.resolved(), "{outcome:?}");
    assert_eq!(
        outcome.entries.iter().map(|e| e.domain).collect::<Vec<_>>(),
        vec![LedgerDomain::SessionLease]
    );
    assert_eq!(
        outcome.entries[0].state,
        crate::connect_ledger::EntryState::Compensated
    );
    assert!(rt.nic.lock().unwrap().is_none());
    assert!(rt.live.lock().unwrap().is_none());
}

#[test]
fn acceptance_fault_atrust_closes_real_control_before_return() {
    let rt = acceptance_fault_runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, mut peer) = atrust(&rt, &ctx, completed.clone());
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    rt.inner_runtime().block_on(async {
        assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
    });
    assert_eq!(
        error.detail,
        "engine: acceptance fault after-control-before-nic"
    );
    acceptance_fault_assert_control_only_rollback(&rt, &[61; 16]);
    // 直接调用局部装配入口的 fixture 需镜像外层 worker 的原记录终结。
    assert!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .finish(&[61; 16], AttemptEnd::Failed)
    );
    // 同 runtime 的新 Cisco 实际控制会话共享一次消费，回到原 NIC 前置错误。
    let mut ctx = context();
    ctx.operation_id = vec![62; 16];
    rt.attempt_ledger
        .lock()
        .unwrap()
        .begin(&ctx.operation_id, TraceAdmission::Unwired);
    let (session, ended) = rt.inner_runtime().block_on(cisco_pair());
    let prepared = rt.hold_cisco_control(
        session,
        &ctx.operation_id,
        ctx.connection_mode,
        RuntimeDiagnostics::new(ctx.log.clone(), &ctx.operation_id),
    );
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert!(error.detail.contains("preflight NIC changed"));
    rt.inner_runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(2), ended)
            .await
            .unwrap()
            .unwrap();
    });
    acceptance_fault_assert_control_only_rollback(&rt, &[62; 16]);
}

#[test]
fn acceptance_fault_cisco_closes_real_tls_before_return() {
    let rt = acceptance_fault_runtime();
    let ctx = context();
    let (session, mut ended) = rt.inner_runtime().block_on(cisco_pair());
    let prepared = rt.hold_cisco_control(
        session,
        &ctx.operation_id,
        ctx.connection_mode,
        RuntimeDiagnostics::new(ctx.log.clone(), &ctx.operation_id),
    );
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    // 原 graceful close 的有界读取已经由返回前 join 完成；peer 必须实际观察 EOF。
    rt.inner_runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(2), &mut ended)
            .await
            .unwrap()
            .unwrap();
    });
    assert_eq!(
        error.detail,
        "engine: acceptance fault after-control-before-nic"
    );
    acceptance_fault_assert_control_only_rollback(&rt, &[61; 16]);
    assert!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .finish(&[61; 16], AttemptEnd::Failed)
    );
    let mut ctx = context();
    ctx.operation_id = vec![62; 16];
    rt.attempt_ledger
        .lock()
        .unwrap()
        .begin(&ctx.operation_id, TraceAdmission::Unwired);
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, mut peer) = atrust(&rt, &ctx, completed.clone());
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert!(error.detail.contains("preflight NIC changed"));
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    rt.inner_runtime().block_on(async {
        assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
    });
    acceptance_fault_assert_control_only_rollback(&rt, &[62; 16]);
}

#[test]
fn acceptance_fault_registration_and_precancel_do_not_consume() {
    for registration_error in [false, true] {
        let rt = acceptance_fault_runtime();
        let ctx = context();
        if registration_error {
            rt.attempt_ledger.lock().unwrap().active = None;
        }
        let completed = Arc::new(AtomicU64::new(0));
        let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
        let error = rt
            .assemble_prepared(
                ctx,
                Default::default(),
                None,
                prepared,
                Arc::new(std::sync::atomic::AtomicBool::new(!registration_error)),
            )
            .unwrap_err();
        assert_eq!(
            error.reason,
            if registration_error {
                "platform/ledger_registration"
            } else {
                "platform/assembly_cancelled"
            }
        );
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        acceptance_fault_assert_next_hit(&rt);
    }
}

fn acceptance_fault_assert_next_hit(rt: &Arc<RealTunnelRuntime>) {
    rt.attempt_ledger
        .lock()
        .unwrap()
        .begin(&[61; 16], TraceAdmission::Unwired);
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(rt, &ctx, completed.clone());
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert_eq!(
        error.detail,
        "engine: acceptance fault after-control-before-nic"
    );
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[test]
fn acceptance_fault_lock_cancel_does_not_consume() {
    let rt = acceptance_fault_runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let resources = rt.resources.lock().unwrap();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_rt = rt.clone();
    let worker_cancel = cancel.clone();
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (continue_tx, continue_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        BEFORE_RESOURCES_LOCK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                reached_tx.send(()).unwrap();
                continue_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }))
        });
        worker_rt.assemble_prepared(ctx, Default::default(), None, prepared, worker_cancel)
    });
    // 测试专用线程局部观察证明入锁前取消检查已完成；不靠 sleep 猜测调度。
    reached_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    cancel.store(true, Ordering::SeqCst);
    continue_tx.send(()).unwrap();
    drop(resources);
    assert_eq!(
        worker.join().unwrap().unwrap_err().reason,
        "platform/assembly_cancelled"
    );
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    acceptance_fault_assert_next_hit(&rt);
}

#[test]
fn acceptance_fault_old_live_cleanup_failure_does_not_consume() {
    let rt = acceptance_fault_runtime();
    let ctx = context();
    let mut old = AttemptOwner::new(
        &rt,
        LiveTunnel::connecting_control(
            ctx.connection_mode,
            RuntimeDiagnostics::new(ctx.log.clone(), &ctx.operation_id),
        ),
        &ctx.operation_id,
    );
    bind_pending(&mut old, true);
    *rt.live.lock().unwrap() = Some(old.into_live());
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert_ne!(
        error.detail,
        "engine: acceptance fault after-control-before-nic"
    );
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    for pending in rt.pending_routes.lock().unwrap().iter_mut() {
        crate::platform_tunnel::tests::ledger_allow_pending_removal(pending);
    }
    if let Some(live) = rt.live.lock().unwrap().as_mut() {
        if let Some(pending) = live.pending_route_cleanup.as_mut() {
            crate::platform_tunnel::tests::ledger_allow_pending_removal(pending);
        }
    }
    acceptance_fault_assert_next_hit(&rt);
}

#[test]
fn acceptance_fault_trace_and_packets_close_along_original_rollback() {
    let (rt, dir, _) = trace_runtime();
    rt.acceptance_fault_once.store(true, Ordering::SeqCst);
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, mut peer) = atrust(&rt, &ctx, completed.clone());
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert_eq!(
        error.detail,
        "engine: acceptance fault after-control-before-nic"
    );
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    rt.inner_runtime().block_on(async {
        assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
    });
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    assert_eq!(
        store.read(TRACE_SID).unwrap(),
        crate::connection_trace::TraceReadOutcome::Absent
    );
    let outcome = trace_rollback_outcome(&rt);
    assert_eq!(outcome.attempt_id, vec![61; 16]);
    assert!(outcome.resolved(), "{outcome:?}");
    assert!(
        outcome
            .entries
            .iter()
            .all(|e| matches!(e.domain, LedgerDomain::SessionLease | LedgerDomain::Trace))
    );
}

#[test]
fn connecting_control_atrust_cancel_awaits_real_worker() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
        .unwrap_err();
    assert_eq!(error.reason, "platform/assembly_cancelled");
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[test]
fn rollback_attempt_control_cancel_records_actual_close() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    rt.assemble_prepared(
        ctx,
        Default::default(),
        None,
        prepared,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    )
    .unwrap_err();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .entries[0]
            .state,
        crate::connect_ledger::EntryState::Compensated
    );
}

#[test]
fn rollback_attempt_registration_identity_error_is_hard_and_closes_owner() {
    for missing in [false, true] {
        let rt = runtime();
        if missing {
            rt.attempt_ledger.lock().unwrap().active = None;
        } else {
            rt.attempt_ledger
                .lock()
                .unwrap()
                .active
                .as_mut()
                .unwrap()
                .attempt_id = vec![62; 16];
        }
        let ctx = context();
        let completed = Arc::new(AtomicU64::new(0));
        let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
        let error = rt
            .assemble_prepared(
                ctx,
                Default::default(),
                None,
                prepared,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .unwrap_err();
        assert!(
            error.detail.contains(if missing {
                "active_missing"
            } else {
                "attempt_id_mismatch"
            }),
            "{}",
            error.detail
        );
        assert_eq!(error.reason, "platform/ledger_registration");
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        if !missing {
            assert!(
                rt.attempt_ledger
                    .lock()
                    .unwrap()
                    .active
                    .as_ref()
                    .unwrap()
                    .entries
                    .is_empty()
            );
        }
    }
}

#[test]
fn rollback_attempt_panic_records_close_before_observer_unwind_returns() {
    let rt = runtime();
    let ctx = context();
    ctx.status
        .set_terminal_observer(Arc::new(|_| panic!("isolated assembly observer")));
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.assemble_prepared(
                ctx,
                Default::default(),
                None,
                prepared,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
        }))
        .is_err()
    );
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .entries[0]
            .state,
        crate::connect_ledger::EntryState::Compensated
    );
    rt.resources.clear_poison();
}

fn isolated_nrpt(
    ready: &str,
    cleanup: &str,
) -> Result<Option<crate::nrpt_worker::NrptWorkerLease>, crate::nrpt_worker::NrptStartError> {
    let script = format!(
        r#"
        $null = [Console]::In.ReadLine()
        [Console]::Out.WriteLine('{ready}')
        [Console]::Out.Flush()
        while ($null -ne [Console]::In.ReadLine()) {{ }}
        [Console]::Out.WriteLine('{{"cleanup":"{cleanup}"}}')
        [Console]::Out.Flush()
    "#
    );
    crate::nrpt_worker::NrptWorkerLease::start_with_script(
        &["campus.invalid".into()],
        &["192.0.2.53".into()],
        &[],
        &script,
    )
}

fn bind_pending(owner: &mut AttemptOwner, fails: bool) {
    owner.pending_route_cleanup =
        Some(crate::platform_tunnel::tests::ledger_isolated_pending_fixture(fails));
    let sequence = owner.register(LedgerDomain::RouteFamily, false).unwrap();
    owner.attempt.as_mut().unwrap().route = Some(sequence);
}

fn bind_threads(owner: &mut AttemptOwner, threads: EngineDataPlaneThreads) {
    owner.data_plane_threads = Some(threads);
    let sequence = owner.register(LedgerDomain::DataPlane, false).unwrap();
    owner.attempt.as_mut().unwrap().data_plane = Some(sequence);
}

#[test]
fn rollback_attempt_install_prechecks_use_same_owner_and_release_each_group_once() {
    use crate::connect_ledger::EntryState;
    for cancelled in [false, true] {
        let rt = runtime();
        let ctx = context();
        let completed = Arc::new(AtomicU64::new(0));
        let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
        let PreparedConnection {
            mut control,
            packets,
            ..
        } = prepared;
        let worker = EngineDataPlaneThreads::test_gated_worker(|| {});
        if !cancelled {
            worker.activate();
            let until = Instant::now() + Duration::from_secs(2);
            while worker.is_alive() && Instant::now() < until {
                std::thread::yield_now();
            }
            assert!(!worker.is_alive());
        }
        bind_threads(&mut control, worker);
        rt.cancel_flag.store(cancelled, Ordering::SeqCst);
        rt.install_live(
            control.into_live(),
            &ctx.status,
            &ctx.stats,
            &ctx.operation_id,
        )
        .unwrap_err();
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        let slot = rt.attempt_ledger.lock().unwrap();
        assert_eq!(
            slot.active
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .map(|e| e.state.clone())
                .collect::<Vec<_>>(),
            vec![EntryState::Compensated, EntryState::Compensated]
        );
        let outcome = slot.last_rollback.as_ref().unwrap();
        assert!(outcome.resolved());
        assert_eq!(
            outcome.entries.iter().map(|e| e.domain).collect::<Vec<_>>(),
            vec![LedgerDomain::DataPlane, LedgerDomain::SessionLease]
        );
        drop(slot);
        drop(packets);
        rt.disconnect().unwrap();
        assert_eq!(completed.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn rollback_attempt_pending_route_failure_preserves_actual_obligation() {
    use crate::connect_ledger::EntryState;
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    bind_pending(&mut control, true);
    drop(control);
    drop(packets);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    let slot = rt.attempt_ledger.lock().unwrap();
    assert_eq!(
        slot.active.as_ref().unwrap().entries[0].state,
        EntryState::Compensated
    );
    assert_eq!(
        slot.active.as_ref().unwrap().entries[1].state,
        EntryState::Unresolved("pending_route_clear")
    );
    assert_eq!(
        slot.last_rollback.as_ref().unwrap().unresolved[0].0,
        LedgerDomain::RouteFamily
    );
    drop(slot);
    assert_eq!(rt.pending_routes.lock().unwrap()[0].len(), 1);
    assert!(rt.retry_pending_routes().is_err());
    assert_eq!(rt.pending_routes.lock().unwrap()[0].len(), 1);
    crate::platform_tunnel::tests::ledger_allow_pending_removal(
        &mut rt.pending_routes.lock().unwrap()[0],
    );
    rt.retry_pending_routes().unwrap();
    assert!(rt.pending_routes.lock().unwrap().is_empty());
}

#[test]
fn rollback_attempt_nrpt_failure_still_clears_route_and_reports_mixed_results() {
    use crate::connect_ledger::EntryState;
    let _ps = crate::nrpt_worker::PS_TEST_GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    bind_pending(&mut control, false);
    bind_threads(
        &mut control,
        EngineDataPlaneThreads::test_gated_worker(|| {}),
    );
    let mut lease = isolated_nrpt(r#"{"ready":false,"error":"verify"}"#, "incomplete");
    assert!(matches!(&lease, Err(error) if error.worker.is_some()));
    control.hold_nrpt_result(&mut lease).unwrap();
    drop(control);
    drop(packets);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    let slot = rt.attempt_ledger.lock().unwrap();
    let entries = &slot.active.as_ref().unwrap().entries;
    assert_eq!(
        entries.iter().map(|e| e.state.clone()).collect::<Vec<_>>(),
        vec![
            EntryState::Compensated,
            EntryState::Compensated,
            EntryState::Compensated,
            EntryState::Unresolved("nrpt_shutdown")
        ]
    );
    let outcome = slot.last_rollback.as_ref().unwrap();
    assert_eq!(
        outcome.unresolved.iter().map(|e| e.0).collect::<Vec<_>>(),
        vec![LedgerDomain::Nrpt]
    );
    assert_eq!(
        outcome.entries.iter().map(|e| e.domain).collect::<Vec<_>>(),
        vec![
            LedgerDomain::DataPlane,
            LedgerDomain::SessionLease,
            LedgerDomain::Nrpt,
            LedgerDomain::RouteFamily
        ]
    );
    drop(slot);
    assert!(rt.pending_routes.lock().unwrap().is_empty());
    let retained = rt.live.lock().unwrap().take().unwrap();
    assert!(retained.campus_dns.is_some());
    assert!(retained.pending_route_cleanup.is_none());
    drop(retained);
}

#[test]
fn rollback_attempt_each_nrpt_lease_is_bound_and_completed_first_is_not_retried() {
    use crate::connect_ledger::EntryState;
    let _ps = crate::nrpt_worker::PS_TEST_GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for first_clean in [false, true] {
        let rt = runtime();
        let ctx = context();
        let mut owner = AttemptOwner::new(
            &rt,
            LiveTunnel::connecting_control(
                ctx.connection_mode,
                RuntimeDiagnostics::new(ctx.log, &ctx.operation_id),
            ),
            &ctx.operation_id,
        );
        let mut first = isolated_nrpt(
            r#"{"ready":false,"error":"conflict"}"#,
            if first_clean {
                "complete"
            } else {
                "incomplete"
            },
        );
        owner.hold_nrpt_result(&mut first).unwrap();
        owner.release_first_nrpt();
        assert!(owner.campus_dns.is_none());
        assert_eq!(owner.retired_nrpt.len(), usize::from(!first_clean));
        let mut second = isolated_nrpt(r#"{"ready":true}"#, "complete");
        owner.hold_nrpt_result(&mut second).unwrap();
        drop(owner);
        let slot = rt.attempt_ledger.lock().unwrap();
        assert_eq!(
            slot.active
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .map(|e| e.state.clone())
                .collect::<Vec<_>>(),
            vec![
                if first_clean {
                    EntryState::Compensated
                } else {
                    EntryState::Unresolved("nrpt_shutdown")
                },
                EntryState::Compensated
            ]
        );
        let outcome = slot.last_rollback.as_ref().unwrap();
        assert_eq!(outcome.entries.len(), 2, "每次真实 lease 只有一个最终结果");
        assert_eq!(
            outcome
                .entries
                .iter()
                .filter(|e| e.sequence == Some(0))
                .count(),
            1
        );
        assert_eq!(outcome.resolved(), first_clean);
        drop(slot);
        if !first_clean {
            drop(rt.live.lock().unwrap().take());
        }
    }
}

#[test]
fn rollback_attempt_nrpt_registration_failure_holds_lease_until_same_rollback() {
    let _ps = crate::nrpt_worker::PS_TEST_GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rt = runtime();
    let ctx = context();
    rt.attempt_ledger.lock().unwrap().active = None;
    let mut owner = AttemptOwner::new(
        &rt,
        LiveTunnel::connecting_control(
            ctx.connection_mode,
            RuntimeDiagnostics::new(ctx.log, &ctx.operation_id),
        ),
        &ctx.operation_id,
    );
    let mut result = isolated_nrpt(r#"{"ready":true}"#, "complete");
    assert_eq!(
        owner.hold_nrpt_result(&mut result).unwrap_err().reason,
        "platform/ledger_registration"
    );
    assert!(owner.campus_dns.is_some());
    drop(owner);
    let slot = rt.attempt_ledger.lock().unwrap();
    let outcome = slot.last_rollback.as_ref().unwrap();
    assert!(outcome.resolved());
    assert_eq!(outcome.entries[0].sequence, None);
    assert_eq!(outcome.entries[0].domain, LedgerDomain::Nrpt);
    assert!(rt.live.lock().unwrap().is_none());
}

#[test]
fn rollback_attempt_join_failure_is_observed_without_skipping_control_close() {
    use crate::connect_ledger::EntryState;
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let worker = EngineDataPlaneThreads::test_gated_worker(|| panic!("isolated worker join error"));
    worker.activate();
    let until = Instant::now() + Duration::from_secs(2);
    while worker.is_alive() && Instant::now() < until {
        std::thread::yield_now();
    }
    assert!(!worker.is_alive());
    bind_threads(&mut control, worker);
    drop(control);
    drop(packets);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    let slot = rt.attempt_ledger.lock().unwrap();
    assert_eq!(
        slot.active.as_ref().unwrap().entries[1].state,
        EntryState::Unresolved("data_plane_stop")
    );
    assert_eq!(
        slot.last_rollback.as_ref().unwrap().unresolved[0].0,
        LedgerDomain::DataPlane
    );
}

#[test]
fn rollback_attempt_second_spawn_error_holds_first_worker_until_real_join() {
    let rt = runtime();
    let ctx = context();
    let closed = Arc::new(AtomicU64::new(0));
    let joined = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, closed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let error = EngineDataPlaneThreads::test_second_spawn_failure(joined.clone());
    assert!(error.message.contains("isolated_second_writer"));
    assert_eq!(
        joined.load(Ordering::SeqCst),
        0,
        "首线程尚未激活/停止，不能把spawn错误当清理成功"
    );
    bind_threads(&mut control, error.threads);
    drop(control);
    drop(packets);
    assert_eq!(
        joined.load(Ordering::SeqCst),
        1,
        "返回前同一组stop/join已完成"
    );
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    assert!(
        rt.attempt_ledger
            .lock()
            .unwrap()
            .last_rollback
            .as_ref()
            .unwrap()
            .resolved()
    );
    rt.disconnect().unwrap();
    assert_eq!(joined.load(Ordering::SeqCst), 1);
}

#[test]
fn connecting_control_atrust_success_transfers_owner_without_early_close() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    assert!(
        control
            .atrust
            .as_ref()
            .is_some_and(|a| a.owner.is_running()),
        "数据面组装前控制 owner 必须已经有清理归属"
    );
    control.data_plane_threads = Some(EngineDataPlaneThreads::test_gated_worker(|| {}));
    rt.install_live(
        control.into_live(),
        &ctx.status,
        &ctx.stats,
        &ctx.operation_id,
    )
    .unwrap();
    assert_eq!(
        completed.load(Ordering::SeqCst),
        0,
        "成功移交不得补偿正在使用的协议"
    );
    rt.disconnect().unwrap();
    rt.disconnect().unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    drop(packets);
}

// 复用 Common CSTP 测试的合成证书材料；仅 localhost TLS，没有学校凭据。
const CERT: &str = concat!(
    "MIIBqTCCAU6gAwIBAgIUCz2E6EXN/6S3gIxuRBEAK/doSI8wCgYIKoZIzj0EAwIw",
    "GzEZMBcGA1UEAwwQdnBuLmV4YW1wbGUudGVzdDAeFw0yNjA4MTIyMDQxNTBaFw0z",
    "NjA4MDkyMDQxNTBaMBsxGTAXBgNVBAMMEHZwbi5leGFtcGxlLnRlc3QwWTATBgcq",
    "hkjOPQIBBggqhkjOPQMBBwNCAAQ621WzgT+Rp6MR+st4LY1gxps9HzBSlIIFbbHW",
    "M3PjUe5vJXzSuEaBQ+t6kBsHEc9FoX5oPA6ivQ7eryJYMrL4o3AwbjAdBgNVHQ4E",
    "FgQU0IZcqxbwuE5X5GGdjRl/4XLJEqkwHwYDVR0jBBgwFoAU0IZcqxbwuE5X5GGd",
    "jRl/4XLJEqkwDwYDVR0TAQH/BAUwAwEB/zAbBgNVHREEFDASghB2cG4uZXhhbXBs",
    "ZS50ZXN0MAoGCCqGSM49BAMCA0kAMEYCIQDQ+3+bij4pa2x1Z5ErL29zX1aXHRE/",
    "8j8r2YsJ2fkeegIhAOBH6bDzde1JgeRR/IuiTm29PrNb3DbM7ksLb5mNTzUt"
);
const KEY: &str = concat!(
    "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgn1Ra5jnCNjCWeCfF",
    "x0h7BnT9tvnlrqAdc+0xZphBfZWhRANCAAQ621WzgT+Rp6MR+st4LY1gxps9HzBS",
    "lIIFbbHWM3PjUe5vJXzSuEaBQ+t6kBsHEc9FoX5oPA6ivQ7eryJYMrL4"
);

async fn cisco_pair() -> (CstpSession, tokio::sync::oneshot::Receiver<()>) {
    use base64::Engine;
    let cert = rustls::pki_types::CertificateDer::from(
        base64::engine::general_purpose::STANDARD
            .decode(CERT)
            .unwrap(),
    );
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        base64::engine::general_purpose::STANDARD
            .decode(KEY)
            .unwrap(),
    ));
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.clone()).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = listener.local_addr().unwrap();
    let (ended_tx, ended_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(3), async {
        // 真 WebVPN 登录解析取得合成 session cookie，再打开独立 CSTP TLS。
        let (login_tcp, _) = listener.accept().await.unwrap();
        let mut login_tls = acceptor.accept(login_tcp).await.unwrap();
        let mut header = vec![];
        while !header.ends_with(b"\r\n\r\n") { header.push(login_tls.read_u8().await.unwrap()); }
        let body_len: usize = String::from_utf8_lossy(&header).lines()
            .find_map(|line| line.split_once(':').filter(|(key,_)| key.eq_ignore_ascii_case("Content-Length")).map(|(_,value)| value.trim().parse().unwrap())).unwrap();
        let mut body = vec![0; body_len]; login_tls.read_exact(&mut body).await.unwrap();
        let response = "<config-auth client=\"vpn\" type=\"auth-request\" aggregate-auth-version=\"2\"><auth id=\"success\"><session-token>synthetic-control-stage</session-token></auth></config-auth>";
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\n\r\n{}", response.len(), response);
        login_tls.write_all(reply.as_bytes()).await.unwrap(); login_tls.flush().await.unwrap();
        drop(login_tls);
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(tcp).await.unwrap();
        let mut request = vec![];
        while !request.ends_with(b"\r\n\r\n") {
            request.push(tls.read_u8().await.unwrap());
        }
        tls.write_all(b"HTTP/1.1 200 CONNECTED\r\nX-CSTP-Address: 10.0.0.1\r\nX-CSTP-Netmask: 255.255.255.0\r\nX-CSTP-MTU: 1400\r\n\r\n").await.unwrap();
        tls.flush().await.unwrap();
        let mut byte = [0];
        match tls.read(&mut byte).await {
            Ok(0) => (),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => (),
            other => panic!("控制通道应以 EOF 结束：{other:?}"),
        }
        let _ = ended_tx.send(());
        }).await.expect("局部 TLS 服务有界退出");
    });
    let trust = TrustPolicy::TestRoots(Arc::new(roots));
    let login = WebvpnLogin::perform_login(
        "vpn.example.test",
        gateway_addr,
        trust.clone(),
        None,
        b"synthetic",
        b"synthetic",
    )
    .await
    .unwrap();
    let session = CstpSession::open(
        BootstrapConfig {
            hostname: "vpn.example.test".into(),
            gateway_addr,
            trust,
            dtls_offered: false,
            deadline: None,
            socket_binder: None,
            gateway_resolver: None,
        },
        Some(&login),
    )
    .await
    .unwrap();
    (session, ended_rx)
}

#[test]
fn connecting_control_cisco_nic_failure_ends_open_tls_session() {
    let rt = runtime();
    let ctx = context();
    let (session, ended) = rt.inner_runtime().block_on(cisco_pair());
    let prepared = rt.hold_cisco_control(
        session,
        &ctx.operation_id,
        ctx.connection_mode,
        RuntimeDiagnostics::new(ctx.log.clone(), &ctx.operation_id),
    );
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    assert!(error.detail.contains("preflight NIC changed"));
    rt.inner_runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(2), ended)
            .await
            .unwrap()
            .unwrap();
    });
    rt.disconnect().unwrap();
}

#[test]
fn connecting_control_cisco_success_transfers_owner_until_disconnect() {
    let rt = runtime();
    let ctx = context();
    let (session, mut ended) = rt.inner_runtime().block_on(cisco_pair());
    let prepared = rt.hold_cisco_control(
        session,
        &ctx.operation_id,
        ctx.connection_mode,
        RuntimeDiagnostics::new(ctx.log.clone(), &ctx.operation_id),
    );
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    drop(packets);
    rt.inner_runtime().block_on(async {
        assert!(
            tokio::time::timeout(Duration::from_millis(60), &mut ended)
                .await
                .is_err(),
            "数据通道移交后仍由控制 owner 保持 TLS"
        );
    });
    control.data_plane_threads = Some(EngineDataPlaneThreads::test_gated_worker(|| {}));
    rt.install_live(
        control.into_live(),
        &ctx.status,
        &ctx.stats,
        &ctx.operation_id,
    )
    .unwrap();
    rt.disconnect().unwrap();
    rt.disconnect().unwrap();
    rt.inner_runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(2), ended)
            .await
            .unwrap()
            .unwrap();
    });
}

// 独立复审发现的并发回归：disconnect 必须等待装配回滚及义务移交。
// 真实 OS 线程的 join + 实际 aTrust graceful close + 隔离 route leaf；不创建原生网卡。
#[test]
fn rollback_attempt_resource_lock_covers_pending_cleanup() {
    let rt = runtime();
    let ctx = context();
    let closed = Arc::new(AtomicU64::new(0));
    let joined = Arc::new(AtomicU64::new(0));
    let (mut prepared, _peer) = atrust(&rt, &ctx, closed.clone());
    let (worker_entered_tx, worker_entered_rx) = std::sync::mpsc::channel();
    let (worker_release_tx, worker_release_rx) = std::sync::mpsc::channel();
    let joined_worker = joined.clone();
    let worker = EngineDataPlaneThreads::test_gated_worker(move || {
        worker_entered_tx.send(()).unwrap();
        // 装配占用直到本线程结束及义务移交才释放；有界 timeout 保证测试本身不死锁。
        let _ = worker_release_rx.recv_timeout(Duration::from_secs(2));
        joined_worker.store(1, Ordering::SeqCst);
    });
    worker.activate();
    worker_entered_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    bind_threads(&mut prepared.control, worker);
    bind_pending(&mut prepared.control, true);

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let started_tx = Mutex::new(Some(started_tx));
    ctx.status.set_terminal_observer(Arc::new(move |_| {
        if let Some(tx) = started_tx.lock().unwrap().take() {
            tx.send(()).unwrap();
        }
    }));
    let disconnect_rt = rt.clone();
    let disconnect_joined = joined.clone();
    let disconnect_closed = closed.clone();
    let disconnect = std::thread::spawn(move || {
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let result = disconnect_rt.disconnect();
        let joined_at_return = disconnect_joined.load(Ordering::SeqCst);
        let closed_at_return = disconnect_closed.load(Ordering::SeqCst);
        let pending_at_return = disconnect_rt.pending_routes.lock().unwrap().len();
        let _ = worker_release_tx.send(());
        (
            result,
            joined_at_return,
            closed_at_return,
            pending_at_return,
        )
    });
    rt.assembling.store(true, Ordering::SeqCst);
    let completion = lifecycle::AssemblyCompletion(&rt);
    let error = rt
        .assemble_prepared(
            ctx,
            Default::default(),
            None,
            prepared,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap_err();
    drop(completion);
    let observed = disconnect.join().unwrap();
    assert!(error.detail.contains("preflight NIC changed"));
    let pending_after_rollback = rt.pending_routes.lock().unwrap().len();
    eprintln!(
        "review observation: disconnect={:?}, joined_at_return={}, closed_at_return={}, pending_at_return={}, pending_after_rollback={}",
        observed.0, observed.1, observed.2, observed.3, pending_after_rollback
    );
    // 先完成隔离义务收尾，避免红测断言截断测试自身清理。
    for pending in rt.pending_routes.lock().unwrap().iter_mut() {
        crate::platform_tunnel::tests::ledger_allow_pending_removal(pending);
    }
    rt.disconnect().unwrap();
    assert_eq!(joined.load(Ordering::SeqCst), 1);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    assert!(
        observed.0.is_err(),
        "回滚路由义务仍未清完，disconnect 不得先返回 Ok: {observed:?}"
    );
    assert_eq!(observed.1, 1, "disconnect 返回前真实线程应已 join");
    assert_eq!(observed.2, 1, "disconnect 返回前真实协议 close 应已完成");
    assert_eq!(observed.3, 1, "disconnect 应已观察同一份未竟路由义务");
}

#[test]
fn rollback_trace_precancel_before_previous_cleanup_preserves_actual_trace() {
    let (rt, dir, _) = trace_runtime();
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    let crate::connection_trace::TraceReadOutcome::Present(before) = store.read(TRACE_SID).unwrap()
    else {
        panic!("实际 trace 必须存在")
    };
    // 使用原 R1 的现存 UUID 来源；直接调用生产 assembly 入口固定最早取消时刻。
    rt.attempt_ledger.lock().unwrap().active = None;
    let actual_id = rt.current_trace_id(&LogSink::null()).unwrap();
    rt.attempt_ledger.lock().unwrap().begin(
        &[61; 16],
        TraceAdmission::Existing {
            connection_id: actual_id.clone(),
        },
    );
    rt.pending_routes
        .lock()
        .unwrap()
        .push(crate::platform_tunnel::tests::ledger_isolated_pending_fixture(true));
    rt.assembling.store(true, Ordering::SeqCst);
    rt.cancel_flag.store(true, Ordering::SeqCst);
    let ctx = context();
    let status = ctx.status.clone();
    let stats = ctx.stats.clone();
    let mut logs = ctx.log.open_stream(0);
    RealTunnelRuntime::run_assemble_with_reset(
        &rt,
        ctx,
        rt.cancel_flag.clone(),
        &stats,
        &status,
        &[61; 16],
    );
    let outcome = trace_rollback_outcome(&rt);
    let trace = store.read(TRACE_SID).unwrap();
    let remaining = rt
        .pending_routes
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.len())
        .sum::<usize>();
    let mut details = vec![];
    while let Ok(event) = logs.try_recv() {
        details.push(event.unwrap());
    }
    // 所有断言前先释放 fixture 的实际 pending 义务，系统路由 leaf 未接线。
    for pending in rt.pending_routes.lock().unwrap().iter_mut() {
        crate::platform_tunnel::tests::ledger_allow_pending_removal(pending);
    }
    rt.retry_pending_routes().unwrap();
    assert!(rt.pending_routes.lock().unwrap().is_empty());
    println!(
        "actual_uuid={actual_id}; remaining_before_fixture_cleanup={remaining}; trace_after_cancel={trace:?}; outcome={outcome:?}; logs={details:?}"
    );
    assert_eq!(actual_id, before.connection_id);
    assert_eq!(remaining, 1);
    assert_eq!(
        status.latest().unwrap().error.unwrap().reason,
        "platform/assembly_cancelled"
    );
    assert!(!rt.is_assembling());
    let crate::connection_trace::TraceReadOutcome::Present(after) = trace else {
        panic!("取消发生在旧义务清理之前，不得将空新 owner 回滚视为旧资源已干净并删痕");
    };
    assert_eq!(after.connection_id, before.connection_id);
    assert!(
        !details
            .iter()
            .any(|e| e.code == "tunnel.rollback.completed")
    );
    assert_eq!(after.rollback_unresolved, vec!["route_family"]);
    assert_eq!(outcome.unresolved_domains(), vec!["route_family"]);
    let route = outcome
        .entries
        .iter()
        .find(|e| e.domain == LedgerDomain::RouteFamily)
        .unwrap();
    assert_eq!(route.sequence, None, "旧义务不是本轮新取得");
    assert!(matches!(
        route.state,
        crate::connect_ledger::EntryState::Unresolved(_)
    ));
    assert!(
        outcome
            .entries
            .iter()
            .any(|e| e.domain == LedgerDomain::Trace
                && e.state == crate::connect_ledger::EntryState::Retained)
    );
    assert!(
        details
            .iter()
            .any(|e| e.code == "tunnel.rollback.unresolved")
    );
}

#[test]
fn rollback_trace_precancel_empty_resources_closes_actual_trace() {
    let (rt, dir, _) = trace_runtime();
    let ctx = context();
    let stats = ctx.stats.clone();
    let status = ctx.status.clone();
    RealTunnelRuntime::run_assemble_with_reset(
        &rt,
        ctx,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
        &stats,
        &status,
        &[61; 16],
    );
    let outcome = trace_rollback_outcome(&rt);
    assert!(outcome.resolved(), "{outcome:?}");
    assert_eq!(outcome.entries.len(), 1, "真正空资源不能臆造未竟域");
    assert_eq!(outcome.entries[0].domain, LedgerDomain::Trace);
    assert_eq!(
        status.latest().unwrap().error.unwrap().reason,
        "platform/assembly_cancelled"
    );
    assert_eq!(
        crate::connection_trace::ConnectionTraceStore::open(dir.path())
            .unwrap()
            .read(TRACE_SID)
            .unwrap(),
        crate::connection_trace::TraceReadOutcome::Absent
    );
}

#[test]
fn rollback_trace_precancel_unassociated_keeps_file_and_pending_facts() {
    for trace in [
        TraceAdmission::Unwired,
        TraceAdmission::Skipped {
            connection_id: None,
        },
    ] {
        let (rt, dir, _) = trace_runtime();
        let path = dir.path().join(format!("trace-{TRACE_SID}.json"));
        let before = std::fs::read(&path).unwrap();
        rt.attempt_ledger.lock().unwrap().active = None;
        rt.attempt_ledger.lock().unwrap().begin(&[61; 16], trace);
        rt.pending_routes
            .lock()
            .unwrap()
            .push(crate::platform_tunnel::tests::ledger_isolated_pending_fixture(false));
        let ctx = context();
        let stats = ctx.stats.clone();
        let status = ctx.status.clone();
        RealTunnelRuntime::run_assemble_with_reset(
            &rt,
            ctx,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            &stats,
            &status,
            &[61; 16],
        );
        let remaining = rt
            .pending_routes
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.len())
            .sum::<usize>();
        let outcome = trace_rollback_outcome(&rt);
        rt.retry_pending_routes().unwrap();
        assert_eq!(remaining, 1, "取消不擅自重试旧义务，即使该 leaf 能成功");
        assert_eq!(
            std::fs::read(path).unwrap(),
            before,
            "无关联不借当前 UUID 写删"
        );
        assert_eq!(outcome.unresolved_domains(), vec!["route_family"]);
        assert!(
            !outcome
                .entries
                .iter()
                .any(|e| e.domain == LedgerDomain::Trace)
        );
        assert!(outcome.entries.iter().all(|e| e.sequence.is_none()));
        assert_eq!(
            status.latest().unwrap().error.unwrap().reason,
            "platform/assembly_cancelled"
        );
    }
}

#[test]
fn rollback_trace_precancel_keeps_skipped_some_live_owner_without_close() {
    let (rt, dir, _) = trace_runtime();
    let store = crate::connection_trace::ConnectionTraceStore::open(dir.path()).unwrap();
    let crate::connection_trace::TraceReadOutcome::Present(before) = store.read(TRACE_SID).unwrap()
    else {
        panic!("实际 trace")
    };
    let closed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &context(), closed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let joined = Arc::new(AtomicU64::new(0));
    // 未激活线程也必须真正退出；callback 被取消时，其捕获仍在线程末尾析构。
    struct WorkerEnd(Arc<AtomicU64>);
    impl Drop for WorkerEnd {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let worker_end = WorkerEnd(joined.clone());
    bind_threads(
        &mut control,
        EngineDataPlaneThreads::test_gated_worker(move || drop(worker_end)),
    );
    *rt.live.lock().unwrap() = Some(control.into_live());
    rt.attempt_ledger.lock().unwrap().active = None;
    rt.attempt_ledger.lock().unwrap().begin(
        &[61; 16],
        TraceAdmission::Skipped {
            connection_id: Some(before.connection_id.clone()),
        },
    );
    let ctx = context();
    let stats = ctx.stats.clone();
    let status = ctx.status.clone();
    RealTunnelRuntime::run_assemble_with_reset(
        &rt,
        ctx,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
        &stats,
        &status,
        &[61; 16],
    );
    let held_before_cleanup = rt
        .live
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|l| l.atrust.is_some());
    let closed_before_cleanup = closed.load(Ordering::SeqCst);
    let joined_before_cleanup = joined.load(Ordering::SeqCst);
    let outcome = trace_rollback_outcome(&rt);
    let trace = store.read(TRACE_SID).unwrap();
    rt.disconnect().unwrap();
    drop(packets);
    assert!(held_before_cleanup, "取消不取走旧协议 owner");
    assert_eq!(closed_before_cleanup, 0, "取消不关闭旧协议");
    assert_eq!(joined_before_cleanup, 0, "取消不停止旧线程组");
    assert_eq!(
        closed.load(Ordering::SeqCst),
        1,
        "原 disconnect 仍是唯一 cleaner"
    );
    assert_eq!(joined.load(Ordering::SeqCst), 1, "原线程组只 join 一次");
    assert_eq!(
        status.latest().unwrap().error.unwrap().reason,
        "platform/assembly_cancelled"
    );
    let crate::connection_trace::TraceReadOutcome::Present(after) = trace else {
        panic!("真实旧 live 仍持有，不得删除同 UUID：{outcome:?}")
    };
    assert_eq!(after.connection_id, before.connection_id);
    assert_eq!(
        after.rollback_unresolved,
        vec!["data_plane", "session_lease"]
    );
    assert_eq!(
        outcome.unresolved_domains(),
        vec!["data_plane", "session_lease"]
    );
    let session = outcome
        .entries
        .iter()
        .find(|e| e.domain == LedgerDomain::SessionLease)
        .unwrap();
    assert_eq!(session.sequence, None);
    assert!(matches!(
        session.state,
        crate::connect_ledger::EntryState::Unresolved(_)
    ));
}

#[test]
fn stop_report_preserves_join_error() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let (entered, observed) = std::sync::mpsc::channel();
    let worker = EngineDataPlaneThreads::test_gated_worker(move || {
        entered.send(()).unwrap();
        panic!("isolated stop join error");
    });
    worker.activate();
    observed
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    bind_threads(&mut control, worker);
    let (report, _) = control.disconnect_for_report("test_stop");
    assert_eq!(completed.load(Ordering::SeqCst), 1, "独立协议清理必须继续");
    assert!(report.result().is_err(), "线程异常已结束也必须保留动作错误");
    assert!(report.stopped);
    assert!(!report.succeeded());
    assert!(!report.blocks_new_connect(), "历史任务异常不是活线程义务");
    assert!(report.remaining.is_empty());
    assert!(
        report
            .actions
            .iter()
            .any(|a| a.step == "data_plane_stop" && a.error.as_deref() == Some("reader_join"))
    );
    drop(packets);
}

#[test]
fn normal_protocol_end_is_not_pending() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let (report, _) = control.disconnect_for_report("test_normal_stop");
    assert!(report.stopped);
    assert!(report.succeeded());
    assert!(!report.blocks_new_connect());
    assert!(report.remaining.is_empty());
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    drop(packets);
}

#[test]
fn protocol_eof_completes_close_without_live_task_obligation() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    drop(peer);
    let until = Instant::now() + Duration::from_secs(2);
    while control.atrust.as_ref().unwrap().owner.is_running() && Instant::now() < until {
        std::thread::yield_now();
    }
    let (report, _) = control.disconnect_for_report("test_eof_stop");
    assert!(report.stopped);
    assert!(
        report.succeeded(),
        "正常EOF已终结，不能成为本次关闭动作失败"
    );
    assert!(!report.blocks_new_connect());
    assert!(report.remaining.is_empty());
    assert!(
        report.actions.iter().any(|a| a.step == "protocol_close"
            && a.state == crate::stop_report::StopActionState::Completed)
    );
    drop(packets);
}

#[test]
fn protocol_host_closed_completes_close_without_pending() {
    let rt = runtime();
    let ctx = context();
    let completed = Arc::new(AtomicU64::new(0));
    let (prepared, _peer) = atrust(&rt, &ctx, completed.clone());
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let PreparedPackets::Atrust { channels } = packets else {
        panic!("aTrust channels")
    };
    let crate::atrust_tunnel::RawPacketChannels {
        outgoing: _outgoing,
        incoming,
    } = channels;
    drop(incoming);
    let until = Instant::now() + Duration::from_secs(2);
    while completed.load(Ordering::SeqCst) == 0 && Instant::now() < until {
        std::thread::yield_now();
    }
    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "宿主队列关闭必须先取得真实worker结束事实"
    );
    let (report, _) = control.disconnect_for_report("test_host_closed_stop");
    assert!(
        report.stopped && report.succeeded(),
        "正常宿主队列终结不使关闭失败"
    );
    assert!(report.remaining.is_empty() && !report.blocks_new_connect());
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[test]
fn stop_cleanup_rejects_new_apply_and_releases_resource_lock() {
    let rt = runtime();
    let ctx = context();
    let (prepared, _peer) = atrust(&rt, &ctx, Arc::new(AtomicU64::new(0)));
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = EngineDataPlaneThreads::test_gated_worker(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(4)).unwrap();
    });
    worker.activate();
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    bind_threads(&mut control, worker);
    *rt.live.lock().unwrap() = Some(control.into_live());
    let stopped_rt = rt.clone();
    let stopped = std::thread::spawn(move || stopped_rt.disconnect_report());
    let deadline = Instant::now() + Duration::from_secs(2);
    while rt.live.lock().unwrap().is_some() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(rt.live.lock().unwrap().is_none(), "真实停止已取走原 owner");
    let resource_available = rt.resources.try_lock().is_ok();
    let admission = rt.clone().start_apply(context());
    let cancelled = rt.cancel_flag.load(Ordering::SeqCst);
    release_tx.send(()).unwrap();
    let report = stopped.join().unwrap();
    assert!(resource_available, "必要 join 不得持有 resources 锁");
    assert!(admission.is_err(), "清理占用期间新 Apply 不得被受理");
    assert!(cancelled, "新 Apply 不得重置正在停止的取消标记");
    assert!(report.stopped);
    drop(packets);
}

#[test]
fn stop_waits_for_assembly_completion_event() {
    let rt = runtime();
    rt.assembling.store(true, Ordering::SeqCst);
    let stopped_rt = rt.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let stopped = std::thread::spawn(move || {
        done_tx.send(stopped_rt.disconnect_report()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !rt.cancel_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let early = done_rx.recv_timeout(Duration::from_millis(30)).ok();
    let ctx = context();
    RealTunnelRuntime::run_assemble_with_reset(
        &rt,
        ctx,
        rt.cancel_flag.clone(),
        &Arc::new(crate::stats::StatsPublisher::new()),
        &Arc::new(StatusPublisher::new()),
        &[61; 16],
    );
    let report = early
        .clone()
        .unwrap_or_else(|| done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
    stopped.join().unwrap();
    assert!(early.is_none(), "装配还未收口，停止必须等待完成事件");
    assert!(report.stopped);
}

#[test]
fn assembly_panic_notifies_stop_even_when_failure_observer_panics() {
    let rt = runtime();
    rt.assembling.store(true, Ordering::SeqCst);
    let ctx = context();
    let status = ctx.status.clone();
    let stats = ctx.stats.clone();
    status.set_terminal_observer(Arc::new(|_| panic!("isolated terminal observer panic")));
    let waiter_rt = rt.clone();
    let waiter = std::thread::spawn(move || {
        waiter_rt
            .begin_cleanup(false, Duration::from_millis(300))
            .is_some()
    });
    let until = Instant::now() + Duration::from_secs(2);
    while rt.lifecycle_commit.lock().unwrap().stopping == 0 && Instant::now() < until {
        std::thread::yield_now();
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RealTunnelRuntime::run_assemble_with_reset(
            &rt,
            ctx,
            rt.cancel_flag.clone(),
            &stats,
            &status,
            &[61; 16],
        );
    }));
    let notified = waiter.join().unwrap();
    assert!(result.is_err(), "真实发布观察者异常必须被夹具捕获");
    assert!(
        notified && !rt.is_assembling(),
        "panic 出口也必须释放装配占用并通知等待者"
    );
}

#[test]
fn stop_during_assembly_prevents_late_install() {
    let rt = runtime();
    let ctx = context();
    let (prepared, _peer) = atrust(&rt, &ctx, Arc::new(AtomicU64::new(0)));
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    bind_threads(
        &mut control,
        EngineDataPlaneThreads::test_gated_worker(|| {}),
    );
    rt.assembling.store(true, Ordering::SeqCst);
    let completion = lifecycle::AssemblyCompletion(&rt);
    let report = rt.disconnect_lifecycle_with_timeout(Duration::from_millis(10));
    assert!(!report.stopped && report.blocks_new_connect());
    assert!(!rt.is_paused(), "超时不能假称暂停完成");
    assert!(rt.clone().start_apply(context()).is_err());
    let late = rt.install_live(
        control.into_live(),
        &ctx.status,
        &ctx.stats,
        &ctx.operation_id,
    );
    assert!(late.is_err());
    assert!(rt.live.lock().unwrap().is_none());
    assert!(ctx.status.latest().is_none());
    drop(completion);
    assert!(rt.disconnect_report().stopped);
    drop(packets);
}

#[test]
fn successful_install_completion_notifies_all_stop_waiters() {
    let rt = runtime();
    let ctx = context();
    let (prepared, _peer) = atrust(&rt, &ctx, Arc::new(AtomicU64::new(0)));
    let PreparedConnection {
        mut control,
        packets,
        ..
    } = prepared;
    bind_threads(
        &mut control,
        EngineDataPlaneThreads::test_gated_worker(|| {}),
    );
    rt.assembling.store(true, Ordering::SeqCst);
    let completion = lifecycle::AssemblyCompletion(&rt);
    rt.install_live(
        control.into_live(),
        &ctx.status,
        &ctx.stats,
        &ctx.operation_id,
    )
    .unwrap();
    assert_eq!(
        ctx.status.latest().unwrap().coarse_phase,
        generated::StatsPhase::Connected
    );
    let mut waiters = Vec::new();
    for _ in 0..2 {
        let rt = rt.clone();
        waiters.push(std::thread::spawn(move || rt.disconnect_report()));
    }
    let until = Instant::now() + Duration::from_secs(2);
    while rt.lifecycle_commit.lock().unwrap().stopping != 2 && Instant::now() < until {
        std::thread::yield_now();
    }
    drop(completion);
    for waiter in waiters {
        assert!(waiter.join().unwrap().stopped);
    }
    assert!(rt.is_paused());
    drop(packets);
}
