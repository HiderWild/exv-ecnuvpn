//! 生命周期：`close_preference` 驱动关闭 + 静默启动 + UI 退出停机（MAC-SHELL-17 S2）。
//!
//! 关闭决策结合 Core 的实时连接状态：连接期间保留后台连接，空闲时退出。
//! `quit` 始终退出，`tray` 始终隐藏；不再通过隐藏时间猜测用户意图。
//!
//! ## UI 退出停机链路（darwin 版 O3；壳不 kill core）
//!
//! 「通知 core 退出」在 darwin 上即**结束 UI 进程**：先做有界 runtime 目录清理
//! （关 stdin → 等 core 有序退出 → `cleanup_empty()`，见 [`notify_core_shutdown`]，
//! L1 修复），再 `app.exit(0)` → 进程 teardown drop 剩余会话（UDS channel、watch
//! 流一并关闭）→ core 的 `kernel_control_service` 经「唯一已认证 UI session 的一次性
//! shutdown 触发器」感知断开 → core 自行有序停机。darwin `CoreChild` 有意不做 Drop
//! 自动 terminate（kill 会打断有序停机）；会话替换（`kernel::bootstrap`）的 kill 仅
//! 用于显式回收旧 session，不在退出路径上。退出前先移除托盘（避免残留幽灵图标）并
//! best-effort 中止状态转发任务（提前释放 watch 流克隆，加速 EOF 感知）。
//!
//! ## 启动可见性
//!
//! `--silent` argv（单次语义）∨ prefs `silent_startup`（持久偏好）→ 主窗口保持隐藏
//! （tauri.conf `visible:false`），仅托盘驻留；否则显示主窗口。两种启动方式（手动 /
//! S4 LaunchAgent）共用同一判定：argv 与偏好文件任一为真即静默。
//!
//! ## 第二实例激活（对齐计划 P8/W2-B，win32 lifecycle.rs 复刻）
//!
//! [`activation_decision`] 纯函数把第二实例 argv（single-instance 插件转发）映射为
//! 「唤出主窗口 / 保持隐藏」；执行点复用 [`show_main_window`]（与托盘 show /
//! Dock Reopen 同一执行点）。
//!
//! ## S3 连接过渡效果的壳侧机械通道
//!
//! 连接建立隐藏（`minimize_to_tray_on_connect`）的决策纯逻辑在前端
//! `lifecycle-effects.ts`（win32 同源迁移）；壳只提供机械 command
//! [`shell_hide_main_window`]（隐藏 + 既有托盘/Dock 唤回接缝），不做决策。

use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{App, AppHandle, Manager};

use crate::kernel::client::{CoreClient, CoreSession, CoreState};
use crate::kernel::state::{RuntimeSnapshot, RuntimeState};
use crate::ui_prefs::{CLOSE_PREFERENCE_VALUES, DEFAULT_CLOSE_PREFERENCE, UiPrefsStore};

/// 关闭决策和完整退出各自只执行一次；驻留后允许下一次关闭。
static CLOSE_REQUEST_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_STARTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_FINISHED: AtomicBool = AtomicBool::new(false);

/// 窗口关闭动作，与显式菜单退出分开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    Quit,
    Hide,
}

/// 实际连接、重连或清理工作进行中时保留后台进程。
fn background_work(snapshot: &RuntimeSnapshot) -> bool {
    needs_background(&snapshot.runtime, snapshot.reconnect.as_ref().is_some_and(|state| state.active))
}

fn needs_background(state: &RuntimeState, reconnecting: bool) -> bool {
    reconnecting || !matches!(state, RuntimeState::Idle { .. } | RuntimeState::FailedClean { .. })
}

/// 状态读取暂不可用时保留窗口对应的后台进程，用户仍可显式退出。
#[must_use]
pub fn close_decision(preference: &str, has_background_work: bool) -> CloseDecision {
    let pref = if CLOSE_PREFERENCE_VALUES.contains(&preference) {
        preference
    } else {
        DEFAULT_CLOSE_PREFERENCE
    };
    match pref {
        "quit" => CloseDecision::Quit,
        "tray" => CloseDecision::Hide,
        _ if has_background_work => CloseDecision::Hide,
        _ => CloseDecision::Quit,
    }
}

/// 启动时是否静默（不弹窗）。`silent_arg` 来自命令行 `--silent`（单次语义）；
/// `silent_pref` 来自 `ui_prefs` 的持久偏好。任一为真即静默。
#[must_use]
pub fn should_start_silent(silent_arg: bool, silent_pref: bool) -> bool {
    silent_arg || silent_pref
}

/// 第二实例激活请求的处置动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationDecision {
    /// 唤出主窗口（`show` + `set_focus`，复用 [`show_main_window`]，与托盘/Dock 同路径）。
    ShowMainWindow,
    /// 保持第一实例当前可见性（`--silent` 静默语义：「确保在运行」，不是「给我亮窗口」）。
    StayHidden,
}

/// 第二实例 argv → 第一实例激活行为。
///
/// 冻结规则（win32 `lifecycle.rs` 复刻，2026-09-05 单实例计划 §4.3）：argv 含
/// `--silent`（精确匹配，与既有 [`apply_startup_visibility`] 同一判定）→
/// [`ActivationDecision::StayHidden`]；否则 → [`ActivationDecision::ShowMainWindow`]。
/// 其余参数一律忽略（不解析、不透传业务语义）。argv 为第二进程完整命令行参数
/// （含可执行文件路径，由插件转发）。
#[must_use]
pub fn activation_decision(argv: &[String]) -> ActivationDecision {
    if argv.iter().any(|arg| arg == "--silent") {
        ActivationDecision::StayHidden
    } else {
        ActivationDecision::ShowMainWindow
    }
}

// ---- 基于连接事实的关闭接线 ----

fn handle_close_request(window: &tauri::WebviewWindow) {
    // 视觉响应先于文件、锁及 Core 查询。驻留决策完成后不再次隐藏用户已唤回的窗口。
    let _ = window.hide();
    if SHUTDOWN_STARTED.load(Ordering::Acquire)
        || CLOSE_REQUEST_IN_FLIGHT.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let prefs_app = app.clone();
        let preference = tauri::async_runtime::spawn_blocking(move || {
            prefs_app.try_state::<UiPrefsStore>()
                .and_then(|store| store.get().ok())
                .and_then(|prefs| prefs.close_preference)
                .unwrap_or_else(|| DEFAULT_CLOSE_PREFERENCE.to_owned())
        }).await.unwrap_or_else(|_| DEFAULT_CLOSE_PREFERENCE.to_owned());
        let busy = if preference == "quit" || preference == "tray" {
            false
        } else if let Some(state) = app.try_state::<CoreState>() {
            match tokio::time::timeout(Duration::from_secs(2), CoreClient.snapshot(&state)).await {
                Ok(Ok(snapshot)) => background_work(&snapshot),
                _ => true,
            }
        } else {
            true
        };
        match close_decision(&preference, busy) {
            CloseDecision::Quit => notify_core_shutdown(&app),
            CloseDecision::Hide => CLOSE_REQUEST_IN_FLIGHT.store(false, Ordering::Release),
        }
    });
}

/// UI 退出的唯一入口（关闭按钮 quit/smart 空闲、托盘菜单「退出」共用）。
///
/// 1. 移除托盘图标（避免残留幽灵图标）；
/// 2. best-effort 中止状态转发任务（提前释放 watch 流与 channel 克隆，让 Core 尽快
///    经认证连接 EOF 感知停机）；
/// 3. **L1 修复**：清掉 `CoreState` 的 Channel 克隆（否则连接不关、Core 不退、清理
///    必然超时），再取出会话所有权做有界 runtime 目录清理——关 stdin、等 Core 自行
///    退出（不 kill）后 `cleanup_empty()`（只 `rmdir` 空目录，非空即保留现场）。
///    清理在独立线程里执行（见下）；任何失败只记一行 stderr：不影响退出码、不阻塞
///    停机（总等待有界 5s）；
/// 4. `app.exit(0)`：进程 teardown drop 剩余句柄，core 经既有一次性 shutdown 触发器
///    收敛（壳不 kill core）。
pub fn notify_core_shutdown(app: &AppHandle) {
    let _ = hide_main_window(app);
    if SHUTDOWN_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || shutdown_core_after_hide(&app));
}

/// 在后台等待完整清理，保留 Core 的路由/网卡回收与空 runtime 目录删除路径。
fn shutdown_core_after_hide(app: &AppHandle) {
    crate::tray::remove(app);

    // 会话替换路径（bootstrap）已回收并发会话，故此处取到 `None` 属正常：进程 teardown
    // 会照旧关闭通道，core 自行停机。
    if let Some(session) = app.try_state::<CoreSession>() {
        session.abort_subscriptions();

        // Core 停机主路径是「认证 UDS 连接 EOF」，不是 stdin EOF：必须先把 CoreState
        // 持有的 Channel 克隆置为 NotWired，连接才会真正关闭。
        if let Some(state) = app.try_state::<CoreState>() {
            state.mark_stopped();
        }

        // 修复前：正常退出只关通道删 socket，runtime 目录（`/private/tmp/exv-vpn-*`）
        // 每次正常退出都残留。清理所有权明确属 UI 壳（core/engine 不得清理本目录）。
        //
        // 清理要等 Core 自行退出（有界 5s），必须放在独立线程里：本函数既可能被主线程
        // 事件循环调用（CloseRequested），也可能被 tokio 任务调用（smart 宽限到期），
        // 两者都不能就地 `block_on`（tokio 会 panic：runtime 内再进 runtime）。
        if let Some(inner) = session.take_inner_for_shutdown()
            && let Err(detail) =
                run_blocking_core_cleanup(move || inner.shutdown_and_cleanup_runtime_blocking())
        {
            // 本壳无日志设施；debug 壳 stderr 是唯一诊断通道，单行降级记录。
            #[allow(clippy::print_stderr)]
            {
                eprintln!("exv.lifecycle: core runtime cleanup skipped (best effort): {detail}");
            }
        }
    }

    SHUTDOWN_FINISHED.store(true, Ordering::Release);
    app.exit(0);
}

/// 在独立线程执行核心 runtime 清理并等待其结束（best-effort；返回失败说明文本）。
///
/// **为什么必须另起线程**：本函数被两个上下文调用——主线程窗口事件循环
/// （`CloseRequested` → quit）与 `tokio` 任务（smart 宽限到期）。清理本身要同步等待 Core
/// 退出（有界 5s），而 tokio 任务内不允许就地 `block_on`（会 panic：
/// `Cannot start a runtime from within a runtime`；这正是早先实现的真实缺陷）。
/// `std::thread::spawn` + `join` 对两种上下文同样成立。
fn run_blocking_core_cleanup<F>(cleanup: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), crate::kernel::core_process::CoreProcessError> + Send + 'static,
{
    match std::thread::spawn(cleanup).join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("cleanup thread panicked".to_owned()),
    }
}

// ---- setup ----

/// 安装托盘（best effort：失败降级为无托盘 + Dock Reopen 唤回，不阻断应用）。
pub fn setup(app: &mut App) {
    if let Err(error) = crate::tray::install(app.handle()) {
        // 降级语义与 win32 同款：无托盘时 CloseRequested → hide 仍可用，
        // macOS 上另有 Dock 点击 Reopen 唤回兜底（main.rs RunEvent::Reopen）。
        // 本壳无日志设施（无 tracing/log 依赖），诊断走 debug 壳 stderr。
        #[allow(clippy::print_stderr)] // debug 壳 stderr 是唯一诊断通道，单行降级记录
        {
            eprintln!("exv.lifecycle: tray install failed (degraded): {error}");
        }
    }
}

/// 安装完成后的启动可见性决策（setup 末尾调用一次）。
///
/// `--silent` 单次参数 ∨ prefs `silent_startup` → 保持隐藏（tauri.conf visible:false），
/// 仅托盘驻留；否则显示主窗口。
pub fn apply_startup_visibility(app: &AppHandle) {
    let silent_arg = std::env::args().any(|arg| arg == "--silent");

    let silent_pref = app
        .try_state::<UiPrefsStore>()
        .and_then(|store| store.get().ok())
        .and_then(|prefs| prefs.silent_startup)
        .unwrap_or(false);

    if should_start_silent(silent_arg, silent_pref) {
        return;
    }
    show_main_window(app);
}

/// 显示并聚焦主窗口（托盘单击 / 菜单「显示 EXV」/ Dock Reopen / 非静默启动 /
/// 第二实例激活共用）。
///
/// `pub(crate)`：main.rs 的 single-instance 回调（可能由任意线程触发）复用同一
/// 执行点，只做线程安全的 `AppHandle` 窗口操作（`unminimize` + `show` +
/// `set_focus`）——与托盘 show 无第二套窗口操作。
///
/// 先 `unminimize` 再 `show`：窗口停在 Dock 最小化态时 `show()` 对「已可见」窗口
/// 是 no-op，且 tao 会跳过已最小化窗口的聚焦；未最小化时该调用为 no-op，与 win32
/// 侧 `show_main_window` 逐行同构。
pub(crate) fn show_main_window(app: &AppHandle) {
    if SHUTDOWN_STARTED.load(Ordering::Acquire) { return; }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 隐藏主窗口（MAC-SHELL-17 S3 `minimize_to_tray_on_connect` 效果的机械通道；
/// 唤回沿用既有托盘单击 / 菜单「显示 EXV」/ Dock Reopen 接缝，窗口只 hide 不 close，
/// 不触发 `CloseRequested`，与 `close_preference` 语义互不干扰）。
///
/// # Errors
/// 主窗口不存在（理论不可达：单 main 窗口）或 hide 平台调用失败。
pub(crate) fn hide_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window missing".to_owned())?;
    window.hide().map_err(|e| e.to_string())
}

/// `shell_hide_main_window` Command：前端 lifecycle-effects `hide-window` 效果调用。
#[allow(clippy::needless_pass_by_value)] // Tauri command 宏的固定签名形状
#[tauri::command]
pub(crate) fn shell_hide_main_window(app: AppHandle) -> Result<(), String> {
    hide_main_window(&app)
}

/// `on_window_event` 的关闭/聚焦接线（main.rs Builder 挂载）。
pub(crate) fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            // 关闭行为由 close_preference 与 Core 连接状态共同决定；
            // prevent_close 统一接管，真正退出走 notify_core_shutdown。
            api.prevent_close();
            if window.label() == "main"
                && let Some(webview) = window.app_handle().get_webview_window("main")
            {
                handle_close_request(&webview);
            }
        }
        _ => {}
    }
}

/// Cmd-Q、Dock 退出与窗口/托盘退出共用同一清理入口。
pub(crate) fn on_run_event(app: &AppHandle, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::Reopen { .. } => show_main_window(app),
        tauri::RunEvent::ExitRequested { api, .. }
            if !SHUTDOWN_FINISHED.load(Ordering::Acquire) => {
                api.prevent_exit();
                notify_core_shutdown(app);
            }
        _ => {}
    }
}

// ---- 纯函数测试（对齐 win32 lifecycle-effects 决策测试模式，落壳侧）----
