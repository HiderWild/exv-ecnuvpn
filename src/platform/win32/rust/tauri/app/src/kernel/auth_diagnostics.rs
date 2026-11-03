//! 认证路径只提交诊断，最多四个后台RPC，各自250ms预算；聚合文件由Core唯一写入。
use super::client::{CoreClient, CoreState};
use exv_vpn_wire::{auth_diagnostic as contract, generated::LogEvent};
use std::{
    future::Future,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::sync::Semaphore;

const SEND_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_IN_FLIGHT: usize = 4;

pub(crate) fn local_error_detail(value: &str) -> Option<String> {
    contract::local_error_detail(value)
}

fn permits() -> Arc<Semaphore> {
    static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(PERMITS.get_or_init(|| Arc::new(Semaphore::new(MAX_IN_FLIGHT))))
}

fn enqueue<F, Fut>(permits: Arc<Semaphore>, event: LogEvent, send: F) -> bool
where
    F: FnOnce(LogEvent) -> Fut + Send + 'static,
    Fut: Future<Output = Result<bool, String>> + Send + 'static,
{
    let event = match contract::normalize(event) {
        Ok(event) => event,
        Err(reason) => {
            eprintln!("auth diagnostic dropped: {reason}");
            return false;
        }
    };
    let Ok(permit) = permits.try_acquire_owned() else {
        eprintln!("auth diagnostic dropped: background sender full");
        return false;
    };
    tokio::spawn(async move {
        let _permit = permit;
        let fallback = event.clone();
        if !matches!(
            tokio::time::timeout(SEND_TIMEOUT, send(event)).await,
            Ok(Ok(true))
        ) {
            // 此载荷已通过同一有限字段校验；旧Core、超时和不可达沿stderr降级。
            eprintln!(
                "auth diagnostic fallback code={} fields={:?}",
                fallback.code, fallback.fields
            );
        }
    });
    true
}

fn submit(app: &AppHandle, event: LogEvent) {
    let app = app.clone();
    enqueue(permits(), event, move |event| async move {
        let state = app
            .try_state::<CoreState>()
            .ok_or_else(|| "auth diagnostic: CoreState absent".to_owned())?;
        CoreClient
            .record_auth_diagnostic(&state, event)
            .await
            .map_err(|e| e.to_string())
    });
}

pub(crate) fn startup(
    app: &AppHandle,
    generation: u64,
    round: u64,
    navigation: u64,
    sample: u64,
    diagnostic: Option<&str>,
) {
    let mut fields = [
        ("generation".into(), generation.to_string()),
        ("round".into(), round.to_string()),
        ("navigation".into(), navigation.to_string()),
        ("sample".into(), sample.to_string()),
    ]
    .into_iter()
    .collect::<std::collections::HashMap<String, String>>();
    if let Some(diagnostic) = diagnostic {
        for field in diagnostic.split_whitespace() {
            if let Some((key, value)) = field.split_once('=') {
                fields.insert(key.into(), value.into());
            }
        }
    } else {
        fields.insert("result".into(), "unavailable".into());
    }
    submit(
        app,
        LogEvent {
            code: contract::STARTUP.into(),
            fields,
            ..Default::default()
        },
    );
}

/// 每轮原始错误直接适配为宿主诊断载荷，终局摘要不经过此入口。
pub(crate) fn round_failure(
    app: &AppHandle,
    generation: u64,
    round: exv_core::browser_auth::AuthRound,
    error: &exv_core::browser_auth::AuthError,
) {
    submit(app, round_failure_event(generation, round, error));
}

fn round_failure_event(
    generation: u64,
    round: exv_core::browser_auth::AuthRound,
    error: &exv_core::browser_auth::AuthError,
) -> LogEvent {
    use exv_core::browser_auth::{AuthErrorCode, AuthOperation, AuthRound};
    let round = match round { AuthRound::First => 1, AuthRound::Second => 2 };
    let kind = match error.failure_kind.map(|kind| format!("{kind:?}")).as_deref() {
        Some("CredentialsRejected") => "credentials_rejected",
        Some("PolicyRestricted") => "policy_restricted",
        Some("GatewayThrottled") => "gateway_throttled",
        Some("StartupFailed") => "browser_startup_failed",
        Some("LocalTimeout") => "browser_auth_stalled",
        Some("TransportFailed") => "browser_auth_transport",
        _ => match error.code {
            AuthErrorCode::AcquisitionTimeout | AuthErrorCode::Stalled => "browser_auth_stalled",
            AuthErrorCode::GatewayThrottled => "gateway_throttled",
            AuthErrorCode::ChannelOpenFailed | AuthErrorCode::ChannelTransportFailed | AuthErrorCode::SessionProbeFailed => "browser_auth_transport",
            _ => "browser_auth_unknown_page",
        },
    };
    let operation = match error.operation {
        AuthOperation::Open => "window_open", AuthOperation::Navigate => "navigation",
        AuthOperation::Observe => "page_observation", AuthOperation::Probe | AuthOperation::Reuse => "session_probe",
        AuthOperation::Inject => "credential_submission", AuthOperation::Extract => "session_extract",
        AuthOperation::Present => "presentation", _ => "authentication",
    };
    // 页面策略只在实际页面失败分类时提供学校摘录；宿主操作详情沿本地净化边界。
    let page_detail = error.operation == AuthOperation::Observe
        && matches!(error.code, AuthErrorCode::FailedPage | AuthErrorCode::GatewayThrottled);
    let detail = error.server_detail.as_deref();
    failure_event(
        generation,
        round,
        kind,
        operation,
        if page_detail { detail } else { None },
        None,
        if page_detail { None } else { detail },
        Some(error.code.machine_code()),
        error.native_code,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn failure(
    app: &AppHandle,
    generation: u64,
    round: u8,
    kind: &str,
    operation: &str,
    school_hint: Option<&str>,
    gateway_code: Option<i32>,
    detail: Option<&str>,
    failure_code: Option<&str>,
    native_code: Option<i64>,
) {
    submit(
        app,
        failure_event(
            generation,
            round,
            kind,
            operation,
            school_hint,
            gateway_code,
            detail,
            failure_code,
            native_code,
        ),
    );
}

#[allow(clippy::too_many_arguments)]
fn failure_event(
    generation: u64,
    round: u8,
    kind: &str,
    operation: &str,
    school_hint: Option<&str>,
    gateway_code: Option<i32>,
    detail: Option<&str>,
    failure_code: Option<&str>,
    native_code: Option<i64>,
) -> LogEvent {
    let mut fields = [
        ("generation".into(), generation.to_string()),
        ("round".into(), round.to_string()),
        ("kind".into(), kind.to_owned()),
        ("operation".into(), operation.to_owned()),
    ]
    .into_iter()
    .collect::<std::collections::HashMap<String, String>>();
    if let Some(hint) = school_hint.and_then(contract::school_error_hint) {
        fields.insert("school_hint".into(), hint);
    }
    if let Some(detail) = detail.and_then(contract::local_error_detail) {
        fields.insert("detail".into(), detail);
    }
    if let Some(code) = gateway_code {
        fields.insert("gateway_code".into(), code.to_string());
    }
    if let Some(code) = failure_code {
        fields.insert("failure_code".into(), code.into());
    }
    if let Some(code) = native_code {
        fields.insert("native_code".into(), code.to_string());
    }
    LogEvent {
        code: contract::FAILURE.into(),
        fields,
        ..Default::default()
    }
}
