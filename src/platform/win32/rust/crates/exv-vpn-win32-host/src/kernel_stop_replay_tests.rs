// Core 真入口 → 真实本地管道/认证/client → Engine 真受理、执行、终态和查询。
// 仅替换特权 TunnelRuntime 与一次受理前传输错误，不伪造 Stop 回复/terminal。
mod stop_replay_integration {
    use super::*;
    use exv_engine::grpc_server::HelperControlService;
    use exv_engine::tunnel_runtime::{ApplyContext, TunnelError, TunnelRuntime};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    struct CountingRuntime {
        calls: AtomicUsize,
        connected: AtomicBool,
    }
    impl TunnelRuntime for CountingRuntime {
        fn start_apply(self: Arc<Self>, _: ApplyContext) -> Result<(), TunnelError> {
            Ok(())
        }
        fn cancel(&self) {}
        fn disconnect(&self) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.connected.store(false, Ordering::SeqCst);
            Err("isolated cleanup failure".into())
        }
        fn teardown(&self) -> Result<(), String> {
            Ok(())
        }
        fn is_connected(&self) -> bool {
            self.connected.load(Ordering::SeqCst)
        }
        fn is_assembling(&self) -> bool {
            false
        }
        fn is_paused(&self) -> bool {
            !self.is_connected()
        }
    }
    struct AbortOnDrop(tokio::task::AbortHandle);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    #[tokio::test]
    async fn stop_alias_after_pre_admission_failure_preserves_original_correlation() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let runtime = Arc::new(CountingRuntime {
                calls: AtomicUsize::new(0),
                connected: AtomicBool::new(true),
            });
            let helper = HelperControlService::with_tunnel_runtime(
                Arc::new(exv_engine::log_sink::LogSink::null()),
                runtime.clone(),
            );
            let status = helper.status_publisher().clone();
            let pipe = format!(r"\\.\pipe\exv-stop-replay-{}", Uuid::new_v4());
            let sid = exv_vpn_win32_ipc::peer_auth::current_user_sid().unwrap();
            let serve_pipe = pipe.clone();
            let serve_sid = sid.clone();
            let server = tokio::spawn(async move {
                exv_engine::grpc_transport::serve_named_pipe(
                    &serve_pipe,
                    &serve_sid,
                    std::process::id(),
                    wire::helper_control_server::HelperControlServer::new(helper),
                )
                .await
            });
            let _server = AbortOnDrop(server.abort_handle());
            let client = crate::grpc_control::EngineControlGrpcClient::connect(
                &pipe,
                std::process::id(),
                &sid,
            )
            .await
            .unwrap();
            let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine {
                stop_delegate: Some(client),
                ..Default::default()
            }));
            let (service, _dir) = service_with(&engine);
            let forwarder = service.spawn_status_forwarder();
            let _forwarder = AbortOnDrop(forwarder.abort_handle());
            let mut ready = service.status_ready();
            while !ready.borrow().attached {
                ready.changed().await.unwrap();
            }
            let (cancelled, _) = watch::channel(false);
            *service.connect_dispatch.lock().unwrap() = Some(ConnectDispatch {
                id: Uuid::new_v4(),
                cancelled,
                target_generation: None,
                attempt_applied: true,
                stop_required: true,
            });
            {
                let mut guard = service.composition.lock().await;
                guard.apply(HostEvent::Connect);
                guard.apply(HostEvent::ProtocolEstablished);
            }
            let mut original = stop_intent();
            original.lookup_key.as_mut().unwrap().runtime_epoch = vec![96; 16];
            original.lookup_key.as_mut().unwrap().operation_id = vec![97; 16];
            let original_id = original.lookup_key.as_ref().unwrap().operation_id.clone();
            let mut alias = original.clone();
            alias.lookup_key.as_mut().unwrap().operation_id = vec![98; 16];
            *engine.lock().await.stop_error.lock().unwrap() = Some(GrpcClientError::ConnectionLost);
            let first = service
                .stop(Request::new(StopRequest {
                    intent: Some(original.clone())
                }))
                .await
                .unwrap().into_inner().terminal.expect("首次受理失败回执");
            assert!(matches!(first.result, Some(wire::operation_terminal::Result::Failed(_))));
            let first_completion = first.stop_completion.as_ref().expect("未确认停止摘要");
            assert!(!first_completion.stopped && !first_completion.blocks_new_connect);
            assert!(first_completion.errors.iter().any(|error| error.reason.contains("stop/rpc")
                && error.reason.contains("engine control unavailable")));
            assert_eq!(
                service.composition.lock().await.phase(),
                HostPhase::Failed
            );
            assert_eq!(service.composition.lock().await.operation_id(), Some([97; 16]));
            assert_eq!(service.composition.lock().await.stop_completion.as_ref(), Some(first_completion));
            assert_eq!(engine.lock().await.stops.lock().unwrap()[0].lookup_key.as_ref().unwrap().operation_id, original_id);
            assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
            *engine.lock().await.stop_error.lock().unwrap() = None;
            let reply = service
                .stop(Request::new(StopRequest {
                    intent: Some(alias.clone()),
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(
                service.composition.lock().await.phase(),
                HostPhase::Idle,
                "别名重放必须结束首次受理 A 的停止屏障"
            );
            let terminal = reply.terminal.as_ref().unwrap();
            let completion = terminal.stop_completion.as_ref().unwrap();
            assert!(completion.stopped && !completion.blocks_new_connect);
            assert_eq!(completion.errors.len(), 1);
            assert_eq!(
                service.composition.lock().await.stop_completion.as_ref(),
                Some(completion)
            );
            let Some(wire::operation_terminal::Result::Failed(failed)) = &terminal.result else {
                panic!("cleanup failure required")
            };
            let Some(wire::journal_operation_identity::Identity::External(key)) = &failed
                .receipt
                .as_ref()
                .unwrap()
                .journal_operation_identity
                .as_ref()
                .unwrap()
                .identity
            else {
                panic!("original external identity required")
            };
            assert_eq!(key.operation_id, original_id);
            assert_eq!(status.latest().unwrap().operation_id, original_id);
            for intent in [&original, &alias] {
                let query = service
                    .get_operation(Request::new(wire::GetKernelOperationRequest {
                        key: intent.lookup_key.clone(),
                    }))
                    .await
                    .unwrap()
                    .into_inner();
                assert_eq!(
                    query.state.unwrap().state,
                    Some(wire::operation_state::State::Terminal(terminal.clone()))
                );
            }
            assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
            // 真实 Host 接纳 C 后，旧回复与原 Engine 事件都不得改写 C。
            {
                let mut guard = service.composition.lock().await;
                assert_eq!(guard.apply(HostEvent::Connect), HostEffect::ConnectAdmitted);
                guard.set_operation_id([99; 16]);
            }
            let (cancelled, _) = watch::channel(false);
            *service.connect_dispatch.lock().unwrap() = Some(ConnectDispatch {
                id: Uuid::new_v4(),
                cancelled,
                target_generation: None,
                attempt_applied: false,
                stop_required: false,
            });
            for intent in [original, alias] {
                assert_eq!(
                    service
                        .stop(Request::new(StopRequest {
                            intent: Some(intent)
                        }))
                        .await
                        .unwrap()
                        .into_inner(),
                    reply
                );
            }
            let event =
                crate::grpc_control::connect_status_from_wire(status.latest().unwrap().to_wire())
                    .unwrap();
            assert!(
                runtime_event_from_status(&event, &service.composition, &service.logs)
                    .await
                    .is_none()
            );
            let guard = service.composition.lock().await;
            assert_eq!(guard.phase(), HostPhase::Connecting);
            assert_eq!(guard.operation_id(), Some([99; 16]));
            assert!(guard.stop_completion.is_none());
            assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        })
        .await
        .expect("Core/Engine Stop alias integration exceeded 10 seconds");
    }
}
