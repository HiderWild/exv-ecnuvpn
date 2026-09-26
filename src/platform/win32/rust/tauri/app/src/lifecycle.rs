//! 生命周期（O3 强绑定，P4-b 真实接线；2026-08-23 托盘迁自管 + close_preference 驱动关闭）。
//!
//!   * 关闭按钮行为由 ui_prefs 的 `close_preference` 驱动（[`close_decision`] 纯函数）：
//!     先隐藏窗口，再在后台处理 `quit` → O3 停机；`tray` → 留在托盘；
//!     `smart` → 读取一次有界的新鲜运行时快照，有活动连接或恢复时驻留，否则退出。
//!   * 彻底退出 → [`notify_core_shutdown`] → O3：core 经管道关闭感知 UI 退出 → 有序停机。
//!   * 启动可见性：`--silent` 参数（单次）∨ prefs `silent_startup` → 不弹窗仅托盘；
//!     否则显示主窗口（[`resolve_startup_visibility`] 纯函数）。
//!   * 第二实例激活（2026-09-05 单实例计划）：[`activation_decision`] 纯函数把第二实例
//!     argv 映射为「唤出主窗口 / 保持隐藏」；执行点复用 [`show_main_window`]（与托盘
//!     show 同一执行点）。
//!
//! ## O3 停机机制（P4-b 确认）
//!
//! core `KernelControl` 无 shutdown RPC——「通知 core 退出」即**断开控制面管道**
//! （host `kernel_control_transport` 注释：serve 任务句柄在 UI 连接断开时 resolve →
//! `CoreRuntime` 感知停机 → `shutdown_core` 有序停机）。UI 侧不 kill core：core 由
//! pipe-close 感知自行退出（[`super::kernel::core_process::CoreChild`] 有意不做 Drop
//! 自动 terminate——kill 会打断有序停机）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{App, AppHandle, Manager, Runtime, WebviewWindow};

use crate::kernel::client::{CoreClient, CoreSession, CoreState};
use crate::kernel::state::{RuntimeSnapshot, RuntimeState};
use crate::ui_prefs::{CLOSE_PREFERENCE_VALUES, DEFAULT_CLOSE_PREFERENCE, UiPrefsStore};

/// smart close 查询 runtime 快照的上限；查询失败不阻止用户退出。
const SMART_CLOSE_SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(1_200);
/// 同一关闭操作只允许一个决策任务，避免双击或多入口重复退出/查询。
static CLOSE_REQUEST_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
/// 标题栏和托盘退出共用一次停机，避免重复释放托盘和订阅。
static SHUTDOWN_STARTED: AtomicBool = AtomicBool::new(false);

// ---- 纯函数决策（单测覆盖）----

/// 关闭请求的处置动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// 直接退出（显式 quit 或 smart 无活动连接）。
    Quit,
    /// 仅隐藏（显式 tray 或 smart 有活动连接）。
    Hide,
    /// smart 偏好：须先读取一次新鲜运行时快照。
    Smart,
}

/// 主窗口关闭请求的来源，只用于诊断自绘按钮和系统入口是否同走协调器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MainWindowCloseSource {
    /// 前端自绘标题栏的关闭按钮。
    WindowChrome,
    /// 操作系统或窗口管理器发出的 `CloseRequested`。
    SystemCloseRequested,
}

impl MainWindowCloseSource {
    const fn label(self) -> &'static str {
        match self {
            Self::WindowChrome => "window_chrome",
            Self::SystemCloseRequested => "system_close_requested",
        }
    }
}

/// 有界快照查询未能产生可信状态时的分类。日志只记录此分类，避免把 core 的自由文本
/// （例如连接摘要或错误详情）写入生命周期日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmartCloseSnapshotError {
    CoreStateUnavailable,
    QueryFailed,
    TimedOut,
}

impl SmartCloseSnapshotError {
    const fn label(self) -> &'static str {
        match self {
            Self::CoreStateUnavailable => "core_state_unavailable",
            Self::QueryFailed => "query_failed",
            Self::TimedOut => "timed_out",
        }
    }
}

/// 由 close_preference 字符串决定关闭处置。未知值视为默认 `smart`（与存储层回落一致）。
#[must_use]
pub fn close_decision(preference: &str) -> CloseDecision {
    let pref = if CLOSE_PREFERENCE_VALUES.contains(&preference) {
        preference
    } else {
        DEFAULT_CLOSE_PREFERENCE
    };
    match pref {
        "quit" => CloseDecision::Quit,
        "tray" => CloseDecision::Hide,
        _ => CloseDecision::Smart,
    }
}

/// `smart` 关闭策略：仅在活动连接、连接操作或恢复操作实际存在时隐藏。
///
/// `reconnect.active` 独立于 RuntimeState；不能只把它当作 Reconciling 的附属字段。
#[must_use]
pub fn smart_close_decision(snapshot: &RuntimeSnapshot) -> CloseDecision {
    let live_runtime = matches!(
        snapshot.runtime,
        RuntimeState::Connecting { .. }
            | RuntimeState::AwaitingInteraction { .. }
            | RuntimeState::Connected { .. }
            | RuntimeState::Stopping { .. }
    );
    let reconnecting = snapshot
        .reconnect
        .as_ref()
        .is_some_and(|status| status.active);
    let respawning = snapshot
        .self_heal
        .as_ref()
        .is_some_and(|status| status.stage == "respawning");

    if live_runtime || reconnecting || respawning {
        CloseDecision::Hide
    } else {
        CloseDecision::Quit
    }
}

/// 将一次有界快照查询的结果映射为最终 smart close 动作。
///
/// 没有可信快照时必须允许用户退出，不能因未知状态无限隐藏到托盘。
fn smart_close_decision_from_snapshot_result(
    snapshot: Result<&RuntimeSnapshot, SmartCloseSnapshotError>,
) -> CloseDecision {
    match snapshot {
        Ok(snapshot) => smart_close_decision(snapshot),
        Err(_) => CloseDecision::Quit,
    }
}

fn runtime_state_label(state: &RuntimeState) -> &'static str {
    match state {
        RuntimeState::Idle { .. } => "idle",
        RuntimeState::Connecting { .. } => "connecting",
        RuntimeState::AwaitingInteraction { .. } => "awaiting_interaction",
        RuntimeState::Connected { .. } => "connected",
        RuntimeState::Stopping { .. } => "stopping",
        RuntimeState::Reconciling { .. } => "reconciling",
        RuntimeState::FailedClean { .. } => "failed_clean",
        RuntimeState::FailedDirty { .. } => "failed_dirty",
    }
}

/// 启动时是否静默（不弹窗）。`silent_arg` 来自命令行 `--silent`（单次语义）；
/// `silent_pref` 来自 ui_prefs 的持久偏好。任一为真即静默。
#[must_use]
pub fn should_start_silent(silent_arg: bool, silent_pref: bool) -> bool {
    silent_arg || silent_pref
}

/// 第二实例激活请求的处置动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationDecision {
    /// 唤出主窗口（show + set_focus，复用 [`show_main_window`]，与托盘 show 同路径）。
    ShowMainWindow,
    /// 保持第一实例当前可见性（`--silent` 静默语义：「确保在运行」，不是「给我亮窗口」）。
    StayHidden,
}

/// 第二实例 argv → 第一实例激活行为。
///
/// 冻结规则（2026-09-05 单实例计划 §4.3）：argv 含 `--silent`（精确匹配，与既有
/// [`apply_startup_visibility`] 同一判定）→ [`ActivationDecision::StayHidden`]；
/// 否则 → [`ActivationDecision::ShowMainWindow`]。其余参数一律忽略（不解析、
/// 不透传业务语义）。argv 为第二进程完整命令行参数（含 exe 路径，由插件转发）。
#[must_use]
pub fn activation_decision(argv: &[String]) -> ActivationDecision {
    if argv.iter().any(|arg| arg == "--silent") {
        ActivationDecision::StayHidden
    } else {
        ActivationDecision::ShowMainWindow
    }
}

// ---- 纯函数测试 ----

// ---- 关闭协调 ----

fn effective_close_preference<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<UiPrefsStore>()
        .and_then(|store| store.get().ok())
        .and_then(|prefs| prefs.close_preference)
        .unwrap_or_else(|| DEFAULT_CLOSE_PREFERENCE.to_owned())
}

fn try_begin_close_request(in_flight: &AtomicBool) -> bool {
    in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn finish_close_request(in_flight: &AtomicBool) {
    in_flight.store(false, Ordering::Release);
}

fn apply_close_decision<R: Runtime>(
    app: &AppHandle<R>,
    decision: CloseDecision,
) {
    match decision {
        CloseDecision::Quit => notify_core_shutdown(app),
        CloseDecision::Hide => {
            // 请求入口已经隐藏。用户在查询期间从托盘重新打开时，不再次隐藏窗口。
            finish_close_request(&CLOSE_REQUEST_IN_FLIGHT);
        }
        CloseDecision::Smart => unreachable!("smart decisions must resolve a snapshot first"),
    }
}

async fn resolve_smart_close<R: Runtime>(
    app: AppHandle<R>,
    source: MainWindowCloseSource,
) {
    let started_at = Instant::now();
    let snapshot = match app.try_state::<CoreState>() {
        Some(state) => {
            match tokio::time::timeout(SMART_CLOSE_SNAPSHOT_TIMEOUT, CoreClient.snapshot(&state))
                .await
            {
                Ok(Ok(snapshot)) => Ok(snapshot),
                Ok(Err(_)) => Err(SmartCloseSnapshotError::QueryFailed),
                Err(_) => Err(SmartCloseSnapshotError::TimedOut),
            }
        }
        None => Err(SmartCloseSnapshotError::CoreStateUnavailable),
    };

    match snapshot {
        Ok(snapshot) => {
            let decision = smart_close_decision_from_snapshot_result(Ok(&snapshot));
            tracing::debug!(
                target: "exv.lifecycle",
                source = source.label(),
                ?decision,
                runtime_state = runtime_state_label(&snapshot.runtime),
                reconnect_active = snapshot.reconnect.as_ref().is_some_and(|status| status.active),
                self_heal_recovery_active = snapshot
                    .self_heal
                    .as_ref()
                    .is_some_and(|status| status.stage == "respawning"),
                elapsed_ms = started_at.elapsed().as_millis(),
                "smart close resolved from fresh runtime snapshot"
            );
            apply_close_decision(&app, decision);
        }
        Err(reason) => {
            tracing::warn!(
                target: "exv.lifecycle",
                source = source.label(),
                elapsed_ms = started_at.elapsed().as_millis(),
                snapshot_error = reason.label(),
                "smart close could not obtain runtime snapshot; quitting"
            );
            apply_close_decision(
                &app,
                smart_close_decision_from_snapshot_result(Err(reason)),
            );
        }
    }
}

/// 唯一主窗口关闭入口：自绘关闭按钮与系统 CloseRequested 共用同一偏好及运行时策略。
///
/// 窗口先隐藏，偏好读取、运行时判断与停机都不阻塞关闭的视觉响应。
/// 显式 `quit` / `tray` 无需查询 Core；`smart` 只做一次有界的 GetSnapshot。状态未知或
/// 查询失败时如实记录并退出，避免把没有可证实活动连接的 UI 无限留在托盘。
pub(crate) fn request_main_window_close<R: Runtime>(
    window: WebviewWindow<R>,
    source: MainWindowCloseSource,
) {
    if window.label() != "main" {
        return;
    }
    hide_before_close(&window);
    if !try_begin_close_request(&CLOSE_REQUEST_IN_FLIGHT) {
        tracing::debug!(target: "exv.lifecycle", source = source.label(), "close request ignored while another close decision is in flight");
        return;
    }

    let app = window.app_handle().clone();
    // 偏好读取涉及文件和锁，不能继续占用窗口事件线程。
    tauri::async_runtime::spawn_blocking(move || {
        let preference = effective_close_preference(&app);
        let decision = close_decision(&preference);
        tracing::debug!(
            target: "exv.lifecycle",
            source = source.label(),
            ?decision,
            "main window close requested"
        );
        match decision {
            CloseDecision::Smart => {
                tauri::async_runtime::spawn(resolve_smart_close(app, source));
            }
            CloseDecision::Quit | CloseDecision::Hide => apply_close_decision(&app, decision),
        }
    });
}

fn hide_before_close<R: Runtime>(window: &WebviewWindow<R>) {
    match window.hide() {
        Ok(()) => tracing::debug!(target: "exv.lifecycle", "main window hidden before close processing"),
        Err(error) => tracing::warn!(target: "exv.lifecycle", "close hide failed; continuing close processing: {error}"),
    }
}

// ---- O3 停机 ----

/// 通知 core 进程退出（O3：窗口彻底关闭 → core+engine 一并终止）。
///
/// 先隐藏窗口，再派发后台任务完成以下步骤：
/// 1. 卸载托盘图标（避免残留幽灵图标）；
/// 2. 中止事件订阅 task（停止 emit）；
/// 3. `app.exit(0)`——进程 teardown drop `CoreState`（channel）→ 控制面管道关闭 →
///    core `serve_task` resolve → core 有序停机（engine StopTunnel → engine 终止）。
///    订阅 task 已中止，其 channel 克隆随之释放，管道在进程退出前已无引用。
pub fn notify_core_shutdown<R: Runtime>(app: &AppHandle<R>) {
    // 托盘退出也先隐藏主窗口；标题栏路径重复隐藏是幂等操作。
    if let Some(window) = app.get_webview_window("main") {
        hide_before_close(&window);
    }
    if SHUTDOWN_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || shutdown_core_after_hide(&app));
}

fn shutdown_core_after_hide<R: Runtime>(app: &AppHandle<R>) {
    tracing::info!(target: "exv.lifecycle", "core shutdown requested (O3: UI exit → core+engine terminate)");

    // 0. 移除托盘图标。
    crate::tray::remove_tray();

    // 1. 中止事件订阅（停止 emit + 释放 channel 克隆，加速管道断开）。
    if let Some(session) = app.try_state::<CoreSession>() {
        let subs = session
            .subscriptions
            .lock()
            .expect("subscriptions lock");
        for handle in subs.iter() {
            handle.abort();
        }
        tracing::info!(target: "exv.lifecycle", count = subs.len(), "event subscriptions aborted");
    }

    // 2. 应用退出。core 由 pipe-close 感知自行有序停机（见模块文档）。
    app.exit(0);
}

// ---- setup ----

/// 注册托盘回调 + 决定启动可见性（托盘本体在 lib.rs 的 Builder 之后经 [`install`] 安装）。
///
/// # Errors
/// 仅 propagate tauri 错误；托盘/可见性失败降级不阻断应用。
pub fn setup(app: &mut App) -> tauri::Result<()> {
    let handle = app.handle().clone();

    // 托盘回调注入：show = 显示主窗口；quit = O3 停机。
    crate::tray::set_show_handler(move || show_main_window(&handle));
    let quit_handle = app.handle().clone();
    crate::tray::set_quit_handler(move || notify_core_shutdown(&quit_handle));

    // 自管托盘安装（best effort：失败降级无托盘，CloseRequested→hide 仍可用）。
    if let Err(error) = crate::tray::install_tray() {
        tracing::warn!(target: "exv.lifecycle", "tray install failed (degraded): {error}");
    }
    // tao/Tauri 的默认窗口图标在 Windows 上只写 ICON_SMALL；任务栏所需
    // ICON_BIG 必须单独安装，否则 Shell 可能把 16px 句柄放大。失败时保留
    // EXE resource fallback，不因外观降级阻断 VPN 主流程。
    if let Err(error) = crate::tray::install_window_icons(app.handle()) {
        tracing::warn!(target: "exv.lifecycle", "window icon install failed (degraded): {error}");
    }

    Ok(())
}

/// 安装完成后的启动可见性决策（lib.rs 在 bootstrap 后调用一次）。
///
/// `--silent` 单次参数 ∨ prefs `silent_startup` → 保持隐藏（tauri.conf visible:false），
/// 仅托盘驻留；否则显示主窗口。
pub fn apply_startup_visibility(app: &AppHandle) {
    let silent_arg = std::env::args().any(|arg| arg == "--silent");

    let silent_pref = match app.try_state::<UiPrefsStore>() {
        Some(store) => store
            .get()
            .ok()
            .and_then(|prefs| prefs.silent_startup)
            .unwrap_or(false),
        None => false,
    };

    if should_start_silent(silent_arg, silent_pref) {
        tracing::info!(target: "exv.lifecycle",
            silent_arg, silent_pref, "starting hidden to tray (silent)");
        return;
    }
    show_main_window(app);
}

/// 显示并聚焦主窗口（托盘单击 / 菜单「显示 EXV」/ 非静默启动 / 第二实例激活共用）。
///
/// `pub(crate)`：lib.rs 的 single-instance 回调（可能由任意线程触发）复用同一执行点，
/// 只做线程安全的 `AppHandle` 窗口操作（unminimize + show + set_focus）——与托盘 show
/// 无第二套窗口操作。
///
/// **必须先 `unminimize`**：窗口最小化到任务栏时，`show()` 对「已可见」的窗口是 no-op，
/// 而 tao（0.35.x）的 `set_focus()` 在 `is_minimized` 为真时直接跳过——只做 show + focus
/// 的旧写法会让「已运行时再次双击快捷方式」完全无反应。
pub(crate) fn show_main_window(app: &AppHandle) {
    if SHUTDOWN_STARTED.load(Ordering::Acquire) {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
