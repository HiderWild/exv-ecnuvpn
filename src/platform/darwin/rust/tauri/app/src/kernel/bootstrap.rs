//! core 启动接线（win32 `kernel/bootstrap.rs` 的 darwin 移植）。
//!
//! 流程（Tauri setup 时执行一次）：
//!   1. manage `CoreState`（Dialed/NotWired 通道句柄 + 快照缓存）与 `CoreSession`
//!      （core 进程会话容器 + 恢复闸门）；
//!   2. best-effort 拉起固定 Core（[`super::core_process::start_core_session`]：
//!      spawn → UDS 认证拨号 → 打开唯一 WatchEvents 流并消费首个 Idle 确认就绪）；
//!   3. 成功 → 通道写入 `CoreState`，watch 流交给事件订阅 task
//!      （[`super::events::spawn_status_forwarder`]）；
//!   4. 失败 → 记录并保持 `NotWired`（UI 照常打开，命令返回 `CoreUnreachable`；
//!      用户点连接时经 [`recover_stopped_core_for_connect`] 恢复）。
//!
//! W3-1/P1 S4：core 已支持 WatchEvents resume 重放与多订阅，事件订阅任务自行
//! 无限退避重连——「整会话替换」兜底（`recover_session`/`replace_session`）退役。
//! Core 死亡的正路恢复只保留 [`recover_stopped_core_for_connect`]（用户点连接时
//! 整会话重建：回收旧 core → 重拉 → 重挂订阅）。

use tauri::{AppHandle, Manager};

use super::client::{CoreHandle, CoreSession, CoreState};
use super::core_process;
use super::error::AppError;
use super::events::spawn_status_forwarder;

/// 执行 core 启动接线（幂等性由调用方保证——setup 只调一次）。
///
/// core 缺失/拨号失败走降级路径（保持 NotWired，连接时恢复），不把启动失败升级
/// 为进程崩溃——本函数无失败返回。
pub(crate) fn bootstrap(app: &tauri::App) {
    let handle = app.handle();

    // 1. manage 状态（命令在 setup 完成后才可 invoke；先占位再按拨号结果填充）。
    handle.manage(CoreState::default());
    handle.manage(CoreSession::default());

    // 2. best-effort 拉起固定 Core（无签名 bundle 布局或固定开发期路径；失败降级）。
    match tauri::async_runtime::block_on(core_process::start_core_session()) {
        Ok(started) => install_session(handle, started),
        Err(error) => {
            eprintln!("exv.bootstrap: core session unavailable (degraded, NotWired): {error}");
        }
    }
}

/// 把已就绪的 core 会话接入 managed 状态：通道句柄 + 事件订阅 + 会话容器。
fn install_session(handle: &AppHandle, mut started: core_process::CoreSession) {
    let channel = started.channel();
    // 会话建立路径必然携带唯一 watch 流（`open_initial_watch` 已消费首事件确认就绪）；
    // 缺流视为会话异常，按降级处理。
    let Some(watch) = started.take_watch() else {
        eprintln!("exv.bootstrap: core session without watch stream (degraded)");
        if let Some(state) = handle.try_state::<CoreState>() {
            state.mark_stopped();
        }
        return;
    };
    if let Some(state) = handle.try_state::<CoreState>() {
        state.replace_handle(CoreHandle::Dialed {
            channel: channel.clone(),
        });
    }
    spawn_status_forwarder(handle.clone(), channel, watch);
    if let Some(session) = handle.try_state::<CoreSession>()
        && let Ok(mut inner) = session.inner.try_lock()
    {
        *inner = Some(started);
    }
}

/// 用户点「连接」时的 Core 恢复入口（win32 `recover_stopped_core_for_connect` 的
/// darwin 对应）——core 死亡的正路恢复。
///
/// 日常 UI 只经 [`CoreState::probe_status`] 观察控制管道；不会枚举进程。当前次
/// Connect 已返回 `CoreUnreachable` 才调用本函数：先复核管道（并发的第一个恢复
/// 可能已完成），仍不可用则整会话重建（中止旧订阅 → 回收旧 core → 重拉 → 重挂
/// 新订阅）后重试原请求。恢复闸门串行化并发恢复。
pub(crate) async fn recover_stopped_core_for_connect(
    app: &AppHandle,
    state: &CoreState,
) -> Result<(), AppError> {
    let Some(session) = app.try_state::<CoreSession>() else {
        return Err(AppError::CoreUnreachable(
            "connect: core session state missing".to_string(),
        ));
    };
    let _gate = session.recovery.lock().await;

    // 并发 Connect 的第一个调用可能已经完成恢复；此处只用管道 RPC 复核，不观察进程。
    if state.probe_status().await == super::client::CoreStatus::Normal {
        return Ok(());
    }

    // 旧事件任务持有旧通道克隆，重开必然失败后自行退避；中止句柄加速释放。
    session.abort_subscriptions();

    // 回收旧 core：固定 runtime socket 路径要求旧进程先退出，新会话才能绑定。
    if let Some(previous) = session.inner.lock().await.take()
        && let Err(error) = previous.close_and_reap().await
    {
        eprintln!("exv.bootstrap: previous core reap failed: {error}");
    }

    match core_process::start_core_session().await {
        Ok(started) => {
            install_session(app, started);
            eprintln!("exv.bootstrap: core session rebuilt for connect");
            Ok(())
        }
        Err(error) => {
            state.mark_stopped();
            Err(AppError::CoreUnreachable(format!(
                "core session rebuild failed: {error}"
            )))
        }
    }
}
