//! 原 WatchEvents 的一次后台认证消费；请求身份属于 Core，认证与提交 lease 沿原连接入口。
use super::{
    browser_auth::{self, AuthArtifact, AuthContext, AuthMode, AuthOptions},
    client::{CoreClient, CoreState},
    connection_request::ConnectionRequest,
    error::AppError,
};
use exv_vpn_atrust::auth::origin::GatewayOrigin;
use exv_vpn_darwin_ipc::auth_deadline::AuthDeadline;
use exv_vpn_wire::generated as wire;
use prost::Message;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tokio::sync::watch;
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Default)]
struct Observation {
    seen: Option<(Vec<u8>, Vec<u8>)>,
    owner: Option<u64>,
    pending: Option<watch::Sender<bool>>,
}
#[derive(Debug, Default)]
pub(super) struct AuthAcquisitions(Mutex<Observation>);
pub(super) struct Acquisition {
    pub(super) wire: wire::AtrustAuthAcquisition,
    pub(super) owner: Arc<ConnectionRequest>,
}
pub(super) struct AcquisitionTicket {
    pub(super) ready: Option<Acquisition>,
    wire: wire::AtrustAuthAcquisition,
    generation: u64,
    changed: watch::Receiver<u64>,
    finished: Option<watch::Receiver<bool>>,
    cancelled: watch::Receiver<bool>,
}
impl AuthAcquisitions {
    /// 同一有序流重放只消费一次；清除不取消后来用户创建的 owner。
    pub(super) fn observe(
        &self,
        requests: &super::connection_request::ConnectionRequests,
        request: Option<&wire::AtrustAuthAcquisition>,
    ) -> Option<AcquisitionTicket> {
        let mut observation = self.0.lock().expect("auth acquisition observation");
        let key = request.map(|request| {
            (
                request.interaction_id.clone(),
                request.runtime_epoch.clone(),
            )
        });
        if key.is_some() && key == observation.seen {
            return None;
        }
        if let Some(owner) = observation.owner.take() {
            requests.cancel_owner(owner);
        }
        if let Some(pending) = observation.pending.take() {
            pending.send_replace(true);
        }
        let request = request?;
        observation.seen = key;
        if request.interaction_id.len() != 16 || request.runtime_epoch.len() != 16 {
            return None;
        }
        let (generation, changed, finished) = requests.recovery_handoff();
        let ready = if finished.is_none() {
            let owner = requests.begin_recovery(generation).ok()?;
            observation.owner = Some(owner.generation());
            Some(Acquisition {
                wire: request.clone(),
                owner,
            })
        } else {
            None
        };
        let (pending, cancelled) = watch::channel(false);
        observation.pending = Some(pending);
        Some(AcquisitionTicket {
            wire: request.clone(),
            ready,
            generation,
            changed,
            finished,
            cancelled,
        })
    }
    async fn begin(
        &self,
        requests: &super::connection_request::ConnectionRequests,
        mut ticket: AcquisitionTicket,
        deadline: tokio::time::Instant,
    ) -> Option<Acquisition> {
        if let Some(ready) = ticket.ready.take() {
            return Some(ready);
        }
        // 旧 submitted lease 的通知仅在真实回执持有者 Drop 后完成。
        if let Some(finished) = ticket.finished.as_mut() {
            let waiting = !*finished.borrow();
            if waiting {
                tokio::select! { biased;
                    _ = tokio::time::sleep_until(deadline) => return None,
                    _ = ticket.cancelled.changed() => return None,
                    _ = ticket.changed.changed() => return None,
                    _ = finished.changed() => {}
                }
            }
        }
        let mut observation = self.0.lock().expect("auth acquisition observation");
        if *ticket.cancelled.borrow()
            || tokio::time::Instant::now() >= deadline
            || !observation.seen.as_ref().is_some_and(|(id, epoch)| {
                *id == ticket.wire.interaction_id && *epoch == ticket.wire.runtime_epoch
            })
        {
            return None;
        }
        let owner = requests.begin_recovery(ticket.generation).ok()?;
        observation.owner = Some(owner.generation());
        Some(Acquisition {
            wire: ticket.wire,
            owner,
        })
    }
    pub(super) fn current(&self, ticket: &Acquisition) -> bool {
        let observation = self.0.lock().expect("auth acquisition observation");
        observation.owner == Some(ticket.owner.generation())
            && observation.seen.as_ref().is_some_and(|(id, epoch)| {
                *id == ticket.wire.interaction_id && *epoch == ticket.wire.runtime_epoch
            })
            && ticket.owner.begin_submit().is_ok()
    }
}

pub(super) fn consume(
    app: &tauri::AppHandle,
    channel: &tonic::transport::Channel,
    request: Option<&wire::AtrustAuthAcquisition>,
) {
    let Some(state) = app.try_state::<CoreState>() else {
        return;
    };
    let Some(ticket) = state
        .connection_requests
        .acquisitions
        .observe(&state.connection_requests, request)
    else {
        return;
    };
    let app = app.clone();
    let channel = channel.clone();
    // 长认证独立运行；原订阅继续消费后续 snapshot/clear/Stop。
    tauri::async_runtime::spawn(async move {
        let state = app.state::<CoreState>();
        let result = acquire_and_reply(
            Some(&app),
            &state,
            channel,
            ticket,
            |context, mode, owner, options| {
                let app = &app;
                let selection = &state.browser_selection;
                async move {
                    browser_auth::authenticate_with_options(
                        app, &context, mode, selection, &owner, options,
                    )
                    .await
                }
            },
        )
        .await;
        if let Err(error) = result {
            eprintln!("aTrust recovery reply: {error}");
        }
    });
}

/// 与普通连接共用原 authenticate 原语，后台只回交材料，不派普通 Connect 或读取最新配置。
pub(super) async fn acquire_and_reply<F, Fut>(
    app: Option<&tauri::AppHandle>,
    state: &CoreState,
    channel: tonic::transport::Channel,
    ticket: AcquisitionTicket,
    authenticate: F,
) -> Result<(), AppError>
where
    F: FnOnce(AuthContext, AuthMode, Arc<ConnectionRequest>, AuthOptions) -> Fut,
    Fut: std::future::Future<Output = Result<AuthArtifact, String>>,
{
    let deadline = AuthDeadline::from_monotonic_ns(ticket.wire.deadline_monotonic_ns)
        .map_err(super::client::auth_deadline_app_error)?;
    let Some(ticket) = state
        .connection_requests
        .acquisitions
        .begin(
            &state.connection_requests,
            ticket,
            deadline
                .tokio_deadline()
                .map_err(super::client::auth_deadline_app_error)?,
        )
        .await
    else {
        return Ok(());
    };
    ticket.owner.set_auth_control_deadline(deadline.monotonic_ns());
    if !matches!(ticket.wire.mode.as_str(), "embedded" | "browser" | "headless") {
        return Err(AppError::Internal("恢复认证方式无效，请重新连接。".into()));
    }
    let mode = AuthMode::Headless;
    let context = AuthContext::freeze(&ticket.wire.gateway, &ticket.wire.user_agent)
        .map_err(AppError::Internal)?;
    if !state.connection_requests.acquisitions.current(&ticket) {
        return Ok(());
    }
    let options = AuthOptions {
        deadline: Some(
            deadline
                .tokio_deadline()
                .map_err(super::client::auth_deadline_app_error)?,
        ),
        auto_login: true,
        hidden: true,
        silent: true,
        config_dir: Some(std::path::PathBuf::from(&ticket.wire.config_dir)),
        explicit: None,
        milestone: None,
        diagnostic: app.map(|app| super::auth_diagnostics::failure_sink(app, ticket.owner.generation())),
    };
    let result = authenticate(context.clone(), mode, ticket.owner.clone(), options).await;
    // 原请求程序取消没有用户取消通知；用户关窗时原 owner 仍当前。
    if !state.connection_requests.acquisitions.current(&ticket) {
        return Ok(());
    }
    let result = result.and_then(|artifact| {
        if GatewayOrigin::parse(artifact.session.gateway())
            .is_ok_and(|origin| origin.endpoint() == context.gateway)
        {
            Ok(artifact)
        } else {
            Err("认证结果的网关与本次恢复请求不同，请重新连接。".into())
        }
    });
    let payload = encode_result(&ticket.wire, result)?;
    let gate = Arc::clone(&state.connection_requests.commit_gate);
    let permit = tokio::select! { biased;
        _ = ticket.owner.cancelled() => return Ok(()),
        permit = tokio::time::timeout_at(deadline.tokio_deadline().map_err(super::client::auth_deadline_app_error)?, gate.lock_owned()) =>
            permit.map_err(|_| AppError::BrowserLoginTimeout)?,
    };
    if !state.connection_requests.acquisitions.current(&ticket) {
        return Ok(());
    }
    deadline
        .remaining()
        .map_err(super::client::auth_deadline_app_error)?;
    ticket.owner.mark_submitted()?;
    // 从此 clear 与调用方结束都不截断 RPC；许可及 owner 随真实回执释放。
    CoreClient::reply_auth_acquisition(
        channel,
        ticket.wire.interaction_id,
        ticket.wire.runtime_epoch,
        payload,
        ticket.owner,
        permit,
    )
    .await
}

fn encode_result(
    request: &wire::AtrustAuthAcquisition,
    result: Result<AuthArtifact, String>,
) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let result = match result {
        Ok(artifact) => {
            let fresh = artifact.fresh_authentication();
            let bytes =
                exv_vpn_darwin_core::UiAtrustCredentialPackage::new(artifact.session, fresh)
        .with_authenticated_username(artifact.authenticated_username)
                    .to_bytes()
                    .map_err(|_| AppError::Internal("组装恢复认证材料失败，请重新连接。".into()))?;
            wire::atrust_auth_acquisition_result::Result::Candidate(bytes.to_vec())
        }
        Err(error) => wire::atrust_auth_acquisition_result::Result::Failed(failure(request, error)),
    };
    let mut message = wire::AtrustAuthAcquisitionResult {
        result: Some(result),
    };
    let bytes = Zeroizing::new(message.encode_to_vec());
    if let Some(wire::atrust_auth_acquisition_result::Result::Candidate(bytes)) =
        message.result.as_mut()
    {
        bytes.zeroize();
    }
    Ok(bytes)
}
fn failure(request: &wire::AtrustAuthAcquisition, error: String) -> wire::VpnError {
    let (code, reason) = match error.as_str() {
        "DARWIN_AUTH_CANCELLED" => (
            wire::ErrorCode::CancelledBeforeStart,
            "atrust/auth_cancelled".to_owned(),
        ),
        browser_auth::TIMEOUT => (
            wire::ErrorCode::DeadlineExceeded,
            "atrust/auth_timeout".to_owned(),
        ),
        browser_auth::CREDENTIAL_REQUIRED => (
            wire::ErrorCode::Unauthorized,
            "DARWIN_CORE_CONNECT_CREDENTIALS_MISSING".to_owned(),
        ),
        _ => (wire::ErrorCode::ObservationFailed, error),
    };
    wire::VpnError {
        code: code as i32,
        stage: wire::ErrorStage::AwaitingInteraction as i32,
        certainty: wire::EffectCertainty::NoEffect as i32,
        retry: wire::RetryAdvice::DoNotRetry as i32,
        subject: Some(wire::ErrorSubject {
            subject: Some(wire::error_subject::Subject::Runtime(
                wire::RuntimeSubject {
                    runtime_epoch: request.runtime_epoch.clone(),
                },
            )),
        }),
        resource: None,
        native: None,
        reason,
        operation_detail: None,
    }
}
