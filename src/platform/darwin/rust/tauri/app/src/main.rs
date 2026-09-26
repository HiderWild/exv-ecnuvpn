//! Darwin Tauri 产品入口。
//!
//! win32 UI 全量接入：前端（win32 拷贝 + adapter 单接缝/标题栏适配）不导入宿主
//! SDK，由下方注入的 `__EXV_DARWIN_COMMAND_ADAPTER__` 提供 call/listen/isFocused；
//! 命令面对齐 win32（名字 + wire DTO 形状，见 `kernel/`）。旧 camelCase 契约
//!（core_start/core_retry/IdleSnapshotDto 等）已随旧 darwin 前端废弃：core 会话
//! 在 setup 时 bootstrap（`kernel::bootstrap`），断流/连接恢复走整会话替换。

use tauri::Manager;

mod autostart;
mod kernel;
mod lifecycle;
mod log_export;
mod notification;
mod notification_modern;
mod tray;
mod ui_prefs;
mod window_chrome;

/// Tauri 文档启动时注入的薄桥：前端只消费自己的 `TauriCommandAdapter`，不导入宿主
/// SDK、更不接触 Core 进程或 UDS。`withGlobalTauri` 在这个脚本之后提供 `window.__TAURI__`，
/// 因此函数体在前端实际调用时才读取它。
///
/// 语义等价映射（win32 前端视角）：
///   * `call(command, args)` = 宿主 invoke；
///   * `listen(event, listener)` = 宿主事件订阅，回调收到**完整事件对象**
///     （`{ payload, ... }`，与宿主 event API 形状一致——win32 前端的
///     `(e) => cb(e.payload)` 回调原样可用）；
///   * `isWindowFocused()` = 宿主 `getCurrentWindow().isFocused()`（lifecycle-effects
///     前台通知抑制用；只读权限在 `core:default` 内）。
const DARWIN_COMMAND_ADAPTER_INIT_SCRIPT: &str = r"
window.__EXV_DARWIN_COMMAND_ADAPTER__ = {
  call(command, args) {
    return window.__TAURI__.core.invoke(command, args);
  },
  listen(event, listener) {
    return window.__TAURI__.event.listen(event, listener);
  },
  isWindowFocused() {
    return window.__TAURI__.window.getCurrentWindow().isFocused();
  },
};
";

fn main() {
    tauri::Builder::default()
        // 应用级单实例（对齐计划 P8/W2-B；win32 lib.rs 同款）：必须在 plugin 链中
        // **先于业务插件**注册——插件在自身初始化期（早于 .setup）以 app identifier
        // 派生的 UDS 单例 socket 判定并把第二实例 argv 转发给第一实例，第二实例在此
        // 之前零副作用退出（不 spawn core、不装托盘、不建窗口）。`.manage(...)` 与
        // 插件注册之间无顺序约束（win32 2026-09-05 单实例计划 §4.2 同款结论）。
        //
        // 回调线程契约（照 win32 lib.rs）：second-instance 回调可能在任意线程触发，
        // 回调内只允许线程安全的 `AppHandle` 操作，不得访问 managed State、不得阻塞
        // （任何锁等待/长任务都会卡死激活转发路径）；禁止派发 connect/stop/
        // serviceControl、改 ui_prefs、发通知、触碰 CoreState——「只激活不打扰」。
        //
        // 本壳无 tracing 设施（kernel/bootstrap.rs 先例）：诊断走 debug 壳 stderr，
        // 带 `[single-instance]` 前缀；StayHidden 与异常都如实记录。
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // 激活决策（lifecycle::activation_decision 冻结规则，win32 复刻）：argv 含
            // `--silent`（精确匹配）→ 仅记录、不改第一实例可见性；否则唤出主窗口
            // （show_main_window 与托盘 show / Dock Reopen 同一执行点，内建 smart
            // 宽限重置）。遵守上述回调线程契约：只有线程安全的 AppHandle 操作。
            match lifecycle::activation_decision(&argv) {
                lifecycle::ActivationDecision::ShowMainWindow => {
                    lifecycle::show_main_window(app);
                }
                lifecycle::ActivationDecision::StayHidden => {
                    #[allow(clippy::print_stderr)] // debug 壳 stderr 是唯一诊断通道
                    {
                        eprintln!(
                            "[single-instance] second instance activated with --silent; keeping current visibility"
                        );
                    }
                }
            }
        }))
        // 系统通知插件（MAC-SHELL-17 S3；init 只注册 handler/state，惰性安全）。
        // 无签名裸 debug 二进制下的不可用性由 notification.rs 能力探测两态降级兜底。
        .plugin(tauri_plugin_notification::init())
        .append_invoke_initialization_script(DARWIN_COMMAND_ADAPTER_INIT_SCRIPT)
        .manage(window_chrome::WindowChromeState::new())
        .setup(|app| {
            // 前端自有设置存储（~/Library/Application Support/EXV/ui-preferences.json；
            // 产品状态目录解析失败时不 manage → Command 返回错误，不猜测路径）。
            if let Some(store) = ui_prefs::UiPrefsStore::from_product_dir() {
                app.manage(store);
            }
            // 托盘（best effort，失败降级 Dock Reopen 兜底）+ 启动可见性
            // （--silent ∨ silent_startup → 仅托盘驻留；MAC-SHELL-17 S2）。
            lifecycle::setup(app);
            // 标题栏原生拖动条（titleBarStyle: Overlay 的「拖动区交还系统」接线；
            // win32 `window_chrome::install` 同位挂载）。
            window_chrome::install(app.handle())?;
            // core 会话 bootstrap（win32 式：spawn → UDS 认证拨号 → 状态/事件接线；
            // 失败降级 NotWired，连接时恢复）。
            kernel::bootstrap::bootstrap(app);
            lifecycle::apply_startup_visibility(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // 窗口尺寸变化后重算标题栏拖动条几何（先于 lifecycle，无相互依赖）。
            window_chrome::on_window_event(window, event);
            lifecycle::on_window_event(window, event);
        })
        .invoke_handler(tauri::generate_handler![
            // ---- kernel 命令面（win32 名字 + win32 DTO 形状）----
            kernel::commands::connect,
            kernel::commands::core_status,
            kernel::commands::stop,
            kernel::commands::snapshot,
            kernel::commands::stats,
            kernel::commands::logs_list,
            kernel::commands::logs_clear,
            kernel::commands::config_get,
            kernel::commands::saved_password,
            kernel::commands::config_set,
            kernel::quick_start::quick_start_apply,
            kernel::commands::tunnel_address,
            kernel::commands::respond_interaction,
            kernel::commands::trigger_latency_refresh,
            kernel::commands::service_control,
            kernel::commands::open_external,
            kernel::commands::pre_uninstall,
            kernel::commands::pre_uninstall_quit,
            // ---- 前端自有设置 / 自启动 / 通知 ----
            ui_prefs::ui_prefs_get,
            ui_prefs::ui_prefs_set,
            // 日志导出（本地文件操作，不经 Core/Engine；win32 logs_export 同名对齐）。
            log_export::logs_export,
            // 登录自启动（MAC-SHELL-17 S4：LaunchAgent plist 执行真相源；
            // 前端 launch_at_login 保存分流指向 autostart_set）。
            autostart::autostart_set,
            // win32 通知通道名（前端 main.ts tray-notify 效果；两态降级壳内收口）。
            notification::tray_notify,
            // ---- 窗口壳（advanced/minimal 尺寸切换 + 显隐；不碰原生装饰）----
            window_chrome::window_chrome_set_mode,
            window_chrome::window_chrome_control,
            window_chrome::window_set_visible,
            // 旧 darwin 通道保留（同一 hide 执行点；win32 前端经 window_set_visible）。
            lifecycle::shell_hide_main_window,
        ])
        .build(tauri::generate_context!())
        .expect("build Darwin Tauri application")
        .run(lifecycle::on_run_event);
}
