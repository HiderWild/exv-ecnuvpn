
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// kernel 对外暴露：供集成测试/诊断驱动（`tests/drive_service_flow.rs`）在进程级
// spawn 真实 core 并驱动 ServiceControl/Connect/Snapshot/LogsList，无需 GUI。
pub mod kernel;
mod autostart;
mod lifecycle;
mod log_export;
mod toast;
mod toast_identity;
mod tray;
mod ui_prefs;
mod window_chrome;

use tauri::Manager;

use kernel::commands;

/// 应用入口（main.rs 调用）。
pub fn run() {
    tauri::Builder::default()
        .manage(window_chrome::WindowChromeState::new())
        // 应用级单实例（2026-09-05 单实例计划 §4.2）：必须在 plugin 链中**先于业务插件**
        // 注册——插件在自身初始化期（早于 `.setup`）判定互斥体并转发第二实例参数，
        // 第二实例在此之前零副作用退出（不 spawn core、不装托盘、不建窗口）。
        // 注意：`.manage(...)` 与插件注册之间无顺序约束（「须置于 manage 之前」的旧表述废止）。
        //
        // 插件初始化失败约定（互斥体不可用等罕见环境）：统一以 tracing target
        // `exv::single_instance` 记录后继续启动——如实记录、不加第二道全局门禁
        // （开放世界纪律）；互斥体名/回收细节以插件实现为准，本仓以行为级验收承载。
        //
        // 回调线程契约：second-instance 回调可能在任意线程触发，回调内只允许线程安全的
        // `AppHandle` 操作，不得访问 managed State、不得阻塞（任何锁等待/长任务都会
        // 卡死激活转发路径）；禁止派发 connect/stop/serviceControl、改 ui_prefs、
        // 发托盘气泡、触碰 CoreState——「只激活不打扰」。
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // 激活决策（§4.3，lifecycle::activation_decision 冻结规则）：argv 含
            // `--silent`（精确匹配）→ 仅记录、不改第一实例可见性；否则唤出主窗口
            // （show_main_window 与托盘 show 同一执行点）。遵守上述回调线程契约：
            // 只有线程安全的 AppHandle 操作 + tracing，零业务派发。
            use lifecycle::ActivationDecision;
            match lifecycle::activation_decision(&argv) {
                ActivationDecision::ShowMainWindow => lifecycle::show_main_window(app),
                ActivationDecision::StayHidden => {
                    tracing::info!(target: "exv::single_instance",
                        "second instance activated with --silent; keeping current visibility");
                }
            }
        }))
        .plugin(tauri_plugin_log::Builder::new().build())
        .setup(|app| {
            // 前端自有设置存储（产品状态目录；解析失败时不 manage → Command 返回错误）。
            if let Some(store) = ui_prefs::UiPrefsStore::from_product_dir() {
                app.manage(store);
            } else {
                tracing::warn!(target: "exv.bootstrap",
                    "ui state dir unavailable; ui prefs commands will error");
            }

            lifecycle::setup(app)?;
            window_chrome::install(&app.handle())?;
            // R8：toast AUMID 注册（幂等）。让系统 toast 以 EXV 身份发出。
            toast_identity::ensure_toast_identity_registered();
            // core 进程归属（O3）：UI 宿主 spawn core → 拨号 → 管理 CoreState/
            // CoreSession → 挂事件订阅。core 缺失/拨号失败走降级路径（NotWired）。
            kernel::bootstrap::bootstrap(app)?;

            // 启动可见性（静默启动 ∨ prefs）：默认窗口 visible:false，此处决定是否亮出。
            lifecycle::apply_startup_visibility(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // 系统关闭与自绘关闭按钮共用 lifecycle 的单一关闭协调器。
                    api.prevent_close();
                    if window.label() == "main" {
                        if let Some(webview) = window.app_handle().get_webview_window("main") {
                            lifecycle::request_main_window_close(
                                webview,
                                lifecycle::MainWindowCloseSource::SystemCloseRequested,
                            );
                        }
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::core_status,
            commands::stop,
            commands::snapshot,
            commands::stats,
            commands::logs_list,
            commands::logs_clear,
            log_export::logs_export,
            commands::config_get,
            commands::saved_password,
            commands::config_set,
            kernel::quick_start::quick_start_apply,
            commands::tunnel_address,
            commands::respond_interaction,
            commands::trigger_latency_refresh,
            commands::service_control,
            commands::open_external,
            ui_prefs::ui_prefs_get,
            ui_prefs::ui_prefs_set,
            autostart::autostart_set,
            tray::tray_notify,
            window_chrome::window_chrome_set_mode,
            window_chrome::window_chrome_control,
            window_chrome::window_set_visible,
        ])
        .run(tauri::generate_context!())
        .expect("error while running EXV Tauri app");
}
