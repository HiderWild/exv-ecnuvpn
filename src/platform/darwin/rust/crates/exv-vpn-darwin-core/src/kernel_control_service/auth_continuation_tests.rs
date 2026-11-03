mod auth_continuation_tests {
    use super::*;
    use crate::kernel_control_service::SessionReconnectPolicy;
    use exv_vpn_darwin_ipc::{
        auth_deadline::AuthDeadline,
        browser_session::session_from_cookie_pairs,
        connect_envelope::{DarwinEngineConnectV2, EngineConnectPayload},
    };
    use prost::Message;

    fn material(fresh: bool) -> UiAtrustCredentialPackage {
        UiAtrustCredentialPackage::new(
            session_from_cookie_pairs(
                "https://vpn.example.edu",
                "frozen-agent",
                vec![("sid".into(), "continuation-candidate".into())],
            )
            .unwrap(),
            fresh,
        )
    }

    async fn wait_until(check: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("原接缝应推进");
    }

    async fn connected(
        auto_login: bool,
        max: u32,
        duration: Duration,
    ) -> (LiveFixture, FixtureEngineHandle) {
        let mut fixture = reconnect_live_fixture(true, max, false, None);
        Arc::get_mut(&mut fixture.live)
            .unwrap()
            .auth_acquisition_duration = duration;
        Arc::get_mut(&mut fixture.live)
            .unwrap()
            .auth_cleanup_duration = Duration::from_millis(150);
        lock_recover(&fixture.live.config)
            .apply_and_save(vec![
                DarwinConfigItem::new("protocol", "atrust"),
                DarwinConfigItem::new("atrust_server", "vpn.example.edu"),
                DarwinConfigItem::new("atrust_auth_mode", "browser"),
                DarwinConfigItem::new("user_agent", "frozen-agent"),
                DarwinConfigItem::new(
                    "atrust_auto_login",
                    if auto_login { "true" } else { "false" },
                ),
            ])
            .unwrap();
        fixture
            .live
            .connect(
                &connect_request(&[112; 16]),
                Some(UiSecretPayload::Atrust(material(true))),
                false,
            )
            .await
            .unwrap();
        let handle = fixture.handles.lock().unwrap()[0].clone();
        send_connecting_then_connected(&handle, &[112; 16]);
        wait_until(|| {
            fixture
                .live
                .events
                .current_snapshot()
                .is_some_and(|snapshot| {
                    matches!(snapshot.state, Some(runtime_snapshot::State::Connected(_)))
                })
        })
        .await;
        (fixture, handle)
    }

    fn remove_candidate(fixture: &LiveFixture) {
        // 缓存键取 Common `GatewayOrigin` 规范端点（缺省端口补全 :443）——与
        // `atrust_session_cache::save` 的键控同源。
        let canonical = exv_vpn_darwin_ipc::browser_session::GatewayOrigin::parse(
            "https://vpn.example.edu",
        )
        .unwrap()
        .endpoint()
        .to_owned();
        std::fs::remove_file(atrust_session_cache::cache_path(
            fixture._config_dir.path(),
            &canonical,
        ))
        .unwrap();
    }

    fn signal(live: &LiveConnectionProjection) {
        lock_recover(&live.reconnect_tx)
            .as_ref()
            .unwrap()
            .send((
                live.reconnect_generation.current(),
                ReconnectSignal::EngineLost,
            ))
            .unwrap();
    }

    async fn acquisition(live: &LiveConnectionProjection) -> wire::AtrustAuthAcquisition {
        wait_until(|| live.auth_acquisition().is_some()).await;
        live.auth_acquisition().unwrap()
    }

    fn receipt(
        request: &wire::AtrustAuthAcquisition,
        result: wire::atrust_auth_acquisition_result::Result,
    ) -> wire::InteractionResponse {
        wire::InteractionResponse {
            interaction_id: request.interaction_id.clone(),
            runtime_epoch: request.runtime_epoch.clone(),
            response_payload: wire::AtrustAuthAcquisitionResult {
                result: Some(result),
            }
            .encode_to_vec(),
        }
    }

    fn success(request: &wire::AtrustAuthAcquisition) -> wire::InteractionResponse {
        receipt(
            request,
            wire::atrust_auth_acquisition_result::Result::Candidate(
                material(true).to_bytes().unwrap().to_vec(),
            ),
        )
    }

    fn auth_error() -> wire::VpnError {
        wire::VpnError {
            code: wire::ErrorCode::Unauthorized as i32,
            stage: wire::ErrorStage::ConnectingControl as i32,
            native: Some(exv_vpn_darwin_ipc::atrust_auth_error::native(true)),
            reason: "atrust/session_invalidated".into(),
            ..Default::default()
        }
    }

    fn reject(handle: &FixtureEngineHandle, operation: &[u8]) {
        handle.send_status(wire::ConnectStatusEvent {
            operation_id: operation.to_vec(),
            error: Some(auth_error()),
            ..Default::default()
        });
    }

    fn done() -> wire::StopCompletion {
        wire::StopCompletion {
            stopped: true,
            blocks_new_connect: false,
            errors: vec![],
        }
    }

    fn stop_pending(handle: &FixtureEngineHandle) {
        handle.reply_next_stop(wire::StopTunnelReply {
            result: Some(wire::stop_tunnel_reply::Result::Pending(Default::default())),
            ..Default::default()
        });
    }

    fn stopped(handle: &FixtureEngineHandle, completion: wire::StopCompletion) {
        handle.send_status(wire::ConnectStatusEvent {
            operation_id: handle.stopped_operation_ids().last().unwrap().clone(),
            stop_completion: Some(completion),
            ..Default::default()
        });
    }

    async fn recovery_pending(fixture: &LiveFixture, handle: &FixtureEngineHandle) -> Vec<u8> {
        signal(&fixture.live);
        wait_until(|| handle.apply_count() == 2).await;
        handle.applied_operation_ids()[1].clone()
    }

    async fn finish(fixture: &LiveFixture, worker: tokio::task::JoinHandle<()>) {
        worker.abort();
        fixture.live.close().await.unwrap();
    }

    #[tokio::test]
    async fn recovery_permission_revoked_during_adopt_stop_retains_real_completion() {
        for clear_password in [false, true] {
            let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
            remove_candidate(&fixture);
            let worker = spawn_reconnect_worker(&fixture.live);
            signal(&fixture.live);
            let request = acquisition(&fixture.live).await;
            let generation = fixture.live.reconnect_generation.current();
            let round_operation = lock_recover(&fixture.live.reconnect_generation.recovery)
                .as_ref()
                .unwrap()
                .round_operation
                .clone();
            handle.fail_next_apply(Status::failed_precondition("DARWIN_ENGINE_ADOPT_NEW_AUTH"));
            handle.delay_next_stop(Duration::from_millis(200));
            handle.reply_next_stop(wire::StopTunnelReply {
                stop_completion: Some(done()),
                result: Some(wire::stop_tunnel_reply::Result::Stopped(Default::default())),
            });
            fixture
                .live
                .respond_auth_acquisition(&mut success(&request))
                .unwrap();
            wait_until(|| {
                fixture.live.events.current_snapshot().is_some_and(|snapshot| {
                    matches!(snapshot.state, Some(runtime_snapshot::State::Stopping(_)))
                        && snapshot.stop_completion.is_none()
                })
            })
            .await;
            let mut service = DarwinKernelControlService::new_inner(
                Arc::clone(&fixture.live.events),
                None,
                Some(Arc::clone(&fixture.live)),
                DarwinUiConfig::default(),
                Arc::clone(&fixture.logs),
                Arc::clone(&fixture.live.proxy_tun),
            );
            service.config = Arc::clone(&fixture.live.config);
            let items = if clear_password {
                vec![config_item("password", ""), config_item("remember_password", "false")]
            } else {
                vec![config_item("auto_reconnect", "false")]
            };
            KernelControl::config_set(&service, Request::new(ConfigSetRequest { items }))
                .await
                .unwrap();
            wait_until(|| {
                fixture.live.events.current_snapshot().is_some_and(|snapshot| {
                    matches!(snapshot.state, Some(runtime_snapshot::State::FailedClean(_))
                        | Some(runtime_snapshot::State::FailedDirty(_)))
                }) && !fixture.live.reconnect_active.load(Ordering::Acquire)
            })
            .await;
            let snapshot = fixture.live.events.current_snapshot().unwrap();
            let completion = snapshot.stop_completion.as_ref().expect("保留真实停止回执");
            assert!(completion.stopped);
            assert!(!completion.blocks_new_connect);
            assert_eq!(completion.errors.len(), 1, "只附加本次取消原因，不合成截止错误");
            assert_eq!(completion.errors[0].code, wire::ErrorCode::CancelledBeforeStart as i32);
            assert_eq!(completion.errors[0].reason, "DARWIN_CORE_AUTO_RECONNECT_DISABLED");
            assert!(matches!(snapshot.state, Some(runtime_snapshot::State::FailedClean(_))));
            assert_eq!(snapshot.operation_id, round_operation);
            assert_eq!(fixture.live.reconnect_generation.current(), generation);
            assert_eq!(handle.apply_count(), 2, "首连接与ADOPT拒绝后不得再次Apply");
            assert_eq!(handle.applied_operation_ids(), vec![vec![112; 16]]);
            assert_eq!(handle.stopped_operation_ids().len(), 1);
            assert!(fixture.live.auth_acquisition().is_none());
            let mut late = success(&request);
            assert!(fixture.live.respond_auth_acquisition(&mut late).is_err());
            assert!(late.response_payload.is_empty());
            assert_eq!(handle.apply_count(), 2);
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_old_auth_cause_yields_only_to_exact_user_cancel_receipt() {
        for (code, reason) in [
            (
                wire::ErrorCode::CancelledBeforeStart,
                "atrust/auth_cancelled",
            ),
            (
                wire::ErrorCode::CancelledBeforeStart,
                "atrust/other_failure",
            ),
            (wire::ErrorCode::Unauthorized, "atrust/auth_cancelled"),
        ] {
            let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            let operation = recovery_pending(&fixture, &handle).await;
            reject(&handle, &operation);
            let request = acquisition(&fixture.live).await;
            assert!(
                fixture
                    .live
                    .events
                    .current_snapshot()
                    .unwrap()
                    .stop_completion
                    .unwrap()
                    .stopped
            );
            let failure = wire::VpnError {
                code: code as i32,
                stage: wire::ErrorStage::AwaitingInteraction as i32,
                reason: reason.into(),
                native: auth_error().native,
                retry: wire::RetryAdvice::UseNewOperation as i32,
                certainty: wire::EffectCertainty::NoEffect as i32,
                ..Default::default()
            };
            let mut response = receipt(
                &request,
                wire::atrust_auth_acquisition_result::Result::Failed(failure.clone()),
            );
            fixture
                .live
                .respond_auth_acquisition(&mut response)
                .unwrap();
            let expected = if code == wire::ErrorCode::CancelledBeforeStart
                && reason == "atrust/auth_cancelled"
            {
                failure
            } else {
                auth_error()
            };
            wait_until(|| {
                fixture
                    .live
                    .events
                    .current_snapshot()
                    .is_some_and(|snapshot| match snapshot.state {
                        Some(runtime_snapshot::State::FailedClean(failed)) => {
                            failed.last_error == Some(expected.clone())
                        }
                        Some(runtime_snapshot::State::FailedDirty(failed)) => {
                            failed.last_error == Some(expected.clone())
                        }
                        _ => false,
                    })
                    && !fixture.live.reconnect_active.load(Ordering::Acquire)
            })
            .await;
            assert_eq!(
                lock_recover(&fixture.live.reconnect_generation.recovery)
                    .as_ref()
                    .unwrap()
                    .first_error(),
                Some(auth_error())
            );
            assert!(fixture.live.auth_acquisition().is_none());
            assert_eq!(handle.apply_count(), 2);
            assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_malformed_tail_clears_partial_candidate_without_consuming_request() {
        let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
        remove_candidate(&fixture);
        let worker = spawn_reconnect_worker(&fixture.live);
        signal(&fixture.live);
        let request = acquisition(&fixture.live).await;
        let mut response = success(&request);
        response.response_payload.push(0); // 完整合法 candidate 后的非法 tag。
        let mut partial = wire::AtrustAuthAcquisitionResult::default();
        let error = crate::kernel_control_service::auth_continuation::merge_acquisition_result(
            &mut partial,
            &response.response_payload,
        )
        .unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert!(matches!(
            partial.result,
            Some(wire::atrust_auth_acquisition_result::Result::Candidate(candidate))
                if candidate.is_empty()
        ));
        assert_eq!(
            fixture
                .live
                .respond_auth_acquisition(&mut response)
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
        assert!(response.response_payload.is_empty());
        assert_eq!(fixture.live.auth_acquisition(), Some(request.clone()));
        assert_eq!(handle.apply_count(), 1);
        fixture
            .live
            .respond_auth_acquisition(&mut success(&request))
            .unwrap();
        wait_until(|| handle.apply_count() == 2).await;
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_missing_waits_same_round_and_frozen_success_applies_once() {
        for auto in [true, false] {
            let (fixture, handle) = connected(auto, 1, Duration::from_secs(5)).await;
            remove_candidate(&fixture);
            let worker = spawn_reconnect_worker(&fixture.live);
            signal(&fixture.live);
            let request = acquisition(&fixture.live).await;
            assert_eq!(handle.apply_count(), 1);
            assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
            assert!(fixture.live.reconnect_active.load(Ordering::Acquire));
            assert_eq!(request.mode, "headless");
            assert!(request.auto_login, "后台保存对能力独立于前台模式");
            assert_eq!(request.user_agent, "frozen-agent");
            assert_eq!(
                request.config_dir,
                fixture._config_dir.path().to_string_lossy()
            );
            assert!(request.browser_profile.is_empty() && !request.browser_profile_explicit);
            let original_round = lock_recover(&fixture.live.reconnect_generation.recovery)
                .as_ref()
                .unwrap()
                .round_operation
                .clone();
            lock_recover(&fixture.live.config)
                .apply_and_save(vec![
                    DarwinConfigItem::new("atrust_server", "changed.example.edu"),
                    DarwinConfigItem::new("atrust_auth_mode", "headless"),
                    DarwinConfigItem::new("atrust_auto_login", if auto { "false" } else { "true" }),
                ])
                .unwrap();
            let mut response = success(&request);
            assert!(
                fixture
                    .live
                    .respond_auth_acquisition(&mut response)
                    .unwrap()
            );
            assert!(response.response_payload.is_empty());
            wait_until(|| handle.apply_count() == 2).await;
            assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
            let snapshot = fixture.live.events.current_snapshot().unwrap();
            assert_eq!(snapshot.reconnect.as_ref().unwrap().max_attempts, 1);
            assert!(snapshot.reconnect.as_ref().unwrap().active);
            let record = lock_recover(&fixture.live.reconnect_generation.recovery);
            assert_eq!(record.as_ref().unwrap().round_operation, original_round);
            drop(record);
            let EngineConnectPayload::Atrust(payload) =
                DarwinEngineConnectV2::decode(handle.applied_payloads()[1].clone())
                    .unwrap()
                    .into_payload()
            else {
                panic!("aTrust材料")
            };
            assert!(payload.fresh_authentication());
            assert!(
                fixture
                    .live
                    .respond_auth_acquisition(&mut success(&request))
                    .is_err()
            );
            assert!(fixture.live.auth_acquisition().is_none());
            let op = handle.applied_operation_ids()[1].clone();
            send_connecting_then_connected(&handle, &op);
            wait_until(|| {
                !fixture
                    .live
                    .has_auth_round(fixture.live.reconnect_generation.current())
            })
            .await;
            let mut late = success(&request);
            assert!(fixture.live.respond_auth_acquisition(&mut late).is_err());
            assert!(late.response_payload.is_empty());
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_wrong_epoch_unknown_failure_and_ordinary_interaction_keep_contract()
    {
        let (fixture, handle) = connected(true, 3, Duration::from_secs(5)).await;
        remove_candidate(&fixture);
        let worker = spawn_reconnect_worker(&fixture.live);
        signal(&fixture.live);
        let request = acquisition(&fixture.live).await;
        let mut wrong = success(&request);
        wrong.runtime_epoch = vec![0; 16];
        assert!(fixture.live.respond_auth_acquisition(&mut wrong).is_err());
        assert!(wrong.response_payload.is_empty());
        assert!(fixture.live.auth_acquisition().is_some());
        let error = wire::VpnError {
            code: 717171,
            certainty: wire::EffectCertainty::Unknown as i32,
            retry: wire::RetryAdvice::UseNewOperation as i32,
            native: Some(exv_vpn_darwin_ipc::atrust_auth_error::native(false)),
            reason: "fixture/unknown_acquisition_failure".into(),
            stage: wire::ErrorStage::ProtocolSession as i32,
            ..Default::default()
        };
        fixture
            .live
            .respond_auth_acquisition(&mut receipt(
                &request,
                wire::atrust_auth_acquisition_result::Result::Failed(error.clone()),
            ))
            .unwrap();
        wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
        wait_until(|| {
            fixture
                .live
                .events
                .current_snapshot()
                .is_some_and(|s| s.stop_completion.is_some())
        })
        .await;
        let snapshot = fixture.live.events.current_snapshot().unwrap();
        assert!(snapshot.stop_completion.unwrap().errors.contains(&error));
        assert_eq!(handle.apply_count(), 1);
        assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
        let mut ordinary = wire::InteractionResponse {
            interaction_id: vec![5; 16],
            runtime_epoch: vec![6; 16],
            response_payload: vec![1, 2, 3],
        };
        assert!(
            !fixture
                .live
                .respond_auth_acquisition(&mut ordinary)
                .unwrap()
        );
        assert_eq!(ordinary.response_payload, vec![1, 2, 3]);
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_pending_invalidated_waits_real_completion_last_round_and_new_source()
    {
        let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
        let worker = spawn_reconnect_worker(&fixture.live);
        let old = recovery_pending(&fixture, &handle).await;
        let round_op = lock_recover(&fixture.live.reconnect_generation.recovery)
            .as_ref()
            .unwrap()
            .round_operation
            .clone();
        stop_pending(&handle);
        reject(&handle, &old);
        reject(&handle, &old);
        signal(&fixture.live);
        wait_until(|| handle.stopped_operation_ids().len() == 1).await;
        assert!(fixture.live.auth_acquisition().is_none());
        assert_eq!(handle.apply_count(), 2);
        assert!(fixture.live.reconnect_active.load(Ordering::Acquire));
        stopped(&handle, done());
        let request = acquisition(&fixture.live).await;
        fixture
            .live
            .respond_auth_acquisition(&mut success(&request))
            .unwrap();
        wait_until(|| handle.apply_count() == 3).await;
        let new = handle.applied_operation_ids()[2].clone();
        assert_ne!(new, old);
        assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert_eq!(
            lock_recover(&fixture.live.reconnect_generation.recovery)
                .as_ref()
                .unwrap()
                .round_operation,
            round_op
        );
        let snapshot = fixture.live.events.current_snapshot().unwrap();
        handle.send_retired_status(wire::ConnectStatusEvent {
            operation_id: old,
            error: Some(auth_error()),
            ..Default::default()
        });
        handle.close_retired_streams();
        tokio::task::yield_now().await;
        assert_eq!(
            fixture.live.events.current_snapshot().unwrap().operation_id,
            snapshot.operation_id
        );
        assert_eq!(handle.apply_count(), 3);
        assert!(fixture.live.auth_acquisition().is_none());
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_stop_blocked_rpc_and_pending_deadline_never_acquire() {
        for mode in ["blocked", "rpc", "pending_expired"] {
            let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            let old = recovery_pending(&fixture, &handle).await;
            match mode {
                "blocked" => handle.reply_next_stop(wire::StopTunnelReply {
                    stop_completion: Some(wire::StopCompletion {
                        stopped: true,
                        blocks_new_connect: true,
                        errors: vec![wire::VpnError {
                            reason: "真实残留资源".into(),
                            ..Default::default()
                        }],
                    }),
                    ..Default::default()
                }),
                "rpc" => handle.fail_next_stop(Status::unavailable("真实停止RPC失败")),
                _ => stop_pending(&handle),
            }
            reject(&handle, &old);
            if mode == "pending_expired" {
                // 原20秒停止期限；使用真实停止接缝，不用假 completion 视作成功。
                wait_until(|| handle.stopped_operation_ids().len() == 1).await;
                // 不发送完成；同一停止截止应自主结束。
            }
            wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
            assert!(fixture.live.auth_acquisition().is_none());
            assert_eq!(handle.apply_count(), 2);
            let completion = fixture
                .live
                .events
                .current_snapshot()
                .unwrap()
                .stop_completion
                .unwrap();
            assert!(completion.errors.contains(&auth_error()));
            assert!(completion.errors.len() >= 2);
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_cleanup_stop_or_new_connect_abandons_old_completion() {
        for new_connect in [true, false] {
            let (fixture, handle) = connected(true, 2, Duration::from_secs(5)).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            let old = recovery_pending(&fixture, &handle).await;
            stop_pending(&handle);
            reject(&handle, &old);
            wait_until(|| handle.stopped_operation_ids().len() == 1).await;
            let generation = fixture.live.reconnect_generation.current();
            let live = fixture.live.clone();
            let action = tokio::spawn(async move {
                if new_connect {
                    live.connect(
                        &connect_request(&[119; 16]),
                        Some(UiSecretPayload::Atrust(material(true))),
                        false,
                    )
                    .await
                } else {
                    live.stop(&stop_request(&[120; 16])).await
                }
            });
            wait_until(|| fixture.live.reconnect_generation.current() != generation).await;
            stopped(&handle, done());
            action.await.unwrap().unwrap();
            assert!(fixture.live.auth_acquisition().is_none());
            assert_eq!(handle.apply_count(), if new_connect { 3 } else { 2 });
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_stop_rpc_deadline_retains_late_completion_without_reopen() {
        for fail in [false, true] {
            let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            let old = recovery_pending(&fixture, &handle).await;
            handle.delay_next_stop(Duration::from_millis(450));
            if fail {
                handle.fail_next_stop(Status::unavailable("迟到的真实停止RPC失败"));
            }
            reject(&handle, &old);
            wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
            assert!(fixture.live.auth_acquisition().is_none());
            assert_eq!(handle.apply_count(), 2);
            let timed_out = fixture
                .live
                .events
                .current_snapshot()
                .unwrap()
                .stop_completion
                .unwrap();
            assert!(timed_out.errors.contains(&auth_error()));
            assert!(
                timed_out
                    .errors
                    .iter()
                    .any(|error| { error.code == wire::ErrorCode::DeadlineExceeded as i32 })
            );
            wait_until(|| {
                fixture
                    .live
                    .events
                    .current_snapshot()
                    .is_some_and(|snapshot| {
                        snapshot.stop_completion.is_some_and(|completion| {
                            if fail {
                                completion.errors.contains(&auth_error())
                                    && completion
                                        .errors
                                        .iter()
                                        .any(|error| error.reason.contains("迟到的真实停止RPC失败"))
                            } else {
                                completion.stopped
                            }
                        })
                    })
            })
            .await;
            assert!(
                fixture
                    .live
                    .auth_round_ended(fixture.live.reconnect_generation.current())
            );
            assert!(fixture.live.auth_acquisition().is_none());
            assert_eq!(handle.apply_count(), 2);
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_late_stop_rpc_error_does_not_replace_user_action() {
        for new_connect in [true, false] {
            let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            let old = recovery_pending(&fixture, &handle).await;
            handle.delay_next_stop(Duration::from_millis(450));
            handle.fail_next_stop(Status::unavailable("旧轮迟到的停止RPC失败"));
            reject(&handle, &old);
            wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
            let generation = fixture.live.reconnect_generation.current();
            let live = Arc::clone(&fixture.live);
            let action = tokio::spawn(async move {
                if new_connect {
                    live.connect(
                        &connect_request(&[119; 16]),
                        Some(UiSecretPayload::Atrust(material(true))),
                        false,
                    )
                    .await
                } else {
                    live.stop(&stop_request(&[120; 16])).await
                }
            });
            wait_until(|| fixture.live.reconnect_generation.current() != generation).await;
            action.await.unwrap().unwrap();
            let snapshot = fixture.live.events.current_snapshot().unwrap();
            assert_ne!(snapshot.operation_id, old);
            assert!(!snapshot.stop_completion.is_some_and(|completion| {
                completion
                    .errors
                    .iter()
                    .any(|error| error.reason.contains("旧轮迟到的停止RPC失败"))
            }));
            assert!(fixture.live.auth_acquisition().is_none());
            assert_eq!(handle.apply_count(), if new_connect { 3 } else { 2 });
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_wait_stop_or_replacement_rejects_late_receipt() {
        for new_connect in [true, false] {
            let (fixture, handle) = connected(true, 2, Duration::from_secs(5)).await;
            remove_candidate(&fixture);
            let worker = spawn_reconnect_worker(&fixture.live);
            signal(&fixture.live);
            let request = acquisition(&fixture.live).await;
            if new_connect {
                fixture
                    .live
                    .connect(
                        &connect_request(&[121; 16]),
                        Some(UiSecretPayload::Atrust(material(true))),
                        false,
                    )
                    .await
                    .unwrap();
            } else {
                fixture.live.stop(&stop_request(&[122; 16])).await.unwrap();
            }
            let mut late = success(&request);
            assert!(fixture.live.respond_auth_acquisition(&mut late).is_err());
            assert!(late.response_payload.is_empty());
            assert_eq!(handle.apply_count(), if new_connect { 2 } else { 1 });
            assert!(fixture.live.auth_acquisition().is_none());
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_fresh_failure_cleans_without_second_acquisition_for_both_entries() {
        for missing in [true, false] {
            let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            if missing {
                remove_candidate(&fixture);
                signal(&fixture.live);
            } else {
                let old = recovery_pending(&fixture, &handle).await;
                reject(&handle, &old);
            }
            let request = acquisition(&fixture.live).await;
            fixture
                .live
                .respond_auth_acquisition(&mut success(&request))
                .unwrap();
            let count = if missing { 2 } else { 3 };
            wait_until(|| handle.apply_count() == count).await;
            let fresh = handle.applied_operation_ids().last().unwrap().clone();
            let mut error = auth_error();
            error.native = Some(exv_vpn_darwin_ipc::atrust_auth_error::native(false));
            handle.send_status(wire::ConnectStatusEvent {
                operation_id: fresh,
                error: Some(error),
                ..Default::default()
            });
            wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
            assert!(fixture.live.auth_acquisition().is_none());
            assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
            assert_eq!(handle.apply_count(), count);
            assert!(
                fixture
                    .live
                    .auth_round_ended(fixture.live.reconnect_generation.current())
            );
            finish(&fixture, worker).await;
        }
    }

    #[tokio::test]
    async fn recovery_material_same_deadline_exits_session_lock_and_attach_without_gate_release() {
        for missing in [true, false] {
            for preparation in ["session", "attach"] {
                let (fixture, handle) = connected(true, 1, Duration::from_millis(180)).await;
                let worker = spawn_reconnect_worker(&fixture.live);
                if missing {
                    remove_candidate(&fixture);
                    signal(&fixture.live);
                } else {
                    let old = recovery_pending(&fixture, &handle).await;
                    reject(&handle, &old);
                }
                let request = acquisition(&fixture.live).await;
                let lock = if preparation == "session" {
                    Some(fixture.live.session.lock().await)
                } else {
                    None
                };
                if preparation == "attach" {
                    handle.delay_next_attach(Duration::from_secs(60));
                }
                fixture
                    .live
                    .respond_auth_acquisition(&mut success(&request))
                    .unwrap();
                wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
                assert_eq!(handle.apply_count(), if missing { 1 } else { 2 });
                assert!(fixture.live.auth_acquisition().is_none());
                assert!(AuthDeadline::from_monotonic_ns(request.deadline_monotonic_ns).is_err());
                let mut late = success(&request);
                assert!(fixture.live.respond_auth_acquisition(&mut late).is_err());
                assert!(late.response_payload.is_empty());
                drop(lock);
                finish(&fixture, worker).await;
            }
        }
    }

    #[tokio::test]
    async fn recovery_material_deadline_ends_wait_and_expired_receipt_is_locally_cleared() {
        let (fixture, handle) = connected(true, 1, Duration::from_millis(100)).await;
        remove_candidate(&fixture);
        let worker = spawn_reconnect_worker(&fixture.live);
        signal(&fixture.live);
        let request = acquisition(&fixture.live).await;
        wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
        let mut late = success(&request);
        assert!(fixture.live.respond_auth_acquisition(&mut late).is_err());
        assert!(late.response_payload.is_empty());
        assert_eq!(handle.apply_count(), 1);
        assert!(fixture.live.auth_acquisition().is_none());
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_waiting_old_source_eof_does_not_open_another_round() {
        let (fixture, handle) = connected(true, 2, Duration::from_secs(5)).await;
        remove_candidate(&fixture);
        let worker = spawn_reconnect_worker(&fixture.live);
        signal(&fixture.live);
        let request = acquisition(&fixture.live).await;
        handle.clone().kill();
        tokio::task::yield_now().await;
        assert!(fixture.live.reconnect_active.load(Ordering::Acquire));
        assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.live.auth_acquisition().unwrap(), request);
        let failure = wire::VpnError {
            reason: "原获取真实失败".into(),
            ..Default::default()
        };
        fixture
            .live
            .respond_auth_acquisition(&mut receipt(
                &request,
                wire::atrust_auth_acquisition_result::Result::Failed(failure),
            ))
            .unwrap();
        wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
        assert_eq!(handle.apply_count(), 1);
        assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_external_policy_plus_ui_or_persist_remains_rejected() {
        let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
        let policy = SessionReconnectPolicy::from_config(&lock_recover(&fixture.live.config));
        for persist in [false, true] {
            assert!(
                fixture
                    .live
                    .prepare_connect_payload(
                        Some(UiSecretPayload::Atrust(material(true))),
                        persist,
                        Some(policy.clone()),
                        false
                    )
                    .is_err()
            );
        }
        assert!(
            fixture
                .live
                .prepare_connect_payload(None, true, Some(policy), false)
                .is_err()
        );
        assert_eq!(handle.apply_count(), 1);
        fixture.live.close().await.unwrap();
    }

    #[tokio::test]
    async fn recovery_material_same_deadline_exits_service_preparation_without_gate_release() {
        let (mut fixture, handle) = connected(true, 1, Duration::from_millis(180)).await;
        Arc::get_mut(&mut fixture.live).unwrap().service_engine = Arc::new(ServiceEnginePath {
            probe: Arc::new(|| {
                Box::pin(async {
                    crate::service_status::ServiceAgentFacts {
                        engine_socket_present: true,
                        engine_socket_connectable: true,
                        ..Default::default()
                    }
                })
            }),
            ensure_engine: Arc::new(|| Box::pin(async { Ok(()) })),
            connect: Arc::new(|_| Box::pin(std::future::pending())),
        });
        remove_candidate(&fixture);
        let worker = spawn_reconnect_worker(&fixture.live);
        signal(&fixture.live);
        let request = acquisition(&fixture.live).await;
        fixture.live.session.lock().await.take();
        fixture
            .live
            .respond_auth_acquisition(&mut success(&request))
            .unwrap();
        wait_until(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
        assert_eq!(handle.apply_count(), 1);
        assert!(fixture.live.auth_acquisition().is_none());
        assert!(AuthDeadline::from_monotonic_ns(request.deadline_monotonic_ns).is_err());
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_preparation_unavailable_returns_original_service_budget() {
        let (fixture, handle) = connected(true, 2, Duration::from_secs(5)).await;
        remove_candidate(&fixture);
        let worker = spawn_reconnect_worker(&fixture.live);
        signal(&fixture.live);
        let request = acquisition(&fixture.live).await;
        handle.fail_next_attach(EngineLifecycleError::Observe(crate::ObserveError::Rpc(
            Status::unavailable("真实挂流服务暂错"),
        )));
        fixture
            .live
            .respond_auth_acquisition(&mut success(&request))
            .unwrap();
        wait_until(|| fixture.live.reconnect_attempts.load(Ordering::Relaxed) == 2).await;
        wait_until(|| fixture.handles.lock().unwrap().len() == 2).await;
        let next = fixture.handles.lock().unwrap()[1].clone();
        wait_until(|| next.apply_count() == 1).await;
        assert_eq!(handle.apply_count(), 1);
        assert!(fixture.live.auth_acquisition().is_none());
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_execution_retry_reuses_own_key_digest_and_replaces_old_execution() {
        let (fixture, handle) = connected(true, 1, Duration::from_secs(5)).await;
        let worker = spawn_reconnect_worker(&fixture.live);
        let old = recovery_pending(&fixture, &handle).await;
        reject(&handle, &old);
        let request = acquisition(&fixture.live).await;
        handle.fail_next_apply(Status::failed_precondition("DARWIN_ENGINE_ADOPT_NEW_AUTH"));
        fixture
            .live
            .respond_auth_acquisition(&mut success(&request))
            .unwrap();
        wait_until(|| handle.apply_count() == 4).await;
        let attempts = handle.apply_attempt_keys();
        assert_eq!(attempts.len(), 4);
        assert_ne!(attempts[1], attempts[2]);
        assert_eq!(attempts[2], attempts[3]);
        assert_ne!(attempts[2].0.as_ref().unwrap().operation_id, old);
        assert_eq!(
            attempts[2].0.as_ref().unwrap().method,
            wire::OperationMethod::ApplyTunnel as i32
        );
        assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert!(fixture.live.reconnect_active.load(Ordering::Acquire));
        finish(&fixture, worker).await;
    }

    #[tokio::test]
    async fn recovery_material_failed_service_receipt_uses_original_budget_and_keeps_error() {
        for missing in [true, false] {
            let (fixture, handle) = connected(true, 2, Duration::from_secs(5)).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            if missing {
                remove_candidate(&fixture);
                signal(&fixture.live);
            } else {
                let old = recovery_pending(&fixture, &handle).await;
                reject(&handle, &old);
            }
            let request = acquisition(&fixture.live).await;
            // 下一轮可重用候选；本轮 Failed 不把服务错改成 Unauthorized。
            atrust_session_cache::save(fixture._config_dir.path(), &material(true).browser_session)
                .unwrap();
            let error = wire::VpnError {
                code: wire::ErrorCode::PlatformDependencyUnavailable as i32,
                stage: wire::ErrorStage::ConnectingControl as i32,
                certainty: wire::EffectCertainty::Unknown as i32,
                retry: wire::RetryAdvice::UseNewOperation as i32,
                native: Some(exv_vpn_darwin_ipc::atrust_auth_error::native(false)),
                reason: "fixture/service_unavailable".into(),
                ..Default::default()
            };
            fixture
                .live
                .respond_auth_acquisition(&mut receipt(
                    &request,
                    wire::atrust_auth_acquisition_result::Result::Failed(error.clone()),
                ))
                .unwrap();
            wait_until(|| {
                fixture
                    .live
                    .events
                    .current_snapshot()
                    .is_some_and(|snapshot| match snapshot.state {
                        Some(runtime_snapshot::State::FailedClean(failed)) => {
                            failed.last_error == Some(error.clone())
                        }
                        _ => false,
                    })
            })
            .await;
            wait_until(|| handle.apply_count() == if missing { 2 } else { 3 }).await;
            assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 2);
            assert!(fixture.live.auth_acquisition().is_none());
            finish(&fixture, worker).await;
        }
    }
}
