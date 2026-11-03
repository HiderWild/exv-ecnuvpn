//! Event 名称与 emit 辅助 + 真实订阅 task（P4-b）。
//!
//! 范式同构（计划 §1）：Event ↔ server-streaming。
//!   * `exv://status`  ← core WatchEvents（RuntimeEvent，实时快照/过渡）
//!   * `exv://logs`    ← core StreamLogs（LogEvent 增量）——**wire 缺口**：core 的
//!     `KernelControl` 服务未向 UI 暴露 StreamLogs（host `log_control.rs` 为进程内
//!     方法，proto 变更须协调者评估），P4-b 保留 emit seam，无真实推送源。
//!   * `exv://interaction` ← 需要用户在 UI 中应答的交互提示（RespondInteraction）
//!
//! [`spawn_subscriptions`] 在 core 拨号成功后由 [`super::bootstrap`] 调用：为每个
//! 订阅源拉起 tokio task，`stream.next()` → `app.emit`。返回句柄供停机时中止。
//! 快照缓存由 `snapshot` 命令维护（事件本身携带快照，前端可即时渲染）。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_stream::StreamExt;
use tonic::transport::Channel;

use super::client::CoreClient;
use super::state::RuntimeEvent;
use super::wire;

/// WatchEvents 订阅的 Event 名。
pub const EVENT_STATUS: &str = "exv://status";

/// core 生命周期事件名（T9 §2.9：core 崩溃弱提示 / 恢复广播）。
pub const EVENT_CORE_LIFECYCLE: &str = "exv://core-lifecycle";

/// core 生命周期事件的结论（serde snake_case：`crashed` / `recovered`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreLifecycleState {
    /// core 已确认死亡（保守判定：管道断 + 子进程退出信号）。
    Crashed,
    /// core 已恢复可用（重拉成功 / 管道复通）。
    Recovered,
}

/// `exv://core-lifecycle` 事件 payload。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CoreLifecycleEvent {
    pub state: CoreLifecycleState,
    /// 涉及的 core 进程 pid（`0` = UI 从未托管子进程）。
    pub core_pid: u32,
}

/// 广播一次 core 生命周期事件（前端据此渲染/撤下「core 已崩溃」弱提示横幅）。
pub fn emit_core_lifecycle(app: &AppHandle, event: &CoreLifecycleEvent) -> tauri::Result<()> {
    app.emit(EVENT_CORE_LIFECYCLE, event)
}

/// 向主窗口广播一次运行时事件。
pub fn emit_status(app: &AppHandle, event: &RuntimeEvent) -> tauri::Result<()> {
    app.emit(EVENT_STATUS, event)
}

/// 启动 core 事件订阅（P4-b）：WatchEvents 实时订阅 task。
///
/// 订阅循环：`watch_events(resume=0)` → 逐事件 `wire::event_from_wire` 映射 →
/// `emit_status` → 流 EOF（core 断开）→ 以本地已见最大 tick 恢复订阅（core
/// `EventBus::subscribe` 按 resume_tick 重放当前快照，断线重放语义）→ 退避重连。
///
/// StreamLogs（`exv://logs`）为 wire 缺口（见模块文档），当前不拉起。
pub fn spawn_subscriptions(
    app: AppHandle,
    channel: Channel,
) -> Vec<tauri::async_runtime::JoinHandle<()>> {
    let mut handles = Vec::new();

    // WatchEvents 订阅（经 CoreClient.open_watch_stream 打开流，语义与命令层一致）。
    let app_status = app.clone();
    let channel_status = channel.clone();
    // Tauri setup 闭包运行在主线程（非 tokio runtime 上下文），`tokio::spawn` 会 panic
    // （"no reactor running"）。改用 `tauri::async_runtime::spawn`（tauri 自带 runtime）。
    handles.push(tauri::async_runtime::spawn(async move {
        let mut resume_tick = 0u64;
        let mut backoff = WATCH_RECONNECT_BASE;
        loop {
            let stream = match CoreClient.open_watch_stream(&channel_status, resume_tick).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::warn!(target: "exv.events", "watch_events subscribe failed: {e}; reconnecting");
                    // 订阅拨号失败与流 EOF 同源（core 亡/管道断）→ 保守死亡复核（T9 弱提示源）。
                    super::bootstrap::handle_core_disconnect(&app_status).await;
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(WATCH_RECONNECT_MAX);
                    continue;
                }
            };
            backoff = WATCH_RECONNECT_BASE;
            let mut stream = stream;
            while let Some(ev) = stream.next().await {
                match ev {
                    Ok(wire_ev) => {
                        // 记录本地已见最大 tick，供断线后 resume（core 断线重放语义）。
                        resume_tick = resume_tick.max(wire_ev.monotonic_tick);
                        let requests = app_status.state::<super::client::CoreState>().connection_requests.clone();
                        let _ = dispatch_auth_acquisition(&requests, &channel_status, &wire_ev, {
                            let app = app_status.clone();
                            let requests = requests.clone();
                            move |request, lease, attempt| async move {
                                let profiles = exv_core::browser_cache_profile::BrowserDataDirs::from_frozen_profile(std::path::PathBuf::from(&request.browser_profile), request.browser_profile_explicit);
                                // 冻结目录合同在原解析入口恢复 Default/Explicit，不迁移 Owned 目录。
                                let school = super::client::school_auto_login_saved(&request.gateway, request.auto_login, std::path::Path::new(&request.config_dir));
                                // Core 后台请求只使用静默 HTTP → 隐藏 Owned，不发布前台认证事件。
                                super::client::acquire_browser_once(&requests, app.clone(), request.gateway, "headless".into(), lease, school, true, true, attempt, profiles,
                                    |_, _| {}).await
                            }
                        });
                        let ui = wire::event_from_wire(&wire_ev);
                        if let Err(e) = emit_status(&app_status, &ui) {
                            tracing::debug!(target: "exv.events", "emit_status failed: {e}");
                        }
                    }
                    Err(status) => {
                        tracing::warn!(target: "exv.events", "watch_events stream error: {status}");
                        break;
                    }
                }
            }
            {
                let state = app_status.state::<super::client::CoreState>();
                state.connection_requests.auth_acquisitions.clear(&state.connection_requests);
            }
            // 流 EOF = core 断开（或 stream 错误）→ T9 保守死亡复核（弱提示源）→ 退避重连。
            super::bootstrap::handle_core_disconnect(&app_status).await;
            tracing::warn!(target: "exv.events", "watch_events stream ended; reconnecting from tick {resume_tick}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(WATCH_RECONNECT_MAX);
        }
    }));

    handles
}
/// WatchEvents 重连初始退避。
const WATCH_RECONNECT_BASE: std::time::Duration = std::time::Duration::from_millis(200);
/// WatchEvents 重连退避上限。
const WATCH_RECONNECT_MAX: std::time::Duration = std::time::Duration::from_secs(5);

/// 与真实订阅相同的同步派发入口；获取期间后续事件继续进入原映射。
pub(super) fn dispatch_auth_acquisition<F, Fut>(
    requests: &super::connection_request::ConnectionRequests,
    channel: &Channel,
    event: &exv_vpn_wire::generated::RuntimeEvent,
    acquire: F,
) -> Option<tokio::task::JoinHandle<()>>
where
    F: FnOnce(
            exv_vpn_wire::generated::AtrustAuthAcquisition,
            super::connection_request::ConnectionRequest,
            exv_core::browser_auth::AuthAttempt,
        ) -> Fut
        + Send
        + 'static,
    Fut: std::future::Future<
            Output = (
                Result<
                    exv_vpn_win32_ipc::browser_session::BrowserSession,
                    exv_core::atrust_browser::BrowserAuthError,
                >,
                super::connection_request::ConnectionRequest,
            ),
        > + Send
        + 'static,
{
    let snapshot = event.snapshot.as_ref()?;
    let ticket = requests
        .auth_acquisitions
        .observe(requests, snapshot.atrust_auth_acquisition.as_ref())?;
    let requests = requests.clone();
    let channel = channel.clone();
    Some(tokio::spawn(super::auth_acquisition::consume(
        requests, channel, ticket, acquire,
    )))
}
