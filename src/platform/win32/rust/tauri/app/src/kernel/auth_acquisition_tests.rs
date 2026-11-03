//! 消费真实事件入口、原 owner/渠道编排与真实 RespondInteraction RPC；不读正式配置或 cache。
use super::*;
use crate::kernel::{connection_request::ConnectionRequests, events::dispatch_auth_acquisition};
use exv_core::atrust_browser::{BrowserAuthError, LoginAction};
use exv_core::browser_auth::*;
use exv_vpn_win32_ipc::{auth_deadline::AuthDeadline, browser_session::BrowserSession};
use prost::Message;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{Notify, watch};
const G: &str = "https://vpn.ecnu.edu.cn";
type Trace = Arc<Mutex<Vec<String>>>;

fn request(id: u8, automatic: bool) -> wire::AtrustAuthAcquisition {
    wire::AtrustAuthAcquisition {
        interaction_id: vec![id; 16],
        runtime_epoch: vec![0x91; 16],
        deadline_monotonic_ns: AuthDeadline::after(Duration::from_secs(20))
            .unwrap()
            .monotonic_ns(),
        gateway: G.into(),
        mode: "embedded".into(),
        auto_login: automatic,
        config_dir: r"C:\fixture\config".into(),
        browser_profile: r"C:\fixture\cache".into(),
        browser_profile_explicit: true,
        user_agent: String::new(),
    }
}
fn event(request: Option<wire::AtrustAuthAcquisition>) -> wire::RuntimeEvent {
    wire::RuntimeEvent {
        snapshot: Some(wire::RuntimeSnapshot {
            atrust_auth_acquisition: request,
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn error(code: AuthErrorCode, operation: AuthOperation) -> AuthError {
    AuthError {
        code,
        operation,
        native_code: Some(123),
        server_detail: Some("fixture 原因".into()),
        failure_kind: None,
    }
}
fn artifact() -> SessionArtifact {
    SessionArtifact {
        session: BrowserSession::new(G.into(), "sid=fixture".into(), "fixture-agent".into())
            .unwrap(),
        source: SessionSource::BrowserCookieJar,
    }
}
struct Dependencies {
    trace: Trace,
    missing: bool,
}
impl AuthEventSink for Dependencies {
    fn milestone(&self, milestone: AuthMilestone) {
        self.trace
            .lock()
            .unwrap()
            .push(format!("milestone:{milestone:?}"));
    }
    fn progress(&self, progress: AuthProgress) {
        self.trace
            .lock()
            .unwrap()
            .push(format!("progress:{progress:?}"));
    }
    fn form_login_reached(&self) {
        self.trace.lock().unwrap().push("form".into());
    }
}
impl CredentialProvider for Dependencies {
    fn delivery(&self, gateway: &str) -> Option<CredentialDelivery> {
        self.trace.lock().unwrap().push("credentials".into());
        (!self.missing).then(|| CredentialDelivery {
            username: zeroize::Zeroizing::new("fixture".into()),
            password: zeroize::Zeroizing::new("fixture".into()),
            gateway_origin: gateway.into(),
        })
    }
}
impl SessionSnapshotProvider for Dependencies {
    fn snapshot<'a>(&'a self, _: &'a str) -> ChannelFuture<'a, Option<SessionArtifact>> {
        Box::pin(async { Ok(None) })
    }
}
impl SessionCacheInvalidator for Dependencies {
    fn forget<'a>(&'a self, _: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
struct FixtureChannel {
    trace: Trace,
    http: bool,
    opens: usize,
    authed: bool,
    fail_both: bool,
    challenge: bool,
    entered: Option<Arc<Notify>>,
    block: bool,
}
impl FixtureChannel {
    fn record(&self, step: &str) {
        self.trace.lock().unwrap().push(format!(
            "{}:{step}",
            if self.http { "http" } else { "owned" }
        ));
    }
}
impl ChannelTrait for FixtureChannel {
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            injection: if self.http {
                InjectionCapability::Unavailable
            } else {
                InjectionCapability::SchoolFormOnce
            },
            can_render: !self.http,
            visibility: if self.http {
                VisibilityCapability::NoSurface
            } else {
                VisibilityCapability::Controlled
            },
            can_read_cookiejar: true,
            can_write_cookiejar: !self.http,
            observation: if self.http {
                ObservationFidelity::NoPage
            } else {
                ObservationFidelity::EvaluatedDom
            },
            probe_scope: ProbeScope::IndependentHttp,
        }
    }
    fn open(&mut self, request: ChannelRequest) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.record("open");
            self.opens += 1;
            if request.initial_presentation == PresentationIntent::Visible {
                self.record("fresh-visible");
                self.authed = true;
            }
            if let Some(entered) = &self.entered {
                entered.notify_one();
            }
            if self.block {
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    }
    fn navigate(&mut self, _: String) -> ChannelFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn observe(&mut self) -> ChannelFuture<'_, PageObservation> {
        Box::pin(async move {
            Ok(PageObservation {
                sequence: 1,
                url: Observation::Observed(if self.authed {
                    format!("{G}/portal")
                } else {
                    "https://sso.ecnu.edu.cn/login".into()
                }),
                origin: Observation::Observed(if self.authed {
                    G.into()
                } else {
                    "https://sso.ecnu.edu.cn".into()
                }),
                dom: Observation::Observed(DomFacts {
                    challenge_visible: self.challenge,
                    client_install_marker: false,
                    shortcut_page: false,
                    download_page: false,
                    body_has_failure_phrase: false,
                    body_char_count: 0,
                    error_container_text: None,
                    short_body_text: None,
                }),
                coherent: true,
                startup: None,
            })
        })
    }
    fn probe_session(&mut self) -> ChannelFuture<'_, SessionStatus> {
        Box::pin(async move {
            Ok(if self.authed {
                SessionStatus::LoggedIn
            } else {
                SessionStatus::LoggedOut {
                    first_auth: Some("/cas/start".into()),
                }
            })
        })
    }
    fn reuse_session(&mut self, _: Duration) -> ChannelFuture<'_, ReuseOutcome> {
        Box::pin(async move {
            self.record("reuse");
            Ok(ReuseOutcome::Interactive(InteractiveReason::JarMissing))
        })
    }
    fn inject_once(&mut self, delivery: CredentialDelivery) -> ChannelFuture<'_, InjectionOutcome> {
        Box::pin(async move {
            assert_eq!(delivery.gateway_origin, G);
            assert_eq!(delivery.username.as_str(), "fixture");
            self.record("inject");
            if self.fail_both || self.opens == 1 {
                let mut failure = error(AuthErrorCode::InjectionFailed, AuthOperation::Inject);
                failure.server_detail = Some(format!("fixture 第{}轮认证渠道实际失败原因", self.opens));
                return Err(failure);
            }
            self.authed = true;
            Ok(InjectionOutcome::Submitted)
        })
    }
    fn extract_session(&mut self) -> ChannelFuture<'_, Option<SessionArtifact>> {
        Box::pin(async move {
            self.record("extract");
            Ok(self.authed.then(artifact))
        })
    }
    fn present(&mut self, intent: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome> {
        Box::pin(async move {
            self.record(if intent == PresentationIntent::Visible {
                "visible"
            } else {
                "hidden"
            });
            // 验证码交给原可见渠道后，fixture 模拟用户完成学校页面。
            if self.challenge && intent == PresentationIntent::Visible {
                self.authed = true;
            }
            Ok(if intent == PresentationIntent::Visible {
                PresentationOutcome::Visible
            } else {
                PresentationOutcome::Hidden
            })
        })
    }
    fn wait_for_change(&mut self, _: Duration) -> ChannelFuture<'_, ()> {
        Box::pin(async {
            tokio::task::yield_now().await;
            Ok(())
        })
    }
    fn close(&mut self) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            self.record("close");
            Ok(())
        })
    }
}

#[derive(Clone, Default)]
struct Fixture {
    trace: Trace,
    fail_both: bool,
    missing: bool,
    challenge: bool,
    entered: Option<Arc<Notify>>,
    block: bool,
}
impl Fixture {
    async fn acquire(
        self,
        requests: ConnectionRequests,
        request: wire::AtrustAuthAcquisition,
        lease: crate::kernel::connection_request::ConnectionRequest,
        attempt: AuthAttempt,
    ) -> (
        Result<BrowserSession, BrowserAuthError>,
        crate::kernel::connection_request::ConnectionRequest,
    ) {
        let generation = lease.generation();
        requests
            .acquire_outcome_with_lease(
                lease,
                |_| {},
                |_| {},
                move |mut action| async move {
                    let intent = AuthIntent::Automatic;
                    let deadline_before = attempt.remaining_budget(Duration::from_secs(300));
                    let fixed_deadline = attempt.deadline(Duration::from_secs(300));
                    let deps = Dependencies {
                        trace: self.trace.clone(),
                        missing: self.missing,
                    };
                    let mut config = AuthConfig::new(
                        request.gateway,
                        "fixture-agent".into(),
                        "/passport/v1/authConfig".into(),
                    );
                    config.tick = Duration::from_millis(1);
                    config.gateway_page_recovery = true;
                    config.allow_interaction = false;
                    let engine = AuthOrchestrator {
                        config,
                        events: &deps,
                        snapshots: &deps,
                        credentials: Some(&deps as &dyn CredentialProvider),
                        cache: &deps,
                        attempt: &attempt,
                    };
                    let mut http = FixtureChannel {
                        trace: self.trace.clone(),
                        http: true,
                        opens: 0,
                        authed: false,
                        fail_both: false,
                        challenge: false,
                        entered: None,
                        block: false,
                    };
                    let mut owned = FixtureChannel {
                        trace: self.trace.clone(),
                        http: false,
                        opens: 0,
                        authed: false,
                        fail_both: self.fail_both,
                        challenge: self.challenge,
                        entered: self.entered,
                        block: self.block,
                    };
                    let mut primary_action = action.clone();
                    let primary = crate::kernel::embedded_auth::drive_channels(
                        &engine,
                        &mut http,
                        &mut owned,
                        &mut primary_action,
                        true,
                        intent,
                    );
                    let primary = async {
                        match primary.await {
                            Ok(AcquisitionOutcome::Ready(artifact)) => Ok(artifact.session),
                            Ok(AcquisitionOutcome::Control(control)) => {
                                Err(BrowserAuthError::control(control, "fixture cancel"))
                            }
                            Ok(AcquisitionOutcome::Interactive(_)) => {
                                Err(BrowserAuthError::Host("fixture interaction".into()))
                            }
                            Err(error) => Err(BrowserAuthError::auth(
                                error,
                                "fixture authentication failed".into(),
                            )),
                        }
                    };
                    let result = crate::kernel::client::acquire_with_system_fallback(
                        primary,
                        generation,
                        &mut action,
                        &attempt,
                        |_, _, _| async { panic!("fixture 未请求系统回退") },
                    )
                    .await;
                    assert!(attempt.remaining_budget(Duration::from_secs(300)) <= deadline_before);
                    assert_eq!(attempt.deadline(Duration::from_secs(300)), fixed_deadline);
                    result
                },
            )
            .await
    }
}
fn dispatch(
    requests: &ConnectionRequests,
    channel: &Channel,
    ev: &wire::RuntimeEvent,
    fixture: Fixture,
) -> Option<tokio::task::JoinHandle<()>> {
    let owner = requests.clone();
    dispatch_auth_acquisition(requests, channel, ev, move |request, lease, attempt| {
        fixture.acquire(owner, request, lease, attempt)
    })
}
fn count(trace: &Trace, step: &str) -> usize {
    trace
        .lock()
        .unwrap()
        .iter()
        .filter(|value| value.as_str() == step)
        .count()
}

fn projected_auth_error(error: &wire::VpnError) -> crate::kernel::state::VpnError {
    let snapshot = crate::kernel::wire::snapshot_from_wire(
        &wire::RuntimeSnapshot {
            state: Some(wire::runtime_snapshot::State::FailedClean(
                wire::FailedCleanState {
                    last_error: Some(error.clone()),
                    ..Default::default()
                },
            )),
            ..Default::default()
        },
        1,
    );
    let crate::kernel::state::RuntimeState::FailedClean { error: Some(error) } = snapshot.runtime
    else {
        panic!("实际失败快照必须保留认证错误")
    };
    error
}

#[tokio::test]
async fn auth_acquisition_original_channels_two_rounds_and_true_failure_reply() {
    for failed in [false, true] {
        let (mock, channel, server) = serve_mock(MockKernel::new()).await;
        let requests = ConnectionRequests::default();
        let req = request(1, true);
        let fixture = Fixture {
            fail_both: failed,
            ..Default::default()
        };
        let trace = fixture.trace.clone();
        dispatch(&requests, &channel, &event(Some(req.clone())), fixture)
            .unwrap()
            .await
            .unwrap();
        let received = mock.interactions.lock().await;
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].interaction_id, req.interaction_id);
        assert_eq!(received[0].runtime_epoch, req.runtime_epoch);
        let result =
            wire::AtrustAuthAcquisitionResult::decode(received[0].response_payload.as_slice())
                .unwrap()
                .result
                .unwrap();
        assert_eq!(
            matches!(
                &result,
                wire::atrust_auth_acquisition_result::Result::Failed(_)
            ),
            failed
        );
        if let wire::atrust_auth_acquisition_result::Result::Failed(error) = &result {
            assert_eq!(error.code, wire::ErrorCode::ObservationFailed as i32);
            assert_eq!(error.stage, wire::ErrorStage::AwaitingInteraction as i32);
            assert_eq!(error.native.as_ref().unwrap().code, 123);
            assert!(error.operation_detail.is_none());
            assert!(error.reason.starts_with(&format!(
                "atrust/{}|",
                AuthErrorCode::InjectionFailed.machine_code()
            )));
            let ui = projected_auth_error(error);
            for cause in [
                "首次认证",
                "fixture 第1轮认证渠道实际失败原因",
                "自动重试",
                "fixture 第2轮认证渠道实际失败原因",
            ] {
                assert!(error.reason.contains(cause), "回执丢失 {cause}");
                assert!(ui.message.contains(cause), "实际 UI 映射丢失 {cause}");
            }
            assert!(error.subject.is_some());
        }
        assert_eq!(count(&trace, "http:reuse"), 1);
        assert_eq!(count(&trace, "owned:open"), 2);
        assert_eq!(count(&trace, "owned:inject"), 2);
        assert!(mock.connects.lock().await.is_empty());
        assert!(requests.begin(true).is_ok());
        server.abort();
    }
}
#[tokio::test]
async fn auth_acquisition_missing_credentials_challenge_and_legacy_manual_are_silent() {
    for (automatic, missing, challenge) in [
        (true, true, false),
        (true, false, true),
        (false, false, false),
    ] {
        let (mock, channel, server) = serve_mock(MockKernel::new()).await;
        let requests = ConnectionRequests::default();
        let fixture = Fixture {
            missing,
            challenge,
            ..Default::default()
        };
        let trace = fixture.trace.clone();
        dispatch(
            &requests,
            &channel,
            &event(Some(request(2, automatic))),
            fixture,
        )
        .unwrap()
        .await
        .unwrap();
        let received = mock.interactions.lock().await;
        assert_eq!(received.len(), 1);
        let result =
            wire::AtrustAuthAcquisitionResult::decode(received[0].response_payload.as_slice())
                .unwrap()
                .result
                .unwrap();
        assert_eq!(
            matches!(
                &result,
                wire::atrust_auth_acquisition_result::Result::Failed(_)
            ),
            missing || challenge
        );
        if let wire::atrust_auth_acquisition_result::Result::Failed(error) = &result {
            assert!(error.operation_detail.is_none());
            assert!(!projected_auth_error(error).message.is_empty());
        }
        assert_eq!(count(&trace, "owned:visible"), 0);
        assert_eq!(count(&trace, "owned:fresh-visible"), 0);
        if challenge { assert_eq!(count(&trace, "owned:inject"), 0); }
        server.abort();
    }
}
#[tokio::test]
async fn auth_acquisition_replay_wrong_epoch_long_acquire_and_clear_do_not_block_events() {
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let requests = ConnectionRequests::default();
    let req = request(3, true);
    let ev = event(Some(req.clone()));
    let entered = Arc::new(Notify::new());
    let first = dispatch(
        &requests,
        &channel,
        &ev,
        Fixture {
            entered: Some(entered.clone()),
            block: true,
            ..Default::default()
        },
    )
    .unwrap();
    entered.notified().await;
    assert!(dispatch(&requests, &channel, &ev, Fixture::default()).is_none());
    let mut wrong = req.clone();
    wrong.runtime_epoch = vec![9; 16];
    assert!(dispatch(&requests, &channel, &event(Some(wrong)), Fixture::default()).is_none());
    // 与订阅逐事件调用相同，认证仍 pending 时后续快照立即被映射。
    let cleared = event(None);
    assert!(dispatch(&requests, &channel, &cleared, Fixture::default()).is_none());
    let _ui = crate::kernel::wire::event_from_wire(&cleared);
    tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap();
    assert!(mock.interactions.lock().await.is_empty());
    assert!(dispatch(&requests, &channel, &ev, Fixture::default()).is_none());
    let later = requests.begin(true).unwrap();
    requests.auth_acquisitions.clear(&requests);
    assert!(later.begin_core_connect());
    drop(later);
    server.abort();
}
#[tokio::test]
async fn auth_acquisition_stop_replacement_and_stream_gone_cancel_only_original_owner() {
    for replacement in [0, 1, 2] {
        let (mock, channel, server) = serve_mock(MockKernel::new()).await;
        let requests = ConnectionRequests::default();
        let entered = Arc::new(Notify::new());
        let first = dispatch(
            &requests,
            &channel,
            &event(Some(request(4, true))),
            Fixture {
                entered: Some(entered.clone()),
                block: true,
                ..Default::default()
            },
        )
        .unwrap();
        entered.notified().await;
        let next = match replacement {
            0 => {
                let _ = requests.auth_acquisitions.clear(&requests);
                let _ = requests.cancel();
                None
            }
            1 => dispatch(
                &requests,
                &channel,
                &event(Some(request(5, false))),
                Fixture::default(),
            ),
            _ => {
                requests.auth_acquisitions.clear(&requests);
                None
            }
        };
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .unwrap()
            .unwrap();
        if let Some(next) = next {
            next.await.unwrap();
        }
        assert_eq!(
            mock.interactions.lock().await.len(),
            usize::from(replacement == 1)
        );
        assert!(requests.begin(true).is_ok());
        server.abort();
    }
}
#[tokio::test]
async fn auth_acquisition_dispatched_rpc_survives_core_clear_and_holds_original_gate() {
    let kernel = MockKernel::new();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    *kernel.interaction_barrier.lock().await = Some((entered.clone(), release.clone()));
    let (mock, channel, server) = serve_mock(kernel).await;
    let requests = ConnectionRequests::default();
    let task = dispatch(
        &requests,
        &channel,
        &event(Some(request(6, false))),
        Fixture::default(),
    )
    .unwrap();
    entered.notified().await;
    dispatch(&requests, &channel, &event(None), Fixture::default());
    assert!(requests.commit_gate.try_lock().is_err());
    assert!(!task.is_finished());
    assert!(requests.has_auth_acquisition_owner());
    let mut state = dialed_state(channel.clone());
    state.connection_requests = requests.clone();
    let stop = tokio::spawn(async move { CoreClient.stop(&state).await });
    tokio::task::yield_now().await;
    assert!(!stop.is_finished());
    release.notify_one();
    task.await.unwrap();
    stop.await.unwrap().unwrap();
    assert_eq!(mock.stops.lock().await.len(), 1);
    assert_eq!(mock.interactions.lock().await.len(), 1);
    assert!(requests.commit_gate.try_lock().is_ok());
    assert!(requests.begin(true).is_ok());
    server.abort();
}
#[tokio::test]
async fn auth_acquisition_fixed_deadline_expires_in_gate_without_late_candidate() {
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let requests = ConnectionRequests::default();
    let held = requests.commit_gate.clone().lock_owned().await;
    let mut req = request(7, false);
    req.deadline_monotonic_ns = AuthDeadline::after(Duration::from_millis(200))
        .unwrap()
        .monotonic_ns();
    let deadline_ns = req.deadline_monotonic_ns;
    let ready = Arc::new(Notify::new());
    let trace = Arc::new(Mutex::new(Vec::new()));
    let fixture = Fixture {
        trace: trace.clone(),
        ..Default::default()
    };
    let owner = requests.clone();
    let acquired = ready.clone();
    let task = dispatch_auth_acquisition(
        &requests,
        &channel,
        &event(Some(req)),
        move |request, lease, attempt| async move {
            let result = fixture.acquire(owner, request, lease, attempt).await;
            assert!(result.0.is_ok(), "认证必须成功后才进入本用例的回交 gate 等待");
            assert!(AuthDeadline::from_monotonic_ns(deadline_ns).is_ok(), "认证与两轮回退仍须在原期限内完成");
            acquired.notify_one();
            result
        },
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), ready.notified())
        .await
        .unwrap();
    assert!(requests.commit_gate.try_lock().is_err());
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    // 后台沿 HTTP → 隐藏 Owned；本 fixture 首轮注入失败，原第二轮成功。
    assert_eq!(count(&trace, "http:open"), 1);
    assert_eq!(count(&trace, "http:reuse"), 1);
    assert_eq!(count(&trace, "http:close"), 1);
    assert_eq!(count(&trace, "owned:open"), 2);
    assert_eq!(count(&trace, "owned:inject"), 2);
    assert_eq!(count(&trace, "owned:extract"), 1);
    assert_eq!(count(&trace, "owned:close"), 2);
    assert_eq!(count(&trace, "owned:visible"), 0);
    assert_eq!(count(&trace, "owned:fresh-visible"), 0);
    assert!(AuthDeadline::from_monotonic_ns(deadline_ns).is_err());
    assert!(mock.interactions.lock().await.is_empty());
    assert!(mock.connects.lock().await.is_empty());
    drop(held);
    assert!(requests.commit_gate.try_lock().is_ok());
    assert!(requests.begin(true).is_ok());
    server.abort();
}
#[tokio::test]
async fn auth_acquisition_local_window_cancel_returns_true_cancelled_failure() {
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let requests = ConnectionRequests::default();
    let entered = Arc::new(Notify::new());
    let task = dispatch(
        &requests,
        &channel,
        &event(Some(request(8, true))),
        Fixture {
            entered: Some(entered.clone()),
            block: true,
            ..Default::default()
        },
    )
    .unwrap();
    entered.notified().await;
    assert!(requests.cancel());
    task.await.unwrap();
    let received = mock.interactions.lock().await;
    assert_eq!(received.len(), 1);
    let result = wire::AtrustAuthAcquisitionResult::decode(received[0].response_payload.as_slice())
        .unwrap()
        .result
        .unwrap();
    let wire::atrust_auth_acquisition_result::Result::Failed(error) = result else {
        panic!("关窗必须回交取消失败")
    };
    assert_eq!(error.code, wire::ErrorCode::CancelledBeforeStart as i32);
    assert_eq!(error.reason, "atrust/auth_cancelled");
    assert!(error.operation_detail.is_none());
    assert_eq!(projected_auth_error(&error).reason, "atrust/auth_cancelled");
    server.abort();
}

#[tokio::test]
async fn auth_acquisition_failure_categories_reach_actual_wire_ui_mapping() {
    for (auth_code, wire_code) in [
        (AuthErrorCode::FailedPage, wire::ErrorCode::Unauthorized),
        (AuthErrorCode::GatewayThrottled, wire::ErrorCode::Unauthorized),
        (AuthErrorCode::ClientInstallRequired, wire::ErrorCode::Unauthorized),
        (AuthErrorCode::CredentialRequired, wire::ErrorCode::Unauthorized),
        (AuthErrorCode::SessionInvalid, wire::ErrorCode::Unauthorized),
        (
            AuthErrorCode::ChannelTransportFailed,
            wire::ErrorCode::ObservationFailed,
        ),
        (
            AuthErrorCode::AcquisitionTimeout,
            wire::ErrorCode::DeadlineExceeded,
        ),
    ] {
        let failure = BrowserAuthError::auth(
            error(auth_code, AuthOperation::Observe),
            "fixture 分类".into(),
        );
        assert_auth_failure_projection(
            failure,
            wire_code,
            Some(auth_code.machine_code()),
            "fixture 原因",
        )
        .await;
    }
    assert_auth_failure_projection(
        BrowserAuthError::Host("fixture 实际存储失败\n请重试".into()),
        wire::ErrorCode::ObservationFailed,
        None,
        "fixture 实际存储失败请重试",
    )
    .await;
}

async fn assert_auth_failure_projection(
    failure: BrowserAuthError,
    expected: wire::ErrorCode,
    machine_code: Option<&str>,
    detail: &str,
) {
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let requests = ConnectionRequests::default();
    dispatch_auth_acquisition(
        &requests,
        &channel,
        &event(Some(request(11, true))),
        move |_, lease, _| async move { (Err(failure), lease) },
    )
    .unwrap()
    .await
    .unwrap();
    let received = mock.interactions.lock().await;
    assert_eq!(received.len(), 1);
    let result = wire::AtrustAuthAcquisitionResult::decode(received[0].response_payload.as_slice())
        .unwrap()
        .result
        .unwrap();
    let wire::atrust_auth_acquisition_result::Result::Failed(error) = result else {
        panic!("实际获取失败必须沿原 RPC 回交 Failed")
    };
    assert_eq!(error.code, expected as i32);
    assert_eq!(error.stage, wire::ErrorStage::AwaitingInteraction as i32);
    assert!(error.operation_detail.is_none());
    if let Some(machine_code) = machine_code {
        assert_eq!(
            error.reason.split_once('|').unwrap().0,
            format!("atrust/{machine_code}")
        );
        assert_eq!(error.native.as_ref().unwrap().code, 123);
    } else {
        assert_eq!(
            error.reason.split_once('|').unwrap().0,
            "atrust/auth_acquisition_failed"
        );
        assert!(error.native.is_none());
    }
    let ui = projected_auth_error(&error);
    assert_eq!(ui.code, expected.as_str_name());
    assert!(ui.message.contains(detail));
    assert!(!ui.message.chars().any(char::is_control));
    assert!(ui.reason.contains(detail));
    server.abort();
}
#[tokio::test]
async fn auth_acquisition_original_stop_reaches_core_after_browser_cleanup() {
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel.clone());
    let entered = Arc::new(Notify::new());
    let task = dispatch(
        &state.connection_requests,
        &channel,
        &event(Some(request(9, true))),
        Fixture {
            entered: Some(entered.clone()),
            block: true,
            ..Default::default()
        },
    )
    .unwrap();
    entered.notified().await;
    CoreClient.stop(&state).await.unwrap();
    task.await.unwrap();
    assert_eq!(mock.stops.lock().await.len(), 1);
    assert!(mock.interactions.lock().await.is_empty());
    server.abort();
}
#[tokio::test]
async fn auth_acquisition_new_user_connect_cleans_original_owner_before_new_request() {
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel.clone());
    let entered = Arc::new(Notify::new());
    let task = dispatch(
        &state.connection_requests,
        &channel,
        &event(Some(request(10, true))),
        Fixture {
            entered: Some(entered.clone()),
            block: true,
            ..Default::default()
        },
    )
    .unwrap();
    entered.notified().await;
    CoreClient
        .connect_with_browser_mode(
            &state,
            ConnectIntent::default(),
            |_, _, lease, _, _, _attempt| async move { Ok((Vec::new(), Some(lease))) },
        )
        .await
        .unwrap();
    task.await.unwrap();
    assert_eq!(mock.connects.lock().await.len(), 1);
    assert!(mock.interactions.lock().await.is_empty());
    server.abort();
}

#[tokio::test]
async fn auth_acquisition_recovery_requiring_interaction_emits_no_window_event() {
    let (mock, channel, server) = serve_mock(MockKernel::new()).await;
    let requests = ConnectionRequests::default();
    let fixture = Fixture { challenge: true, ..Default::default() };
    let trace = fixture.trace.clone();
    let mut req = request(41, true);
    req.mode = "headless".into();
    dispatch(&requests, &channel, &event(Some(req)), fixture).unwrap().await.unwrap();
    let received = mock.interactions.lock().await;
    let result = wire::AtrustAuthAcquisitionResult::decode(received[0].response_payload.as_slice()).unwrap().result.unwrap();
    server.abort();
    assert_eq!(count(&trace, "owned:visible"), 0, "后台验证码不得转可见窗口");
    assert_eq!(count(&trace, "owned:fresh-visible"), 0);
    assert!(matches!(result, wire::atrust_auth_acquisition_result::Result::Failed(_)));
    assert!(mock.connects.lock().await.is_empty());
}
