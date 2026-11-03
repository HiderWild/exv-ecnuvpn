//! 本次请求的官方接收端/证书准备交互；所有动作经既有Core认证控制面。
use super::*;
use crate::kernel::{
    client::{CoreClient, CoreState},
    connection_request::AuthPreparationAction as Action,
    local_agent,
};
use exv_vpn_wire::generated as wire;
use tauri::{Emitter, Manager};

pub(super) const EVENT: &str = "exv://auth-preparation";

fn emit(
    app: &AppHandle,
    generation: u64,
    kind: &str,
    busy: bool,
    message: &str,
    saved: bool,
    reply: Option<&wire::AuthReceiverControlReply>,
) {
    emit_with_native(app, generation, kind, busy, message, saved, reply, None);
}

fn emit_with_native(
    app: &AppHandle,
    generation: u64,
    kind: &str,
    busy: bool,
    message: &str,
    saved: bool,
    reply: Option<&wire::AuthReceiverControlReply>,
    native_code: Option<i64>,
) {
    let stage = match kind {
        "agent_start" => "agent_start",
        "certificate" | "certificate_approval" => "certificate",
        "closed" => "complete",
        _ => "receiver",
    };
    let approval_required = kind == "certificate_approval";
    let kind = if approval_required {
        "certificate"
    } else if kind == "agent_start" {
        "exclusive"
    } else {
        kind
    };
    use crate::kernel::connection_request::AuthPreparationPhase;
    let phase = match kind {
        "closed" => AuthPreparationPhase::Idle,
        "certificate" => AuthPreparationPhase::Certificate { approval_required },
        _ => AuthPreparationPhase::Exclusive,
    };
    app.state::<CoreState>()
        .connection_requests
        .set_auth_preparation(generation, phase, busy);
    let receiver = reply.and_then(|reply| reply.receiver.as_ref()).map(|state| serde_json::json!({
        "running": state.running, "can_start": state.can_start, "recovery_available": state.recovery_available,
        "source": state.source,
        "original_config": state.original_config.as_ref().map(|c| serde_json::json!({"windows_start_type": c.windows_start_type, "windows_delayed_auto_start": c.windows_delayed_auto_start})),
        "current_config": state.current_config.as_ref().map(|c| serde_json::json!({"windows_start_type": c.windows_start_type, "windows_delayed_auto_start": c.windows_delayed_auto_start})),
    }));
    let _ = app.emit(EVENT, serde_json::json!({
        "certificate_approval_required": approval_required,
        "generation": generation, "kind": kind, "busy": busy, "message": message,
        "saved_login_available": saved, "receiver": receiver,
        "stage":stage, "completed_steps": reply.map(|r| &r.completed_steps),
        "native_code": native_code.or_else(|| reply.and_then(|r| r.error.as_ref()).and_then(|e| e.native.as_ref()).map(|n| n.code)),
    }));
}

/// 准备交互的事件收口；回调与具体 UI runtime 解耦，便于验证取消时序。
struct PreparationScope<F: Fn()> {
    close: Option<F>,
}
impl<F: Fn()> PreparationScope<F> {
    fn new(close: F) -> Self {
        Self { close: Some(close) }
    }
    fn close(&mut self) {
        if let Some(close) = self.close.take() {
            close();
        }
    }
}
impl<F: Fn()> Drop for PreparationScope<F> {
    fn drop(&mut self) {
        self.close();
    }
}

async fn next_action(
    actions: &mut watch::Receiver<Option<Action>>,
    cancel: &mut watch::Receiver<LoginAction>,
    deadline: tokio::time::Instant,
) -> Result<Action, BrowserAuthError> {
    loop {
        if *cancel.borrow() == LoginAction::Cancel {
            return Err(BrowserAuthError::control(
                AuthControl::Cancel,
                "浏览器登录已取消",
            ));
        }
        tokio::select! { biased;
            changed = cancel.changed() => {
                if changed.is_err() { return Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")); }
            }
            _ = tokio::time::sleep_until(deadline) => return Err(BrowserAuthError::Host("准备系统浏览器登录超过本次认证期限".into())),
            changed = actions.changed() => {
                if changed.is_err() { return Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")); }
                if let Some(action) = *actions.borrow_and_update() { return Ok(action); }
            }
        }
    }
}

pub(super) async fn prepare(
    app: &AppHandle,
    generation: u64,
    gateway: &str,
    cancel: &mut watch::Receiver<LoginAction>,
    attempt: &AuthAttempt,
    initial_error: Option<local_agent::AgentStartFailure>,
) -> Result<local_agent::PreparedAgentTls, BrowserAuthError> {
    let state = app.state::<CoreState>();
    let requests = &state.connection_requests;
    let deadline_ns = requests
        .auth_control_deadline(generation)
        .map_err(BrowserAuthError::Host)?;
    let deadline = attempt.deadline(Duration::from_secs(300));
    let mut actions = requests
        .auth_preparation_actions(generation)
        .map_err(BrowserAuthError::Host)?;
    actions.borrow_and_update();
    let saved = super::super::client::school_auto_login_saved(
        gateway,
        true,
        &exv_vpn_win32_config::config_dir(),
    )
    .is_some();
    let operation_id = uuid::Uuid::new_v4().as_bytes().to_vec();
    use crate::kernel::connection_request::AuthPreparationPhase;
    requests.set_auth_preparation(generation, AuthPreparationPhase::Exclusive, true);
    let mut scope =
        PreparationScope::new(|| emit(app, generation, "closed", false, "", saved, None));
    let mut visible = initial_error.is_some();
    let mut next = if let Some(error) = initial_error {
        emit_with_native(
            app,
            generation,
            "agent_start",
            false,
            &error.message,
            saved,
            None,
            error.native_code,
        );
        next_action(&mut actions, cancel, deadline).await?
    } else {
        Action::Retry
    };
    let mut pending: Option<(&str, String)> = None;
    let mut last_reply: Option<wire::AuthReceiverControlReply> = None;
    loop {
        if let Some((kind, message)) = pending.take() {
            visible = true;
            emit(
                app,
                generation,
                kind,
                false,
                &message,
                saved,
                last_reply.as_ref(),
            );
            next = next_action(&mut actions, cancel, deadline).await?;
        }
        match next {
            Action::Cancel => {
                return Err(BrowserAuthError::control(
                    AuthControl::Cancel,
                    "浏览器登录已取消",
                ));
            }
            Action::UseEmbedded => {
                scope.close();
                return Err(BrowserAuthError::FallBackEmbedded);
            }
            Action::TrySavedLogin if saved => {
                scope.close();
                return Err(BrowserAuthError::FallBackEmbedded);
            }
            Action::TrySavedLogin => {
                pending = Some((
                    "exclusive",
                    "未保存完整的学校用户名和密码，请使用内嵌浏览器登录".into(),
                ));
                continue;
            }
            _ => {}
        }
        let approve = next == Action::ApproveCertificate;
        let action = if next == Action::DisableOfficialAndRetry {
            wire::AuthReceiverAction::DisableAndStop
        } else {
            wire::AuthReceiverAction::Inspect
        };
        if visible {
            emit(
                app,
                generation,
                if approve { "certificate" } else { "exclusive" },
                true,
                "正在准备本次系统浏览器登录",
                saved,
                last_reply.as_ref(),
            );
        }
        log::info!(target: "exv.local_agent", "agent.receiver_control generation={generation} action={action:?}");
        let reply = tokio::select! { biased;
            _ = async {
                while *cancel.borrow() != LoginAction::Cancel {
                    if cancel.changed().await.is_err() { break; }
                }
            } => return Err(BrowserAuthError::control(AuthControl::Cancel, "浏览器登录已取消")),
            reply = tokio::time::timeout_at(
            deadline,
            CoreClient.auth_receiver_control(
                &state,
                wire::AuthReceiverControlRequest {
                    operation_id: operation_id.clone(),
                    action: action as i32,
                    deadline_monotonic_ns: deadline_ns,
                },
            ),
        ) => reply,
        };
        let reply = match reply {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) => {
                pending = Some(("exclusive", format!("检查本机认证接收端失败：{error}")));
                continue;
            }
            Err(_) => {
                return Err(BrowserAuthError::Host(
                    "本机认证接收端准备超过本次认证期限".into(),
                ));
            }
        };
        let failure = reply.error.as_ref().map(|error| {
            error.operation_detail.clone().unwrap_or_else(|| {
                format!(
                    "本机认证接收端操作失败：{}，错误码 {:?}",
                    error.reason,
                    error.native.as_ref().map(|n| n.code)
                )
            })
        });
        let occupied = reply
            .receiver
            .as_ref()
            .is_some_and(|r| r.running || r.can_start);
        let absent = reply.receiver.is_none();
        last_reply = Some(reply);
        if let Some(message) = failure {
            log::warn!(target:"exv.local_agent", "agent.receiver_failed generation={generation} error={message}");
            pending = Some(("exclusive", message));
            continue;
        }
        if absent {
            pending = Some(("exclusive", "本机认证接收端查询未返回状态，请重试".into()));
            continue;
        }
        if occupied {
            pending = Some((
                "exclusive",
                "系统浏览器认证通道独占，继续需停止并禁用 aTrust 服务".into(),
            ));
            continue;
        }
        if *cancel.borrow() == LoginAction::Cancel {
            return Err(BrowserAuthError::control(
                AuthControl::Cancel,
                "浏览器登录已取消",
            ));
        }
        match local_agent::inspect_agent_certificate() {
            Ok(local_agent::CertificatePreparation::ApprovalRequired { .. }) if !approve => {
                pending = Some(("certificate_approval", String::new()));
                continue;
            }
            Err(error) => {
                pending = Some(("certificate", format!("检查本地登录证书失败：{error}")));
                continue;
            }
            _ => {}
        }
        match local_agent::prepare_agent_certificate(approve) {
            Ok(prepared) => {
                scope.close();
                return Ok(prepared);
            }
            Err(error) => {
                log::warn!(target:"exv.local_agent", "agent.certificate_failed generation={generation} error={error}");
                pending = Some(("certificate", format!("准备本地登录证书失败：{error}")));
            }
        }
    }
}
