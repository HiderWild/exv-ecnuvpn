//! core 启动接线：Tauri setup 时 spawn core → 拨号 → 管理状态 → 挂订阅。
//!
//! 流程（O3 强绑定：UI 宿主管理 core 生命周期）：
//!   1. 定位 core 二进制（[`super::core_process::core_bin_path`]）并 spawn
//!      （[`super::core_process::spawn_core`]）——core 是普通用户 token 的纯协调层，
//!      非特权直接拉起（host main 已是真实入口，本模块在 Tauri 壳内托管其进程）；
//!   2. 拨号 core 控制面 Named Pipe（[`super::client::dial_core`]，按 UI 侧已知的
//!      core pid + 当前用户 SID 验证 server）；
//!   3. 组装 [`CoreState`]（Dialed/NotWired handle + 快照缓存）与 [`CoreSession`]
//!      （core 子进程句柄 + 订阅 task 句柄）并 `app.manage`；
//!   4. 拨号成功 → 拉起事件订阅（[`super::events::spawn_subscriptions`]）。
//!
//! **防御降级**：core 二进制缺失或拨号失败（例如开发壳未随附 core、启动竞态）→
//! 记录 warning，保留 `NotWired` handle，UI 照常打开（命令返回 `CoreUnreachable`，
//! 前端展示 "core 未连接"；用户业务动作时经 [`ensure_core_available`] 唤醒恢复）
//! ——不把启动失败升级为进程崩溃。
//!
//! ## T9（§2.9）未连接态 core 崩溃响应
//!
//! - **死亡判定（保守）**：[`assess_core_death`]——控制管道 RPC 不可达 **且** UI 托管
//!   子进程确认退出（OS 级 `try_wait` 信号）才判死；单次 RPC 失败/超时（core 忙/
//!   抖动/挂死）一律归 [`CoreDeathVerdict::Unresponsive`]，不判死、不重拉。
//! - **弱提示源**：WatchEvents 订阅流断开（EOF/拨号失败）→ [`handle_core_disconnect`]
//!   保守复核，确认死亡才广播 `exv://core-lifecycle`（crashed）事件。
//! - **业务唤醒**：[`ensure_core_available`]——确认死亡后重拉（参数沿用
//!   [`super::core_process::core_args_for_spawn`] 既有契约），有界等待就绪并原子替换
//!   会话；失败返回 typed `AppError::CoreWakeFailed`（前端弹「尝试重启程序 / 仅退出」
//!   模态）。日志走 `exv.core_recovery` target 的稳定码。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};

use super::client::{dial_core, CoreHandle, CoreSession, CoreState, CoreStatus};
use super::core_process::{core_bin_path, spawn_core};
use super::core_transport::current_user_sid;
use super::error::AppError;
use super::events::{
    emit_core_lifecycle, spawn_subscriptions, CoreLifecycleEvent, CoreLifecycleState,
};

/// 执行 core 启动接线（幂等性由调用方保证——setup 只调一次）。
///
/// # Errors
/// 仅 propagate tauri 管理错误（不可恢复）；core 缺失/拨号失败走降级路径不报错。
pub fn bootstrap(app: &mut tauri::App) -> tauri::Result<()> {
    // 1. spawn core（best effort）。
    let (child, core_pid) = match core_bin_path().and_then(|p| spawn_core(&p).ok()) {
        Some(child) => {
            let pid = child.pid();
            tracing::info!(
                target: "exv.bootstrap",
                pid,
                "core process spawned"
            );
            (Some(child), pid)
        }
        None => {
            tracing::warn!(
                target: "exv.bootstrap",
                "core binary unavailable; running degraded with NotWired handle (connect will attempt recovery)"
            );
            (None, 0)
        }
    };

    // 2. 拨号 core 控制面管道（best effort；身份验证 fail closed）。
    let handle = match core_pid {
        0 => CoreHandle::NotWired,
        pid => {
            // setup 阶段 tauri 的 tokio runtime 已就绪：阻塞等待拨号完成。
            match tauri::async_runtime::block_on(dial_spawned_core(pid)) {
                Ok((channel, _)) => {
                    tracing::info!(
                        target: "exv.bootstrap",
                        core_pid = pid,
                        "dialed core KernelControl endpoint"
                    );
                    CoreHandle::Dialed { channel }
                }
                Err(e) => {
                    tracing::warn!(target: "exv.bootstrap", "dial core failed (degraded): {e}");
                    CoreHandle::NotWired
                }
            }
        }
    };

    // 3. 管理状态（setup 阶段 manage 合法；命令在 setup 完成后才可 invoke）。
    // `Manager` trait 在 `AppHandle` 上实现（`&mut App` 无 manage/state）。
    let channel = handle.channel().cloned();
    let handle_ref = app.handle();
    handle_ref.manage(CoreState {
        handle: std::sync::RwLock::new(handle),
        last_snapshot: Default::default(),
        last_stats: Default::default(),
        browser_login: Default::default(),
        browser_commit_gate: Default::default(),
    });
    handle_ref.manage(CoreSession {
        child: std::sync::Mutex::new(child),
        subscriptions: std::sync::Mutex::new(Vec::new()),
        recovery: tokio::sync::Mutex::new(()),
        crash_announced: std::sync::atomic::AtomicBool::new(false),
    });

    // 4. 拨号成功 → 挂事件订阅（emit 到前端；快照缓存由 snapshot 命令维护）。
    if let Some(channel) = channel {
        let app_handle = handle_ref.clone();
        let handles = spawn_subscriptions(app_handle, channel);
        handle_ref
            .state::<CoreSession>()
            .subscriptions
            .lock()
            .expect("subscriptions lock")
            .extend(handles);
        tracing::info!(target: "exv.bootstrap", "event subscriptions started");
    }

    Ok(())
}

/// UI 侧 core 死亡判定的三态结论（T9 §2.9；保守准则见 [`assess_core_death`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreDeathVerdict {
    /// 控制管道 RPC 可通——core 存活。
    Normal,
    /// 管道不可达 **且** UI 托管子进程已确认退出（OS 级退出信号）——判定死亡。
    /// `core_pid = 0` 表示 UI 从未托管子进程（bootstrap 降级；唤醒路径允许直接拉起）。
    Dead { core_pid: u32 },
    /// 管道不可达但子进程仍在运行——core 挂死/忙/抖动，**不判死**（不自动重启）。
    Unresponsive,
}

/// 死亡判定的有界探测时限：core 挂死时 RPC 可能永远不回，超时按不可达处理
/// （不判死的子进程校验兜底，不会把挂死误判为存活之外的任何动作）。
const CORE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// 保守死亡判定：管道不可达 **且** 子进程退出信号确认才判死。
///
/// 依据（§2.9）：单次管道失败/超时可能是 core 忙或传输抖动——只有 OS 级
/// `try_wait` 返回退出（或从未托管）才允许判死并重拉；子进程查询句柄失败时
/// 保守归 [`CoreDeathVerdict::Unresponsive`]（与 [`CoreChild::has_exited`] 的
/// fail-closed 语义一致）。
pub async fn assess_core_death(state: &CoreState, session: &CoreSession) -> CoreDeathVerdict {
    // 日常探测只依据已验证管道上的轻量 snapshot RPC；有界等待防挂死 core 卡死判定。
    let reachable = match tokio::time::timeout(CORE_PROBE_TIMEOUT, state.probe_status()).await {
        Ok(status) => status == CoreStatus::Normal,
        Err(_) => {
            tracing::warn!(target: "exv.core_recovery", "core_death_assess.probe_timeout");
            false
        }
    };
    if reachable {
        return CoreDeathVerdict::Normal;
    }
    // 这是唯一允许的“进程是否已退出”检查：依赖 UI 自己 spawn 后保存的 Child 句柄，
    // 不扫描系统进程表，也不会误把同名外部进程视为本 core。
    let exited_pid = session
        .child
        .lock()
        .map(|mut child| match child.as_mut() {
            None => Some(0),
            Some(core) => core.has_exited().then_some(core.pid()),
        })
        .unwrap_or(None);
    match exited_pid {
        Some(core_pid) => CoreDeathVerdict::Dead { core_pid },
        None => CoreDeathVerdict::Unresponsive,
    }
}

/// 崩溃广播的边沿触发：首次进入崩溃态返回 `Some`（调用方应 emit），重复返回 `None`。
pub(super) fn crash_event_if_newly_dead(
    announced: &AtomicBool,
    core_pid: u32,
) -> Option<CoreLifecycleEvent> {
    if announced.swap(true, Ordering::AcqRel) {
        return None;
    }
    Some(CoreLifecycleEvent {
        state: CoreLifecycleState::Crashed,
        core_pid,
    })
}

/// 恢复广播：此前广播过崩溃才返回 `Some`（调用方应 emit recovered），并复位标志。
pub(super) fn recovered_event_if_previously_crashed(
    announced: &AtomicBool,
    core_pid: u32,
) -> Option<CoreLifecycleEvent> {
    if !announced.swap(false, Ordering::AcqRel) {
        return None;
    }
    Some(CoreLifecycleEvent {
        state: CoreLifecycleState::Recovered,
        core_pid,
    })
}

/// WatchEvents 流断开后的保守死亡复核（T9 弱提示源）。
///
/// 由订阅循环在流 EOF / 重订阅失败时调用；与唤醒路径共用 `recovery` 串行闸门，
/// 消除「判死-广播崩溃」与「重拉-广播恢复」之间的竞态窗口。
pub async fn handle_core_disconnect(app: &AppHandle) {
    let (Some(state), Some(session)) = (app.try_state::<CoreState>(), app.try_state::<CoreSession>())
    else {
        return;
    };
    let _gate = session.recovery.lock().await;
    if let CoreDeathVerdict::Dead { core_pid } = assess_core_death(&state, &session).await {
        if let Some(event) = crash_event_if_newly_dead(&session.crash_announced, core_pid) {
            tracing::warn!(target: "exv.core_recovery", core_pid, "core_crash_detected");
            if let Err(error) = emit_core_lifecycle(app, &event) {
                tracing::warn!(target: "exv.core_recovery", "core crash event emit failed: {error}");
            }
        }
    }
}

/// 业务唤醒（T9 §2.9）的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreWake {
    /// core 本就可用（管道复通；若此前广播过崩溃则顺带广播恢复）。
    AlreadyAvailable,
    /// 确认死亡后已重拉并就绪。
    Restarted { core_pid: u32 },
}

/// 业务唤醒入口（连接 / 保存设置 / 快速入门 / 弱提示「重启 core」按钮共用）。
///
/// 语义：core 活着直接放行（零额外成本）；管道不可达时按 [`assess_core_death`]
/// 保守判定——确认死亡才静默重拉（参数沿用既有 spawn 契约）、有界等待就绪并原子
/// 替换会话（handle + 订阅 + 子进程句柄）；唤醒失败返回 typed
/// [`AppError::CoreWakeFailed`]（前端弹强报错模态），core 挂死（进程在、管道不通）
/// 不强杀、同样归唤醒失败。并发调用经 `recovery` 闸门串行。
///
/// # Errors
/// 唤醒失败 → `AppError::CoreWakeFailed`（重拉失败 / 子进程仍在但管道不可达）。
pub async fn ensure_core_available(
    app: &AppHandle,
    state: &CoreState,
    session: &CoreSession,
) -> Result<CoreWake, AppError> {
    let _gate = session.recovery.lock().await;

    match assess_core_death(state, session).await {
        CoreDeathVerdict::Normal => {
            // 管道复通（例如并发恢复已完成 / 外部拉起的新 core 已接管同名管道）。
            let current_pid = session
                .child
                .lock()
                .ok()
                .and_then(|child| child.as_ref().map(super::core_process::CoreChild::pid))
                .unwrap_or(0);
            announce_recovery_if_previously_crashed(app, session, current_pid);
            tracing::info!(target: "exv.core_recovery", "core_wake.already_available");
            Ok(CoreWake::AlreadyAvailable)
        }
        CoreDeathVerdict::Dead { core_pid } => {
            tracing::warn!(target: "exv.core_recovery", dead_pid = core_pid, "core_wake.restart_started");
            let executable = core_bin_path().ok_or_else(|| {
                tracing::warn!(target: "exv.core_recovery", "core_wake.restart_failed: executable unavailable");
                state.mark_stopped();
                AppError::CoreWakeFailed(
                    "core wake: core executable is unavailable for recovery".to_string(),
                )
            })?;
            let mut replacement = spawn_core(&executable).map_err(|error| {
                tracing::warn!(target: "exv.core_recovery", "core_wake.restart_failed: spawn failed");
                state.mark_stopped();
                AppError::CoreWakeFailed(format!("core wake: restart core failed: {error}"))
            })?;
            let pid = replacement.pid();
            // core 刚 spawn 时命名管道尚在创建。有限重试只等待本次受控重拉的就绪。
            match dial_spawned_core(pid).await {
                Ok((channel, _)) => {
                    state.replace_handle(CoreHandle::Dialed {
                        channel: channel.clone(),
                    });
                    replace_subscriptions(app, session, channel);
                    if let Ok(mut child) = session.child.lock() {
                        *child = Some(replacement);
                    }
                    announce_recovery_if_previously_crashed(app, session, pid);
                    tracing::info!(target: "exv.core_recovery", core_pid = pid, "core_wake.restart_succeeded");
                    Ok(CoreWake::Restarted { core_pid: pid })
                }
                Err(error) => {
                    replacement.terminate();
                    state.mark_stopped();
                    tracing::warn!(target: "exv.core_recovery", core_pid = pid, "core_wake.restart_failed: dial failed");
                    Err(AppError::CoreWakeFailed(format!(
                        "core wake: restarted core did not become ready: {error}"
                    )))
                }
            }
        }
        CoreDeathVerdict::Unresponsive => {
            // core 挂死（进程在、管道不通）：不强杀（kill 会打断有序停机），交由
            // 唤醒失败模态的「尝试重启程序」走 O3 整程重启兜底。
            tracing::warn!(target: "exv.core_recovery", "core_wake.refused_core_alive");
            Err(AppError::CoreWakeFailed(
                "core wake: core process is still running but its control pipe is unreachable"
                    .to_string(),
            ))
        }
    }
}

/// 唤醒成功后若此前广播过崩溃，广播一次恢复（撤下前端弱提示横幅）。
fn announce_recovery_if_previously_crashed(
    app: &AppHandle,
    session: &CoreSession,
    core_pid: u32,
) {
    if let Some(event) = recovered_event_if_previously_crashed(&session.crash_announced, core_pid) {
        tracing::info!(target: "exv.core_recovery", core_pid, "core_recovery_announced");
        if let Err(error) = emit_core_lifecycle(app, &event) {
            tracing::warn!(target: "exv.core_recovery", "core recovery event emit failed: {error}");
        }
    }
}

/// 受 UI 管理的新 Core 的有限就绪等待。只在 UI 初启和已确认退出后的连接恢复中调用；
/// 不属于日常健康检查，也不构成后台重启循环。
async fn dial_spawned_core(pid: u32) -> Result<(tonic::transport::Channel, super::core_transport::CorePeer), AppError> {
    let pipe = super::core_process::core_control_pipe_name();
    let sid = current_user_sid().unwrap_or_default();
    let mut last_error = None;
    for _ in 0..25 {
        match dial_core(&pipe, pid, &sid).await {
            Ok(dialed) => return Ok(dialed),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AppError::CoreUnreachable("core did not expose its control pipe before readiness timeout".to_string())
    }))
}

/// 新 Core 认证成功后更换 status 订阅，避免旧管道的退避任务继续占用 UI。
fn replace_subscriptions(app: &AppHandle, session: &CoreSession, channel: tonic::transport::Channel) {
    if let Ok(mut subscriptions) = session.subscriptions.lock() {
        for subscription in subscriptions.drain(..) {
            subscription.abort();
        }
        subscriptions.extend(spawn_subscriptions(app.clone(), channel));
    }
}
