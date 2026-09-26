//! Event 名称与 emit 辅助 + 真实订阅 task（win32 `kernel/events.rs` 的 darwin 移植）。
//!
//! 范式同构：Event ↔ server-streaming。
//!   * `exv://status` ← core WatchEvents（RuntimeEvent，实时快照/过渡）
//!   * `exv://logs`   ← wire 缺口（core 未向 UI 暴露 StreamLogs），保留 emit seam，
//!     无真实推送源（与 win32 同款；前端日志页以 `logs_list` 轮询追尾）。
//!
//! 订阅循环与 win32 完全同形：逐事件 `wire::event_from_wire` 映射 → `emit_status`
//! → 记录本地已见最大 tick → 流 EOF 后以该 tick 恢复订阅 + 指数退避，无限重试
//!（W3-1/P1 S4：core 已支持 resume 重放与多订阅，摘除旧「重开必然失败→整会话
//! 替换」兜底；core 死亡的正路恢复是连接时的
//! [`super::bootstrap::recover_stopped_core_for_connect`]）。

use tauri::{AppHandle, Emitter, Manager};
use tonic::Streaming;
use tonic::transport::Channel;

use exv_vpn_wire::generated::RuntimeEvent as WireRuntimeEvent;

use super::client::{CoreClient, CoreState};
use super::state::{RuntimeEvent, RuntimeState};
use super::wire;

/// WatchEvents 订阅的 Event 名。
pub(crate) const EVENT_STATUS: &str = "exv://status";

/// 向主窗口广播一次运行时事件。
pub(crate) fn emit_status(app: &AppHandle, event: &RuntimeEvent) -> tauri::Result<()> {
    app.emit(EVENT_STATUS, event)
}

/// tray 状态行消费的粗粒度连接状态字符串（与 `exv://status` 同源）。
fn tray_state_of(event: &RuntimeEvent) -> &'static str {
    match event.snapshot.runtime {
        RuntimeState::Idle { .. } => "idle",
        RuntimeState::Connecting { .. } | RuntimeState::AwaitingInteraction { .. } => "connecting",
        RuntimeState::Connected { .. } => "connected",
        RuntimeState::Stopping { .. } => "stopping",
        RuntimeState::Reconciling { .. }
        | RuntimeState::FailedClean { .. }
        | RuntimeState::FailedDirty { .. } => "failed",
    }
}

/// 启动 core 事件订阅（win32 `spawn_subscriptions` 的 darwin 形态）。
///
/// `initial_stream`：会话建立时 bootstrap 拨号已打开的 WatchEvents 流（首段直接
/// 消费，不重复订阅）。此后循环与 win32 一致：流 EOF → 以本地已见最大 tick
/// `open_watch_stream(resume_tick)` 重开（core `EventBus::subscribe` 按 resume
/// 重放当前快照——UI 内重连断档由重放补齐）→ 失败/EOF 指数退避，无限重试。
pub(crate) fn spawn_status_forwarder(
    app: AppHandle,
    channel: Channel,
    initial_stream: Streaming<WireRuntimeEvent>,
) {
    tauri::async_runtime::spawn(async move {
        // 首段直接消费会话建立时 core 已授权的流；此后每轮按 resume tick 重开。
        let mut stream = Some(initial_stream);
        let mut resume_tick = 0u64;
        let mut backoff = WATCH_RECONNECT_BASE;
        loop {
            if stream.is_none() {
                match CoreClient.open_watch_stream(&channel, resume_tick).await {
                    Ok(reopened) => {
                        backoff = WATCH_RECONNECT_BASE;
                        stream = Some(reopened);
                    }
                    Err(error) => {
                        // win32 同款：订阅失败退避后重试（通道死时由连接恢复正路
                        // `recover_stopped_core_for_connect` 重建会话）。
                        eprintln!("exv.events: watch re-subscribe failed: {error}; retrying");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(WATCH_RECONNECT_MAX);
                        continue;
                    }
                }
            }
            let mut active = stream.take().expect("watch stream present");
            let mut last_tick = resume_tick;
            loop {
                let wire_ev = match active.message().await {
                    Ok(Some(wire_ev)) => wire_ev,
                    // 流 EOF（core 断开）或流错误 → 断流，走恢复语义。
                    Ok(None) => break,
                    Err(status) => {
                        eprintln!("exv.events: watch_events stream error: {status}");
                        break;
                    }
                };
                // 记录本地已见最大 tick，供断线后 resume（core 断线重放语义）。
                last_tick = last_tick.max(wire_ev.monotonic_tick);
                let ui = wire::event_from_wire(&wire_ev);
                // 快照缓存 + 托盘状态行与前端 `exv://status` 同源同步
                //（托盘未安装时 tray 侧静默丢弃）。
                if let Some(state) = app.try_state::<CoreState>() {
                    state.update_snapshot(ui.snapshot.clone());
                }
                crate::tray::update_connection_state(tray_state_of(&ui));
                if let Err(error) = emit_status(&app, &ui) {
                    eprintln!("exv.events: emit_status failed: {error}");
                }
            }
            resume_tick = last_tick;
            // win32 逻辑：断流后指数退避，再在下一轮循环顶部尝试 resume 重开。
            eprintln!(
                "exv.events: watch_events stream ended; reconnecting from tick {resume_tick}"
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(WATCH_RECONNECT_MAX);
        }
    });
}

/// WatchEvents 重连初始退避（win32 同款）。
const WATCH_RECONNECT_BASE: std::time::Duration = std::time::Duration::from_millis(200);
/// WatchEvents 重连退避上限（win32 同款）。
const WATCH_RECONNECT_MAX: std::time::Duration = std::time::Duration::from_secs(5);
