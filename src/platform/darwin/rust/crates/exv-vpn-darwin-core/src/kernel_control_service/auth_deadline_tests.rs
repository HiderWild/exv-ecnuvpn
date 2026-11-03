mod auth_deadline_tests {
    use super::*;
    use crate::atrust_session_probe::SessionProbe;
    use crate::kernel_control_service::cached_headless;
    use exv_vpn_darwin_ipc::auth_deadline::{AUTH_DEADLINE_METADATA_KEY, AuthDeadline};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    const GATEWAY: &str = "https://vpn.example.edu:443";
    fn candidate() -> exv_vpn_darwin_ipc::browser_session::BrowserSession {
        exv_vpn_darwin_ipc::browser_session::session_from_cookie_pairs(
            GATEWAY,
            "fixture-agent",
            vec![("sid".into(), "deadline-fixture".into())],
        )
        .unwrap()
    }
    fn deadline_fixture(probe: Option<cached_headless::ProbeSeam>) -> LiveFixture {
        let mut fixture = live_fixture();
        lock_recover(&fixture.live.config)
            .apply_and_save(vec![
                DarwinConfigItem::new("protocol", "atrust"),
                DarwinConfigItem::new("atrust_server", "vpn.example.edu"),
                DarwinConfigItem::new("atrust_auth_mode", "headless"),
            ])
            .unwrap();
        atrust_session_cache::save(fixture._config_dir.path(), &candidate()).unwrap();
        // 无专用探针时用默认「缓存有效」接缝：期限夹具聚焦各等待点的到期语义，
        // 不重复验证探测事实（探测由 headless_cache_tests 覆盖）。
        Arc::get_mut(&mut fixture.live)
            .unwrap()
            .cached_session_probe = Some(probe.unwrap_or_else(login_probe_seam));
        fixture
    }
    fn counted_probe(count: Arc<AtomicUsize>, pending: bool) -> cached_headless::ProbeSeam {
        Arc::new(move || {
            let count = count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                if pending {
                    std::future::pending::<()>().await;
                }
                Ok(SessionProbe {
                    is_login: true,
                    first_auth: None,
                })
            })
        })
    }
    fn service(fixture: &LiveFixture) -> DarwinKernelControlService {
        let mut service = DarwinKernelControlService::new_inner(
            fixture.live.events.clone(),
            None,
            Some(fixture.live.clone()),
            DarwinUiConfig::default(),
            fixture.logs.clone(),
            fixture.live.proxy_tun.clone(),
        );
        service.config = fixture.live.config.clone();
        service
    }
    fn assert_no_apply(fixture: &LiveFixture) {
        assert!(
            fixture
                .handles
                .lock()
                .unwrap()
                .iter()
                .all(|handle| handle.apply_count() == 0)
        );
    }
    fn assert_expired(status: Status) {
        assert_eq!(status.code(), Code::DeadlineExceeded);
        assert_eq!(status.message(), "DARWIN_CORE_AUTH_DEADLINE_EXPIRED");
    }

    #[tokio::test]
    async fn auth_deadline_handler_rejects_invalid_duplicate_and_expired_headers_before_probe_or_apply()
     {
        for values in [vec!["invalid"], vec!["1"], vec!["1", "2"]] {
            let probes = Arc::new(AtomicUsize::new(0));
            let fixture = deadline_fixture(Some(counted_probe(probes.clone(), false)));
            let service = service(&fixture);
            let mut request = Request::new(connect_request(&[91; 16]));
            for value in &values {
                request
                    .metadata_mut()
                    .append(AUTH_DEADLINE_METADATA_KEY, value.parse().unwrap());
            }
            let status = KernelControl::connect(&service, request).await.unwrap_err();
            assert_eq!(
                status.code(),
                if values == ["1"] {
                    Code::DeadlineExceeded
                } else {
                    Code::InvalidArgument
                }
            );
            assert_eq!(probes.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
            assert_no_apply(&fixture);
        }
    }

    /// 真实OS UDS→IPC uid门与warm connector→tonic→生产KernelControl handler。
    /// interceptor只延迟交付，不重建期限、不替代handler或生产认证逻辑。
    #[tokio::test]
    async fn auth_deadline_survives_real_uds_transport_and_expires_before_core_receives_request() {
        use exv_vpn_darwin_ipc::{
            connect_service_authenticated_channel,
            path::SocketPath,
            peer::SystemPeerLookup,
            service_listener::{ServiceListener, ServiceListenerConfig},
        };
        use wire::{
            kernel_control_client::KernelControlClient, kernel_control_server::KernelControlServer,
        };
        let probes = Arc::new(AtomicUsize::new(0));
        let fixture = deadline_fixture(Some(counted_probe(probes.clone(), false)));
        let service = service(&fixture);
        let socket_dir = tempfile::Builder::new()
            .prefix("exv-dl-")
            .tempdir_in("/private/tmp")
            .unwrap();
        let path = socket_dir.path().join("core.sock");
        // SAFETY：geteuid只读本测试进程身份，不修改宿主配置。
        let uid = unsafe { libc::geteuid() };
        let listener = Arc::new(
            ServiceListener::bind(ServiceListenerConfig::new(
                SocketPath::new(path.clone()).unwrap(),
                uid,
            ))
            .unwrap(),
        );
        let delayed = Arc::new(AtomicBool::new(false));
        let server = {
            let listener = listener.clone();
            let delayed = delayed.clone();
            tokio::spawn(async move {
                let connection = listener.accept_uid_gated().await.unwrap();
                let incoming = tokio_stream::iter(vec![Ok::<_, std::io::Error>(
                    connection.into_authenticated_unix_stream(),
                )]);
                let server =
                    KernelControlServer::with_interceptor(service, move |request: Request<()>| {
                        let deadline = AuthDeadline::from_metadata(request.metadata())
                            .unwrap()
                            .unwrap();
                        let remaining = deadline.remaining().unwrap();
                        delayed.store(true, Ordering::SeqCst);
                        std::thread::sleep(remaining + Duration::from_millis(10));
                        Ok(request)
                    });
                tonic::transport::Server::builder()
                    .add_service(server)
                    .serve_with_incoming(incoming)
                    .await
                    .unwrap();
            })
        };
        let io = tokio::net::UnixStream::connect(&path).await.unwrap();
        let (channel, _) = connect_service_authenticated_channel(io, uid, &SystemPeerLookup)
            .await
            .unwrap();
        let mut client = KernelControlClient::new(channel);
        let mut request = Request::new(connect_request(&[92; 16]));
        AuthDeadline::after(Duration::from_millis(150))
            .unwrap()
            .write_metadata(request.metadata_mut())
            .unwrap();
        let status = client.connect(request).await.unwrap_err();
        assert!(
            delayed.load(Ordering::SeqCst),
            "真实header必须先到达UDS server"
        );
        assert_expired(status);
        assert_eq!(probes.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
        assert_no_apply(&fixture);
        drop(client);
        server.abort();
        let _ = server.await;
        listener.cleanup().unwrap();
    }

    #[tokio::test]
    async fn auth_deadline_missing_header_keeps_legacy_handler_compatible() {
        let fixture = deadline_fixture(None);
        KernelControl::connect(&service(&fixture), Request::new(connect_request(&[93; 16])))
            .await
            .unwrap();
        assert_eq!(fixture.handles.lock().unwrap()[0].apply_count(), 1);
        fixture.live.close().await.unwrap();
    }

    #[tokio::test]
    async fn auth_deadline_short_total_probe_timeout_is_terminal_but_six_second_local_timeout_allows_fallback()
     {
        for short in [true, false] {
            let probes = Arc::new(AtomicUsize::new(0));
            let fixture = deadline_fixture(Some(counted_probe(probes.clone(), true)));
            let duration = if short {
                Duration::from_millis(50)
            } else {
                Duration::from_secs(8)
            };
            let deadline = AuthDeadline::after(duration).unwrap();
            let error = tokio::time::timeout(
                Duration::from_secs(7),
                fixture.live.connect_with_auth_deadline(
                    &connect_request(&[94; 16]),
                    None,
                    false,
                    Some(deadline),
                ),
            )
            .await
            .expect("局部probe上界为6秒")
            .unwrap_err();
            if short {
                assert_expired(error);
            } else {
                // S2：6s 局部探测超时=网络层未知结果——独立探测失败码
                //（不再合流为凭据缺失）。
                assert_eq!(error.code(), Code::FailedPrecondition);
                assert_eq!(error.message(), CONNECTION_PROBE_UNAVAILABLE_CODE);
                assert!(
                    deadline.remaining().is_ok(),
                    "局部超时不重置或耗尽8秒总期限"
                );
            }
            assert_eq!(probes.load(Ordering::SeqCst), 1);
            assert!(
                atrust_session_cache::load_candidate(fixture._config_dir.path(), GATEWAY).is_some(),
                "未知超时保留缓存"
            );
            assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
            assert_no_apply(&fixture);
        }
    }

    #[tokio::test]
    async fn auth_deadline_waiting_for_session_lock_cannot_dispatch_after_expiry() {
        let fixture = deadline_fixture(None);
        let held = fixture.live.session.lock().await;
        let error = fixture
            .live
            .connect_with_auth_deadline(
                &connect_request(&[95; 16]),
                None,
                false,
                Some(AuthDeadline::after(Duration::from_millis(30)).unwrap()),
            )
            .await
            .unwrap_err();
        drop(held);
        assert_expired(error);
        assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
        assert_no_apply(&fixture);
    }

    #[tokio::test]
    async fn auth_deadline_config_capture_wait_cannot_turn_expiry_into_material_fallback() {
        let fixture = deadline_fixture(None);
        let held = fixture.live.config.lock().unwrap();
        let deadline = AuthDeadline::after(Duration::from_millis(30)).unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread = {
            let live = fixture.live.clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result = runtime.block_on(live.connect_with_auth_deadline(
                    &connect_request(&[96; 16]),
                    None,
                    false,
                    Some(deadline),
                ));
                done_tx.send(result).unwrap();
            })
        };
        std::thread::sleep(Duration::from_millis(80));
        drop(held);
        let result = done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
        assert_expired(result.unwrap_err());
        assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
        assert_no_apply(&fixture);
    }

    #[tokio::test]
    async fn auth_deadline_launch_and_occupied_rebuild_crossing_expiry_never_apply() {
        for occupied in [false, true] {
            let mut fixture = deadline_fixture(None);
            let serial = Arc::new(AtomicU32::new(0));
            let handles = fixture.handles.clone();
            let count = serial.clone();
            Arc::get_mut(&mut fixture.live).unwrap().launch_session = Arc::new(move || {
                let serial = count.fetch_add(1, Ordering::SeqCst);
                let handles = handles.clone();
                Box::pin(async move {
                    if !occupied || serial == 1 {
                        tokio::time::sleep(Duration::from_millis(80)).await;
                    }
                    let (session, handle) = fixture_engine_session_with_mode(serial + 1, "");
                    if occupied && serial == 0 {
                        handle.fail_next_attach(EngineLifecycleError::Observe(ObserveError::Rpc(
                            Status::failed_precondition("DARWIN_ENGINE_ATTEMPT_ACTIVE"),
                        )));
                    }
                    handles.lock().unwrap().push(handle);
                    Ok(LiveEngineSession::Fixture(session))
                })
            });
            let error = fixture
                .live
                .connect_with_auth_deadline(
                    &connect_request(&[97; 16]),
                    None,
                    false,
                    Some(AuthDeadline::after(Duration::from_millis(40)).unwrap()),
                )
                .await
                .unwrap_err();
            assert_expired(error);
            assert_no_apply(&fixture);
            assert_eq!(serial.load(Ordering::SeqCst), if occupied { 2 } else { 1 });
            fixture.live.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn auth_deadline_attach_and_adopt_stop_crossing_expiry_cannot_apply_again() {
        for adopt in [false, true] {
            let mut fixture = deadline_fixture(None);
            if adopt {
                fixture
                    .live
                    .connect(&connect_request(&[98; 16]), None, false)
                    .await
                    .unwrap();
                let handle = fixture.handles.lock().unwrap()[0].clone();
                handle.fail_next_apply(Status::failed_precondition("DARWIN_ENGINE_ADOPT_NEW_AUTH"));
                handle.delay_next_stop(Duration::from_millis(80));
                let error = fixture
                    .live
                    .connect_with_auth_deadline(
                        &connect_request(&[99; 16]),
                        Some(UiSecretPayload::Atrust(UiAtrustCredentialPackage::new(
                            candidate(),
                            true,
                        ))),
                        false,
                        Some(AuthDeadline::after(Duration::from_millis(40)).unwrap()),
                    )
                    .await
                    .unwrap_err();
                assert_expired(error);
                assert_eq!(
                    handle
                        .calls()
                        .iter()
                        .filter(|call| **call == FixtureCall::Apply)
                        .count(),
                    2,
                    "只允许首连接Apply及ADOPT拒绝，不得Stop后第三次Apply"
                );
                assert_eq!(handle.applied_operation_ids(), vec![vec![98; 16]]);
                assert_eq!(handle.stopped_operation_ids().len(), 1);
                let snapshot = fixture.live.events.current_snapshot().unwrap();
                assert!(
                    !matches!(snapshot.state, Some(runtime_snapshot::State::Stopping(_))),
                    "ADOPT停止成功后过期不能把用户留在Stopping"
                );
                let done = snapshot.stop_completion.as_ref().expect("保留真实停止回执");
                assert!(done.stopped);
                assert!(!done.blocks_new_connect);
                assert!(done.errors.is_empty());
            } else {
                let base_launch = fixture.live.launch_session.clone();
                let handles = fixture.handles.clone();
                Arc::get_mut(&mut fixture.live).unwrap().launch_session = Arc::new(move || {
                    let future = base_launch();
                    let handles = handles.clone();
                    Box::pin(async move {
                        let session = future.await?;
                        handles
                            .lock()
                            .unwrap()
                            .last()
                            .unwrap()
                            .delay_next_attach(Duration::from_millis(80));
                        Ok(session)
                    })
                });
                let error = fixture
                    .live
                    .connect_with_auth_deadline(
                        &connect_request(&[100; 16]),
                        None,
                        false,
                        Some(AuthDeadline::after(Duration::from_millis(40)).unwrap()),
                    )
                    .await
                    .unwrap_err();
                assert_expired(error);
                assert_no_apply(&fixture);
                assert!(
                    fixture.handles.lock().unwrap()[0]
                        .calls()
                        .contains(&FixtureCall::AttachStatus)
                );
            }
            fixture.live.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn auth_deadline_probe_result_waiting_for_config_recheck_expires_without_deleting_cache()
    {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (returned_tx, returned_rx) = std::sync::mpsc::channel();
        let probe: cached_headless::ProbeSeam = {
            let entered = entered.clone();
            let release = release.clone();
            Arc::new(move || {
                let entered = entered.clone();
                let release = release.clone();
                let returned = returned_tx.clone();
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    returned.send(()).unwrap();
                    // 只有复核仍在期限内才能使用明确登出事实删除当前缓存。
                    Ok(SessionProbe {
                        is_login: false,
                        first_auth: None,
                    })
                })
            })
        };
        let fixture = deadline_fixture(Some(probe));
        let deadline = AuthDeadline::after(Duration::from_millis(150)).unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread = {
            let live = fixture.live.clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result = runtime.block_on(live.connect_with_auth_deadline(
                    &connect_request(&[101; 16]),
                    None,
                    false,
                    Some(deadline),
                ));
                done_tx.send(result).unwrap();
            })
        };
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        let config = fixture.live.config.lock().unwrap();
        release.notify_one();
        let returned = returned_rx.recv_timeout(Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(200));
        drop(config);
        let result = done_rx.recv_timeout(Duration::from_secs(1));
        thread.join().unwrap();
        assert!(returned.is_ok(), "probe必须先实际返回，再阻塞配置复核");
        assert_expired(result.unwrap().unwrap_err());
        assert!(
            atrust_session_cache::load_candidate(fixture._config_dir.path(), GATEWAY).is_some()
        );
        assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
        assert_no_apply(&fixture);
    }
}
