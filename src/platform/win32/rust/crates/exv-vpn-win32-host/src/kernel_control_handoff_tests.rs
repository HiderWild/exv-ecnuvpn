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
        if result.is_ready() {
            if let Some(polled) = &self.polled {
                polled.notify_one();
            }
        }
        result
    }
}

mod handoff_regression {
    use super::*;

    fn request() -> Request<ConnectRequest> {
        Request::new(ConnectRequest {
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
                tx.send(EngineStatusEvent::Progress {
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
