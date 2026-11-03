//! C5：原有序消费者 → 原 AuthOrchestrator/provider 夹具 → 真实 tonic RespondInteraction。
use super::super::{
    auth_acquisition::{self, Acquisition, AcquisitionTicket},
    browser_auth::{self, AuthMode, PresentationIntent},
};
use super::*;
use prost::Message;
use zeroize::Zeroize;

fn acquisition(
    dir: &std::path::Path,
    id: u8,
    auto_login: bool,
    mode: &str,
) -> wire::AtrustAuthAcquisition {
    wire::AtrustAuthAcquisition {
        interaction_id: vec![id; 16],
        runtime_epoch: vec![7; 16],
        deadline_monotonic_ns: AuthDeadline::after(std::time::Duration::from_secs(3))
            .unwrap()
            .monotonic_ns(),
        gateway: "https://vpn.example.edu:443".into(),
        mode: mode.into(),
        auto_login,
        config_dir: dir.to_string_lossy().into(),
        user_agent: "frozen-agent".into(),
        browser_profile: "unused-windows-profile".into(),
        browser_profile_explicit: true,
    }
}
fn observe(
    state: &CoreState,
    request: Option<&wire::AtrustAuthAcquisition>,
) -> Option<AcquisitionTicket> {
    state
        .connection_requests
        .acquisitions
        .observe(&state.connection_requests, request)
}
fn save_school_fixture(dir: &std::path::Path) {
    let mut config = exv_vpn_darwin_core::DarwinUiConfig::load_from_dir(dir).unwrap();
    config
        .apply_ui_credentials_and_save("fixture-user", Some("fixture-password"))
        .unwrap();
}
#[tokio::test]
async fn acquisition_consumer_real_auth_success_failure_missing_challenge_and_frozen_manual() {
    use browser_auth::tests::AcquisitionScenario as Scenario;
    for scenario in [
        Scenario::Http,
        Scenario::Fallback,
        Scenario::Manual,
        Scenario::TwoRounds,
        Scenario::Failure,
        Scenario::Missing,
        Scenario::Challenge,
        Scenario::WindowCancel,
    ] {
        for requested in ["embedded", "browser", "headless"] {
            // 两轮学校表单/验证码沿原 embedded；系统浏览器本来由人工提交。
            if !matches!(
                scenario,
                Scenario::Http | Scenario::Manual | Scenario::Fallback
            ) && requested != "embedded"
            {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            if !matches!(scenario, Scenario::Missing) {
                save_school_fixture(dir.path());
            }
            let (mock, channel, server) = serve_mock(MockKernel::new()).await;
            let state = dialed_state(channel.clone());
            let request = acquisition(
                dir.path(),
                1,
                !matches!(scenario, Scenario::Manual),
                requested,
            );
            let ticket = observe(&state, Some(&request)).unwrap();
            assert!(
                observe(&state, Some(&request)).is_none(),
                "首拍重放不启动第二个认证"
            );
            let trace = std::sync::Mutex::new(None);
            auth_acquisition::acquire_and_reply(
                None,
                &state,
                channel,
                ticket,
                |context, mode, owner, options| {
                    let trace = &trace;
                    let request = &request;
                    async move {
                        assert_eq!(context.gateway, request.gateway);
                        assert_eq!(context.user_agent, request.user_agent);
                        assert!(options.auto_login);
                        assert!(options.silent && options.hidden);
                        assert_eq!(
                            options.config_dir.as_ref().unwrap().to_string_lossy(),
                            request.config_dir
                        );
                        assert!(options.explicit.is_none(), "后台只用原学校 provider");
                        assert!(
                            options.deadline.unwrap()
                                <= tokio::time::Instant::now()
                                    + AuthDeadline::from_monotonic_ns(
                                        request.deadline_monotonic_ns
                                    )
                                    .unwrap()
                                    .remaining()
                                    .unwrap()
                                    + std::time::Duration::from_millis(1)
                        );
                        assert_eq!(mode, AuthMode::Headless);
                        let (result, observed) =
                            browser_auth::tests::authenticate_acquisition_fixture(
                                context, mode, owner, options, scenario,
                            )
                            .await;
                        *trace.lock().unwrap() = Some(observed);
                        result
                    }
                },
            )
            .await
            .unwrap();
            assert!(
                mock.connects.lock().await.is_empty(),
                "恢复没有缓存或普通 Connect RPC"
            );
            assert_eq!(
                mock.config_reads.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "不重读最新设置作为恢复门禁"
            );
            let interactions = mock.interactions.lock().await;
            assert_eq!(interactions.len(), 1);
            assert_eq!(interactions[0].interaction_id, request.interaction_id);
            assert_eq!(interactions[0].runtime_epoch, request.runtime_epoch);
            let mut decoded = wire::AtrustAuthAcquisitionResult::decode(
                interactions[0].response_payload.as_slice(),
            )
            .unwrap();
            match decoded.result.as_mut().unwrap() {
                wire::atrust_auth_acquisition_result::Result::Candidate(bytes) => {
                    assert!(!matches!(
                        scenario,
                        Scenario::Failure | Scenario::Missing | Scenario::WindowCancel | Scenario::Challenge
                    ));
                    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                    assert_eq!(value["fresh_authentication"], true);
                    assert_eq!(value["version"], 2);
                    assert!(value.get("password").is_none());
                    bytes.zeroize();
                }
                wire::atrust_auth_acquisition_result::Result::Failed(error) => {
                    let expected = match scenario {
                        Scenario::Missing => (
                            wire::ErrorCode::Unauthorized,
                            "DARWIN_CORE_CONNECT_CREDENTIALS_MISSING",
                        ),
                        Scenario::WindowCancel => (
                            wire::ErrorCode::CancelledBeforeStart,
                            "atrust/auth_cancelled",
                        ),
                        Scenario::Challenge => (
                            wire::ErrorCode::ObservationFailed,
                            "学校认证需要人工操作",
                        ),
                        Scenario::Failure => (
                            wire::ErrorCode::ObservationFailed,
                            "atrust/auth_acquisition_failed",
                        ),
                        _ => panic!("unexpected failure: {scenario:?} {error:?}"),
                    };
                    assert_eq!(error.code, expected.0 as i32);
                    if matches!(scenario, Scenario::Challenge) {
                        assert!(error.reason.contains("人工操作"));
                    } else if !matches!(scenario, Scenario::Failure) {
                        assert_eq!(error.reason, expected.1);
                    } else {
                        assert!(
                            error.reason.contains("自动恢复")
                                && error.reason.matches("诊断编号").count() == 1
                                && error.reason.chars().count() <= 160
                        );
                    }
                    assert!(
                        error.operation_detail.is_none(),
                        "认证失败不使用Stop专用详情字段"
                    );
                    assert_eq!(error.stage, wire::ErrorStage::AwaitingInteraction as i32);
                    assert_eq!(error.certainty, wire::EffectCertainty::NoEffect as i32);
                    assert!(error.subject.is_some());
                }
            }
            drop(interactions);
            let trace = trace.into_inner().unwrap().unwrap();
            assert!(trace.cleaned > 0);
            if matches!(scenario, Scenario::TwoRounds | Scenario::Failure) {
                assert_eq!(trace.injections, 2);
            }
            if matches!(scenario, Scenario::Challenge | Scenario::Missing) {
                assert_eq!(trace.injections, 0);
            }
            assert!(trace.opened.iter().all(|(_, presentation)| *presentation == PresentationIntent::Hidden));
            assert_eq!(trace.visible_presentations, 0);
            assert_eq!(trace.opened[0].0, AuthMode::Headless);
            if matches!(scenario, Scenario::Fallback | Scenario::Manual) {
                assert_eq!(trace.opened[1].0, AuthMode::Embedded);
            }
            assert!(
                observe(&state, Some(&request)).is_none(),
                "回执之后重放也只执行一次"
            );
            assert!(
                state.connection_requests.begin().is_ok(),
                "真实回执释放原 owner"
            );
            server.abort();
        }
    }
}

#[tokio::test]
async fn acquisition_consumer_long_auth_clear_stop_new_user_stream_and_wrong_epoch_isolate_owner() {
    // clear / 流 EOF 沿同一个 None；新用户由原 begin 替代旧恢复 owner。
    for action in ["clear", "stream", "stop", "new_user", "epoch"] {
        let dir = tempfile::tempdir().unwrap();
        let (mock, channel, server) = serve_mock(MockKernel::new()).await;
        let state = Arc::new(dialed_state(channel.clone()));
        let request = acquisition(dir.path(), 2, false, "embedded");
        let ticket = observe(&state, Some(&request)).unwrap();
        let old = ticket.ready.as_ref().unwrap().owner.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            auth_acquisition::acquire_and_reply(
                None,
                &task_state,
                channel,
                ticket,
                |context, mode, owner, options| async move {
                    started.send(()).unwrap();
                    // 原获取在等待真实取消；生产消费者仍可接纳下一拍。
                    owner.cancelled().await;
                    browser_auth::tests::authenticate_acquisition_fixture(
                        context,
                        mode,
                        owner,
                        options,
                        browser_auth::tests::AcquisitionScenario::Manual,
                    )
                    .await
                    .0
                },
            )
            .await
        });
        ready.await.unwrap();
        assert!(observe(&state, Some(&request)).is_none());
        let mut successor = None;
        match action {
            "clear" | "stream" => {
                observe(&state, None);
            }
            "stop" => {
                CoreClient.stop(&state).await.unwrap();
                assert_eq!(mock.stops.lock().await.len(), 1);
            }
            "new_user" => {
                successor = Some(state.connection_requests.begin().unwrap());
                observe(&state, None);
            }
            "epoch" => {
                let mut next = request.clone();
                next.runtime_epoch = vec![9; 16];
                let next_ticket = observe(&state, Some(&next)).unwrap();
                assert!(
                    !state
                        .connection_requests
                        .acquisitions
                        .current(&Acquisition {
                            wire: request.clone(),
                            owner: old.clone()
                        })
                );
                successor = Some(next_ticket.ready.unwrap().owner);
            }
            _ => unreachable!(),
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(mock.interactions.lock().await.is_empty(), "旧获取不能回交");
        if let Some(successor) = successor {
            assert!(
                successor.begin_submit().is_ok(),
                "旧清除/Drop不能取消后来owner"
            );
        }
        assert!(old.begin_submit().is_err());
        server.abort();
    }
}

#[tokio::test]
async fn acquisition_reply_clear_and_caller_abort_keep_lease_until_real_receipt_then_real_stop() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockKernel::new();
    let (release, gate) = tokio::sync::oneshot::channel();
    *mock.interaction_gate.lock().await = Some(gate);
    let (mock, channel, server) = serve_mock(mock).await;
    let state = Arc::new(dialed_state(channel.clone()));
    let request = acquisition(dir.path(), 3, false, "browser");
    let ticket = observe(&state, Some(&request)).unwrap();
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        auth_acquisition::acquire_and_reply(
            None,
            &task_state,
            channel,
            ticket,
            |context, mode, owner, options| async move {
                browser_auth::tests::authenticate_acquisition_fixture(
                    context,
                    mode,
                    owner,
                    options,
                    browser_auth::tests::AcquisitionScenario::Manual,
                )
                .await
                .0
            },
        )
        .await
    });
    mock.interaction_started.notified().await;
    observe(&state, None);
    assert!(state.connection_requests.commit_gate.try_lock().is_err());
    assert!(state.connection_requests.begin().is_err());
    task.abort();
    let stop_state = state.clone();
    let mut stop = tokio::spawn(async move { CoreClient.stop(&stop_state).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut stop)
            .await
            .is_err()
    );
    assert!(mock.stops.lock().await.is_empty());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), stop)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(mock.interactions.lock().await.len(), 1);
    assert_eq!(
        mock.stops.lock().await.len(),
        1,
        "Core原恢复活跃，清除后Stop仍须真实派发"
    );
    assert!(state.connection_requests.begin().is_ok());
    server.abort();
}

#[tokio::test]
async fn acquisition_submitted_owner_handoff_waits_once_and_obeys_clear_stop_user_and_deadline() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for action in ["release", "clear", "stop", "new_user", "deadline", "epoch"] {
        let dir = tempfile::tempdir().unwrap();
        let mock = MockKernel::new();
        let (release, gate) = tokio::sync::oneshot::channel();
        *mock.interaction_gate.lock().await = Some(gate);
        let (mock, channel, server) = serve_mock(mock).await;
        let state = Arc::new(dialed_state(channel.clone()));
        let first = acquisition(dir.path(), 10, false, "browser");
        let first_ticket = observe(&state, Some(&first)).unwrap();
        // 保留额外引用，使 new_user 分支能在通知唤醒前确定性地建立后来 owner。
        let old = first_ticket.ready.as_ref().unwrap().owner.clone();
        let first_state = state.clone();
        let first_channel = channel.clone();
        let first_task = tokio::spawn(async move {
            auth_acquisition::acquire_and_reply(
                None,
                &first_state,
                first_channel,
                first_ticket,
                |context, mode, owner, options| async move {
                    browser_auth::tests::authenticate_acquisition_fixture(
                        context,
                        mode,
                        owner,
                        options,
                        browser_auth::tests::AcquisitionScenario::Manual,
                    )
                    .await
                    .0
                },
            )
            .await
        });
        mock.interaction_started.notified().await;
        // A 的 companion 可以先清除，真实 unary receipt 仍被原屏障持有。
        observe(&state, None);
        let mut second = acquisition(dir.path(), 11, false, "browser");
        if action == "deadline" {
            second.deadline_monotonic_ns =
                AuthDeadline::after(std::time::Duration::from_millis(60))
                    .unwrap()
                    .monotonic_ns();
        }
        let second_ticket = observe(&state, Some(&second)).unwrap();
        assert!(second_ticket.ready.is_none());
        assert!(
            observe(&state, Some(&second)).is_none(),
            "B 重放只保留一次任务"
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let task_calls = calls.clone();
        let second_state = state.clone();
        let second_channel = channel.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let mut second_task = tokio::spawn(async move {
            started.send(()).unwrap();
            auth_acquisition::acquire_and_reply(
                None,
                &second_state,
                second_channel,
                second_ticket,
                |context, mode, owner, options| async move {
                    task_calls.fetch_add(1, Ordering::SeqCst);
                    browser_auth::tests::authenticate_acquisition_fixture(
                        context,
                        mode,
                        owner,
                        options,
                        browser_auth::tests::AcquisitionScenario::Manual,
                    )
                    .await
                    .0
                },
            )
            .await
        });
        ready.await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut second_task)
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(state.connection_requests.commit_gate.try_lock().is_err());
        assert!(
            state.connection_requests.begin().is_err(),
            "A 的真实 receipt 前仍持原 owner"
        );
        let mut stop = None;
        let mut replacement = None;
        match action {
            "clear" => {
                observe(&state, None);
            }
            "stop" => {
                let stop_state = state.clone();
                let mut task = tokio::spawn(async move { CoreClient.stop(&stop_state).await });
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(10), &mut task)
                        .await
                        .is_err()
                );
                assert!(mock.stops.lock().await.is_empty());
                stop = Some(task);
            }
            "epoch" => {
                let mut next = second.clone();
                next.runtime_epoch = vec![12; 16];
                replacement = Some((next.clone(), observe(&state, Some(&next)).unwrap()));
            }
            _ => {}
        }
        if matches!(action, "clear" | "stop" | "deadline" | "epoch") {
            tokio::time::timeout(std::time::Duration::from_secs(1), &mut second_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(state.connection_requests.commit_gate.try_lock().is_err());
            assert_eq!(mock.interactions.lock().await.len(), 1);
        }
        release.send(()).unwrap();
        first_task.await.unwrap().unwrap();
        drop(old);
        let user = if action == "new_user" {
            Some(state.connection_requests.begin().unwrap())
        } else {
            None
        };
        if matches!(action, "release" | "new_user") {
            tokio::time::timeout(std::time::Duration::from_secs(1), second_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        if let Some((next, ticket)) = replacement {
            auth_acquisition::acquire_and_reply(
                None,
                &state,
                channel.clone(),
                ticket,
                |context, mode, owner, options| async move {
                    browser_auth::tests::authenticate_acquisition_fixture(
                        context,
                        mode,
                        owner,
                        options,
                        browser_auth::tests::AcquisitionScenario::Manual,
                    )
                    .await
                    .0
                },
            )
            .await
            .unwrap();
            let interactions = mock.interactions.lock().await;
            assert_eq!(interactions.len(), 2);
            assert_eq!(interactions[1].runtime_epoch, next.runtime_epoch);
        } else {
            let interactions = mock.interactions.lock().await;
            assert_eq!(interactions.len(), if action == "release" { 2 } else { 1 });
            if action == "release" {
                assert_eq!(interactions[1].interaction_id, second.interaction_id);
                assert_eq!(interactions[1].runtime_epoch, second.runtime_epoch);
            }
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(action == "release")
        );
        if action != "epoch" {
            assert!(observe(&state, Some(&second)).is_none());
        }
        if let Some(user) = user {
            observe(&state, None);
            assert!(
                user.begin_submit().is_ok(),
                "等待票据与旧清除不取消后来用户 owner"
            );
        }
        if let Some(stop) = stop {
            stop.await.unwrap().unwrap();
            assert_eq!(mock.stops.lock().await.len(), 1);
        }
        server.abort();
    }
}

#[tokio::test]
async fn acquisition_same_deadline_exits_gate_without_reply_or_new_budget() {
    let dir = tempfile::tempdir().unwrap();
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let state = Arc::new(dialed_state(channel.clone()));
    let mut request = acquisition(dir.path(), 4, false, "embedded");
    request.deadline_monotonic_ns = AuthDeadline::after(std::time::Duration::from_millis(60))
        .unwrap()
        .monotonic_ns();
    let ticket = observe(&state, Some(&request)).unwrap();
    let permit = state
        .connection_requests
        .commit_gate
        .clone()
        .lock_owned()
        .await;
    let result = auth_acquisition::acquire_and_reply(
        None,
        &state,
        channel,
        ticket,
        |context, mode, owner, options| async move {
            browser_auth::tests::authenticate_acquisition_fixture(
                context,
                mode,
                owner,
                options,
                browser_auth::tests::AcquisitionScenario::Manual,
            )
            .await
            .0
        },
    )
    .await;
    assert!(result.is_err());
    assert!(mock.interactions.lock().await.is_empty());
    assert!(
        state.connection_requests.commit_gate.try_lock().is_err(),
        "原许可仍属原实际持有者"
    );
    drop(permit);
    assert!(state.connection_requests.begin().is_ok());
    server.abort();
}

#[tokio::test]
async fn auth_acquisition_recovery_requiring_interaction_emits_no_window_event() {
    let dir = tempfile::tempdir().unwrap();
    save_school_fixture(dir.path());
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel.clone());
    let req = acquisition(dir.path(), 41, true, "headless");
    let ticket = observe(&state, Some(&req)).unwrap();
    let trace = std::sync::Mutex::new(None);
    auth_acquisition::acquire_and_reply(None, &state, channel, ticket, |context, mode, owner, options| {
        let trace = &trace;
        async move {
            let (result, observed) = browser_auth::tests::authenticate_acquisition_fixture(
                context, mode, owner, options, browser_auth::tests::AcquisitionScenario::Challenge).await;
            *trace.lock().unwrap() = Some(observed);
            result
        }
    }).await.unwrap();
    let observed = trace.into_inner().unwrap().unwrap();
    server.abort();
    assert_eq!(observed.visible_presentations, 0);
    assert!(observed.opened.iter().all(|(_, presentation)| *presentation != PresentationIntent::Visible),
        "后台验证码不得打开可见窗口");
    let received = mock.interactions.lock().await;
    let result = wire::AtrustAuthAcquisitionResult::decode(received[0].response_payload.as_slice()).unwrap().result.unwrap();
    assert!(matches!(result, wire::atrust_auth_acquisition_result::Result::Failed(_)));
    assert!(mock.connects.lock().await.is_empty());
}
