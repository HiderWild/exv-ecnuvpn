mod headless_cache_tests {
    //! 经真实 Core Connect 受理路径验证缓存准备；探针只替代网络事实，不替代派发。
    use super::*;
    use crate::atrust_session_probe::{ProbeError, SessionProbe};
    use crate::kernel_control_service::{SessionReconnectPolicy, cached_headless};
    use exv_vpn_darwin_ipc::browser_session::{BrowserSession, session_from_cookie_pairs};
    use exv_vpn_darwin_ipc::connect_envelope::{DarwinEngineConnectV2, EngineConnectPayload};
    use tokio::sync::Notify;

    const HEADLESS_GATEWAY: &str = "https://vpn.example.edu:443";

    fn headless_candidate(value: &str) -> BrowserSession {
        session_from_cookie_pairs(
            HEADLESS_GATEWAY,
            "fixture-cache-agent",
            vec![("sid".to_owned(), value.to_owned())],
        )
        .unwrap()
    }
    fn cached_headless_fixture(probe: cached_headless::ProbeSeam) -> LiveFixture {
        let mut fixture = live_fixture();
        lock_recover(&fixture.live.config)
            .apply_and_save(vec![
                DarwinConfigItem::new("protocol", "atrust"),
                DarwinConfigItem::new("atrust_server", "vpn.example.edu"),
                DarwinConfigItem::new("atrust_auth_mode", "headless"),
                DarwinConfigItem::new("atrust_auto_login", "true"),
            ])
            .unwrap();
        atrust_session_cache::save(
            fixture._config_dir.path(),
            &headless_candidate("old-cached-session"),
        )
        .unwrap();
        Arc::get_mut(&mut fixture.live)
            .expect("夹具尚未共享live所有权")
            .cached_session_probe = Some(probe);
        fixture
    }
    fn static_probe(result: Result<SessionProbe, ProbeError>) -> cached_headless::ProbeSeam {
        let result = Arc::new(Mutex::new(Some(result)));
        Arc::new(move || {
            let result = result
                .lock()
                .unwrap()
                .take()
                .expect("一次Connect只探测一次缓存");
            Box::pin(async move { result })
        })
    }
    fn assert_no_engine_dispatch(fixture: &LiveFixture) {
        assert_eq!(
            fixture.launches.load(Ordering::SeqCst),
            0,
            "缓存准备拒绝应发生在拉起Engine之前"
        );
        assert!(fixture.handles.lock().unwrap().is_empty());
    }
    fn assert_current_cache(fixture: &LiveFixture, value: &str) {
        let candidate =
            atrust_session_cache::load_candidate(fixture._config_dir.path(), HEADLESS_GATEWAY)
                .expect("原缓存或新版本仍在");
        assert_eq!(candidate.cookie_header(), format!("sid={value}"));
    }
    fn assert_cached_apply_is_not_fresh(fixture: &LiveFixture, operation: &[u8]) {
        let engine = fixture.handles.lock().unwrap()[0].clone();
        assert_eq!(engine.applied_operation_ids(), vec![operation.to_vec()]);
        let payloads = engine.applied_payloads();
        assert_eq!(payloads.len(), 1);
        let EngineConnectPayload::Atrust(material) =
            DarwinEngineConnectV2::decode(payloads[0].clone())
                .unwrap()
                .into_payload()
        else {
            panic!("缓存恢复必须派发aTrust信封")
        };
        assert!(!material.fresh_authentication(), "缓存候选不是本次新认证");
    }

    #[tokio::test]
    async fn frozen_manual_recovery_reuses_candidate_without_foreground_auto_login() {
        let fixture = cached_headless_fixture(Arc::new(|| panic!("手动授权不得探测缓存")));
        lock_recover(&fixture.live.config).apply_and_save(vec![
            DarwinConfigItem::new("atrust_auth_mode", "browser"),
        ]).unwrap();
        let policy = SessionReconnectPolicy::from_config(&lock_recover(&fixture.live.config));
        lock_recover(&fixture.live.config).apply_and_save(vec![
            DarwinConfigItem::new("atrust_auto_login", "true"),
        ]).unwrap();
        assert!(fixture.live.connect_payload_plan_and_candidate(None, false, Some(policy)).is_ok(), "后台恢复允许复用有效会话");
        assert_no_engine_dispatch(&fixture);
        assert_current_cache(&fixture, "old-cached-session");
    }

    #[tokio::test]
    async fn fresh_manual_direct_core_requires_current_payload_and_preserves_recovery_cache() {
        for mode in ["embedded", "browser"] {
            let fixture = cached_headless_fixture(static_probe(Ok(SessionProbe {
                is_login: true, first_auth: None,
            })));
            let recovery = SessionReconnectPolicy::from_config(&lock_recover(&fixture.live.config));
            lock_recover(&fixture.live.config).apply_and_save(vec![
                DarwinConfigItem::new("atrust_auto_login", "false"),
                DarwinConfigItem::new("atrust_auth_mode", mode),
            ]).unwrap();
            let error = fixture.live.connect(&connect_request(&[79; 16]), None, false).await.unwrap_err();
            assert_eq!(error.code(), Code::FailedPrecondition);
            assert_eq!(error.message(), CONNECTION_CREDENTIALS_MISSING_CODE);
            assert_current_cache(&fixture, "old-cached-session");
            assert_no_engine_dispatch(&fixture);
            // 已建立会话恢复显式带 frozen policy，继续使用缓存且 fresh=false。
            let (payload, _, _, _, _) = fixture.live.connect_payload_plan_and_candidate(None, false, Some(recovery)).unwrap();
            let EngineConnectPayload::Atrust(material) = DarwinEngineConnectV2::decode(payload).unwrap().into_payload() else { panic!("aTrust恢复") };
            assert!(!material.fresh_authentication());
            // 同一手动配置接纳本次浏览器成果，不触碰无载荷缓存入口。
            fixture.live.connect(&connect_request(&[80; 16]),
                Some(UiSecretPayload::Atrust(UiAtrustCredentialPackage::new(headless_candidate("current-manual-session"), true))), false,
            ).await.unwrap();
            let engine = fixture.handles.lock().unwrap()[0].clone();
            assert_eq!(engine.applied_operation_ids(), vec![vec![80; 16]]);
            let EngineConnectPayload::Atrust(material) = DarwinEngineConnectV2::decode(engine.applied_payloads()[0].clone()).unwrap().into_payload() else { panic!("aTrust本次候选") };
            assert!(material.fresh_authentication());
            assert_current_cache(&fixture, "current-manual-session");
            fixture.live.close().await.unwrap();
        }
        // headless = 模式权威派生的自动登录：无 UI 载荷 + 有效缓存 + 探针确认登录
        // 即可直达 Apply（fresh=false），不需要「本次浏览器交付」载荷。
        let fixture = cached_headless_fixture(static_probe(Ok(SessionProbe {
            is_login: true, first_auth: None,
        })));
        let operation = [79; 16];
        fixture.live.connect(&connect_request(&operation), None, false).await.unwrap();
        assert_cached_apply_is_not_fresh(&fixture, &operation);
        assert_current_cache(&fixture, "old-cached-session");
        fixture.live.close().await.unwrap();
    }

    #[tokio::test]
    async fn headless_valid_cache_reaches_actual_apply_with_fresh_false() {
        let fixture = cached_headless_fixture(static_probe(Ok(SessionProbe {
            is_login: true,
            first_auth: None,
        })));
        let operation = [71; 16];
        fixture
            .live
            .connect(&connect_request(&operation), None, false)
            .await
            .unwrap();
        assert_cached_apply_is_not_fresh(&fixture, &operation);
        assert_current_cache(&fixture, "old-cached-session");
        fixture.live.close().await.unwrap();
    }

    #[tokio::test]
    async fn headless_logged_out_cache_returns_credentials_synchronously_without_runtime_failure() {
        let fixture = cached_headless_fixture(static_probe(Ok(SessionProbe {
            is_login: false,
            first_auth: Some("/passport/cas".to_owned()),
        })));
        let tick = fixture.live.events.current_tick();
        let error = fixture
            .live
            .connect(&connect_request(&[72; 16]), None, false)
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::FailedPrecondition);
        assert_eq!(error.message(), CONNECTION_CREDENTIALS_MISSING_CODE);
        assert_no_engine_dispatch(&fixture);
        assert_eq!(
            fixture.live.events.current_tick(),
            tick,
            "凭据准备不能发布Pending或Failed"
        );
        assert!(
            atrust_session_cache::load_candidate(fixture._config_dir.path(), HEADLESS_GATEWAY)
                .is_none()
        );
    }

    /// S2（2026-10-26 验收修复计划）：探测网络层失败（underlay/tls/timeout）从
    /// 凭据缺失合流中拆出——独立探测失败码拒绝（前端不弹凭据模态），缓存保留。
    #[tokio::test]
    async fn headless_unknown_network_or_tls_preserves_cache_and_returns_probe_unavailable()
    {
        for code in ["underlay", "tls", "timeout"] {
            let fixture = cached_headless_fixture(static_probe(Err(ProbeError {
                code,
                http_status: None,
            })));
            let tick = fixture.live.events.current_tick();
            let error = fixture
                .live
                .connect(&connect_request(&[73; 16]), None, false)
                .await
                .unwrap_err();
            assert_eq!(error.code(), Code::FailedPrecondition);
            assert_eq!(error.message(), CONNECTION_PROBE_UNAVAILABLE_CODE);
            assert_no_engine_dispatch(&fixture);
            assert_eq!(fixture.live.events.current_tick(), tick);
            assert_current_cache(&fixture, "old-cached-session");
        }
    }

    fn pending_cache_probe() -> (cached_headless::ProbeSeam, Arc<Notify>, Arc<Notify>) {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let probe = {
            let entered = entered.clone();
            let release = release.clone();
            Arc::new(move || {
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    // 明确LoggedOut可检验旧结果不会误删新版本。
                    Ok(SessionProbe {
                        is_login: false,
                        first_auth: None,
                    })
                })
                    as std::pin::Pin<
                        Box<dyn Future<Output = Result<SessionProbe, ProbeError>> + Send>,
                    >
            }) as cached_headless::ProbeSeam
        };
        (probe, entered, release)
    }

    #[tokio::test]
    async fn headless_pending_probe_releases_locks_and_rejects_stop_generation_config_or_cache_changes()
     {
        for scenario in ["stop", "new_generation", "config_changed", "cache_replaced"] {
            let (probe, entered, release) = pending_cache_probe();
            let fixture = cached_headless_fixture(probe);
            let connecting = {
                let live = fixture.live.clone();
                tokio::spawn(
                    async move { live.connect(&connect_request(&[74; 16]), None, false).await },
                )
            };
            tokio::time::timeout(Duration::from_secs(1), entered.notified())
                .await
                .expect("必须进入真实缓存探针挂起点");
            assert!(
                fixture.live.config.try_lock().is_ok(),
                "网络等待期间配置锁可取得"
            );
            assert!(
                fixture.live.session.try_lock().is_ok(),
                "网络等待期间会话锁可取得"
            );
            match scenario {
                "stop" => {
                    fixture.live.stop(&stop_request(&[75; 16])).await.unwrap();
                }
                "new_generation" => {
                    let error = fixture
                        .live
                        .connect(&ConnectRequest::default(), None, false)
                        .await
                        .unwrap_err();
                    assert_eq!(error.code(), Code::InvalidArgument);
                }
                "config_changed" => {
                    // `atrust_auto_login` 由认证入口模式派生（单独提交不构成真实
                    // 变更）；改模式本身才是探针复核识别的真实配置变更。
                    lock_recover(&fixture.live.config)
                        .apply_and_save(vec![DarwinConfigItem::new("atrust_auth_mode", "browser")])
                        .unwrap();
                }
                "cache_replaced" => {
                    atrust_session_cache::save(
                        fixture._config_dir.path(),
                        &headless_candidate("newer-cached-session"),
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            release.notify_one();
            let error = tokio::time::timeout(Duration::from_secs(1), connecting)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(
                error.code(),
                if matches!(scenario, "stop" | "new_generation") {
                    Code::Cancelled
                } else {
                    Code::Aborted
                }
            );
            assert_no_engine_dispatch(&fixture);
            assert_current_cache(
                &fixture,
                if scenario == "cache_replaced" {
                    "newer-cached-session"
                } else {
                    "old-cached-session"
                },
            );
        }
    }

    #[tokio::test]
    async fn frozen_recovery_policy_with_new_session_false_never_probes_headless_cache() {
        let fixture =
            cached_headless_fixture(Arc::new(|| panic!("后台恢复不得进入用户认证准备探针")));
        // 后台恢复派发要求当前许可（auto_reconnect + 已保存凭据对）。
        lock_recover(&fixture.live.config)
            .apply_and_save(vec![DarwinConfigItem::new("auto_reconnect", "true")])
            .unwrap();
        let policy = SessionReconnectPolicy::from_config(&lock_recover(&fixture.live.config));
        // 用户保存新协议不改已冻结会话的恢复协议。
        lock_recover(&fixture.live.config)
            .apply_and_save(vec![DarwinConfigItem::new("protocol", "cisco"),
                DarwinConfigItem::new("atrust_auto_login", "false"),
                DarwinConfigItem::new("atrust_server", "other.example.edu")])
            .unwrap();
        let generation = fixture.live.reconnect_generation.advance();
        let operation = [76; 16];
        fixture
            .live
            .connect_in_generation(
                &connect_request(&operation),
                None,
                false,
                generation,
                false,
                Some(policy),
            )
            .await
            .unwrap();
        assert_cached_apply_is_not_fresh(&fixture, &operation);
        assert_eq!(
            lock_recover(&fixture.live.config).protocol(),
            Protocol::Cisco
        );
        assert_current_cache(&fixture, "old-cached-session");
        fixture.live.close().await.unwrap();
    }

    /// 真实旧派发已安装 source，新 Connect 在独立 runtime 完成探针并等待复核。
    /// 旧临界区用 try_lock 有界退出：错误锁序也会先释放 source，再报告失败。
    #[tokio::test]
    async fn headless_probe_recheck_keeps_source_then_config_order_with_old_accepted_source() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicU32::new(0));
        let (probe_returned_tx, probe_returned_rx) = std::sync::mpsc::channel();
        let probe: cached_headless::ProbeSeam = {
            let entered = entered.clone();
            let release = release.clone();
            let calls = calls.clone();
            Arc::new(move || {
                let entered = entered.clone();
                let release = release.clone();
                let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                let returned = probe_returned_tx.clone();
                Box::pin(async move {
                    if !first {
                        entered.notify_one();
                        release.notified().await;
                        returned.send(()).unwrap();
                    }
                    Ok(SessionProbe {
                        is_login: true,
                        first_auth: None,
                    })
                })
            })
        };
        let fixture = cached_headless_fixture(probe);
        let old_operation = [77; 16];
        fixture
            .live
            .connect(&connect_request(&old_operation), None, false)
            .await
            .unwrap();
        let old_generation = fixture.live.reconnect_generation.current();
        let (connect_done_tx, connect_done_rx) = std::sync::mpsc::channel();
        let new_connect_thread = {
            let live = fixture.live.clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result =
                    runtime.block_on(live.connect(&connect_request(&[78; 16]), None, false));
                connect_done_tx.send(result).unwrap();
            })
        };
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        let (source_held_tx, source_held_rx) = std::sync::mpsc::channel();
        let (finish_old_tx, finish_old_rx) = std::sync::mpsc::channel();
        let (old_config_tx, old_config_rx) = std::sync::mpsc::channel();
        let old_source_thread = {
            let live = fixture.live.clone();
            std::thread::spawn(move || {
                let accepted =
                    live.reconnect_generation
                        .accept(old_generation, &old_operation, |_| {
                            source_held_tx.send(()).unwrap();
                            // 超时也必须退出source临界区，不能让错误实现拖死整个测试进程。
                            let signalled =
                                finish_old_rx.recv_timeout(Duration::from_secs(2)).is_ok();
                            let limit = std::time::Instant::now() + Duration::from_millis(200);
                            let acquired = loop {
                                if let Ok(config) = live.config.try_lock() {
                                    drop(config);
                                    break true;
                                }
                                if std::time::Instant::now() >= limit {
                                    break false;
                                }
                                std::thread::sleep(Duration::from_millis(1));
                            };
                            old_config_tx.send(signalled && acquired).unwrap();
                        });
                assert!(accepted.is_some(), "旧已接纳来源应仍可完成临界区");
            })
        };
        let held = source_held_rx.recv_timeout(Duration::from_secs(1));
        release.notify_one();
        let returned = probe_returned_rx.recv_timeout(Duration::from_secs(1));
        // 独立 runtime 已返回probe，留出短窗口使其实际进入复核锁获取。
        std::thread::sleep(Duration::from_millis(100));
        let config_available = fixture.live.config.try_lock().is_ok();
        // 所有断言均放在释放source之后；反向锁序会得到false而非无限互等。
        let _ = finish_old_tx.send(());
        let old_config = old_config_rx.recv_timeout(Duration::from_secs(1));
        let connected = connect_done_rx.recv_timeout(Duration::from_secs(2));
        old_source_thread.join().unwrap();
        assert!(held.is_ok(), "旧source必须先进入临界区");
        assert!(returned.is_ok(), "新请求必须实际完成probe");
        assert!(config_available, "等待source复核时不能预先持有config");
        assert_eq!(
            old_config.unwrap(),
            true,
            "旧source应能按既有顺序取得config并完成"
        );
        connected
            .expect("新请求应在旧source释放后正常完成")
            .unwrap();
        new_connect_thread.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let engine = fixture.handles.lock().unwrap()[0].clone();
        assert_eq!(
            engine.applied_operation_ids(),
            vec![vec![77; 16], vec![78; 16]]
        );
        fixture.live.close().await.unwrap();
    }
}
