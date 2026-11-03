// 此文件包含在既有 tests 模块内，共用真实服务路径及 FakeEngine，不另建状态机。
struct ObservedStatusStream {
    rx: tokio::sync::mpsc::UnboundedReceiver<EngineStatusEvent>,
    polled: Option<Arc<tokio::sync::Notify>>,
}

impl tokio_stream::Stream for ObservedStatusStream {
    type Item = EngineStatusEvent;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let result = self.rx.poll_recv(cx);
        if result.is_ready()
            && let Some(polled) = &self.polled {
                polled.notify_one();
            }
        result
    }
}

mod handoff_regression {
    use super::*;

    fn request() -> Request<ConnectRequest> {
        Request::new(ConnectRequest {
            auth_binding: None,
            intent: Some(connect_intent()),
            secret_payload: vec![],
        })
    }

    async fn cancel(service: &KernelControlService) {
        tokio::time::timeout(
            Duration::from_secs(1),
            service.stop(Request::new(StopRequest {
                intent: Some(stop_intent()),
            })),
        )
        .await
        .expect("取消必须有界")
        .expect("取消成功");
        assert_eq!(service.composition.lock().await.phase(), HostPhase::Idle);
    }

    async fn routed(service: &KernelControlService) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if service
                    .connect_dispatch
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|state| state.target_generation.is_some())
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("路由完成");
    }

    fn attach(service: &KernelControlService, generation: u64) {
        service.status_attachment.send_replace(StatusAttachment {
            generation,
            attached: true,
        });
    }

    #[tokio::test]
    async fn gate_requires_exact_generation_and_attachment() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with(&engine);
        let service = service.with_status_ready_wait(Duration::from_millis(15));
        for attachment in [
            StatusAttachment {
                generation: 0,
                attached: true,
            },
            StatusAttachment {
                generation: 1,
                attached: false,
            },
            StatusAttachment {
                generation: 2,
                attached: true,
            },
        ] {
            service.status_attachment.send_replace(attachment);
            let error = service
                .await_status_ready(1)
                .await
                .expect_err("非同代就绪不得放行");
            assert!(error.message().starts_with("status_stream_not_ready|"));
        }
        attach(&service, 1);
        service.await_status_ready(1).await.expect("同代就绪通过");
    }

    #[tokio::test]
    async fn gate_timeout_never_applies_or_repairs_and_retry_is_admitted() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let ops = Arc::new(FakeServiceOps::default());
        let service = service
            .with_service_ops(ops.clone())
            .with_status_ready_wait(Duration::from_millis(15));
        let error = service.connect(request()).await.expect_err("订阅缺失");
        assert!(error.message().starts_with("status_stream_not_ready|"));
        assert!(engine.lock().await.applies.lock().unwrap().is_empty());
        assert_eq!(ops.installs.load(Ordering::Relaxed), 0);
        assert_eq!(ops.starts.load(Ordering::Relaxed), 0);
        attach(&service, 0);
        service.connect(request()).await.expect("失败后可立即重试");
        assert_eq!(engine.lock().await.applies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_during_gate_reopens_admission_without_engine_stop() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service);
        let worker = tokio::spawn({
            let service = service.clone();
            async move { service.connect(request()).await }
        });
        routed(&service).await;
        cancel(&service).await;
        assert_eq!(worker.await.unwrap().unwrap_err().code(), Code::Cancelled);
        assert!(engine.lock().await.applies.lock().unwrap().is_empty());
        assert!(engine.lock().await.stops.lock().unwrap().is_empty());
        attach(&service, 0);
        service.connect(request()).await.expect("取消后再次连接");
    }

    #[tokio::test]
    async fn cancel_during_engine_lock_wait_does_not_wait_for_engine() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service);
        attach(&service, 0);
        let guard = engine.lock().await;
        let worker = tokio::spawn({
            let service = service.clone();
            async move { service.connect(request()).await }
        });
        routed(&service).await;
        cancel(&service).await;
        assert_eq!(worker.await.unwrap().unwrap_err().code(), Code::Cancelled);
        assert!(guard.applies.lock().unwrap().is_empty());
        assert!(guard.stops.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancel_during_owner_lease_wait_drops_preparation() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        engine
            .lock()
            .await
            .lease_entered
            .lock()
            .unwrap()
            .replace(entered.clone());
        engine
            .lock()
            .await
            .lease_block
            .lock()
            .unwrap()
            .replace(release);
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service);
        attach(&service, 0);
        let worker = tokio::spawn({
            let service = service.clone();
            async move { service.connect(request()).await }
        });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("进入 lease");
        cancel(&service).await;
        assert_eq!(worker.await.unwrap().unwrap_err().code(), Code::Cancelled);
        assert!(engine.lock().await.applies.lock().unwrap().is_empty());
        assert!(engine.lock().await.stops.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn attachment_revoked_during_lease_prevents_final_apply() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        engine
            .lock()
            .await
            .lease_entered
            .lock()
            .unwrap()
            .replace(entered.clone());
        engine
            .lock()
            .await
            .lease_block
            .lock()
            .unwrap()
            .replace(release.clone());
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service);
        attach(&service, 0);
        let worker = tokio::spawn({
            let service = service.clone();
            async move { service.connect(request()).await }
        });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        StatusAttachment::revoke(&service.status_attachment, 0);
        release.notify_one();
        assert!(
            worker
                .await
                .unwrap()
                .unwrap_err()
                .message()
                .starts_with("status_stream_not_ready|")
        );
        assert!(engine.lock().await.applies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancel_after_apply_arbitration_preserves_real_stop_obligation() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        engine
            .lock()
            .await
            .apply_obs
            .lock()
            .unwrap()
            .replace(applied.clone());
        engine
            .lock()
            .await
            .apply_block
            .lock()
            .unwrap()
            .replace(release.clone());
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service);
        attach(&service, 0);
        let connect = tokio::spawn({
            let service = service.clone();
            async move { service.connect(request()).await }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while applied.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let stop = tokio::spawn({
            let service = service.clone();
            async move { cancel(&service).await }
        });
        tokio::task::yield_now().await;
        assert!(
            !stop.is_finished(),
            "真实 Apply 在途时 Stop 必须保留清理义务"
        );
        release.notify_one();
        connect.await.unwrap().expect("Apply 完成");
        stop.await.unwrap();
        assert_eq!(engine.lock().await.stops.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_forwarder_is_rejected_and_stale_revoke_cannot_clear_new_attachment() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with(&engine);
        let owner = spawn_status_ready(&service, &engine).await;
        tokio::time::timeout(Duration::from_secs(1), service.spawn_status_forwarder())
            .await
            .unwrap()
            .unwrap();
        assert!(service.status_ready().borrow().attached);
        attach(&service, 1);
        StatusAttachment::revoke(&service.status_attachment, 0);
        assert_eq!(
            *service.status_ready().borrow(),
            StatusAttachment {
                generation: 1,
                attached: true
            }
        );
        owner.abort();
    }

    #[tokio::test]
    async fn queued_old_attempt_failure_and_local_stop_cannot_overwrite_new_attempt() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service.with_status_ready_wait(Duration::from_millis(10)));
        let _ = service.connect(request()).await;
        let old = service
            .connect_dispatch
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .id;
        cancel(&service).await;
        attach(&service, 0);
        service.connect(request()).await.unwrap();
        let before = service.events().current_snapshot().unwrap();
        service
            .apply_connect_failed_for_route(old, &Status::unavailable("late old failure"))
            .await;
        service.finish_local_stop(Some(old)).await;
        assert_eq!(service.events().current_snapshot().unwrap(), before);
    }

    // FakeEngine 的通知确认旧流已产出事件/EOF/错误；composition 锁精准停住发布路径。
    // 换代后才释放锁，验证三条生产投影路径均在取得锁后再次核对 generation。
    #[tokio::test]
    async fn old_generation_progress_attach_failure_and_eof_cannot_publish_after_swap() {
        for kind in ["progress", "attach_failure", "eof"] {
            let observed = Arc::new(tokio::sync::Notify::new());
            let engine = Arc::new(Mutex::new(FakeEngine::default()));
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            if kind == "attach_failure" {
                engine.lock().await.status_attach_entered = Some(observed.clone());
            } else {
                engine.lock().await.status_polled = Some(observed.clone());
                engine.lock().await.events.lock().unwrap().replace(rx);
            }
            let (service, _logs) = service_with(&engine);
            let mut composition = service.composition.lock().await;
            composition.apply(HostEvent::Connect);
            composition.set_operation_id(uuid16_bytes(3));
            let forwarder = service.spawn_status_forwarder();
            if kind != "attach_failure" {
                service.await_status_ready(0).await.unwrap();
            }
            if kind == "progress" {
                tx.send(EngineStatusEvent::Progress { reconnect_state: None,
                    operation_id: uuid16(3),
                    connect_phase: wire::ConnectPhase::ApplyingPlatformTunnel,
                })
                .unwrap();
            }
            if kind == "eof" {
                drop(tx);
            }
            tokio::time::timeout(Duration::from_secs(1), observed.notified())
                .await
                .unwrap();
            let replacement = Arc::new(Mutex::new(FakeEngine::default()));
            let (_new_tx, new_rx) = tokio::sync::mpsc::unbounded_channel();
            replacement
                .lock()
                .await
                .events
                .lock()
                .unwrap()
                .replace(new_rx);
            service.engine.swap(replacement).await;
            let expected = snapshot_for_phase(composition.phase(), &composition);
            service
                .events()
                .publish(wire::RuntimeEventKind::Transition, expected.clone());
            let tick = service.events().current_tick();
            drop(composition);
            service
                .await_status_ready(1)
                .await
                .expect("swap 抢占退避并挂接新流");
            assert_eq!(
                service.events().current_tick(),
                tick,
                "{kind} 的旧事件不得发布"
            );
            assert_eq!(
                service.events().current_snapshot().unwrap().state,
                expected.state,
                "{kind}"
            );
            forwarder.abort();
        }
    }
}

mod session_orchestration_regression {
    use super::*;

    #[tokio::test]
    async fn late_query_does_not_override_new_dispatch() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let engine = Arc::new(Mutex::new(FakeEngine {
            observe_barrier: Some(barrier.clone()),
            ..Default::default()
        }));
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service);
        *engine.lock().await.owned_state.lock().unwrap() = Some(RuntimeSnapshot {
            operation_id: connect_intent().lookup_key.unwrap().operation_id,
            state: Some(wire::runtime_snapshot::State::Connected(Default::default())),
            ..Default::default()
        });
        let query = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .get_snapshot(Request::new(SnapshotRequest::default()))
                    .await
                    .unwrap()
                    .into_inner()
            }
        });
        barrier.wait().await;
        // 真实 Connect 在旧查询占用 Engine 时已完成本地受理。
        let mut events = Box::pin(service.events().subscribe(service.events().current_tick()));
        let connect = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .connect(Request::new(ConnectRequest {
                        intent: Some(connect_intent()),
                        ..Default::default()
                    }))
                    .await
            }
        });
        let admitted = tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            admitted.snapshot.as_ref().unwrap().state,
            Some(wire::runtime_snapshot::State::Connecting(_))
        ));
        barrier.wait().await;
        let observed = query.await.unwrap();
        assert_eq!(
            observed.operation_id,
            admitted.snapshot.unwrap().operation_id,
            "旧查询不得返回旧会话覆盖新派发"
        );
        assert!(matches!(
            observed.state,
            Some(wire::runtime_snapshot::State::Connecting(_))
        ));
        connect.abort();
    }

    #[tokio::test]
    async fn pipe_eof_is_not_user_stop() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with(&engine);
        let (tx, rx) = mpsc::unbounded_channel();
        engine.lock().await.events.lock().unwrap().replace(rx);
        *engine.lock().await.observe_error.lock().unwrap() = Some(GrpcClientError::ConnectionLost);
        {
            let mut state = service.composition.lock().await;
            state.apply(HostEvent::Connect);
            state.apply(HostEvent::Disconnect);
            state.apply(HostEvent::TeardownSideJoined(TeardownSide::ProtocolControl));
        }
        let stops_before = service.composition.lock().await.stop_requests();
        let mut events = Box::pin(service.events().subscribe(service.events().current_tick()));
        let forwarder = service.spawn_status_forwarder();
        service.await_status_ready(0).await.unwrap();
        drop(tx);
        tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap();
        let state = service.composition.lock().await;
        assert_eq!(state.stop_requests(), stops_before, "EOF 不是用户 Stop");
        assert_eq!(
            state.phase(),
            HostPhase::Stopping,
            "查询失联不能假造数据面已停止的 join"
        );
        assert!(engine.lock().await.stops.lock().unwrap().is_empty());
        forwarder.abort();
    }

    #[tokio::test]
    async fn connect_policy_uses_admission_snapshot() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = seeded_config_dir("test");
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = service.with_status_ready_wait(Duration::from_millis(1));
        let prepared = service.reconnect_policy_for_new_session();
        let mut changed = ExvConfig::load_from_dir(config.path()).unwrap();
        changed.auto_reconnect = !prepared.auto_reconnect;
        changed.save_to_dir(config.path()).unwrap();
        let _ = service
            .run_connect(
                connect_intent(),
                RunConnectInputs {
                    auth_deadline: None,
                    auth_binding: None,
                    ui_credentials: None,
                    persist_credentials: false,
                    new_session_policy: Some(prepared),
                    new_session_identity: None,
                    expected_dispatch_id: None,
                    frozen_protocol: prepared.protocol,
                    connection_mode: prepared.connection_mode,
                },
            )
            .await;
        assert_eq!(
            service
                .reconnect_session_policy
                .lock()
                .unwrap()
                .unwrap()
                .auto_reconnect,
            changed.auto_reconnect,
            "无绑定请求也必须在实际接纳边界冻结配置"
        );
    }
    #[tokio::test]
    async fn admission_config_failure_drops_prepared_policy_and_identity() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = reconnect_config_dir_full(true, 7, false);
        let (service, _logs) = service_with_config(&engine, config.path().into());
        service.status_attachment.send_replace(StatusAttachment {
            generation: 0,
            attached: true,
        });
        let prepared = service.reconnect_policy_for_new_session();
        let identity = service.session_identity_for_new_session();
        assert!(prepared.auto_reconnect);
        // 仅隔离临时配置：用同名目录使现有读取入口产生真实文件I/O错误。
        let path = config.path().join("config.json");
        std::fs::rename(&path, config.path().join("saved-config.json")).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(ExvConfig::load_from_dir(config.path()).is_err());
        let result = service
            .run_connect(
                connect_intent(),
                RunConnectInputs {
                    auth_deadline: None,
                    auth_binding: None,
                    ui_credentials: None,
                    persist_credentials: false,
                    new_session_policy: Some(prepared),
                    new_session_identity: identity,
                    expected_dispatch_id: None,
                    frozen_protocol: prepared.protocol,
                    connection_mode: prepared.connection_mode,
                },
            )
            .await;
        assert!(result.is_err(), "配置/凭据组装仍必须报告读取失败");
        assert!(
            !service
                .reconnect_session_policy
                .lock()
                .unwrap()
                .unwrap()
                .auto_reconnect
        );
        assert!(service.events().current_session_identity().is_none());
        assert!(engine.lock().await.applies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn adopt_session_does_not_auth_or_apply() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with(&engine);
        let observed = RuntimeSnapshot {
            operation_id: uuid16(88),
            state: Some(wire::runtime_snapshot::State::Connected(
                wire::ConnectedState {
                    session_established_at_ms: 12345,
                    ..Default::default()
                },
            )),
            engine_reconnect: Some(wire::ReconnectState {
                attempt: 3,
                reason: "healed".into(),
                next_retry_at_ms: 0,
            }),
            headless: Some(Default::default()),
            relaunched_by_engine: true,
            ..Default::default()
        };
        assert!(service.adopt_observed_session(&observed).await);
        assert!(service.adopt_observed_session(&observed).await);
        let snapshot = service.events().current_snapshot().unwrap();
        assert_eq!(snapshot.operation_id, observed.operation_id);
        assert_eq!(snapshot.engine_reconnect, observed.engine_reconnect);
        assert_eq!(snapshot.headless, observed.headless);
        assert_eq!(
            service.composition.lock().await.session_established_at_ms(),
            Some(12345)
        );
        assert!(service.connect_dispatch.lock().unwrap().is_none());
        assert!(service.reconnect_session_policy.lock().unwrap().is_none());
        let fake = engine.lock().await;
        assert!(fake.applies.lock().unwrap().is_empty());
        assert!(fake.stops.lock().unwrap().is_empty());
        assert_eq!(
            *fake.lease_ensures.lock().unwrap(),
            0,
            "采纳不申请变更lease或准备认证"
        );
    }

    #[tokio::test]
    async fn late_adopt_after_stop_preserves_correlation_and_summary() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with(&engine);
        let observed = RuntimeSnapshot {
            operation_id: uuid16(88),
            state: Some(wire::runtime_snapshot::State::Connected(Default::default())),
            ..Default::default()
        };
        assert!(service.adopt_observed_session(&observed).await);
        *engine.lock().await.stop_error.lock().unwrap() = Some(GrpcClientError::ConnectionLost);
        let terminal = service
            .stop(Request::new(StopRequest {
                intent: Some(stop_intent()),
            }))
            .await.unwrap().into_inner().terminal.expect("明确失败回执");
        assert!(matches!(terminal.result, Some(wire::operation_terminal::Result::Failed(_))));
        let completion = terminal.stop_completion.as_ref().expect("未确认停止摘要");
        assert!(!completion.stopped && !completion.blocks_new_connect);
        assert!(completion.errors.iter().any(|error| error.reason.contains("stop/rpc")
            && error.reason.contains("engine control unavailable")));
        let before = {
            let guard = service.composition.lock().await;
            (
                guard.phase(),
                guard.operation_id(),
                guard.stop_completion.clone(),
                guard.stop_requests(),
            )
        };
        assert_eq!(before.0, HostPhase::Failed);
        assert_eq!(before.1, Some([0; 16]));
        assert_eq!(before.2.as_ref(), Some(completion));
        assert!(!service.adopt_observed_session(&observed).await);
        let guard = service.composition.lock().await;
        assert_eq!(
            (
                guard.phase(),
                guard.operation_id(),
                guard.stop_completion.clone(),
                guard.stop_requests()
            ),
            before
        );
        assert!(service.events().current_session_identity().is_none());
        assert!(service.reconnect_session_policy.lock().unwrap().is_none());
        let fake = engine.lock().await;
        assert!(fake.applies.lock().unwrap().is_empty());
        assert_eq!(fake.stops.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reconnect_keeps_origin_and_policy() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = reconnect_config_dir_full(true, 7, false);
        let (service, _logs) = service_with_config(&engine, config.path().into());
        service.status_attachment.send_replace(StatusAttachment {
            generation: 0,
            attached: true,
        });
        service
            .connect(Request::new(ConnectRequest {
                intent: Some(connect_intent()),
                ..Default::default()
            }))
            .await
            .unwrap();
        let origin = service.current_reconnect_dispatch_id().unwrap();
        let identity = service.events().current_session_identity();
        let frozen = service.reconnect_session_policy.lock().unwrap().unwrap();
        let mut changed = ExvConfig::load_from_dir(config.path()).unwrap();
        // 保持当前恢复许可，只改下一次会话预算，继续验证来源隔离与当前预算冻结。
        changed.auto_reconnect_max_attempts = 1;
        changed.save_to_dir(config.path()).unwrap();
        service
            .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
            .await;
        assert_eq!(engine.lock().await.applies.lock().unwrap().len(), 2);
        assert_eq!(service.events().current_session_identity(), identity);
        assert_eq!(
            service
                .reconnect_session_policy
                .lock()
                .unwrap()
                .unwrap()
                .max_attempts,
            frozen.max_attempts
        );
        assert!(
            service
                .reconnect_session_policy
                .lock()
                .unwrap()
                .unwrap()
                .auto_reconnect
        );
        let next = service.current_reconnect_dispatch_id().unwrap();
        assert_ne!(next, origin, "R2有独立Core派发身份");
        service
            .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
            .await;
        assert_eq!(
            service.current_reconnect_dispatch_id(),
            Some(next),
            "旧信号不覆盖新派发"
        );
        assert_eq!(engine.lock().await.applies.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn stop_invalidates_r2_before_route_release() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = reconnect_config_dir_full(true, 7, false);
        let (service, _logs) = service_with_config(&engine, config.path().into());
        let service = Arc::new(service);
        service.status_attachment.send_replace(StatusAttachment {
            generation: 0,
            attached: true,
        });
        service
            .connect(Request::new(ConnectRequest {
                intent: Some(connect_intent()),
                ..Default::default()
            }))
            .await
            .unwrap();
        let origin = service.current_reconnect_dispatch_id().unwrap();
        let route = service.connect_route_lock.lock().await;
        let mut events = Box::pin(service.events().subscribe(service.events().current_tick()));
        let recovery = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
                    .await;
            }
        });
        tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap();
        service
            .stop(Request::new(StopRequest {
                intent: Some(stop_intent()),
            }))
            .await
            .unwrap();
        drop(route);
        tokio::time::timeout(Duration::from_secs(2), recovery)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            engine.lock().await.applies.lock().unwrap().len(),
            1,
            "Stop先失效R2，放行路由后不能Apply"
        );
        assert_eq!(engine.lock().await.stops.lock().unwrap().len(), 1);
        assert_eq!(service.composition.lock().await.phase(), HostPhase::Idle);
        assert!(service.reconnect_session_policy.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn service_transition_transport_loss_does_not_confirm_stop() {
        for during_lease in [true, false] {
            let engine = Arc::new(Mutex::new(FakeEngine::default()));
            let (service, _logs) = service_with(&engine);
            service.composition.lock().await.apply(HostEvent::Connect);
            service.record_mode(ServiceMode::Oneshot);
            if during_lease {
                *engine.lock().await.owner_lease_error.lock().unwrap() =
                    Some(GrpcClientError::ConnectionLost);
            } else {
                *engine.lock().await.stop_error.lock().unwrap() =
                    Some(GrpcClientError::ConnectionLost);
            }
            let result = service
                .stop_engine_for_transition(ServiceMode::Oneshot)
                .await;
            assert!(
                result.is_err(),
                "通道失联不证明活动隧道已停止，不能放行服务维护"
            );
            assert_eq!(
                service.composition.lock().await.phase(),
                HostPhase::Connecting
            );
            assert_eq!(service.mode_string(), "oneshot");
            assert!(service.composition.lock().await.stop_completion.is_none());
        }
    }
    #[tokio::test]
    async fn rebind_preserves_current_connect_and_never_reopens_stopping() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with(&engine);
        {
            let mut guard = service.composition.lock().await;
            guard.apply(HostEvent::Connect);
            guard.set_operation_id(uuid16_bytes(76));
        }
        crate::crash_recovery::rebind_composition(&service.composition, &helper(), &helper())
            .await
            .unwrap();
        assert_eq!(
            service.composition.lock().await.phase(),
            HostPhase::Connecting,
            "重绑必须保留同一锁内实际在途Connect"
        );
        assert_eq!(
            service.composition.lock().await.operation_id(),
            Some(uuid16_bytes(76))
        );
        let mut guard = service.composition.lock().await;
        // 固定重绑排队点，再先提交Stop；释放锁后不允许旧恢复覆盖Stop。
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let rebind = tokio::spawn({
            let composition = service.composition.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                crate::crash_recovery::rebind_composition(&composition, &helper(), &helper()).await
            }
        });
        barrier.wait().await;
        guard.apply(HostEvent::Disconnect);
        guard.apply(HostEvent::TeardownSideJoined(TeardownSide::ProtocolControl));
        let stop_id = uuid16_bytes(77);
        guard.set_operation_id(stop_id);
        drop(guard);
        assert!(
            rebind.await.unwrap().is_err(),
            "停止未确认时重绑不能伪造Idle"
        );
        assert_eq!(
            service.composition.lock().await.phase(),
            HostPhase::Stopping
        );
        assert_eq!(
            service.composition.lock().await.operation_id(),
            Some(stop_id)
        );
    }
    #[tokio::test]
    async fn r2_pre_admission_failure_releases_active_and_requeues_origin() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = reconnect_config_dir_full(true, 7, false);
        let (service, _logs) = service_with_config(&engine, config.path().into());
        service.status_attachment.send_replace(StatusAttachment {
            generation: 0,
            attached: true,
        });
        service
            .connect(Request::new(ConnectRequest {
                intent: Some(connect_intent()),
                ..Default::default()
            }))
            .await
            .unwrap();
        let origin = service.current_reconnect_dispatch_id().unwrap();
        let original_operation = service.composition.lock().await.operation_id();
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        *engine.lock().await.events.lock().unwrap() = Some(status_rx);
        *engine.lock().await.observe_error.lock().unwrap() = Some(GrpcClientError::ConnectionLost);
        let (reconnect_tx, mut reconnect_rx) = mpsc::unbounded_channel();
        *service.reconnect_tx.lock().unwrap() = Some(reconnect_tx);
        service.record_mode(ServiceMode::Service);
        {
            let mut guard = service.composition.lock().await;
            super::super::session_orchestration::accept_process_fact(
                &mut guard,
                super::super::session_orchestration::ProcessFact::ControlChannelLost,
            )
            .unwrap();
        }
        assert_eq!(
            service.composition.lock().await.phase(),
            HostPhase::Reconciling
        );
        let forwarder = service.spawn_status_forwarder();
        service.await_status_ready(0).await.unwrap();
        drop(status_tx);
        let (trigger, queued_origin) =
            tokio::time::timeout(Duration::from_secs(2), reconnect_rx.recv())
                .await
                .unwrap()
                .unwrap();
        forwarder.abort();
        let _ = forwarder.await;
        assert_eq!(queued_origin, origin);
        // 生产worker实际调用的同一driver；此处截住队列，精确观察一次收口。
        service
            .drive_reconnect_from(trigger, Some(queued_origin))
            .await;
        assert!(
            !service.reconnect_active.load(Ordering::Acquire),
            "Connect受理前已失败，R2不能遗留在途"
        );
        let (retry_trigger, retry_origin) =
            reconnect_rx.try_recv().expect("服务重试须保留原origin入队");
        assert_eq!(retry_trigger, ReconnectTrigger::EngineLostService);
        assert_eq!(retry_origin, origin);
        assert_eq!(service.current_reconnect_dispatch_id(), Some(origin));
        assert_eq!(
            service.composition.lock().await.operation_id(),
            original_operation
        );
        assert_eq!(service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert_eq!(engine.lock().await.applies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn r2_admitted_failure_requeues_only_its_received_dispatch() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = reconnect_config_dir_full(true, 7, false);
        let (service, _logs) = service_with_config(&engine, config.path().into());
        service.status_attachment.send_replace(StatusAttachment {
            generation: 0,
            attached: true,
        });
        service
            .connect(Request::new(ConnectRequest {
                intent: Some(connect_intent()),
                ..Default::default()
            }))
            .await
            .unwrap();
        let origin = service.current_reconnect_dispatch_id().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        *service.reconnect_tx.lock().unwrap() = Some(tx);
        *engine.lock().await.apply_error.lock().unwrap() = Some(GrpcClientError::ConnectionLost);
        service
            .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
            .await;
        assert!(!service.reconnect_active.load(Ordering::Acquire));
        let accepted = service.current_reconnect_dispatch_id().unwrap();
        assert_ne!(accepted, origin);
        assert_eq!(rx.try_recv().unwrap().1, accepted);
        assert_eq!(service.composition.lock().await.phase(), HostPhase::Failed);
        let failed_operation = service.composition.lock().await.operation_id();
        assert_eq!(engine.lock().await.applies.lock().unwrap().len(), 2);
        *engine.lock().await.apply_error.lock().unwrap() = None;
        // 新用户Connect故意复用operation，旧实际dispatch回执仍不得匹配它。
        let mut intent = connect_intent();
        intent.lookup_key.as_mut().unwrap().operation_id = failed_operation.unwrap().to_vec();
        service
            .connect(Request::new(ConnectRequest {
                intent: Some(intent),
                ..Default::default()
            }))
            .await
            .unwrap();
        let new_origin = service.current_reconnect_dispatch_id().unwrap();
        let mut guard = service.composition.lock().await;
        service
            .accept_session_input(
                &mut guard,
                SessionInput::Reconnect {
                    trigger: ReconnectTrigger::EngineLostService,
                    expected_origin: Some(new_origin),
                },
            )
            .unwrap();
        let result = service
            .accept_session_input(
                &mut guard,
                SessionInput::ReconnectFinished {
                    operation_id: failed_operation,
                    origin_dispatch_id: origin,
                    admitted_dispatch_id: Some(accepted),
                    attempt: 1,
                },
            )
            .unwrap();
        assert!(matches!(result, SessionOutput::Origin(None)));
        assert!(service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(guard.phase(), HostPhase::Connecting);
    }

    #[tokio::test]
    async fn r2_unadmitted_completion_cannot_clear_stop_new_session_or_new_round() {
        let engine = Arc::new(Mutex::new(FakeEngine::default()));
        let config = reconnect_config_dir_full(true, 7, false);
        let (service, _logs) = service_with_config(&engine, config.path().into());
        service.status_attachment.send_replace(StatusAttachment {
            generation: 0,
            attached: true,
        });
        service
            .connect(Request::new(ConnectRequest {
                intent: Some(connect_intent()),
                ..Default::default()
            }))
            .await
            .unwrap();
        let old_origin = service.current_reconnect_dispatch_id().unwrap();
        {
            let mut guard = service.composition.lock().await;
            service
                .accept_session_input(
                    &mut guard,
                    SessionInput::Reconnect {
                        trigger: ReconnectTrigger::EngineLostService,
                        expected_origin: Some(old_origin),
                    },
                )
                .unwrap();
            assert!(service.reconnect_active.load(Ordering::Acquire));
        }
        service
            .stop(Request::new(StopRequest {
                intent: Some(stop_intent()),
            }))
            .await
            .unwrap();
        {
            let mut guard = service.composition.lock().await;
            let before = (
                guard.phase(),
                guard.operation_id(),
                guard.stop_completion.clone(),
            );
            let result = service
                .accept_session_input(
                    &mut guard,
                    SessionInput::ReconnectFinished {
                        operation_id: Some([99; 16]),
                        origin_dispatch_id: old_origin,
                        admitted_dispatch_id: None,
                        attempt: 1,
                    },
                )
                .unwrap();
            assert!(matches!(result, SessionOutput::Origin(None)));
            assert_eq!(
                (
                    guard.phase(),
                    guard.operation_id(),
                    guard.stop_completion.clone()
                ),
                before
            );
            assert!(!service.reconnect_active.load(Ordering::Acquire));
        }
        service
            .connect(Request::new(ConnectRequest {
                intent: Some(connect_intent()),
                ..Default::default()
            }))
            .await
            .unwrap();
        // 本用例直接注入 EngineLostService，固定为服务形态；默认 fake 路由选择
        // oneshot，而其恢复开关关闭时真实 ChannelFault 应直接失败。
        service.record_mode(ServiceMode::Service);
        let current = service.current_reconnect_dispatch_id().unwrap();
        let mut guard = service.composition.lock().await;
        assert_eq!(guard.phase(), HostPhase::Connecting);
        let current_operation = guard.operation_id();
        assert!(guard.stop_completion.is_none());
        service
            .accept_session_input(
                &mut guard,
                SessionInput::Reconnect {
                    trigger: ReconnectTrigger::EngineLostService,
                    expected_origin: Some(current),
                },
            )
            .unwrap();
        let stale = service
            .accept_session_input(
                &mut guard,
                SessionInput::ReconnectFinished {
                    operation_id: Some([99; 16]),
                    origin_dispatch_id: old_origin,
                    admitted_dispatch_id: None,
                    attempt: 1,
                },
            )
            .unwrap();
        assert!(matches!(stale, SessionOutput::Origin(None)));
        assert!(service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(guard.phase(), HostPhase::Connecting);
        assert_eq!(guard.operation_id(), current_operation);
        let basis = service.observation_basis(&guard);
        service
            .accept_session_input(
                &mut guard,
                SessionInput::ChannelFault {
                    basis,
                    subscribed: true,
                },
            )
            .unwrap();
        service
            .accept_session_input(
                &mut guard,
                SessionInput::Reconnect {
                    trigger: ReconnectTrigger::EngineLostService,
                    expected_origin: Some(current),
                },
            )
            .unwrap();
        assert_eq!(service.reconnect_attempts.load(Ordering::Relaxed), 2);
        let stale_round = service
            .accept_session_input(
                &mut guard,
                SessionInput::ReconnectFinished {
                    operation_id: Some([99; 16]),
                    origin_dispatch_id: current,
                    admitted_dispatch_id: None,
                    attempt: 1,
                },
            )
            .unwrap();
        assert!(matches!(stale_round, SessionOutput::Origin(None)));
        assert!(service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(guard.phase(), HostPhase::Connecting);
        assert_eq!(guard.operation_id(), current_operation);
        assert!(guard.stop_completion.is_none());
    }
}
