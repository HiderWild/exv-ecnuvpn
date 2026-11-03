//! 应用启动只登记空状态；系统浏览器认证独占本轮Agent组与等待通道。
use exv_vpn_darwin_core::local_agent_host::{NativeAgentHandle, TicketWait};
use std::sync::{Arc, Mutex};
use tauri::Manager as _;

pub(crate) use exv_vpn_darwin_core::agent_certs::{
    AgentCertError as CertificateError, CertificatePreparation, PreparedAgentTls,
};

/// 只读检查本次登录的证书准备状态，保留 EXV_AGENT_CERT_DIR 覆盖。
pub(crate) fn inspect_agent_certificate() -> Result<CertificatePreparation, CertificateError> {
    let dir = certificate_directory()?;
    exv_vpn_darwin_core::agent_certs::inspect_agent_certificate(&dir)
}

/// 当前连接明确确认后才安装用户根信任。
pub(crate) fn prepare_agent_certificate(
    approved: bool,
) -> Result<PreparedAgentTls, CertificateError> {
    let dir = certificate_directory()?;
    exv_vpn_darwin_core::agent_certs::prepare_agent_certificate(&dir, approved)
}

fn certificate_directory() -> Result<std::path::PathBuf, CertificateError> {
    exv_vpn_darwin_core::agent_certs::default_cert_dir().ok_or_else(|| {
        CertificateError::Io("无法定位证书目录（HOME 与 EXV_AGENT_CERT_DIR 均未设置）".into())
    })
}

#[derive(Default)]
pub(crate) struct NativeAgentState {
    handle: Mutex<Option<Arc<NativeAgentHandle>>>,
    exit_deadline: Arc<Mutex<Option<std::time::Instant>>>,
}
impl NativeAgentState {
    async fn start_request(
        &self,
        start: impl std::future::Future<Output = Result<NativeAgentHandle, AgentStartFailure>>,
    ) -> Result<Arc<NativeAgentHandle>, AgentStartFailure> {
        let previous = self.handle.lock().expect("native agent state").clone();
        if let Some(previous) = previous {
            let deadline = self
                .exit_deadline
                .lock()
                .expect("agent exit deadline")
                .unwrap_or_else(|| std::time::Instant::now() + std::time::Duration::from_secs(5));
            // 旧轮结果已由关闭worker记录；返回时其全部任务已join，再创建新轮。
            let _ = previous.shutdown_until(deadline).await;
        }
        if self
            .exit_deadline
            .lock()
            .expect("agent exit deadline")
            .is_some()
        {
            return Err(AgentStartFailure {
                message: "应用正在退出，本次登录已取消".into(),
                native_code: None,
            });
        }
        let host = Arc::new(start.await?);
        *self.handle.lock().expect("native agent state") = Some(host.clone());
        Ok(host)
    }

    pub(crate) fn status(&self) -> NativeAgentStatus {
        let online = self
            .handle
            .lock()
            .expect("native agent state")
            .as_ref()
            .is_some_and(|h| h.is_running());
        NativeAgentStatus {
            online,
            message: String::new(),
            phase: if online { "active" } else { "idle" },
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct NativeAgentStatus {
    pub(crate) online: bool,
    pub(crate) message: String,
    pub(crate) phase: &'static str,
}

pub(crate) struct SystemAuthSession {
    exit_deadline: Arc<Mutex<Option<std::time::Instant>>>,
    host: Arc<NativeAgentHandle>,
    pub(crate) wait: TicketWait,
    generation: u64,
}
impl SystemAuthSession {
    pub(crate) async fn finish_result<T>(
        &self,
        result: Result<T, String>,
        deadline: std::time::Instant,
    ) -> Result<T, String> {
        match (result, self.finish(deadline).await) {
            (result, Ok(())) => result,
            (Ok(_), Err(error)) => Err(format!("结束本次本地登录服务失败：{error}")),
            (Err(original), Err(error)) => {
                Err(format!("{original}；结束本次本地登录服务失败：{error}"))
            }
        }
    }
    pub(crate) async fn finish(&self, deadline: std::time::Instant) -> Result<(), String> {
        let deadline = self
            .exit_deadline
            .lock()
            .expect("agent exit deadline")
            .map_or(deadline, |outer| outer.min(deadline));
        let result = self
            .host
            .shutdown_until(deadline)
            .await
            .map_err(|e| e.to_string());
        eprintln!(
            "aTrust agent.finish owner={} success={}",
            self.generation,
            result.is_ok()
        );
        result
    }
}
impl Drop for SystemAuthSession {
    fn drop(&mut self) {
        let host = self.host.clone();
        let deadline = self
            .exit_deadline
            .lock()
            .expect("agent exit deadline")
            .unwrap_or_else(|| std::time::Instant::now() + std::time::Duration::from_secs(5));
        tauri::async_runtime::spawn(async move {
            let _ = host.shutdown_until(deadline).await;
        });
    }
}

#[derive(Debug)]
pub(crate) struct AgentStartFailure {
    pub(crate) message: String,
    pub(crate) native_code: Option<i64>,
}

pub(crate) async fn prepare_system_auth(
    app: &tauri::AppHandle,
    generation: u64,
    gateway: &str,
    user_agent: &str,
    prepared: PreparedAgentTls,
) -> Result<SystemAuthSession, AgentStartFailure> {
    let state = app.state::<NativeAgentState>();
    if state
        .exit_deadline
        .lock()
        .expect("agent exit deadline")
        .is_some()
    {
        return Err(AgentStartFailure {
            message: "应用正在退出，本次登录已取消".into(),
            native_code: None,
        });
    }
    let callback_app = app.clone();
    let callback = Arc::new(move |_gate| {
        if let Some(state) = callback_app.try_state::<super::client::CoreState>() {
            let _ = state.connection_requests.respond_auth_preparation(
                generation,
                super::connection_request::AuthPreparationAction::TrySavedLogin,
            );
        }
    });
    let host = state.start_request(async {
        exv_vpn_darwin_core::local_agent_host::start_prepared(prepared, gateway, callback)
            .await
            .map_err(|error| {
                let native_code = match &error {
                    exv_vpn_darwin_core::local_agent_host::NativeAgentStartError::Server(error) => error.bind_error().and_then(std::io::Error::raw_os_error).map(i64::from),
                    _ => None,
                };
                eprintln!("aTrust agent.start_failed owner={generation} native_code={native_code:?} error={error}");
                AgentStartFailure { message: format!("本地登录服务启动失败：{error}"), native_code }
            })
    }).await?;
    let wait = host.register_ticket_wait(gateway.to_owned(), user_agent.to_owned());
    let exit_deadline = *state.exit_deadline.lock().expect("agent exit deadline");
    if let Some(deadline) = exit_deadline {
        let _ = host.shutdown_until(deadline).await;
        return Err(AgentStartFailure {
            message: "应用正在退出，本次登录已取消".into(),
            native_code: None,
        });
    }
    eprintln!("aTrust agent.started owner={generation}");
    Ok(SystemAuthSession {
        exit_deadline: state.exit_deadline.clone(),
        host,
        wait,
        generation,
    })
}

pub(crate) async fn stop_until(app: &tauri::AppHandle, deadline: std::time::Instant) {
    let host = app.try_state::<NativeAgentState>().and_then(|state| {
        let mut outer = state.exit_deadline.lock().ok()?;
        *outer = Some(outer.map_or(deadline, |existing| existing.min(deadline)));
        state.handle.lock().ok()?.clone()
    });
    if let Some(host) = host {
        let _ = host.shutdown_until(deadline).await;
    }
}

#[tauri::command]
pub(crate) fn native_agent_status(state: tauri::State<'_, NativeAgentState>) -> NativeAgentStatus {
    state.status()
}
