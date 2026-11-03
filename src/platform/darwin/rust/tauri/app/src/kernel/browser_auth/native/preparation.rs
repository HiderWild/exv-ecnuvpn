//! 本次请求的官方接收端与证书准备；所有特权操作经Core。
use crate::kernel::{
    client::{CoreClient, CoreState},
    connection_request::{AuthPreparationAction as Action, ConnectionRequest},
    native_agent,
};
use exv_vpn_wire::generated as wire;
use tauri::{Emitter, Manager};
use tokio::sync::watch;

fn saved_login_available() -> bool {
    let dir = exv_vpn_darwin_core::DarwinUiConfig::default()
        .config_dir()
        .to_path_buf();
    exv_vpn_darwin_core::DarwinUiConfig::load_from_dir(&dir)
        .ok()
        .is_some_and(|config| {
            exv_vpn_darwin_core::atrust_auth_storage::school_credentials(&config, None)
                .ok()
                .flatten()
                .is_some()
        })
}
fn emit(
    app: &tauri::AppHandle,
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
    app: &tauri::AppHandle,
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
    let config = |value: &wire::AuthReceiverConfigSummary| serde_json::json!({"launchd_targets":value.launchd_targets.iter().map(|target| serde_json::json!({"domain":target.domain,"label":target.label,"disabled":target.disabled})).collect::<Vec<_>>()});
    let receiver = reply.and_then(|r| r.receiver.as_ref()).map(|r| serde_json::json!({
        "running":r.running,"can_start":r.can_start,"recovery_available":r.recovery_available,"source":r.source,
        "original_config":r.original_config.as_ref().map(&config),"current_config":r.current_config.as_ref().map(&config),
    }));
    let _ = app.emit("exv://auth-preparation", serde_json::json!({
        "certificate_approval_required":approval_required,
        "generation":generation,"kind":kind,"busy":busy,"message":message,"saved_login_available":saved,"receiver":receiver,
        "stage":stage, "completed_steps":reply.map(|r| &r.completed_steps),
        "native_code":native_code.or_else(|| reply.and_then(|r| r.error.as_ref()).and_then(|e| e.native.as_ref()).map(|n| n.code)),
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
    request: &ConnectionRequest,
    deadline: tokio::time::Instant,
) -> Result<Action, String> {
    loop {
        tokio::select! { biased;
            _ = request.cancelled() => return Err(super::CANCELLED.into()),
            _ = tokio::time::sleep_until(deadline) => return Err("准备系统浏览器登录超过本次认证期限".into()),
            changed = actions.changed() => {
                if changed.is_err() { return Err(super::CANCELLED.into()); }
                if let Some(action) = *actions.borrow_and_update() { return Ok(action); }
            }
        }
    }
}
fn recovery_read_notice(
    action: wire::AuthReceiverAction,
    reply: &wire::AuthReceiverControlReply,
) -> Option<String> {
    let error = reply.error.as_ref()?;
    let receiver = reply.receiver.as_ref()?;
    if action != wire::AuthReceiverAction::Inspect
        || error.reason != "official_auth_receiver/read_recovery"
        || receiver.current_config.is_none()
        || receiver.running
        || receiver.can_start
    {
        return None;
    }
    let mut message = format!(
        "当前认证通道无占用，继续本次登录。读取 aTrust 原启动设置失败：{}",
        error
            .operation_detail
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&error.reason)
    );
    if let Some(native) = &error.native {
        message.push_str(&format!("（系统错误码：{}）", native.code));
    }
    Some(message)
}
fn receiver_error_message(error: &wire::VpnError, action: wire::AuthReceiverAction) -> String {
    let step = error.reason.rsplit('/').next().unwrap_or("");
    let operation = match step {
        "read_recovery" | "recovery_identity" => "读取 aTrust 原启动设置失败",
        "save_recovery" => "保存 aTrust 原启动设置失败，停用操作已中止",
        "recovery_directory" | "recovery_lock" => "准备 aTrust 启动设置恢复记录失败",
        "disable" => "禁用 aTrust 启动项失败",
        "bootout" | "wait_stopped" | "cleanup" => "停止 aTrust 认证接收端失败",
        "restore" | "remove_recovery" => "恢复 aTrust 原启动设置失败",
        _ => match action {
            wire::AuthReceiverAction::DisableAndStop => "停用 aTrust 认证接收端失败",
            wire::AuthReceiverAction::Restore => "恢复 aTrust 原启动设置失败",
            _ => "检查 aTrust 认证接收端失败",
        },
    };
    format!(
        "{operation}：{}。请重试或使用其他认证通道。",
        error
            .operation_detail
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&error.reason)
    )
}
pub(super) async fn prepare(
    app: &tauri::AppHandle,
    request: &ConnectionRequest,
    deadline: tokio::time::Instant,
    initial_error: Option<native_agent::AgentStartFailure>,
) -> Result<native_agent::PreparedAgentTls, String> {
    let state = app.state::<CoreState>();
    let generation = request.generation();
    let deadline_ns = request.auth_control_deadline()?;
    let mut actions = state
        .connection_requests
        .auth_preparation_actions(generation)?;
    actions.borrow_and_update();
    let saved = saved_login_available();
    let mut operation_id = vec![0; 16];
    getrandom::fill(&mut operation_id)
        .map_err(|error| format!("创建本次认证操作身份失败：{error}"))?;
    use crate::kernel::connection_request::AuthPreparationPhase;
    state.connection_requests.set_auth_preparation(
        generation,
        AuthPreparationPhase::Exclusive,
        true,
    );
    let notice = std::sync::Mutex::new(String::new());
    let mut scope = PreparationScope::new(|| {
        let message = notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        emit(app, generation, "closed", false, &message, saved, None);
    });
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
        next_action(&mut actions, request, deadline).await?
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
            next = next_action(&mut actions, request, deadline).await?;
        }
        match next {
            Action::Cancel => return Err(super::CANCELLED.into()),
            Action::UseEmbedded => {
                scope.close();
                return Err(super::USE_EMBEDDED.into());
            }
            Action::TrySavedLogin if saved => {
                scope.close();
                return Err(super::TRY_SAVED_LOGIN.into());
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
        eprintln!("aTrust agent.receiver_control owner={generation} action={action:?}");
        let reply = tokio::select! { biased;
            _ = request.cancelled() => return Err(super::CANCELLED.into()),
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
            Ok(Err(crate::kernel::error::AppError::ServiceConnectFailed(message)))
                if message.starts_with(crate::kernel::client::AUTH_RECEIVER_UPDATE_REQUIRED) =>
            {
                return Err(message);
            }
            Ok(Err(error)) => {
                last_reply = None;
                pending = Some(("exclusive", format!("检查本机认证接收端失败：{error}")));
                continue;
            }
            Err(_) => return Err("本机认证接收端准备超过本次认证期限".into()),
        };
        let warning = recovery_read_notice(action, &reply);
        let failure = reply
            .error
            .as_ref()
            .map(|error| receiver_error_message(error, action));
        let occupied = reply
            .receiver
            .as_ref()
            .is_some_and(|r| r.running || r.can_start);
        let absent = reply.receiver.is_none();
        last_reply = Some(reply);
        if let Some(message) = failure.filter(|_| warning.is_none()) {
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
        request
            .begin_submit()
            .map_err(|_| super::CANCELLED.to_owned())?;
        match native_agent::inspect_agent_certificate() {
            Ok(native_agent::CertificatePreparation::ApprovalRequired { .. }) if !approve => {
                pending = Some(("certificate_approval", String::new()));
                continue;
            }
            Err(error) => {
                pending = Some(("certificate", format!("检查本地登录证书失败：{error}")));
                continue;
            }
            _ => {}
        }
        match native_agent::prepare_agent_certificate(approve) {
            Ok(prepared) => {
                *notice
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    warning.unwrap_or_default();
                scope.close();
                return Ok(prepared);
            }
            Err(error) => {
                pending = Some(("certificate", format!("准备本地登录证书失败：{error}")));
            }
        }
    }
}
