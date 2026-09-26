//! macOS 托盘（Tauri v2 `TrayIconBuilder`；MAC-SHELL-17 S2 自 win32 自管
//! `Shell_NotifyIcon` 托盘完全重写，计划 §2「完全重写」项）。
//!
//! ## 选型（计划 §S2「图标资源」拍板）
//!
//!   * Win32 先例是自管 `Shell_NotifyIcon`（气泡通知需要原始 NOTIFYICONDATA 句柄）；
//!     macOS 无气泡需求（连接通知走 S3 插件），故直接用 Tauri 托盘 API，不写裸 objc。
//!   * 菜单栏图标为黑白 template 剪影（2026-09-20 用户需求「图标希望黑白两色」，
//!     取代 MAC-SHELL-17 S2 的「保留双色、禁用模板」拍板）：盾牌+勾号简化剪影，
//!     纯黑前景、alpha 表达形状，勾号镂空；`icon_as_template(true)` 让系统在浅色
//!     菜单栏画黑、深色画白（template 是 macOS 菜单栏图标的标准做法）。资源来自
//!     resources/menu-icon-template.svg，内嵌 2× RGBA；tray-icon 的 macOS 后端按
//!     18 pt 显示，36 px 资源覆盖 Retina。
//!
//! ## 行为（对齐 win32 tray.rs 菜单结构，适配 macOS 惯例）
//!
//!   * 左键单击 = 显示主窗口；右键 = 菜单（`show_menu_on_left_click(false)`）。
//!   * 菜单：状态行（禁用项，文案随 kernel 事件流经
//!     [`update_connection_state`] 更新）/ 「显示 EXV」/ 「退出 EXV」（退出走
//!     O3 停机 [`crate::lifecycle::notify_core_shutdown`]：UI 退出 → core 经既有
//!     EOF 链路有序停机，壳不 kill core）。

use std::sync::OnceLock;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};

/// 托盘 id（单实例固定；退出时按 id 移除）。
const TRAY_ID: &str = "exv-main-tray";
/// 18 pt 菜单栏图标的 Retina 资源，保留矢量母版（12:13）的纵横比；template 剪影。
const TRAY_ICON_WIDTH: u32 = 34;
const TRAY_ICON_HEIGHT: u32 = 36;
const TRAY_ICON_RGBA: &[u8; 34 * 36 * 4] =
    include_bytes!("../resources/menu-icon-template@2x.rgba");

/// 状态菜单项句柄（`MenuItem<Wry>` 内部 Arc + Send/Sync；`set_text` 内部派发主线程）。
static STATE_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();

/// 连接状态 → 托盘状态行文案（纯函数；状态值来自壳侧 `exv://status` 转发器的
/// `RuntimeEvent` 快照派生：idle/connecting/connected/stopping/failed）。
#[must_use]
pub(crate) fn connection_state_label(state: &str) -> &'static str {
    match state {
        "idle" => "状态：未连接",
        "connecting" => "状态：连接中…",
        "connected" => "状态：已连接",
        "stopping" => "状态：断开中…",
        "failed" => "状态：连接失败",
        _ => "状态：未知",
    }
}

/// 连接状态变化时更新托盘状态行（壳侧 `exv://status` 转发点调用；托盘未安装时静默丢弃）。
pub(crate) fn update_connection_state(state: &str) {
    if let Some(item) = STATE_ITEM.get() {
        let _ = item.set_text(connection_state_label(state));
    }
}

/// 退出前移除托盘（`lifecycle::notify_core_shutdown` 调用；未安装时无操作）。
///
/// 必须把 `remove_tray_by_id` 派发到主线程执行：调用方（`shutdown_core_after_hide`）
/// 运行在 `spawn_blocking` 后台线程，而 `NSStatusBar removeStatusItem` 是主线程限定
/// 操作（BoardServices `assertBarrierOnQueue` 断言 → SIGTRAP，即 2026-09 实测的
/// 「退出 EXV 后弹意外退出」崩溃）；同时 tauri 2.x 的 `TrayIcon` 是 `unsafe Send` 包着
/// `Rc<RefCell<…>>`，在后台线程 drop 属于跨线程触碰非原子 Rc 的 UB 面，主线程内 drop
/// 一并消除。AppHandle 克隆进闭包，使返回的 `TrayIcon` 从产生到析构全程不离开主线程
/// ——**不得**在闭包外提前 `remove_tray_by_id`（那样 Rc 句柄会落回后台线程，UB 依旧）。
/// 派发失败（主循环已在 teardown）按 best-effort 忽略：进程退出本身会清除状态项。
pub(crate) fn remove(app: &AppHandle) {
    let app_for_main_thread = app.clone();
    let _ = app.run_on_main_thread(move || {
        // 闭包内只做移除这一件事：时序上托盘退出任务与 `app.exit(0)` 经同一 proxy
        // channel FIFO，remove 先于 Exit 处理；主线程内联同步、无死锁面。
        drop(app_for_main_thread.remove_tray_by_id(TRAY_ID));
    });
}

/// template 资源：黑像素 + alpha 表达形状（NSImage template 渲染只取 alpha 通道，
/// 系统按菜单栏外观自动反色；见 `resources/README.md` 的资产管线）。
fn tray_icon() -> tauri::image::Image<'static> {
    tauri::image::Image::new(TRAY_ICON_RGBA, TRAY_ICON_WIDTH, TRAY_ICON_HEIGHT)
}

/// 安装托盘。失败返回 Err（调用方记录并降级：CloseRequested → hide 仍可用 +
/// Dock Reopen 唤回兜底）。
///
/// # Errors
/// 菜单/托盘构建失败，或托盘已安装（重复 install 视为错误）。
pub(crate) fn install(app: &AppHandle) -> Result<(), String> {
    let state_item = MenuItem::with_id(
        app,
        "tray-state",
        connection_state_label("idle"),
        false,
        None::<&str>,
    )
    .map_err(|e| format!("state menu item: {e}"))?;
    let show_item = MenuItem::with_id(app, "tray-show", "显示 EXV", true, None::<&str>)
        .map_err(|e| format!("show menu item: {e}"))?;
    let quit_item = MenuItem::with_id(app, "tray-quit", "退出 EXV", true, None::<&str>)
        .map_err(|e| format!("quit menu item: {e}"))?;
    let separator = PredefinedMenuItem::separator(app).map_err(|e| format!("separator: {e}"))?;
    let menu = Menu::with_items(app, &[&state_item, &separator, &show_item, &quit_item])
        .map_err(|e| format!("tray menu: {e}"))?;

    // 状态句柄全局登记（重复 install 在此失败，与 win32 TRAY.set 同语义）。
    if STATE_ITEM.set(state_item.clone()).is_err() {
        return Err("tray already installed".to_owned());
    }

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // 左键留给「显示主窗口」，菜单只在右键出现（对齐 win32 惯例）。
        .show_menu_on_left_click(false)
        // template 渲染：形状由 alpha 表达，浅色菜单栏画黑、深色画白（2026-09-20
        // 用户需求取代 MAC-SHELL-17 S2 的「保留双色」拍板，见模块头注释）。
        .icon_as_template(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-show" => crate::lifecycle::show_main_window(app),
            "tray-quit" => crate::lifecycle::notify_core_shutdown(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::lifecycle::show_main_window(tray.app_handle());
            }
        });
    builder = builder.icon(tray_icon());

    builder.build(app).map_err(|e| format!("tray build: {e}"))?;
    Ok(())
}
