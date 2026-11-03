// 真实恢复入口的候选交接；只替代缓存档案位置，Engine/派发守卫仍走原路径。
mod recovery_material_regression {
    use super::*;

    #[tokio::test]
    async fn r2_uses_frozen_authorized_candidate_and_missing_material_does_not_requeue() {
        for (automatic, has_candidate, rejected) in [
            (true, true, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
            let dir = seeded_config_dir("fixture");
            let profile = tempfile::tempdir().unwrap();
            let mut config = ExvConfig::load_from_dir(dir.path()).unwrap();
            config.protocol = Protocol::Atrust;
            config.server = "vpn.ecnu.edu.cn".into();
            config.atrust_auto_login = automatic;
            config.atrust_auth_mode = if automatic { "headless" } else { "browser" }.into();
            config.auto_reconnect = true;
            config.auto_reconnect_backoff = false;
            config.save_to_dir(dir.path()).unwrap();
            let (service, _logs) = service_with_config(&engine, dir.path().into());
            let (status_tx, status_rx) = mpsc::unbounded_channel();
            engine
                .lock()
                .await
                .events
                .lock()
                .unwrap()
                .replace(status_rx);
            let forwarder = service.spawn_status_forwarder();
            let mut ready = service.status_ready();
            while !ready.borrow().attached {
                ready.changed().await.unwrap();
            }
            let mut initial = bound_browser_request("https://vpn.ecnu.edu.cn");
            initial.auth_binding.as_mut().unwrap().mode = if automatic { "headless" } else { "browser" }.into();
            service.connect_request(&mut initial, false).await.unwrap();
            let candidate = exv_vpn_win32_ipc::browser_session::BrowserSession::new(
                "https://vpn.ecnu.edu.cn".into(),
                "sid=recovery-fixture".into(),
                "fixture-agent".into(),
            )
            .unwrap();
            if has_candidate {
                crate::atrust_session_cache::save(profile.path(), &candidate).unwrap();
            }
            service
                .reconnect_atrust_context
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .profile = Ok(crate::browser_cache_profile::BrowserDataDirs::Explicit(
                profile.path().into(),
            ));
            // 新设置不覆盖本会话授权或网关。
            config.atrust_auto_login = !automatic;
            config.server = "other.example.edu".into();
            config.save_to_dir(dir.path()).unwrap();
            let (tx, mut rx) = mpsc::unbounded_channel();
            *service.reconnect_tx.lock().unwrap() = Some(tx);
            if rejected {
                *engine.lock().await.apply_error.lock().unwrap() = Some(GrpcClientError::Rpc(
                    tonic::Code::Unauthenticated,
                    "fixture authentication rejected".into(),
                ));
            }
            let origin = service.current_reconnect_dispatch_id().unwrap();
            let recovery = tokio::spawn({
                let service = service.clone();
                async move {
                    service
                        .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
                        .await;
                }
            });
            if !has_candidate || rejected {
                let request = wait_acquisition(&service).await;
                reply_acquisition(
                    &service,
                    &request,
                    Err(auth_error(if rejected {
                        "fixture authentication rejected"
                    } else {
                        "atrust_browser_required|fixture missing school credentials"
                    })),
                )
                .await
                .unwrap();
            }
            recovery.await.unwrap();
            assert_eq!(
                engine.lock().await.applies.lock().unwrap().len(),
                if has_candidate { 2 } else { 1 }
            );
            assert!(rx.try_recv().is_err(), "缺材料不得续排同一空请求");
            if has_candidate && !rejected {
                let applied = engine.lock().await.applies.lock().unwrap()[1].1.clone();
                let restored =
                    exv_vpn_win32_ipc::browser_session::BrowserSession::decode(&applied).unwrap();
                assert_eq!(restored.cookie_header(), "sid=recovery-fixture");
                assert_eq!(restored.gateway(), "https://vpn.ecnu.edu.cn");
                let operation = engine.lock().await.applies.lock().unwrap()[1]
                    .0
                    .lookup_key
                    .as_ref()
                    .unwrap()
                    .operation_id
                    .clone();
                status_tx
                    .send(EngineStatusEvent::Connected {
                        reconnect_state: None,
                        operation_id: operation,
                        session_established_at_ms: Some(1),
                    })
                    .unwrap();
                wait_snapshot_state(&service.events, "connected").await;
                assert!(!service.reconnect_active.load(Ordering::Acquire));
            } else {
                assert!(!service.reconnect_active.load(Ordering::Acquire));
                let snapshot = service.events.current_snapshot().unwrap();
                let error = match snapshot.state.unwrap() {
                    wire::runtime_snapshot::State::FailedDirty(f) => f.last_error.unwrap(),
                    wire::runtime_snapshot::State::FailedClean(f) => f.last_error.unwrap(),
                    other => panic!("恢复材料缺失应投影失败: {other:?}"),
                };
                assert_ne!(error.code, wire::ErrorCode::SessionBusy as i32);
                assert_eq!(
                    error.operation_detail, None,
                    "材料与认证失败不生产停止操作详情"
                );
                assert!(if rejected {
                    error.reason == "fixture authentication rejected"
                } else {
                    error.reason.starts_with("atrust_browser_required|")
                });
            }
            forwarder.abort();
        }
    }
    #[tokio::test]
    async fn recovered_candidate_cannot_apply_after_stop_or_new_connect() {
        for replace in [false, true] {
            let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
            let dir = seeded_config_dir("fixture");
            let profile = tempfile::tempdir().unwrap();
            let mut config = ExvConfig::load_from_dir(dir.path()).unwrap();
            config.protocol = Protocol::Atrust;
            config.server = "vpn.ecnu.edu.cn".into();
            config.atrust_auto_login = true;
            config.atrust_auth_mode = "headless".into();
            config.auto_reconnect = true;
            config.auto_reconnect_backoff = false;
            config.save_to_dir(dir.path()).unwrap();
            let (service, _logs) = service_with_config(&engine, dir.path().into());
            let forwarder = spawn_status_ready(&service, &engine).await;
            let mut initial = bound_browser_request("https://vpn.ecnu.edu.cn");
            initial.auth_binding.as_mut().unwrap().mode = "headless".into();
            service.connect_request(&mut initial, false).await.unwrap();
            let candidate = exv_vpn_win32_ipc::browser_session::BrowserSession::new(
                "https://vpn.ecnu.edu.cn".into(),
                "sid=late-recovery".into(),
                "fixture-agent".into(),
            )
            .unwrap();
            crate::atrust_session_cache::save(profile.path(), &candidate).unwrap();
            service
                .reconnect_atrust_context
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .profile = Ok(crate::browser_cache_profile::BrowserDataDirs::Explicit(
                profile.path().into(),
            ));
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            *engine.lock().await.lease_entered.lock().unwrap() = Some(entered.clone());
            *engine.lock().await.lease_block.lock().unwrap() = Some(release.clone());
            let origin = service.current_reconnect_dispatch_id().unwrap();
            let recovery = tokio::spawn({
                let service = service.clone();
                async move {
                    service
                        .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
                        .await
                }
            });
            tokio::time::timeout(Duration::from_secs(1), entered.notified())
                .await
                .unwrap();
            if replace {
                let mut fresh = bound_browser_request("https://vpn.ecnu.edu.cn");
                fresh.auth_binding.as_mut().unwrap().mode = "headless".into();
                fresh
                    .intent
                    .as_mut()
                    .unwrap()
                    .lookup_key
                    .as_mut()
                    .unwrap()
                    .operation_id = vec![99; 16];
                fresh.intent.as_mut().unwrap().request_digest = vec![99; 32];
                tokio::time::timeout(
                    Duration::from_secs(1),
                    service.connect_request(&mut fresh, false),
                )
                .await
                .unwrap()
                .unwrap();
            } else {
                tokio::time::timeout(
                    Duration::from_secs(1),
                    service.stop(Request::new(StopRequest {
                        intent: Some(stop_intent()),
                    })),
                )
                .await
                .unwrap()
                .unwrap();
            }
            release.notify_one();
            recovery.await.unwrap();
            let applies = engine.lock().await.applies.lock().unwrap().clone();
            assert_eq!(applies.len(), if replace { 2 } else { 1 });
            assert!(applies.iter().all(|(_, payload)| {
                exv_vpn_win32_ipc::browser_session::BrowserSession::decode(payload)
                    .unwrap()
                    .cookie_header()
                    != "sid=late-recovery"
            }));
            forwarder.abort();
        }
    }

    use exv_vpn_win32_ipc::auth_deadline::AuthDeadline;
    use prost::Message;

    struct AuthFixture {
        service: KernelControlService,
        engine: Arc<tokio::sync::Mutex<FakeEngine>>,
        status: mpsc::UnboundedSender<EngineStatusEvent>,
        forwarder: tokio::task::JoinHandle<()>,
        worker: tokio::task::JoinHandle<()>,
        _dir: tempfile::TempDir,
        _profile: tempfile::TempDir,
    }
    impl Drop for AuthFixture {
        fn drop(&mut self) {
            self.worker.abort();
            self.forwarder.abort();
        }
    }
    async fn auth_fixture(candidate: bool, max_attempts: u32) -> AuthFixture {
        let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
        *engine.lock().await.pending_apply.lock().unwrap() = true;
        let dir = seeded_config_dir("fixture");
        let profile = tempfile::tempdir().unwrap();
        let mut config = ExvConfig::load_from_dir(dir.path()).unwrap();
        config.protocol = Protocol::Atrust;
        config.server = "vpn.ecnu.edu.cn".into();
        config.atrust_auto_login = true;
        config.atrust_auth_mode = "headless".into();
        config.auto_reconnect = true;
        config.auto_reconnect_max_attempts = max_attempts;
        config.auto_reconnect_backoff = false;
        config.save_to_dir(dir.path()).unwrap();
        let (service, _) = service_with_config(&engine, dir.path().into());
        let (status, receiver) = mpsc::unbounded_channel();
        engine.lock().await.events.lock().unwrap().replace(receiver);
        let forwarder = service.spawn_status_forwarder();
        let mut ready = service.status_ready();
        while !ready.borrow().attached {
            ready.changed().await.unwrap();
        }
        let mut request = bound_browser_request("https://vpn.ecnu.edu.cn");
        request.auth_binding.as_mut().unwrap().mode = "headless".into();
        service.connect_request(&mut request, false).await.unwrap();
        if candidate {
            crate::atrust_session_cache::save(profile.path(), &auth_candidate("sid=old-candidate"))
                .unwrap();
        }
        service
            .reconnect_atrust_context
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .profile = Ok(crate::browser_cache_profile::BrowserDataDirs::Explicit(
            profile.path().into(),
        ));
        let worker = service.spawn_reconnect_worker();
        AuthFixture {
            service,
            engine,
            status,
            forwarder,
            worker,
            _dir: dir,
            _profile: profile,
        }
    }
    fn auth_candidate(cookie: &str) -> exv_vpn_win32_ipc::browser_session::BrowserSession {
        exv_vpn_win32_ipc::browser_session::BrowserSession::new(
            "https://vpn.ecnu.edu.cn".into(),
            cookie.into(),
            "fixture-agent".into(),
        )
        .unwrap()
    }
    fn auth_error(reason: &str) -> wire::VpnError {
        wire::VpnError {
            code: wire::ErrorCode::Unauthorized as i32,
            stage: wire::ErrorStage::ProtocolSession as i32,
            reason: reason.into(),
            retry: wire::RetryAdvice::UseNewOperation as i32,
            ..Default::default()
        }
    }
    async fn wait_acquisition(service: &KernelControlService) -> wire::AtrustAuthAcquisition {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(request) = service
                    .events
                    .current_snapshot()
                    .and_then(|s| s.atrust_auth_acquisition)
                {
                    return request;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("original event publishes acquisition")
    }
    async fn reply_acquisition(
        service: &KernelControlService,
        request: &wire::AtrustAuthAcquisition,
        result: Result<&str, wire::VpnError>,
    ) -> Result<Response<OperationReply>, Status> {
        let result = match result {
            Ok(cookie) => wire::atrust_auth_acquisition_result::Result::Candidate(
                auth_candidate(cookie).encode().unwrap().to_vec(),
            ),
            Err(error) => wire::atrust_auth_acquisition_result::Result::Failed(error),
        };
        let mut payload = wire::AtrustAuthAcquisitionResult {
            result: Some(result),
        }
        .encode_to_vec();
        let response = InteractionResponse {
            interaction_id: request.interaction_id.clone(),
            runtime_epoch: request.runtime_epoch.clone(),
            response_payload: std::mem::take(&mut payload),
        };
        service.respond_interaction(Request::new(response)).await
    }
    async fn assert_cancelled_receipt_cleared(
        service: &KernelControlService,
        request: &wire::AtrustAuthAcquisition,
    ) {
        let mut response = InteractionResponse {
            interaction_id: request.interaction_id.clone(),
            runtime_epoch: request.runtime_epoch.clone(),
            response_payload: b"cancelled acquisition secret fixture".to_vec(),
        };
        assert!(
            service
                .respond_auth_acquisition(&mut response)
                .await
                .is_err()
        );
        assert!(
            response.response_payload.is_empty(),
            "本地槽必须接管秘密并走 Zeroizing 清零路径"
        );
    }

    #[tokio::test]
    async fn reconnect_requires_decryptable_saved_pair() {
        for protocol in [Protocol::Cisco, Protocol::Atrust] {
            for missing in ["username", "password", "key", "decrypt"] {
                let dir = seeded_config_dir("fixture-password");
                let mut config = ExvConfig::load_from_dir(dir.path()).unwrap();
                config.protocol = protocol;
                config.set_password_encrypted("fixture-password", &ExvConfig::load_key(dir.path()).unwrap().unwrap()).unwrap();
                match missing {
                    "username" => config.username = "  ".into(),
                    "password" => config.password.clear(),
                    "key" => std::fs::remove_file(dir.path().join("key.bin")).unwrap(),
                    "decrypt" => std::fs::write(dir.path().join("key.bin"), [71_u8; 32]).unwrap(),
                    _ => unreachable!(),
                }
                config.auto_reconnect = false;
                config.save_to_dir(dir.path()).unwrap();
                let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
                let (service, _logs) = service_with_config(&engine, dir.path().into());
                let result = service.config_set(Request::new(ConfigSetRequest { items: vec![ConfigItem { key: "auto_reconnect".into(), value: "true".into() }] })).await;
                assert!(result.is_err(), "{protocol:?}/{missing} 缺完整保存对不得接受开启");
                assert!(!ExvConfig::load_from_dir(dir.path()).unwrap().auto_reconnect);
            }
        }
    }

    #[tokio::test]
    async fn clear_password_disables_reconnect_atomically() {
        for protocol in [Protocol::Cisco, Protocol::Atrust] {
            let dir = seeded_config_dir("fixture-password");
            let mut config = ExvConfig::load_from_dir(dir.path()).unwrap();
            config.protocol = protocol;
            config.auto_reconnect = true;
            config.save_to_dir(dir.path()).unwrap();
            let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
            let (service, _logs) = service_with_config(&engine, dir.path().into());
            service.config_set(Request::new(ConfigSetRequest { items: vec![
                ConfigItem { key: "password".into(), value: String::new() },
                ConfigItem { key: "remember_password".into(), value: "false".into() },
            ] })).await.unwrap();
            let saved = ExvConfig::load_from_dir(dir.path()).unwrap();
            assert!(saved.password.is_empty());
            assert_eq!(saved.username, "student");
            assert!(!saved.auto_reconnect, "清除密码与关闭重连必须同次持久化");
        }
    }

    #[tokio::test]
    async fn saving_pair_does_not_enable_reconnect() {
        for protocol in ["cisco", "atrust"] {
            let dir = seeded_config_dir("fixture-password");
            let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
            let (service, _logs) = service_with_config(&engine, dir.path().into());
            service.config_set(Request::new(ConfigSetRequest { items: vec![
                ConfigItem { key: "protocol".into(), value: protocol.into() },
                ConfigItem { key: "username".into(), value: "new-student".into() },
                ConfigItem { key: "password".into(), value: "replacement".into() },
                ConfigItem { key: "remember_password".into(), value: "true".into() },
            ] })).await.unwrap();
            assert!(!ExvConfig::load_from_dir(dir.path()).unwrap().auto_reconnect);
        }
    }

    #[tokio::test]
    async fn legacy_atrust_new_submission_mode_is_authoritative_in_both_key_orders() {
        for mode in ["browser", "embedded", "headless"] {
            for reverse in [false, true] {
                let dir = seeded_config_dir("fixture-password");
                let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
                let (service, _logs) = service_with_config(&engine, dir.path().into());
                let mut items = vec![
                    ConfigItem { key: "atrust_auth_mode".into(), value: mode.into() },
                    ConfigItem { key: "atrust_auto_login".into(), value: (mode != "headless").to_string() },
                ];
                if reverse { items.reverse(); }
                service.config_set(Request::new(ConfigSetRequest { items })).await.unwrap();
                let saved = ExvConfig::load_from_dir(dir.path()).unwrap();
                assert_eq!(saved.atrust_auth_mode, mode);
                assert_eq!(saved.atrust_auto_login, mode == "headless", "新版 mode 为权威，兼容 bool 只派生");
            }
        }
    }

    #[tokio::test]
    async fn disabling_reconnect_while_connected_prevents_engine_loss_acquisition() {
        let f = auth_fixture(false, 2).await;
        f.service.config_set(Request::new(ConfigSetRequest { items: vec![ConfigItem { key: "auto_reconnect".into(), value: "false".into() }] })).await.unwrap();
        let recovery = start_recovery(&f.service);
        tokio::time::timeout(Duration::from_secs(1), recovery).await.expect("关闭后恢复决策应立即结束").unwrap();
        assert!(f.service.events.current_snapshot().unwrap().atrust_auth_acquisition.is_none());
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn clear_password_revokes_pending_recovery_before_apply() {
        let f = auth_fixture(false, 2).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        f.service.config_set(Request::new(ConfigSetRequest { items: vec![
            ConfigItem { key: "password".into(), value: String::new() },
            ConfigItem { key: "remember_password".into(), value: "false".into() },
        ] })).await.unwrap();
        let _ = reply_acquisition(&f.service, &request, Ok("sid=late-cleared")).await;
        tokio::time::timeout(Duration::from_secs(1), recovery).await.expect("原恢复轮应收束").unwrap();
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1, "清除后迟到候选不能 Apply");
    }

    #[tokio::test]
    async fn unknown_manual_browser_identity_stays_empty() {
        let f = auth_fixture(false, 2).await;
        assert_eq!(f.service.events.current_session_identity().unwrap().username, "", "仅有配置用户名不能代表网页实际登录身份");
    }

    #[tokio::test]
    async fn accepted_identity_uses_actual_credentials_not_later_config() {
        let dir = seeded_config_dir("fixture-password");
        let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with_config(&engine, dir.path().into());
        let forwarder = spawn_status_ready(&service, &engine).await;
        let mut request = ConnectRequest { intent: Some(connect_intent()), auth_binding: None, secret_payload: ui_credential_payload() };
        service.connect_request(&mut request, false).await.unwrap();
        assert_eq!(service.events.current_session_identity().unwrap().username, "alice", "当前账户来自本次真实凭据");
        service.config_set(Request::new(ConfigSetRequest { items: vec![ConfigItem { key: "username".into(), value: "later-user".into() }] })).await.unwrap();
        assert_eq!(service.events.current_session_identity().unwrap().username, "alice");
        forwarder.abort();
    }

    #[tokio::test]
    async fn accepted_identity_atrust_uses_authenticated_payload_and_stays_frozen() {
        let f = auth_fixture(false, 2).await;
        let mut request = bound_browser_request("https://vpn.ecnu.edu.cn");
        request.intent.as_mut().unwrap().lookup_key.as_mut().unwrap().operation_id = vec![87; 16];
        request.intent.as_mut().unwrap().request_digest = vec![88; 32];
        request.auth_binding.as_mut().unwrap().mode = "headless".into();
        let mut value: serde_json::Value = serde_json::from_slice(&request.secret_payload).unwrap();
        value["authenticated_username"] = serde_json::Value::String("actual-school-user".into());
        request.secret_payload = serde_json::to_vec(&value).unwrap();
        f.service.connect_request(&mut request, false).await.unwrap();
        assert_eq!(f.service.events.current_session_identity().unwrap().username, "actual-school-user");
        f.service.config_set(Request::new(ConfigSetRequest { items: vec![ConfigItem { key: "username".into(), value: "later-user".into() }] })).await.unwrap();
        assert_eq!(f.service.events.current_session_identity().unwrap().username, "actual-school-user");
    }

    #[tokio::test]
    async fn legacy_atrust_config_get_migrates_before_startup_hydration() {
        let dir = seeded_config_dir("fixture-password");
        let mut value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("atrust_auth_mode_version");
        value["atrust_auth_mode"] = serde_json::Value::String("browser".into());
        value["atrust_auto_login"] = serde_json::Value::Bool(true);
        std::fs::write(dir.path().join("config.json"), value.to_string()).unwrap();
        let engine = Arc::new(tokio::sync::Mutex::new(FakeEngine::default()));
        let (service, _logs) = service_with_config(&engine, dir.path().into());
        for _ in 0..2 {
            let payload = service.config_get(Request::new(ConfigGetRequest {})).await.unwrap().into_inner();
            assert_eq!(payload.items.iter().find(|item| item.key == "atrust_auth_mode").unwrap().value, "headless");
            assert_eq!(payload.items.iter().find(|item| item.key == "atrust_auto_login").unwrap().value, "true");
        }
    }

    fn start_recovery(service: &KernelControlService) -> tokio::task::JoinHandle<()> {
        let origin = service.current_reconnect_dispatch_id().unwrap();
        let service = service.clone();
        tokio::spawn(async move {
            service
                .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
                .await;
        })
    }
    async fn fail_candidate(f: &AuthFixture, index: usize, reason: &str) -> Vec<u8> {
        let operation = apply_op_id(&f.engine, index).await;
        f.status
            .send(EngineStatusEvent::Failed {
                operation_id: operation.clone(),
                error: auth_error(reason),
                reconnect_state: None,
            })
            .unwrap();
        operation
    }
    async fn wait_round_ended(service: &KernelControlService) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if service
                    .recovery_round
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|r| r.ended)
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn pending_cleanup(f: &AuthFixture) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(key) = f
                    .engine
                    .lock()
                    .await
                    .stops
                    .lock()
                    .unwrap()
                    .last()
                    .and_then(|r| r.lookup_key.clone())
                {
                    return key.operation_id;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    fn failed_error(service: &KernelControlService) -> wire::VpnError {
        match service.events.current_snapshot().unwrap().state.unwrap() {
            wire::runtime_snapshot::State::FailedDirty(f) => f.last_error.unwrap(),
            wire::runtime_snapshot::State::FailedClean(f) => f.last_error.unwrap(),
            other => panic!("expected failure: {other:?}"),
        }
    }

    #[tokio::test]
    async fn recovery_material_request_preserves_default_and_explicit_selection() {
        for explicit in [false, true] {
            let f = auth_fixture(false, 1).await;
            let profile = f._profile.path().to_path_buf();
            f.service
                .reconnect_atrust_context
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .profile = Ok(
                crate::browser_cache_profile::BrowserDataDirs::from_frozen_profile(
                    profile.clone(),
                    explicit,
                ),
            );
            let recovery = start_recovery(&f.service);
            let request = wait_acquisition(&f.service).await;
            assert_eq!(request.browser_profile, profile.to_string_lossy());
            assert_eq!(request.browser_profile_explicit, explicit);
            reply_acquisition(&f.service, &request, Ok("sid=fresh"))
                .await
                .unwrap();
            recovery.await.unwrap();
        }
    }

    #[tokio::test]
    async fn recovery_material_missing_waits_same_round_and_success_applies_once() {
        let f = auth_fixture(false, 1).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
        assert!(f.service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert_eq!(request.gateway, "https://vpn.ecnu.edu.cn");
        assert_eq!(request.mode, "headless");
        assert!(request.auto_login);
        assert_eq!(request.config_dir, f._dir.path().to_string_lossy());
        assert_eq!(request.browser_profile, f._profile.path().to_string_lossy());
        assert!(request.browser_profile_explicit);
        assert!(request.user_agent.is_empty());
        assert_ne!(request.runtime_epoch, vec![0; 16]);
        assert!(
            AuthDeadline::from_monotonic_ns(request.deadline_monotonic_ns)
                .unwrap()
                .remaining()
                .unwrap()
                <= Duration::from_secs(300)
        );
        let mut changed = ExvConfig::load_from_dir(f._dir.path()).unwrap();
        changed.server = "changed.example.edu".into();
        changed.atrust_auto_login = false;
        changed.atrust_auth_mode = "browser".into();
        changed.save_to_dir(f._dir.path()).unwrap();
        reply_acquisition(&f.service, &request, Ok("sid=fresh"))
            .await
            .unwrap();
        recovery.await.unwrap();
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
        assert!(
            f.engine
                .lock()
                .await
                .interactions
                .lock()
                .unwrap()
                .is_empty()
        );
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert!(
            reply_acquisition(&f.service, &request, Ok("sid=duplicate"))
                .await
                .is_err()
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        f.status
            .send(EngineStatusEvent::Connected {
                operation_id: apply_op_id(&f.engine, 1).await,
                reconnect_state: None,
                session_established_at_ms: Some(1),
            })
            .unwrap();
        wait_snapshot_state(&f.service.events, "connected").await;
        assert!(f.service.recovery_round.lock().unwrap().is_none());
        assert!(
            reply_acquisition(&f.service, &request, Ok("sid=old-after-success"))
                .await
                .is_err()
        );
        assert_cancelled_receipt_cleared(&f.service, &request).await;
        assert!(
            f.engine
                .lock()
                .await
                .interactions
                .lock()
                .unwrap()
                .is_empty()
        );
        let ordinary = InteractionResponse {
            interaction_id: vec![0x11; 16],
            runtime_epoch: f
                .service
                .composition
                .lock()
                .await
                .runtime_epoch_bytes()
                .to_vec(),
            response_payload: b"ordinary answer".to_vec(),
        };
        f.service
            .respond_interaction(Request::new(ordinary.clone()))
            .await
            .unwrap();
        assert_eq!(
            *f.engine.lock().await.interactions.lock().unwrap(),
            vec![ordinary]
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn recovery_material_wrong_epoch_is_not_consumed_and_true_failure_is_preserved() {
        let f = auth_fixture(false, 2).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        let mut wrong = request.clone();
        wrong.runtime_epoch = vec![44; 16];
        assert!(
            reply_acquisition(&f.service, &wrong, Ok("sid=wrong"))
                .await
                .is_err()
        );
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_some()
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
        let failure = auth_error("fixture exact credential failure");
        reply_acquisition(&f.service, &request, Err(failure.clone()))
            .await
            .unwrap();
        recovery.await.unwrap();
        assert_eq!(failed_error(&f.service), failure);
        assert!(!f.service.reconnect_active.load(Ordering::Acquire));
        assert!(
            reply_acquisition(&f.service, &request, Ok("sid=late"))
                .await
                .is_err()
        );
        f.service
            .drive_reconnect(ReconnectTrigger::EngineLostService)
            .await;
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn recovery_material_malformed_tail_clears_partial_candidate_without_consuming_request() {
        let f = auth_fixture(false, 1).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        let mut response = InteractionResponse {
            interaction_id: request.interaction_id.clone(),
            runtime_epoch: request.runtime_epoch.clone(),
            response_payload: wire::AtrustAuthAcquisitionResult {
                result: Some(wire::atrust_auth_acquisition_result::Result::Candidate(
                    auth_candidate("sid=malformed-tail")
                        .encode()
                        .unwrap()
                        .to_vec(),
                )),
            }
            .encode_to_vec(),
        };
        response.response_payload.push(0);
        let mut partial = wire::AtrustAuthAcquisitionResult::default();
        let error = crate::kernel_control_service::auth_continuation::merge_acquisition_result(
            &mut partial,
            &response.response_payload,
        )
        .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        assert!(matches!(
            partial.result,
            Some(wire::atrust_auth_acquisition_result::Result::Candidate(candidate))
                if candidate.is_empty()
        ));
        assert_eq!(
            f.service
                .respond_auth_acquisition(&mut response)
                .await
                .unwrap_err()
                .code(),
            Code::InvalidArgument
        );
        assert!(response.response_payload.is_empty());
        assert_eq!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition,
            Some(request.clone())
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
        reply_acquisition(&f.service, &request, Ok("sid=fresh-after-invalid"))
            .await
            .unwrap();
        recovery.await.unwrap();
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn recovery_material_pending_failure_waits_actual_stop_then_last_round_new_execution() {
        let f = auth_fixture(true, 1).await;
        start_recovery(&f.service).await.unwrap();
        *f.engine.lock().await.stop_reply.lock().unwrap() = Some(wire::StopTunnelReply {
            result: Some(wire::stop_tunnel_reply::Result::Pending(
                wire::EmptyOperationState {},
            )),
            stop_completion: None,
        });
        let old_operation = fail_candidate(&f, 1, "candidate rejected").await;
        let cleanup_operation = pending_cleanup(&f).await;
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        assert!(f.service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        // 同一旧执行重复失效与EOF都不能再占预算或另排获取。
        f.status
            .send(EngineStatusEvent::Failed {
                operation_id: old_operation.clone(),
                error: auth_error("duplicate"),
                reconnect_state: None,
            })
            .unwrap();
        {
            let mut composition = f.service.composition.lock().await;
            let basis = f.service.observation_basis(&composition);
            assert!(matches!(
                f.service
                    .accept_session_input(
                        &mut composition,
                        SessionInput::ChannelFault {
                            basis,
                            subscribed: true
                        }
                    )
                    .unwrap(),
                SessionOutput::Recovery(None)
            ));
        }
        f.status
            .send(EngineStatusEvent::Stopped {
                operation_id: cleanup_operation,
                reconnect_state: None,
                stop_completion: Some(wire::StopCompletion {
                    stopped: true,
                    blocks_new_connect: false,
                    errors: vec![],
                }),
            })
            .unwrap();
        let request = wait_acquisition(&f.service).await;
        let (round_operation, old_key, old_digest) = {
            let record = f.service.recovery_round.lock().unwrap();
            let round = record.as_ref().unwrap();
            (
                round.round_operation,
                round.execution.lookup_key.clone().unwrap(),
                round.execution.request_digest.clone(),
            )
        };
        assert_eq!(round_operation.as_slice(), old_operation.as_slice());
        let old_apply_digest = f.engine.lock().await.applies.lock().unwrap()[1]
            .0
            .request_digest
            .clone();
        assert_eq!(
            connection_mode_bound_digest(&old_digest, ConnectionMode::Standard),
            old_apply_digest
        );
        reply_acquisition(&f.service, &request, Ok("sid=new-execution"))
            .await
            .unwrap();
        wait_for_apply_count(&f.engine, 3).await;
        let (new_key, new_digest) = {
            let engine = f.engine.lock().await;
            let applies = engine.applies.lock().unwrap();
            let request = &applies[2].0;
            (
                request.lookup_key.clone().unwrap(),
                request.request_digest.clone(),
            )
        };
        assert_ne!(old_key.operation_id, new_key.operation_id);
        assert_ne!(old_key.runtime_epoch, new_key.runtime_epoch);
        // 本地保存 ConnectIntent；Engine Apply 的摘要还要绑定本次连接模式。
        // 新旧摘要必须在同一 Engine 阶段比较，不能把原始摘要与派生摘要混比。
        assert_ne!(old_apply_digest, new_digest);
        {
            let record = f.service.recovery_round.lock().unwrap();
            let round = record.as_ref().unwrap();
            assert_eq!(round.round_operation, round_operation);
            assert_eq!(round.attempt, 1);
            let mut execution_key = round.execution.lookup_key.clone().unwrap();
            assert_eq!(execution_key.method, wire::OperationMethod::Connect as i32);
            execution_key.method = wire::OperationMethod::ApplyTunnel as i32;
            assert_eq!(execution_key, new_key);
            assert_ne!(old_digest, round.execution.request_digest);
            assert_eq!(
                connection_mode_bound_digest(
                    &round.execution.request_digest,
                    ConnectionMode::Standard,
                ),
                new_digest
            );
        }
        f.status
            .send(EngineStatusEvent::Failed {
                operation_id: old_operation.clone(),
                error: auth_error("old late failed"),
                reconnect_state: None,
            })
            .unwrap();
        f.status
            .send(EngineStatusEvent::Progress {
                operation_id: old_operation,
                reconnect_state: None,
                connect_phase: ConnectPhase::ConnectingControl,
            })
            .unwrap();
        f.status
            .send(EngineStatusEvent::Connected {
                operation_id: new_key.operation_id,
                reconnect_state: None,
                session_established_at_ms: Some(1),
            })
            .unwrap();
        wait_snapshot_state(&f.service.events, "connected").await;
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 3);
        assert_eq!(f.engine.lock().await.stops.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recovery_material_blocking_stop_retains_auth_cause_and_cleanup_error() {
        let f = auth_fixture(true, 1).await;
        start_recovery(&f.service).await.unwrap();
        let cleanup_error = wire::VpnError {
            code: wire::ErrorCode::EffectUnknown as i32,
            stage: wire::ErrorStage::Teardown as i32,
            reason: "fixture adapter cleanup failed".into(),
            ..Default::default()
        };
        *f.engine.lock().await.stop_reply.lock().unwrap() = Some(wire::StopTunnelReply {
            result: Some(wire::stop_tunnel_reply::Result::Failed(
                wire::OperationFailed {
                    receipt: None,
                    error: Some(cleanup_error.clone()),
                },
            )),
            stop_completion: Some(wire::StopCompletion {
                stopped: false,
                blocks_new_connect: true,
                errors: vec![cleanup_error.clone()],
            }),
        });
        fail_candidate(&f, 1, "original authentication cause").await;
        wait_round_ended(&f.service).await;
        assert_eq!(
            failed_error(&f.service).reason,
            "original authentication cause"
        );
        assert_eq!(
            f.service
                .composition
                .lock()
                .await
                .stop_completion
                .as_ref()
                .unwrap()
                .errors,
            vec![cleanup_error]
        );
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        assert!(!f.service.reconnect_active.load(Ordering::Acquire));
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
            let f = auth_fixture(true, 1).await;
            start_recovery(&f.service).await.unwrap();
            fail_candidate(&f, 1, "old candidate rejected").await;
            let request = wait_acquisition(&f.service).await;
            assert!(
                f.service
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
                native: Some(wire::RedactedNativeError {
                    code: 67,
                    ..Default::default()
                }),
                retry: wire::RetryAdvice::UseNewOperation as i32,
                certainty: wire::EffectCertainty::NoEffect as i32,
                ..Default::default()
            };
            reply_acquisition(&f.service, &request, Err(failure.clone()))
                .await
                .unwrap();
            wait_round_ended(&f.service).await;
            if code == wire::ErrorCode::CancelledBeforeStart && reason == "atrust/auth_cancelled" {
                assert_eq!(failed_error(&f.service), failure);
            } else {
                assert_eq!(
                    failed_error(&f.service),
                    auth_error("old candidate rejected")
                );
            }
            assert_eq!(
                f.service
                    .recovery_round
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .first_error,
                Some(auth_error("old candidate rejected"))
            );
            assert!(!f.service.reconnect_active.load(Ordering::Acquire));
            assert!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .atrust_auth_acquisition
                    .is_none()
            );
            assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
            assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        }
    }

    #[tokio::test]
    async fn recovery_material_stop_rpc_error_is_not_auth_success_or_new_apply() {
        let f = auth_fixture(true, 2).await;
        start_recovery(&f.service).await.unwrap();
        *f.engine.lock().await.stop_error.lock().unwrap() = Some(GrpcClientError::Rpc(
            Code::Unavailable,
            "fixture stop transport failed".into(),
        ));
        fail_candidate(&f, 1, "auth first cause").await;
        wait_round_ended(&f.service).await;
        assert_eq!(failed_error(&f.service).reason, "auth first cause");
        assert!(
            f.service
                .composition
                .lock()
                .await
                .stop_completion
                .as_ref()
                .unwrap()
                .errors[0]
                .reason
                .contains("fixture stop transport failed")
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_material_cleanup_user_stop_or_connect_cancels_old_continuation() {
        for replacement in [false, true] {
            let f = auth_fixture(true, 1).await;
            start_recovery(&f.service).await.unwrap();
            *f.engine.lock().await.stop_reply.lock().unwrap() = Some(wire::StopTunnelReply {
                result: Some(wire::stop_tunnel_reply::Result::Pending(
                    wire::EmptyOperationState {},
                )),
                stop_completion: None,
            });
            fail_candidate(&f, 1, "candidate rejected").await;
            let cleanup_operation = pending_cleanup(&f).await;
            tokio::time::advance(STOP_TOTAL_BUDGET + Duration::from_secs(1)).await;
            wait_round_ended(&f.service).await;
            if replacement {
                let mut request = bound_browser_request("https://vpn.ecnu.edu.cn");
                request.auth_binding.as_mut().unwrap().mode = "headless".into();
                request
                    .intent
                    .as_mut()
                    .unwrap()
                    .lookup_key
                    .as_mut()
                    .unwrap()
                    .operation_id = vec![94; 16];
                request.intent.as_mut().unwrap().request_digest = vec![94; 32];
                f.service
                    .connect_request(&mut request, false)
                    .await
                    .unwrap();
            } else {
                *f.engine.lock().await.stop_reply.lock().unwrap() = None;
                f.service
                    .stop(Request::new(StopRequest {
                        intent: Some(stop_intent()),
                    }))
                    .await
                    .unwrap();
            }
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let replacement_snapshot = f.service.events.current_snapshot().unwrap();
            f.status
                .send(EngineStatusEvent::Stopped {
                    operation_id: cleanup_operation,
                    reconnect_state: None,
                    stop_completion: Some(wire::StopCompletion {
                        stopped: true,
                        blocks_new_connect: false,
                        errors: vec![],
                    }),
                })
                .unwrap();
            // 原worker处理完旧续段之后的普通信号没有第二份获取机会。
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                f.service.events.current_snapshot(),
                Some(replacement_snapshot)
            );
            assert!(f.service.recovery_round.lock().unwrap().is_none());
            assert!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .atrust_auth_acquisition
                    .is_none()
            );
            assert_eq!(
                f.engine.lock().await.applies.lock().unwrap().len(),
                if replacement { 3 } else { 2 }
            );
        }
    }

    #[tokio::test]
    async fn recovery_material_fresh_failure_cleans_and_ends_without_second_acquisition() {
        let f = auth_fixture(true, 2).await;
        start_recovery(&f.service).await.unwrap();
        fail_candidate(&f, 1, "first candidate rejected").await;
        let request = wait_acquisition(&f.service).await;
        reply_acquisition(&f.service, &request, Ok("sid=fresh-rejected"))
            .await
            .unwrap();
        wait_for_apply_count(&f.engine, 3).await;
        fail_candidate(&f, 2, "fresh authentication rejected").await;
        wait_round_ended(&f.service).await;
        assert_eq!(f.engine.lock().await.stops.lock().unwrap().len(), 2);
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 3);
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert!(!f.service.reconnect_active.load(Ordering::Acquire));
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
        f.service
            .drive_reconnect(ReconnectTrigger::EngineLostService)
            .await;
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn recovery_material_waiting_stop_or_new_connect_rejects_late_receipt() {
        for replacement in [false, true] {
            let f = auth_fixture(false, 2).await;
            let recovery = start_recovery(&f.service);
            let request = wait_acquisition(&f.service).await;
            if replacement {
                let mut next = bound_browser_request("https://vpn.ecnu.edu.cn");
                next.auth_binding.as_mut().unwrap().mode = "headless".into();
                next.intent
                    .as_mut()
                    .unwrap()
                    .lookup_key
                    .as_mut()
                    .unwrap()
                    .operation_id = vec![95; 16];
                next.intent.as_mut().unwrap().request_digest = vec![95; 32];
                f.service.connect_request(&mut next, false).await.unwrap();
            } else {
                f.service
                    .stop(Request::new(StopRequest {
                        intent: Some(stop_intent()),
                    }))
                    .await
                    .unwrap();
            }
            recovery.await.unwrap();
            assert!(
                reply_acquisition(&f.service, &request, Ok("sid=late-cancelled"))
                    .await
                    .is_err()
            );
            assert_cancelled_receipt_cleared(&f.service, &request).await;
            assert!(
                f.engine
                    .lock()
                    .await
                    .interactions
                    .lock()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                f.engine.lock().await.applies.lock().unwrap().len(),
                if replacement { 2 } else { 1 }
            );
            assert!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .atrust_auth_acquisition
                    .is_none()
            );
            if replacement {
                // 替代会话再次进入 acquisition 后，旧域回执仍不能转发或消费新槽。
                f.service
                    .reconnect_atrust_context
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .profile = Ok(crate::browser_cache_profile::BrowserDataDirs::Explicit(
                    f._profile.path().into(),
                ));
                let next_recovery = start_recovery(&f.service);
                let next_request = wait_acquisition(&f.service).await;
                assert_ne!(next_request.interaction_id, request.interaction_id);
                assert_eq!(next_request.runtime_epoch, request.runtime_epoch);
                assert!(
                    reply_acquisition(&f.service, &request, Ok("sid=old-during-new"))
                        .await
                        .is_err()
                );
                assert_cancelled_receipt_cleared(&f.service, &request).await;
                assert!(
                    f.engine
                        .lock()
                        .await
                        .interactions
                        .lock()
                        .unwrap()
                        .is_empty()
                );
                reply_acquisition(&f.service, &next_request, Ok("sid=new-acquisition"))
                    .await
                    .unwrap();
                next_recovery.await.unwrap();
                assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 3);
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_material_same_deadline_covers_return_and_final_apply_guard() {
        for asynchronous in [false, true] {
            let f = auth_fixture(asynchronous, 1).await;
            let recovery = start_recovery(&f.service);
            let recovery = if asynchronous {
                recovery.await.unwrap();
                fail_candidate(&f, 1, "candidate rejected before lease expiry").await;
                None
            } else {
                Some(recovery)
            };
            let request = wait_acquisition(&f.service).await;
            let entered = Arc::new(tokio::sync::Notify::new());
            // 故意永不 release：D 本身必须使准备栈退出并释放 Engine/active。
            let release = Arc::new(tokio::sync::Notify::new());
            *f.engine.lock().await.lease_entered.lock().unwrap() = Some(entered.clone());
            *f.engine.lock().await.lease_block.lock().unwrap() = Some(release);
            reply_acquisition(&f.service, &request, Ok("sid=expires-at-guard"))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), entered.notified())
                .await
                .unwrap();
            tokio::time::advance(Duration::from_secs(301)).await;
            wait_round_ended(&f.service).await;
            if let Some(recovery) = recovery {
                tokio::time::timeout(Duration::from_secs(2), recovery)
                    .await
                    .unwrap()
                    .unwrap();
            }
            assert_eq!(
                f.engine.lock().await.applies.lock().unwrap().len(),
                if asynchronous { 2 } else { 1 }
            );
            assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
            assert!(!f.service.reconnect_active.load(Ordering::Acquire));
            assert!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .atrust_auth_acquisition
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn recovery_material_expired_receipt_is_not_consumed_and_pending_wait_exits() {
        let f = auth_fixture(false, 1).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        {
            let _composition = f.service.composition.lock().await;
            f.service
                .recovery_round
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .deadline = Some(AuthDeadline::after(Duration::from_nanos(1)).unwrap());
        }
        tokio::time::sleep(Duration::from_millis(32)).await;
        assert_eq!(
            reply_acquisition(&f.service, &request, Ok("sid=expired"))
                .await
                .unwrap_err()
                .code(),
            Code::DeadlineExceeded
        );
        // 固定等待期限仍为原D；原取消结束测试等待，不另给期限。
        f.service
            .stop(Request::new(StopRequest {
                intent: Some(stop_intent()),
            }))
            .await
            .unwrap();
        recovery.await.unwrap();
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_material_wait_deadline_exits_and_clears_request() {
        let f = auth_fixture(false, 1).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        tokio::time::advance(Duration::from_secs(301)).await;
        recovery.await.unwrap();
        assert_eq!(
            failed_error(&f.service).code,
            wire::ErrorCode::DeadlineExceeded as i32
        );
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
        assert!(!f.service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
        assert!(
            reply_acquisition(&f.service, &request, Ok("sid=late-timeout"))
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_material_stop_rpc_deadline_accepts_real_ok_error_and_pending_completion() {
        for outcome in [0, 1, 2] {
            let f = auth_fixture(true, 1).await;
            start_recovery(&f.service).await.unwrap();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            {
                let mut engine = f.engine.lock().await;
                engine.stop_entered = Some(entered.clone());
                engine.stop_block = Some(release.clone());
                if outcome == 1 {
                    *engine.stop_error.lock().unwrap() = Some(GrpcClientError::Rpc(
                        Code::Unavailable,
                        "late stop RPC actual failure".into(),
                    ));
                } else if outcome == 2 {
                    *engine.stop_reply.lock().unwrap() = Some(wire::StopTunnelReply {
                        result: Some(wire::stop_tunnel_reply::Result::Pending(Default::default())),
                        stop_completion: None,
                    });
                }
            }
            fail_candidate(&f, 1, "old candidate rejected before RPC deadline").await;
            entered.notified().await;
            tokio::time::advance(STOP_TOTAL_BUDGET + Duration::from_secs(1)).await;
            wait_round_ended(&f.service).await;
            assert_eq!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .stop_completion
                    .unwrap()
                    .errors[0]
                    .code,
                wire::ErrorCode::DeadlineExceeded as i32
            );
            assert!(!f.service.reconnect_active.load(Ordering::Acquire));
            assert!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .atrust_auth_acquisition
                    .is_none()
            );
            release.notify_one();
            if outcome == 2 {
                let operation = pending_cleanup(&f).await;
                f.status
                    .send(EngineStatusEvent::Stopped {
                        operation_id: operation,
                        reconnect_state: None,
                        stop_completion: Some(wire::StopCompletion {
                            stopped: true,
                            blocks_new_connect: false,
                            errors: vec![],
                        }),
                    })
                    .unwrap();
            }
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let summary = f
                        .service
                        .events
                        .current_snapshot()
                        .unwrap()
                        .stop_completion
                        .unwrap();
                    if (outcome != 1 && summary.stopped)
                        || (outcome == 1
                            && summary
                                .errors
                                .iter()
                                .any(|error| error.reason.contains("late stop RPC actual failure")))
                    {
                        if outcome == 1 {
                            assert!(!summary.stopped);
                            assert!(!summary.errors.iter().any(
                                |error| error.code == wire::ErrorCode::DeadlineExceeded as i32
                            ));
                        } else {
                            assert_eq!(
                                summary,
                                wire::StopCompletion {
                                    stopped: true,
                                    blocks_new_connect: false,
                                    errors: vec![]
                                }
                            );
                            assert!(
                                !f.service
                                    .connect_dispatch
                                    .lock()
                                    .unwrap()
                                    .as_ref()
                                    .unwrap()
                                    .stop_required
                            );
                        }
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                failed_error(&f.service),
                auth_error("old candidate rejected before RPC deadline")
            );
            assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
            assert_eq!(f.engine.lock().await.stops.lock().unwrap().len(), 1);
            assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
            assert!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .atrust_auth_acquisition
                    .is_none()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_material_late_stop_rpc_cannot_update_replaced_engine() {
        for failed in [false, true] {
            let f = auth_fixture(true, 1).await;
            start_recovery(&f.service).await.unwrap();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            {
                let mut engine = f.engine.lock().await;
                engine.stop_entered = Some(entered.clone());
                engine.stop_block = Some(release.clone());
                if failed {
                    *engine.stop_error.lock().unwrap() = Some(GrpcClientError::Rpc(
                        Code::Unavailable,
                        "old engine late stop failure".into(),
                    ));
                }
            }
            fail_candidate(&f, 1, "candidate rejected before engine replacement").await;
            entered.notified().await;
            tokio::time::advance(STOP_TOTAL_BUDGET + Duration::from_secs(1)).await;
            wait_round_ended(&f.service).await;
            // 只观察原 owned RPC 的代际接纳；停止夹具的旧状态流投递。
            f.forwarder.abort();
            assert!(
                f.service
                    .engine
                    .swap(Arc::new(tokio::sync::Mutex::new(FakeEngine::default())))
                    .await
            );
            let snapshot = f.service.events.current_snapshot();
            release.notify_one();
            let _old_rpc_finished = f.engine.lock().await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            assert_eq!(f.service.events.current_snapshot(), snapshot);
            assert_eq!(_old_rpc_finished.applies.lock().unwrap().len(), 2);
            assert!(
                f.service
                    .events
                    .current_snapshot()
                    .unwrap()
                    .atrust_auth_acquisition
                    .is_none()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_material_pending_stop_deadline_preserves_auth_and_cleanup_failure() {
        let f = auth_fixture(true, 1).await;
        start_recovery(&f.service).await.unwrap();
        *f.engine.lock().await.stop_reply.lock().unwrap() = Some(wire::StopTunnelReply {
            result: Some(wire::stop_tunnel_reply::Result::Pending(
                wire::EmptyOperationState {},
            )),
            stop_completion: None,
        });
        fail_candidate(&f, 1, "auth rejected before stop timeout").await;
        let cleanup_operation = pending_cleanup(&f).await;
        tokio::time::advance(STOP_TOTAL_BUDGET + Duration::from_secs(1)).await;
        wait_round_ended(&f.service).await;
        assert_eq!(
            failed_error(&f.service).reason,
            "auth rejected before stop timeout"
        );
        assert_eq!(
            f.service
                .composition
                .lock()
                .await
                .stop_completion
                .as_ref()
                .unwrap()
                .errors[0]
                .code,
            wire::ErrorCode::DeadlineExceeded as i32
        );
        f.status
            .send(EngineStatusEvent::Stopped {
                operation_id: cleanup_operation,
                reconnect_state: None,
                stop_completion: Some(wire::StopCompletion {
                    stopped: true,
                    blocks_new_connect: false,
                    errors: vec![],
                }),
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !f
                .service
                .events
                .current_snapshot()
                .unwrap()
                .stop_completion
                .as_ref()
                .unwrap()
                .stopped
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            f.service.events.current_snapshot().unwrap().stop_completion,
            Some(wire::StopCompletion {
                stopped: true,
                blocks_new_connect: false,
                errors: vec![]
            })
        );
        assert_eq!(
            failed_error(&f.service).reason,
            "auth rejected before stop timeout"
        );
        assert!(
            !f.service
                .connect_dispatch
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .stop_required
        );
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
    }

    #[tokio::test]
    async fn recovery_material_missing_then_fresh_auth_failure_uses_same_one_time_slot() {
        let f = auth_fixture(false, 2).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        reply_acquisition(&f.service, &request, Ok("sid=fresh-from-missing"))
            .await
            .unwrap();
        recovery.await.unwrap();
        fail_candidate(&f, 1, "fresh from missing rejected").await;
        wait_round_ended(&f.service).await;
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        assert_eq!(f.engine.lock().await.stops.lock().unwrap().len(), 1);
        assert_eq!(
            failed_error(&f.service).reason,
            "fresh from missing rejected"
        );
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
    }

    #[tokio::test]
    async fn recovery_material_fresh_transport_failure_keeps_original_service_budget() {
        let f = auth_fixture(true, 2).await;
        start_recovery(&f.service).await.unwrap();
        fail_candidate(&f, 1, "old candidate rejected").await;
        let request = wait_acquisition(&f.service).await;
        *f.engine.lock().await.apply_error.lock().unwrap() = Some(GrpcClientError::Rpc(
            Code::Unavailable,
            "fixture service transport failure".into(),
        ));
        // 只替代本地服务形态事实，不访问真实SCM或网络。
        f.service.record_mode(ServiceMode::Service);
        reply_acquisition(&f.service, &request, Ok("sid=fresh-transport"))
            .await
            .unwrap();
        wait_for_apply_count(&f.engine, 4).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.service.reconnect_active.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 2);
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 4);
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
        assert_ne!(
            failed_error(&f.service).code,
            wire::ErrorCode::Unauthorized as i32
        );
    }

    #[tokio::test]
    async fn recovery_material_same_deadline_covers_engine_queue_without_late_apply() {
        let f = auth_fixture(false, 1).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        let engine_guard = f.engine.lock().await;
        {
            let _composition = f.service.composition.lock().await;
            f.service
                .recovery_round
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .deadline = Some(AuthDeadline::after(Duration::from_millis(32)).unwrap());
        }
        reply_acquisition(&f.service, &request, Ok("sid=queued-expiry"))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), recovery)
            .await
            .unwrap()
            .unwrap();
        assert!(!f.service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(engine_guard.applies.lock().unwrap().len(), 1);
        assert_eq!(
            failed_error(&f.service).code,
            wire::ErrorCode::DeadlineExceeded as i32
        );
        drop(engine_guard);
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
    }
    async fn wait_service_retry(
        f: &AuthFixture,
        receiver: &mut mpsc::UnboundedReceiver<(ReconnectTrigger, Uuid)>,
    ) -> Uuid {
        let (trigger, origin) = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(trigger, ReconnectTrigger::EngineLostService);
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert!(!f.service.reconnect_active.load(Ordering::Acquire));
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
        assert!(receiver.try_recv().is_err(), "同轮不得重复排队或完整获取");
        origin
    }

    #[tokio::test]
    async fn recovery_material_status_preparation_unavailable_keeps_service_budget() {
        let mut f = auth_fixture(true, 2).await;
        f.service.status_ready_wait = Duration::from_millis(20);
        f.worker.abort();
        f.worker = f.service.spawn_reconnect_worker();
        start_recovery(&f.service).await.unwrap();
        fail_candidate(&f, 1, "candidate authentication first cause").await;
        let request = wait_acquisition(&f.service).await;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        *f.service.reconnect_tx.lock().unwrap() = Some(sender);
        f.service.record_mode(ServiceMode::Service);
        let mut attachment = *f.service.status_attachment.borrow();
        attachment.attached = false;
        f.service.status_attachment.send_replace(attachment);
        reply_acquisition(&f.service, &request, Ok("sid=subscription-unavailable"))
            .await
            .unwrap();
        let origin = wait_service_retry(&f, &mut receiver).await;
        let error = failed_error(&f.service);
        assert_eq!(
            error.code,
            wire::ErrorCode::PlatformDependencyUnavailable as i32
        );
        assert!(error.reason.starts_with("status_stream_not_ready|"));
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        assert!(
            reply_acquisition(&f.service, &request, Ok("sid=duplicate-service-failure"))
                .await
                .is_err()
        );
        attachment.attached = true;
        f.service.status_attachment.send_replace(attachment);
        f.service
            .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
            .await;
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 2);
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 3);
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
    }

    #[tokio::test]
    async fn recovery_material_failed_service_receipt_preserves_error_and_original_budget() {
        let f = auth_fixture(true, 2).await;
        start_recovery(&f.service).await.unwrap();
        fail_candidate(&f, 1, "candidate rejected before acquisition service fault").await;
        let request = wait_acquisition(&f.service).await;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        *f.service.reconnect_tx.lock().unwrap() = Some(sender);
        f.service.record_mode(ServiceMode::Service);
        let mut error = route_failure_wire_error(&Status::unavailable(
            "fixture authentication service unavailable",
        ));
        error.reason = "fixture authentication service unavailable".into();
        error.stage = wire::ErrorStage::ConnectingControl as i32;
        reply_acquisition(&f.service, &request, Err(error.clone()))
            .await
            .unwrap();
        let origin = wait_service_retry(&f, &mut receiver).await;
        assert_eq!(failed_error(&f.service), error);
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 2);
        assert!(
            reply_acquisition(&f.service, &request, Ok("sid=duplicate-failed-receipt"))
                .await
                .is_err()
        );
        f.service
            .drive_reconnect_from(ReconnectTrigger::EngineLostService, Some(origin))
            .await;
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 2);
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 3);
        assert!(
            f.service
                .events
                .current_snapshot()
                .unwrap()
                .atrust_auth_acquisition
                .is_none()
        );
    }

    #[tokio::test]
    async fn recovery_material_unknown_failed_receipt_preserves_error_without_service_retry() {
        let f = auth_fixture(false, 2).await;
        let recovery = start_recovery(&f.service);
        let request = wait_acquisition(&f.service).await;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        *f.service.reconnect_tx.lock().unwrap() = Some(sender);
        let error = wire::VpnError {
            code: 9876,
            reason: "fixture unknown business failure".into(),
            stage: wire::ErrorStage::AwaitingInteraction as i32,
            ..Default::default()
        };
        reply_acquisition(&f.service, &request, Err(error.clone()))
            .await
            .unwrap();
        recovery.await.unwrap();
        assert_eq!(failed_error(&f.service), error);
        assert!(receiver.try_recv().is_err());
        assert_eq!(f.service.reconnect_attempts.load(Ordering::Relaxed), 1);
        assert!(!f.service.reconnect_active.load(Ordering::Acquire));
        assert_eq!(f.engine.lock().await.applies.lock().unwrap().len(), 1);
    }
}
