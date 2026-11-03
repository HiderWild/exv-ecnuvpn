mod r2_material_tests {
    use super::*;
    use crate::DarwinConfigError;
    use crate::credential::SavedConnectEnvelopeError;
    use crate::kernel_control_service::{
        reconnect_dispatch_vpn_error, reconnect_materials_missing, saved_envelope_status,
    };

    const GATEWAY: &str = "https://vpn.example.edu:443";
    fn cache_material(fixture: &LiveFixture) {
        let candidate = exv_vpn_darwin_ipc::browser_session::session_from_cookie_pairs(
            GATEWAY,
            "fixture-agent",
            vec![("sid".into(), "r2-material-fixture".into())],
        )
        .unwrap();
        atrust_session_cache::save(fixture._config_dir.path(), &candidate).unwrap();
    }
    async fn connected_fixture(protocol: &str, max: u32) -> (LiveFixture, FixtureEngineHandle) {
        let mut fixture = reconnect_live_fixture(
            true,
            max,
            true,
            Some((Duration::from_millis(30), Duration::from_millis(30))),
        );
        lock_recover(&fixture.live.config)
            .apply_and_save(vec![
                DarwinConfigItem::new("protocol", protocol),
                DarwinConfigItem::new("atrust_server", "vpn.example.edu"),
                DarwinConfigItem::new("atrust_auth_mode", "headless"),
            ])
            .unwrap();
        cache_material(&fixture);
        let handle = drive_manual_connect_to_connected(
            &fixture.live,
            &mut fixture.events,
            &fixture.handles,
            &[81; 16],
        )
        .await;
        (fixture, handle)
    }
    fn remove_frozen_material_and_switch_protocol(fixture: &LiveFixture, missing: &str) {
        let (next, change) = match missing {
            "username" => ("atrust", Some(DarwinConfigItem::new("username", ""))),
            "password" => (
                "atrust",
                Some(DarwinConfigItem::new("remember_password", "false")),
            ),
            "cache" => ("cisco", None),
            _ => unreachable!(),
        };
        let mut changes = vec![DarwinConfigItem::new("protocol", next)];
        changes.extend(change);
        lock_recover(&fixture.live.config)
            .apply_and_save(changes)
            .unwrap();
        if missing == "cache" {
            std::fs::remove_file(atrust_session_cache::cache_path(
                fixture._config_dir.path(),
                GATEWAY,
            ))
            .unwrap();
        }
    }
    fn send_engine_lost(live: &Arc<LiveConnectionProjection>) {
        lock_recover(&live.reconnect_tx)
            .as_ref()
            .unwrap()
            .send((
                live.reconnect_generation.current(),
                ReconnectSignal::EngineLost,
            ))
            .unwrap();
    }
    async fn answer_missing_atrust(live: &LiveConnectionProjection) {
        use prost::Message;
        wait_for(|| live.auth_acquisition().is_some()).await;
        let request = live.auth_acquisition().unwrap();
        let failed = wire::VpnError {
            code: wire::ErrorCode::EffectUnknown as i32,
            retry: wire::RetryAdvice::UseNewOperation as i32,
            reason: CONNECTION_CREDENTIALS_MISSING_CODE.into(),
            ..Default::default()
        };
        let mut response = wire::InteractionResponse {
            interaction_id: request.interaction_id,
            runtime_epoch: request.runtime_epoch,
            response_payload: wire::AtrustAuthAcquisitionResult {
                result: Some(wire::atrust_auth_acquisition_result::Result::Failed(failed)),
            }
            .encode_to_vec(),
        };
        live.respond_auth_acquisition(&mut response).unwrap();
    }

    fn log_count(fixture: &LiveFixture, code: &str) -> usize {
        fixture
            .logs
            .aggregator()
            .list(0, 0)
            .entries
            .iter()
            .filter(|entry| entry.code == code)
            .count()
    }
    async fn wait_for(f: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("实际worker应在有界时间推进");
    }
    fn last_error(snapshot: &RuntimeSnapshot) -> &wire::VpnError {
        match snapshot.state.as_ref().unwrap() {
            runtime_snapshot::State::FailedClean(state) => state.last_error.as_ref().unwrap(),
            runtime_snapshot::State::FailedDirty(state) => state.last_error.as_ref().unwrap(),
            other => panic!("预期真实清理终态，得到{other:?}"),
        }
    }

    #[tokio::test]
    async fn r2_missing_frozen_material_stops_after_one_attempt_for_finite_and_unlimited_budgets() {
        for max in [2, 0] {
            for missing in ["username", "password", "cache"] {
                let protocol = if missing == "cache" {
                    "atrust"
                } else {
                    "cisco"
                };
                let (fixture, handle) = connected_fixture(protocol, max).await;
                let worker = spawn_reconnect_worker(&fixture.live);
                remove_frozen_material_and_switch_protocol(&fixture, missing);
                send_engine_lost(&fixture.live);
                if protocol == "atrust" {
                    answer_missing_atrust(&fixture.live).await;
                }
                // aTrust 缺缓存材料：同轮获取失败一次即收束（获取结果取代派发缺
                // 材料错误，无独立日志）。Cisco 凭据对被移除：后台重连资格即时失效
                //（b63987bb「重连只允许读取已持久化凭据」），同样零重试一次收束
                //（disabled 日志）。
                wait_for(|| {
                    let log_ready = protocol == "atrust"
                        || log_count(&fixture, "kernel.reconnect.disabled") == 1;
                    log_ready
                        && fixture.live.events.current_snapshot().is_some_and(|s| {
                            matches!(s.state, Some(runtime_snapshot::State::FailedClean(_)))
                        })
                })
                .await;
                let snapshot = fixture.live.events.current_snapshot().unwrap();
                if protocol == "atrust" {
                    assert_eq!(
                        last_error(&snapshot).reason,
                        CONNECTION_CREDENTIALS_MISSING_CODE
                    );
                    assert_eq!(
                        last_error(&snapshot).code,
                        wire::ErrorCode::EffectUnknown as i32
                    );
                    assert_eq!(
                        last_error(&snapshot).retry,
                        wire::RetryAdvice::UseNewOperation as i32
                    );
                    assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 1);
                } else {
                    // 资格失效不消耗重连预算（派发从未发生）。
                    assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 0);
                    assert_eq!(last_error(&snapshot).retry,
                        wire::RetryAdvice::UseNewOperation as i32);
                }
                assert!(
                    snapshot
                        .stop_completion
                        .as_ref()
                        .is_some_and(|done| done.stopped && !done.blocks_new_connect)
                );
                assert!(!fixture.live.reconnect_active.load(Ordering::Acquire));
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert_eq!(
                    handle.apply_count(),
                    1,
                    "缺材料不能改用已保存另一协议或再次Apply"
                );
                assert_eq!(log_count(&fixture, "kernel.reconnect.r2_retry"), 0);
                assert_eq!(log_count(&fixture, "kernel.reconnect.exhausted"), 0);
                // 已结束的active不能挡住显式用户请求；新请求采用当前已保存协议。
                cache_material(&fixture);
                lock_recover(&fixture.live.config)
                    .apply_ui_credentials_and_save("next-student", Some("next-password"))
                    .unwrap();
                fixture
                    .live
                    .connect(&connect_request(&[82; 16]), None, false)
                    .await
                    .unwrap();
                assert_eq!(handle.apply_count(), 2);
                let expected = if protocol == "cisco" {
                    "atrust"
                } else {
                    "cisco"
                };
                assert_eq!(
                    fixture
                        .live
                        .events
                        .current_snapshot()
                        .unwrap()
                        .session_identity
                        .unwrap()
                        .protocol,
                    expected
                );
                worker.abort();
                fixture.live.close().await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn r2_missing_material_keeps_real_cleanup_failure_and_releases_active() {
        for protocol in ["cisco", "atrust"] {
            let (fixture, handle) = connected_fixture(protocol, 0).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            remove_frozen_material_and_switch_protocol(
                &fixture,
                if protocol == "cisco" {
                    "username"
                } else {
                    "cache"
                },
            );
            handle.fail_next_stop(Status::unavailable("fixture real cleanup RPC failure"));
            send_engine_lost(&fixture.live);
            if protocol == "atrust" {
                answer_missing_atrust(&fixture.live).await;
            }
            // aTrust：同轮获取失败收束；Cisco：凭据对移除即时撤销资格。两者都经
            // 同一真实清理 Stop 收口，注入的 Stop RPC 失败必须原样保留。
            wait_for(|| {
                let log_ready = protocol == "atrust"
                    || log_count(&fixture, "kernel.reconnect.disabled") == 1;
                log_ready
                    && fixture.live.events.current_snapshot().is_some_and(|s| {
                        matches!(s.state, Some(runtime_snapshot::State::FailedDirty(_)))
                    })
            })
            .await;
            let snapshot = fixture.live.events.current_snapshot().unwrap();
            let done = snapshot.stop_completion.as_ref().unwrap();
            assert!(!done.stopped);
            assert!(!done.errors.is_empty());
            assert_eq!(
                last_error(&snapshot),
                &done.errors[0],
                "真实清理错误优先于缺材料origin"
            );
            if protocol == "atrust" {
                assert!(
                    done.errors
                        .iter()
                        .any(|error| error.reason == CONNECTION_CREDENTIALS_MISSING_CODE)
                );
                assert!(done.errors.len() >= 2);
            } else {
                assert_ne!(
                    last_error(&snapshot).reason,
                    CONNECTION_CREDENTIALS_MISSING_CODE
                );
            }
            assert!(!fixture.live.reconnect_active.load(Ordering::Acquire));
            assert_eq!(handle.apply_count(), 1);
            assert_eq!(log_count(&fixture, "kernel.reconnect.r2_retry"), 0);
            worker.abort();
            fixture.live.stop(&stop_request(&[83; 16])).await.unwrap();
            fixture.live.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn disabling_reconnect_while_connected_prevents_engine_loss_acquisition() {
        let (fixture, handle) = connected_fixture("atrust", 2).await;
        let live = &fixture.live;
        let mut service = DarwinKernelControlService::new_inner(Arc::clone(&live.events), None, Some(Arc::clone(live)), DarwinUiConfig::default(), Arc::clone(&fixture.logs), Arc::clone(&live.proxy_tun));
        service.config = Arc::clone(&live.config);
        KernelControl::config_set(&service, Request::new(ConfigSetRequest { items: vec![config_item("auto_reconnect", "false")] })).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), live.drive_reconnect(ReconnectSignal::EngineLost)).await.expect("关闭后恢复决策应立即结束");
        assert!(live.auth_acquisition().is_none());
        assert_eq!(handle.apply_count(), 1);
        live.close().await.unwrap();
    }

    #[tokio::test]
    async fn clear_password_revokes_pending_recovery_before_apply() {
        use prost::Message;
        let (fixture, handle) = connected_fixture("atrust", 2).await;
        std::fs::remove_file(atrust_session_cache::cache_path(fixture._config_dir.path(), GATEWAY)).unwrap();
        let live = Arc::clone(&fixture.live);
        let recovery = tokio::spawn(async move { live.drive_reconnect(ReconnectSignal::EngineLost).await });
        wait_for(|| fixture.live.auth_acquisition().is_some()).await;
        let request = fixture.live.auth_acquisition().unwrap();
        let mut service = DarwinKernelControlService::new_inner(Arc::clone(&fixture.live.events), None, Some(Arc::clone(&fixture.live)), DarwinUiConfig::default(), Arc::clone(&fixture.logs), Arc::clone(&fixture.live.proxy_tun));
        service.config = Arc::clone(&fixture.live.config);
        KernelControl::config_set(&service, Request::new(ConfigSetRequest { items: vec![config_item("password", ""), config_item("remember_password", "false")] })).await.unwrap();
        let candidate = exv_vpn_darwin_ipc::browser_session::session_from_cookie_pairs(GATEWAY, "fixture-agent", vec![("sid".into(), "late-cleared".into())]).unwrap();
        let mut response = wire::InteractionResponse { interaction_id: request.interaction_id, runtime_epoch: request.runtime_epoch,
            response_payload: wire::AtrustAuthAcquisitionResult { result: Some(wire::atrust_auth_acquisition_result::Result::Candidate(crate::credential::UiAtrustCredentialPackage::new(candidate, true).to_bytes().unwrap().to_vec())) }.encode_to_vec() };
        let _ = fixture.live.respond_auth_acquisition(&mut response);
        tokio::time::timeout(Duration::from_secs(1), recovery).await.unwrap().unwrap();
        assert_eq!(handle.apply_count(), 1, "清除后的迟到候选不能Apply");
        fixture.live.close().await.unwrap();
    }

    #[tokio::test]
    async fn accepted_identity_atrust_uses_authenticated_payload_and_stays_frozen() {
        let (fixture, _) = connected_fixture("atrust", 2).await;
        let candidate = exv_vpn_darwin_ipc::browser_session::session_from_cookie_pairs(GATEWAY, "fixture-agent", vec![("sid".into(), "actual-account".into())]).unwrap();
        let credentials = crate::credential::UiAtrustCredentialPackage::new(candidate, true).with_authenticated_username("actual-school-user");
        fixture.live.connect(&connect_request(&[85; 16]), Some(UiSecretPayload::Atrust(credentials)), false).await.unwrap();
        assert_eq!(fixture.live.events.current_snapshot().unwrap().session_identity.unwrap().username, "actual-school-user");
        lock_recover(&fixture.live.config).apply_and_save(vec![DarwinConfigItem::new("username", "later-user")]).unwrap();
        assert_eq!(fixture.live.events.current_snapshot().unwrap().session_identity.unwrap().username, "actual-school-user");
        fixture.live.close().await.unwrap();
    }

    fn with_cause(mut status: Status, cause: &'static str) -> Status {
        status.metadata_mut().append(
            "x-exv-connect-cause",
            tonic::metadata::MetadataValue::from_static(cause),
        );
        status
    }
    #[test]
    fn r2_exact_metadata_classification_excludes_missing_unknown_duplicate_causes_and_storage_errors()
     {
        for cause in ["atrust-material-missing", "cisco-material-missing"] {
            let missing = with_cause(Status::failed_precondition("材料提示文案已改变"), cause);
            assert!(
                reconnect_materials_missing(&missing),
                "原因与message文案无关"
            );
            assert_eq!(
                reconnect_dispatch_vpn_error(&missing).reason,
                CONNECTION_CREDENTIALS_MISSING_CODE
            );
            let duplicate = with_cause(
                with_cause(
                    Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE),
                    cause,
                ),
                cause,
            );
            assert!(
                !reconnect_materials_missing(&duplicate),
                "重复cause不得命中"
            );
            for status in [
                with_cause(
                    Status::invalid_argument(CONNECTION_CREDENTIALS_MISSING_CODE),
                    cause,
                ),
                with_cause(
                    Status::unavailable(CONNECTION_CREDENTIALS_MISSING_CODE),
                    cause,
                ),
            ] {
                assert!(!reconnect_materials_missing(&status));
                assert!(reconnect_dispatch_vpn_error(&status).reason.is_empty());
            }
        }
        for status in [
            Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE),
            with_cause(
                Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE),
                "unknown-cause",
            ),
            Status::failed_precondition("DARWIN_ENGINE_ATTEMPT_ACTIVE"),
        ] {
            assert!(!reconnect_materials_missing(&status));
            assert!(reconnect_dispatch_vpn_error(&status).reason.is_empty());
        }
        for error in [
            DarwinConfigError::Storage,
            DarwinConfigError::PasswordCrypto,
        ] {
            let status = saved_envelope_status(&SavedConnectEnvelopeError::Config(error));
            assert_eq!(status.code(), Code::FailedPrecondition);
            assert_eq!(status.message(), CONNECTION_ENGINE_SESSION_CODE);
            assert_eq!(
                status
                    .metadata()
                    .get_all("x-exv-connect-cause")
                    .iter()
                    .count(),
                0
            );
            assert!(!reconnect_materials_missing(&status));
        }
    }

    #[tokio::test]
    async fn r2_other_failed_preconditions_retry_then_succeed_with_finite_or_unlimited_budget() {
        for max in [2, 0] {
            for status in [
                Status::failed_precondition(CONNECTION_CREDENTIALS_MISSING_CODE),
                Status::failed_precondition("DARWIN_ENGINE_ATTEMPT_ACTIVE"),
                Status::unavailable(CONNECTION_CREDENTIALS_MISSING_CODE),
            ] {
                let (fixture, handle) = connected_fixture("cisco", max).await;
                let worker = spawn_reconnect_worker(&fixture.live);
                handle.fail_next_apply(status);
                send_engine_lost(&fixture.live);
                wait_for(|| handle.applied_operation_ids().len() == 2).await;
                assert_eq!(log_count(&fixture, "kernel.reconnect.r2_retry"), 1);
                assert_eq!(log_count(&fixture, "kernel.reconnect.materials_missing"), 0);
                assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 2);
                worker.abort();
                fixture.live.stop(&stop_request(&[84; 16])).await.unwrap();
                fixture.live.close().await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn r2_temporary_dispatch_failure_exhausts_finite_budget_but_unlimited_can_stop_and_retry()
    {
        for max in [2, 0] {
            let (fixture, handle) = connected_fixture("cisco", max).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            for _ in 0..2 {
                handle.fail_next_apply(Status::unavailable(
                    "fixture engine temporarily unavailable",
                ));
            }
            send_engine_lost(&fixture.live);
            wait_for(|| log_count(&fixture, "kernel.reconnect.r2_retry") >= 2).await;
            if max == 2 {
                wait_for(|| log_count(&fixture, "kernel.reconnect.exhausted") == 1).await;
                wait_for(|| !fixture.live.reconnect_active.load(Ordering::Acquire)).await;
                assert_eq!(fixture.live.reconnect_attempts.load(Ordering::Relaxed), 2);
            } else {
                assert_eq!(log_count(&fixture, "kernel.reconnect.exhausted"), 0);
                fixture.live.stop(&stop_request(&[85; 16])).await.unwrap();
                tokio::time::sleep(Duration::from_millis(80)).await;
                assert!(!fixture.live.reconnect_active.load(Ordering::Acquire));
            }
            assert_eq!(
                handle.applied_operation_ids().len(),
                1,
                "临时失败未被受理为新Apply"
            );
            assert_eq!(log_count(&fixture, "kernel.reconnect.materials_missing"), 0);
            // 两个真实派发拒绝已消费，Engine恢复后用户可显式重试。
            fixture
                .live
                .connect(&connect_request(&[86; 16]), None, false)
                .await
                .unwrap();
            assert_eq!(handle.applied_operation_ids().len(), 2);
            worker.abort();
            fixture.live.close().await.unwrap();
        }
    }

    /// S5b（2026-10-26）：受理快照 + 一次性资格关闭 + ENGINE_LOST 时点活评估（C-10）
    /// 的 darwin core 合同组。夹具复用本文件既有 helper；config_set 走与生产同构的
    /// `KernelControl::config_set`（触发 `refresh_background_permission` 降权联动）。
    mod s5b_reconnect_snapshot_tests {
        use super::*;

        /// 经生产 config_set 入口提交配置（驱动 refresh_background_permission）。
        async fn config_set(live: &Arc<LiveConnectionProjection>, logs: &Arc<LogControl>, items: Vec<ConfigItem>) {
            let mut service = DarwinKernelControlService::new_inner(
                Arc::clone(&live.events), None, Some(Arc::clone(live)),
                DarwinUiConfig::default(), Arc::clone(logs), Arc::clone(&live.proxy_tun));
            service.config = Arc::clone(&live.config);
            KernelControl::config_set(&service, Request::new(ConfigSetRequest { items })).await.unwrap();
        }

        /// 断言当前快照到达指定终态并返回之。
        async fn await_terminal(live: &Arc<LiveConnectionProjection>) -> RuntimeSnapshot {
            wait_for(|| {
                live.events.current_snapshot().is_some_and(|s| {
                    matches!(s.state, Some(runtime_snapshot::State::FailedClean(_)))
                })
            })
            .await;
            live.events.current_snapshot().unwrap()
        }

        /// S5b-1：受理快照免疫连接中的凭据/用户名修改——替换凭据对（新用户名+新
        /// 密码，owner 随写、磁盘对仍有效）后掉线，自动重连的信封用户名/密码必须
        /// 等于受理快照值（受理账户），绝不改用磁盘上已替换的新账户。
        #[tokio::test]
        async fn reconnect_uses_admission_snapshot_after_mid_connection_credential_replacement() {
            let LiveFixture { _config_dir, live, mut events, handles, .. } = reconnect_live_fixture(true, 0, false, None);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![31_u8; 16];
            let handle = drive_manual_connect_to_connected(&live, &mut events, &handles, &operation_id).await;
            assert!(
                lock_recover(&live.reconnect_session_policy).as_ref().unwrap().credential_snapshot.is_some(),
                "磁盘回退受理冻结受理快照"
            );
            // 连接中 replace：新用户名+新密码（磁盘对仍 owner 有效，auto_reconnect 不变）。
            lock_recover(&live.config)
                .apply_ui_credentials_and_save("replacement-owner", Some("replacement-password"))
                .unwrap();
            send_retryable_drop(&handle, &operation_id);
            let _ = await_engine_execution_end(&mut events).await;
            live.signal_reconnect();
            wait_for_apply_count(&handle, 2).await;
            let payloads = handle.applied_payloads();
            let (username, password) = decode_applied_credentials(&payloads[1]);
            assert_eq!(username, "student", "重连信封用户名=受理快照值");
            assert_eq!(password, "fixture-password", "重连信封密码=受理快照值");
            worker.abort();
            live.close().await.unwrap();
        }

        /// S5b-2：一次性连接（persist=false 且用户名≠已保存账户）受理期关闭重连资格，
        /// 置暂停 reason（toast 通道），无快照；ENGINE_LOST 活评估（磁盘臂被受理身份
        /// 失配挡住）不派发重连。
        #[tokio::test]
        async fn one_shot_foreign_username_closes_eligibility_and_reports_pause_reason() {
            let LiveFixture { _config_dir, live, handles, .. } = reconnect_live_fixture(true, 0, false, None);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![32_u8; 16];
            live.connect(
                &connect_request(&operation_id),
                Some(UiSecretPayload::Cisco(UiCredentialPackage::new("one-shot-user", "one-shot-secret"))),
                false,
            ).await.unwrap();
            let handle = engine_handle(&handles);
            send_connecting_then_connected(&handle, &operation_id);
            wait_for(|| live.events.current_snapshot().is_some_and(|s| matches!(s.state, Some(runtime_snapshot::State::Connected(_))))).await;
            {
                let policy = lock_recover(&live.reconnect_session_policy);
                let policy = policy.as_ref().unwrap();
                assert!(!policy.enabled, "一次性异用户名连接受理期关闭资格");
                assert!(policy.credential_snapshot.is_none(), "一次性连接不建快照");
                assert_eq!(policy.pause_reason, Some(RECONNECT_PAUSE_OWNER_MISMATCH));
            }
            // 接受快照后第一条发布即携带暂停 reason（toast lane 可观察）。
            wait_for(|| {
                live.events.current_snapshot().is_some_and(|s|
                    s.reconnect.as_ref().is_some_and(|r| r.reason == RECONNECT_PAUSE_OWNER_MISMATCH))
            }).await;
            send_engine_lost(&live);
            let _terminal = await_terminal(&live).await;
            // 终态由清理 Stop 收口（策略随 Stop 清空，reason lane 已在上方连接期快照
            // 验证——此处只证明资格关闭不派发）。
            assert_eq!(live.reconnect_attempts.load(Ordering::Relaxed), 0, "资格关闭不消耗预算");
            assert_eq!(handle.apply_count(), 1, "无重连派发");
            worker.abort();
            live.close().await.unwrap();
        }

        /// S5b-3：显式清除密码降权联动丢弃受理快照——暂停 reason=credentials_cleared，
        /// 活评估无任何有效凭据源，ENGINE_LOST 不派发。
        #[tokio::test]
        async fn clearing_saved_password_drops_snapshot_and_pauses_reconnect() {
            let LiveFixture { _config_dir, live, mut events, handles, logs, .. } = reconnect_live_fixture(true, 0, false, None);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![33_u8; 16];
            let handle = drive_manual_connect_to_connected(&live, &mut events, &handles, &operation_id).await;
            config_set(&live, &logs, vec![
                config_item("password", ""),
                config_item("remember_password", "false"),
            ]).await;
            {
                let policy = lock_recover(&live.reconnect_session_policy);
                let policy = policy.as_ref().unwrap();
                assert!(!policy.enabled);
                assert!(policy.credential_snapshot.is_none(), "清除密码联动丢弃受理快照");
                assert_eq!(policy.pause_reason, Some(RECONNECT_PAUSE_CREDENTIALS_CLEARED));
            }
            wait_for(|| {
                live.events.current_snapshot().is_some_and(|s|
                    s.reconnect.as_ref().is_some_and(|r| r.reason == RECONNECT_PAUSE_CREDENTIALS_CLEARED))
            }).await;
            send_engine_lost(&live);
            let _terminal = await_terminal(&live).await;
            assert_eq!(handle.apply_count(), 1, "快照已丢弃且磁盘对已清除——不派发");
            worker.abort();
            live.close().await.unwrap();
        }

        /// S5b-4a：三臂之一——受理快照单独有效（磁盘对不可解密时仍按快照重连）。
        #[tokio::test]
        async fn engine_lost_snapshot_arm_alone_carries_reconnect() {
            let LiveFixture { _config_dir, live, mut events, handles, .. } = reconnect_live_fixture(true, 0, false, None);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![34_u8; 16];
            let handle = drive_manual_connect_to_connected(&live, &mut events, &handles, &operation_id).await;
            // 磁盘臂独立失效：key.bin 移除（密文不可解密 → has_saved_credential_pair=false）。
            std::fs::remove_file(_config_dir.path().join("key.bin")).unwrap();
            assert!(!DarwinUiConfig::load_from_dir(_config_dir.path()).unwrap().has_saved_credential_pair());
            send_engine_lost(&live);
            wait_for_apply_count(&handle, 2).await;
            let payloads = handle.applied_payloads();
            let (username, password) = decode_applied_credentials(&payloads[1]);
            assert_eq!(username, "student", "快照臂重连用受理快照账密");
            assert_eq!(password, "fixture-password");
            worker.abort();
            live.close().await.unwrap();
        }

        /// S5b-4b：三臂之二——owner 匹配磁盘对单独有效（同用户名一次性连接无快照，
        /// 磁盘账密对 owner 匹配且与受理身份一致即触发重连）。
        #[tokio::test]
        async fn engine_lost_owner_matched_disk_pair_arm_alone_carries_reconnect() {
            let LiveFixture { _config_dir, live, handles, .. } = reconnect_live_fixture(true, 0, false, None);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![35_u8; 16];
            live.connect(
                &connect_request(&operation_id),
                Some(UiSecretPayload::Cisco(UiCredentialPackage::new("student", "one-shot-variant-secret"))),
                false,
            ).await.unwrap();
            let handle = engine_handle(&handles);
            send_connecting_then_connected(&handle, &operation_id);
            wait_for(|| live.events.current_snapshot().is_some_and(|s| matches!(s.state, Some(runtime_snapshot::State::Connected(_))))).await;
            assert!(lock_recover(&live.reconnect_session_policy).as_ref().unwrap().credential_snapshot.is_none());
            assert!(lock_recover(&live.reconnect_session_policy).as_ref().unwrap().enabled, "同账户一次性连接保留资格（磁盘臂承担）");
            send_engine_lost(&live);
            wait_for_apply_count(&handle, 2).await;
            let payloads = handle.applied_payloads();
            let (username, password) = decode_applied_credentials(&payloads[1]);
            assert_eq!(username, "student", "磁盘臂重连用磁盘账密对");
            assert_eq!(password, "fixture-password");
            worker.abort();
            live.close().await.unwrap();
        }

        /// S5b-4c：三臂之三——aTrust 会话臂单独有效（无任何 Cisco 账密对时 ENGINE_LOST
        /// 仍可触发重连；消灭凭据门槛对网页登录用户的恒假）。config.json 直接落盘
        /// 绕过 apply 门（darwin 配置层在无凭据对时强制关闭 auto_reconnect——该层
        /// 行为不属本步骤文件面；活评估公式按 auto_reconnect 原值求值）。
        #[tokio::test]
        async fn engine_lost_atrust_session_arm_alone_carries_reconnect() {
            let fixture = reconnect_live_fixture(true, 0, false, None);
            std::fs::write(fixture._config_dir.path().join("config.json"), r#"{"protocol":"atrust","atrust_server":"vpn.example.edu","atrust_auth_mode":"headless","atrust_auto_login":true,"auto_reconnect":true,"auto_reconnect_max_attempts":0,"auto_reconnect_backoff":false}"#).unwrap();
            *lock_recover(&fixture.live.config) = DarwinUiConfig::load_from_dir(fixture._config_dir.path()).unwrap();
            assert!(!DarwinUiConfig::load_from_dir(fixture._config_dir.path()).unwrap().has_saved_credential_pair(), "无 Cisco 账密对");
            assert!(lock_recover(&fixture.live.config).auto_reconnect());
            cache_material(&fixture);
            let live = Arc::clone(&fixture.live);
            let mut events = fixture.events;
            let handles = Arc::clone(&fixture.handles);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![36_u8; 16];
            let handle = drive_manual_connect_to_connected(&live, &mut events, &handles, &operation_id).await;
            send_engine_lost(&live);
            wait_for_apply_count(&handle, 2).await;
            assert_ne!(handle.applied_operation_ids()[1], operation_id, "aTrust 会话臂触发真实重连派发");
            worker.abort();
            live.close().await.unwrap();
        }

        /// S5b-5：policy==None 生命周期洞（用户 Stop 清空后复用会话在连、内部清理路径
        /// 不再安装）——真实 EOF 的活评估不依赖 policy：按当前活配置求值仍发射
        /// ReconnectSignal；worker 合成策略派发重连。
        #[tokio::test]
        async fn engine_lost_live_evaluation_survives_missing_session_policy() {
            let LiveFixture { _config_dir, live, mut events, handles, .. } =
                reconnect_live_fixture_with_launch_mode("service", true, 0, false, None);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![37_u8; 16];
            let handle = drive_manual_connect_to_connected(&live, &mut events, &handles, &operation_id).await;
            // 模拟诊断观察到的生命周期洞：连接在途而策略槽已被清空（Stop 清空 +
            // 后续会话复用未重装）。快照随策略一并丢失——活评估改由磁盘臂承担。
            *lock_recover(&live.reconnect_session_policy) = None;
            handle.clone().kill();
            assert_engine_lost_terminal("engine-lost", &mut events, &operation_id).await;
            // service 形态失联后槽位清空——重连会重新拉起 Engine（新 handle 承接 apply）。
            wait_for(|| {
                handles.lock().expect("fixture handles").iter().map(|engine| engine.apply_count()).sum::<usize>() >= 2
            }).await;
            let reconnected = handles.lock().expect("fixture handles").last().cloned().unwrap();
            let applied = reconnected.applied_operation_ids();
            assert!(!applied.is_empty() && applied[applied.len() - 1] != operation_id, "活评估仍触发重连");
            let (username, password) = decode_applied_credentials(&reconnected.applied_payloads()[applied.len() - 1]);
            assert_eq!(username, "student");
            assert_eq!(password, "fixture-password");
            worker.abort();
            live.close().await.unwrap();
        }

        /// S5b-6：快照随会话策略同生命周期——受理在位、显式 Stop 清空（Drop 零化），
        /// 新会话受理重建新快照。
        #[tokio::test]
        async fn snapshot_lifecycle_follows_session_policy_slot() {
            let LiveFixture { _config_dir, live, mut events, handles, .. } = reconnect_live_fixture(true, 0, false, None);
            let mut second_events: EventStream = Box::pin(live.events.subscribe(live.events.current_tick()));
            let handle = drive_manual_connect_to_connected(&live, &mut events, &handles, &[38_u8; 16]).await;
            assert!(lock_recover(&live.reconnect_session_policy).as_ref().unwrap().credential_snapshot.is_some());
            live.stop(&stop_request(&[39_u8; 16])).await.unwrap();
            wait_for_stop_count(&handle, 1).await;
            assert!(lock_recover(&live.reconnect_session_policy).is_none(), "显式 Stop 清空策略（快照随之零化）");
            // 新会话受理重建快照（快照值=新受理时的已持久化对）。
            lock_recover(&live.config)
                .apply_ui_credentials_and_save("second-owner", Some("second-password"))
                .unwrap();
            let handle2 = drive_manual_connect_to_connected(&live, &mut second_events, &handles, &[40_u8; 16]).await;
            let _ = handle2;
            {
                let policy = lock_recover(&live.reconnect_session_policy);
                let snapshot = policy.as_ref().unwrap().credential_snapshot.as_ref().unwrap();
                assert_eq!(snapshot.username, "second-owner");
                assert_eq!(snapshot.password.as_str(), "second-password");
            }
            live.close().await.unwrap();
        }

        /// S5b-7：owner 失配暂停 toast 通道——keep 分支仅改用户名（密文保留、owner 失
        /// 配，配置层同时强制关闭 auto_reconnect）→ 暂停 reason=owner_mismatch 经快照
        /// reason lane 可观察；ENGINE_LOST 不派发。受理快照保留（只有清除密码才丢
        /// 弃），但 auto_reconnect 已关——资格暂停。
        #[tokio::test]
        async fn owner_mismatch_pause_is_observable_and_blocks_reconnect() {
            let LiveFixture { _config_dir, live, mut events, handles, logs, .. } = reconnect_live_fixture(true, 0, false, None);
            let worker = spawn_reconnect_worker(&live);
            let operation_id = vec![41_u8; 16];
            let handle = drive_manual_connect_to_connected(&live, &mut events, &handles, &operation_id).await;
            config_set(&live, &logs, vec![config_item("username", "other-student")]).await;
            {
                let policy = lock_recover(&live.reconnect_session_policy);
                let policy = policy.as_ref().unwrap();
                assert!(!policy.enabled, "owner 失配暂停重连");
                assert!(policy.credential_snapshot.is_some(), "keep 分支不丢弃快照（仅清除密码丢弃）");
                assert_eq!(policy.pause_reason, Some(RECONNECT_PAUSE_OWNER_MISMATCH));
            }
            wait_for(|| {
                live.events.current_snapshot().is_some_and(|s|
                    s.reconnect.as_ref().is_some_and(|r| r.reason == RECONNECT_PAUSE_OWNER_MISMATCH))
            }).await;
            send_engine_lost(&live);
            let _terminal = await_terminal(&live).await;
            assert_eq!(handle.apply_count(), 1, "暂停期不派发重连");
            worker.abort();
            live.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn r2_material_cleanup_await_cannot_overwrite_new_stop_or_connect_intent() {
        for next in ["stop", "connect"] {
            let (fixture, handle) = connected_fixture("cisco", 0).await;
            let worker = spawn_reconnect_worker(&fixture.live);
            remove_frozen_material_and_switch_protocol(&fixture, "username");
            // Cisco 凭据对移除→资格即时失效（b63987bb）；R2 仍走同一真实清理 Stop
            // 收口。先占用 session 锁：清理 Stop 的 owned 等待被阻塞，期间用户的
            // 新意图换代会取消旧清理的接纳资格。
            let session_lock = fixture.live.session.lock().await;
            send_engine_lost(&fixture.live);
            wait_for(|| log_count(&fixture, "kernel.reconnect.disabled") == 1).await;
            let generation = fixture.live.reconnect_generation.current();
            lock_recover(&fixture.live.config)
                .apply_and_save(vec![DarwinConfigItem::new("protocol", "cisco")])
                .unwrap();
            lock_recover(&fixture.live.config)
                .apply_ui_credentials_and_save("new-owner", Some("new-password"))
                .unwrap();
            let new_request = {
                let live = fixture.live.clone();
                tokio::spawn(async move {
                    if next == "stop" {
                        live.stop(&stop_request(&[87; 16])).await
                    } else {
                        live.connect(&connect_request(&[88; 16]), None, false).await
                    }
                })
            };
            wait_for(|| fixture.live.reconnect_generation.current() != generation).await;
            drop(session_lock);
            new_request.await.unwrap().unwrap();
            tokio::time::sleep(Duration::from_millis(80)).await;
            let snapshot = fixture.live.events.current_snapshot().unwrap();
            if next == "connect" {
                assert_eq!(snapshot.operation_id, vec![88; 16]);
                assert_eq!(
                    snapshot.session_identity.as_ref().unwrap().username,
                    "new-owner"
                );
                assert_eq!(handle.apply_count(), 2);
                assert!(lock_recover(&fixture.live.reconnect_session_policy).is_some());
            } else {
                assert!(!matches!(
                    snapshot.state,
                    Some(runtime_snapshot::State::FailedClean(_))
                ));
                assert!(snapshot.session_identity.is_none());
                assert_eq!(handle.apply_count(), 1);
                assert!(lock_recover(&fixture.live.reconnect_session_policy).is_none());
            }
            assert!(!fixture.live.reconnect_active.load(Ordering::Acquire));
            worker.abort();
            fixture.live.close().await.unwrap();
        }
    }
}
