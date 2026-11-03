//! 原 WatchEvents 的一次认证伴生消费者；只绑定原 ConnectionRequest，不建立恢复 worker。
use super::connection_request::{ConnectionRequest, ConnectionRequests};
use exv_core::atrust_browser::BrowserAuthError;
use exv_core::browser_auth::{AuthAttempt, AuthControl, AuthErrorCode};
use exv_vpn_win32_ipc::{auth_deadline::AuthDeadline, browser_session::BrowserSession};
use exv_vpn_wire::generated as wire;
use prost::Message;
use std::sync::Mutex;
use tokio::sync::watch;
use tonic::transport::Channel;
use zeroize::Zeroize;

#[derive(Debug, Default)]
pub(super) struct AuthAcquisitions {
    observed: Mutex<Option<Observed>>,
}
#[derive(Debug)]
struct Observed {
    id: Vec<u8>,
    epoch: Vec<u8>,
    generation: Option<u64>,
    cleared: bool,
}

pub(super) struct Ticket {
    request: wire::AtrustAuthAcquisition,
    wait: Option<watch::Receiver<bool>>,
    expected_generation: u64,
}
impl AuthAcquisitions {
    /// 在 UI 映射前同步消费有序事件；保留最后已见 id，清除和流断开后重放也不重复获取。
    pub(super) fn observe(
        &self,
        requests: &ConnectionRequests,
        request: Option<&wire::AtrustAuthAcquisition>,
    ) -> Option<Ticket> {
        let mut observed = self.observed.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(request) = request {
            if request.interaction_id.len() != 16 || request.runtime_epoch.len() != 16 {
                return None;
            }
            if observed
                .as_ref()
                .is_some_and(|old| old.id == request.interaction_id)
            {
                // 同 id 的错 epoch 也不能替换已登记的原请求。
                return None;
            }
        }
        let wait = observed
            .as_ref()
            .and_then(|old| old.generation)
            .and_then(|generation| requests.cancel_auth_owner(generation));
        let Some(request) = request else {
            if let Some(old) = observed.as_mut() {
                old.cleared = true;
            }
            return None;
        };
        let expected_generation = requests.generation();
        *observed = Some(Observed {
            id: request.interaction_id.clone(),
            epoch: request.runtime_epoch.clone(),
            generation: None,
            cleared: false,
        });
        Some(Ticket {
            request: request.clone(),
            wait,
            expected_generation,
        })
    }

    async fn begin(
        &self,
        requests: &ConnectionRequests,
        ticket: &mut Ticket,
    ) -> Option<ConnectionRequest> {
    let deadline = AuthDeadline::from_monotonic_ns(ticket.request.deadline_monotonic_ns)
            .and_then(AuthDeadline::tokio_deadline)
            .ok()?;
        if let Some(finished) = ticket.wait.as_mut() {
            while !*finished.borrow() {
                tokio::select! { biased;
                    () = tokio::time::sleep_until(deadline) => return None,
                    changed = finished.changed() => if changed.is_err() { break; },
                }
            }
        }
        let mut observed = self.observed.lock().unwrap_or_else(|e| e.into_inner());
        let current = observed.as_mut().filter(|old| {
            old.id == ticket.request.interaction_id && old.epoch == ticket.request.runtime_epoch
        })?;
        // 清除事件在获取开始前同样废止预约。
        if current.cleared {
            return None;
        }
        let lease = requests
            .begin_with_generation(
                matches!(ticket.request.mode.as_str(), "embedded" | "headless"),
                ticket.expected_generation,
            )
            .ok()?;
        lease.mark_auth_acquisition();
        current.generation = Some(lease.generation());
        Some(lease)
    }

    pub(super) fn is_current_owner(
        &self,
        request: &wire::AtrustAuthAcquisition,
        lease: &ConnectionRequest,
    ) -> bool {
        self.observed.lock().is_ok_and(|observed| {
            observed.as_ref().is_some_and(|current| {
                !current.cleared
                    && current.id == request.interaction_id
                    && current.epoch == request.runtime_epoch
                    && current.generation == Some(lease.generation())
            })
        })
    }

    fn begin_reply(
        &self,
        request: &wire::AtrustAuthAcquisition,
        lease: &ConnectionRequest,
        failed: bool,
    ) -> bool {
        let observed = self.observed.lock().unwrap_or_else(|e| e.into_inner());
        if !observed.as_ref().is_some_and(|current| {
            !current.cleared
                && current.id == request.interaction_id
                && current.epoch == request.runtime_epoch
                && current.generation == Some(lease.generation())
        }) {
            return false;
        }
        if failed {
            lease.begin_auth_failure_reply()
        } else {
            lease.begin_core_connect()
        }
    }

    pub(super) fn clear(&self, requests: &ConnectionRequests) -> Option<watch::Receiver<bool>> {
        let mut observed = self.observed.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = observed.as_mut() {
            let finished = current
                .generation
                .and_then(|generation| requests.cancel_auth_owner(generation));
            current.cleared = true;
            return finished;
        }
        None
    }
}

/// 获取与回交在原 owner 上完成；RPC 派发之后不再 select 本地清除，真实回执负责释放 gate。
pub(super) async fn consume<F, Fut>(
    requests: ConnectionRequests,
    channel: Channel,
    mut ticket: Ticket,
    acquire: F,
) where
    F: FnOnce(wire::AtrustAuthAcquisition, ConnectionRequest, AuthAttempt) -> Fut,
    Fut:
        std::future::Future<Output = (Result<BrowserSession, BrowserAuthError>, ConnectionRequest)>,
{
    let Some(lease) = requests
        .auth_acquisitions
        .begin(&requests, &mut ticket)
        .await
    else {
        return;
    };
    lease.set_auth_control_deadline(ticket.request.deadline_monotonic_ns);
    let deadline = AuthDeadline::from_monotonic_ns(ticket.request.deadline_monotonic_ns)
        .and_then(AuthDeadline::tokio_deadline);
    let (result, lease) = match deadline {
        Ok(deadline) => {
            acquire(
                ticket.request.clone(),
                lease,
                AuthAttempt::with_deadline(deadline),
            )
            .await
        }
        Err(_) => (Err(timeout_error()), lease),
    };
    let result = match result {
        Ok(session) => {
            let encoded = super::client::encode_browser_candidate(&session, std::path::Path::new(&ticket.request.browser_profile));
            match encoded {
            Ok(bytes) => wire::atrust_auth_acquisition_result::Result::Candidate(bytes),
            Err(_) => wire::atrust_auth_acquisition_result::Result::Failed(failure(
                &ticket.request,
                BrowserAuthError::Host("无法编码浏览器登录会话".into()),
            )),
            }
        },
        Err(error) => {
            wire::atrust_auth_acquisition_result::Result::Failed(failure(&ticket.request, error))
        }
    };
    let mut message = wire::AtrustAuthAcquisitionResult {
        result: Some(result),
    };
    let failed = matches!(
        message.result,
        Some(wire::atrust_auth_acquisition_result::Result::Failed(_))
    );
    let mut payload = message.encode_to_vec();
    if let Some(wire::atrust_auth_acquisition_result::Result::Candidate(bytes)) =
        &mut message.result
    {
        bytes.zeroize();
    }
    lease.set_auth_control_deadline(ticket.request.deadline_monotonic_ns);
    let deadline = AuthDeadline::from_monotonic_ns(ticket.request.deadline_monotonic_ns)
        .and_then(AuthDeadline::tokio_deadline)
        .unwrap_or_else(|_| tokio::time::Instant::now());
    let guard = tokio::select! { biased;
        () = tokio::time::sleep_until(deadline) => { payload.zeroize(); return; }
        () = lease.cancelled(), if !failed => { payload.zeroize(); return; }
        guard = requests.commit_gate.clone().lock_owned() => guard,
    };
    // 队列与认证共同消费 D；真实超期回交可被 Core 本地拒绝，不能派发迟到候选。
    if AuthDeadline::from_monotonic_ns(ticket.request.deadline_monotonic_ns).is_err() {
        payload.zeroize();
        payload = wire::AtrustAuthAcquisitionResult {
            result: Some(wire::atrust_auth_acquisition_result::Result::Failed(
                failure(&ticket.request, timeout_error()),
            )),
        }
        .encode_to_vec();
    }
    if !requests
        .auth_acquisitions
        .begin_reply(&ticket.request, &lease, failed)
    {
        payload.zeroize();
        return;
    }
    let mut response = super::wire::auth_acquisition_response(&ticket.request, payload);
    let owned = tokio::spawn(async move {
        let _guard = guard;
        let _lease = lease;
        let mut client = wire::kernel_control_client::KernelControlClient::new(channel);
        let reply = client.respond_interaction(response.clone()).await;
        response.response_payload.zeroize();
        reply
    });
    match owned.await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            log::warn!(target: "exv.atrust_auth", "auth acquisition response RPC failed: {}", error.code())
        }
        Err(error) => {
            log::warn!(target: "exv.atrust_auth", "auth acquisition response task failed: {error}")
        }
    }
}
fn timeout_error() -> BrowserAuthError {
    BrowserAuthError::auth(
        exv_core::browser_auth::AuthError {
            code: AuthErrorCode::AcquisitionTimeout,
            operation: exv_core::browser_auth::AuthOperation::Wait,
            native_code: None,
            server_detail: None,
            failure_kind: None,
        },
        "浏览器登录超时，请重试".into(),
    )
}

/// 原认证错误到既有 VpnError 的边界映射；不把传输/存储/未知错误伪作认证拒绝。
fn failure(request: &wire::AtrustAuthAcquisition, error: BrowserAuthError) -> wire::VpnError {
    let (code, reason, native) = match &error {
        BrowserAuthError::Auth { error, .. } => {
            let code = match error.code {
                AuthErrorCode::AcquisitionTimeout => wire::ErrorCode::DeadlineExceeded,
                AuthErrorCode::FailedPage
                | AuthErrorCode::GatewayThrottled
                | AuthErrorCode::ClientInstallRequired
                | AuthErrorCode::CredentialRequired
                | AuthErrorCode::SessionInvalid => wire::ErrorCode::Unauthorized,
                _ => wire::ErrorCode::ObservationFailed,
            };
            (
                code,
                format!("atrust/{}", error.code.machine_code()),
                error.native_code.map(|code| wire::RedactedNativeError {
                    category: wire::NativeErrorCategory::Unknown as i32,
                    namespace: wire::NativeErrorNamespace::Unknown as i32,
                    code,
                }),
            )
        }
        BrowserAuthError::Control {
            control: AuthControl::Cancel,
            ..
        } => (
            wire::ErrorCode::CancelledBeforeStart,
            "atrust/auth_cancelled".into(),
            None,
        ),
        _ => (
            wire::ErrorCode::ObservationFailed,
            "atrust/auth_acquisition_failed".into(),
            None,
        ),
    };
    let detail = match &error {
        BrowserAuthError::Auth {
            error: auth,
            compatibility_message,
        } => {
            let summary = auth
                .server_detail
                .as_deref()
                .unwrap_or(compatibility_message);
            // 原编排已将两轮原因合为至多 59 字；先净化摘要，再加操作名，避免截掉末轮。
            let summary = exv_core::browser_auth::page_facts::sanitize_server_detail(summary);
            format!("aTrust {:?}: {summary}", auth.operation)
        }
        _ => exv_core::browser_auth::page_facts::sanitize_server_detail(&error.legacy_message()),
    };
    // 复用既有 reason 的 code|detail 形式；取消 reason 是原单通知的精确判据。
    let reason = if code == wire::ErrorCode::CancelledBeforeStart || detail.is_empty() {
        reason
    } else {
        format!("{reason}|{detail}")
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
        native,
        reason,
        operation_detail: None,
    }
}
