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
) {
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
    submit(
        app,
        LogEvent {
            code: contract::FAILURE.into(),
            fields,
            ..Default::default()
        },
    );
}

pub(crate) fn failure_sink(
    app: &AppHandle,
    generation: u64,
) -> Arc<dyn Fn(super::browser_auth::AuthFailureDiagnostic) + Send + Sync> {
    let app = app.clone();
    Arc::new(move |event| {
        failure(
            &app,
            generation,
            event.round,
            event.kind,
            event.operation,
            event.school_hint.as_deref(),
            event.gateway_code,
            event.detail.as_deref(),
        )
    })
}
