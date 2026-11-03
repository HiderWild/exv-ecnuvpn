
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// kernel 对外暴露：供集成测试/诊断驱动（`tests/drive_service_flow.rs`）在进程级
// spawn 真实 core 并驱动 ServiceControl/Connect/Snapshot/LogsList，无需 GUI。
mod atrust_scheme;
mod autostart;
mod config_acl_repair;
pub mod kernel;
mod lifecycle;
mod log_export;
mod quick_start_marker;
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
            //
            // L4 `aTrust://` 协议激活（S4）：注册命令以 URL 为参数启动/激活本应用，
            // 走同一条「唤出主窗口」路径；此处仅额外记录激活来源，不新增业务语义。
            if atrust_scheme::is_scheme_activation(&argv) {
                tracing::info!(target: "exv::single_instance",
                    "aTrust:// 协议激活：唤出主窗口（最小处理，不自动发起连接）");
            }
            use lifecycle::ActivationDecision;
            match lifecycle::activation_decision(&argv) {
                ActivationDecision::ShowMainWindow => lifecycle::show_main_window(app),
                ActivationDecision::StayHidden => {
                    tracing::info!(target: "exv::single_instance",
                        "second instance activated with --silent; keeping current visibility");
                }
            }
        }))
        // CDP 消息包含会话 Cookie；开发构建也不能记录底层 WebSocket 帧。
        //
        // A1-3（2026-10-09 区域 A，复审 R4 + 六区裁定 §二.A-2）：UI 生命周期事件的
        // 文件 target 显式化。落点核实结论：tauri-plugin-log 2.9.0 的文件落点配置
        // 是 `TargetKind::LogDir`（Windows 解析为 `%LOCALAPPDATA%\<identifier>\logs`
        // 即 `com.exv.vpn.desktop\logs`，插件自管目录创建与轮转）；`TargetKind::Folder`
        // 是「自定义路径」类别——按裁定 UI 日志**不写** `~/.exv/logs`（core 聚合日志
        // 的领地）。2.9.0 的默认 targets 恰为 [Stdout, LogDir{None}]（本行旧形状
        // 隐式依赖该默认），但复审对默认值的事实判断（误记为 Stdout+Webview）说明
        // 这层隐式依赖很脆弱：插件升级改默认值会静默丢掉文件通道，2026-10-08 整程
        // 更换事故「谁换了 UI 进程」的归因盲区（lifecycle target 持久证据零痕迹）
        // 就会复现。此处显式钉住通道集合与文件名 ui.log，轮转上限从默认 40KB 提到
        // 1MB（KeepOne），保证事故窗口的 lifecycle 事件不被日常日志快速刷掉；
        // 仅既有 lifecycle 事件，不新增日志面。
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("ui.log".to_string()),
                    }),
                ])
                .max_file_size(1024 * 1024)
                .build(),
        )
        .setup(|app| {
            // C6 观测加固（2026-10-09 区域 C 计划 §2.6-1）：setup 最早处 logger canary。
            // 刻意用 `log::`（tauri_plugin_log 桥接 fern 双通道：stdout + LogDir 轮转
            // 文件）而非本文件其他位置的 `tracing::`（无全局 tracing subscriber，
            // 恒丢弃——LogDir 里物理上不会出现）。与 main() 入口的 eprintln! canary
            // 组成两层判读：入口行在+本行缺席=未到 setup 或 logger 未初始化；
            // 两行都在而 stdout 无输出=纯 stdout 句柄问题。本行同时落 LogDir，
            // 不依赖 stdout 句柄存活。
            log::info!(
                target: "exv.bootstrap",
                "ui process setup reached profile={} pid={}",
                if cfg!(debug_assertions) { "debug" } else { "release" },
                std::process::id()
            );

            // 前端自有设置存储（产品状态目录；解析失败时不 manage → Command 返回错误）。
            if let Some(store) = ui_prefs::UiPrefsStore::from_product_dir() {
                app.manage(store);
            } else {
                tracing::warn!(target: "exv.bootstrap",
                    "ui state dir unavailable; ui prefs commands will error");
            }

            // 学校登录尝试计数器（验证码风险预告）：与 ui_prefs 同一产品状态目录；
            // 目录不可解析时不 manage——计数停用（embedded_auth 只记 warn），不影响认证。
            if let Some(tracker) = kernel::login_attempt_tracker::LoginAttemptTracker::from_product_dir() {
                app.manage(tracker);
            } else {
                tracing::warn!(target: "exv.bootstrap",
                    "ui state dir unavailable; login attempt counting disabled");
            }

            // 快速入门「已见」标记（KI-13：与 core config.json 同目录；
            // 路径推导不失败，无需降级分支——config_dir() 恒有回落值）。
            app.manage(quick_start_marker::QuickStartMarkerStore::from_product_dir());

            lifecycle::setup(app)?;
            window_chrome::install(&app.handle())?;
            // R8：toast AUMID 注册（幂等）。让系统 toast 以 EXV 身份发出。
            toast_identity::ensure_toast_identity_registered();
            // core 进程归属（O3）：UI 宿主 spawn core → 拨号 → 管理 CoreState/
            // CoreSession → 挂事件订阅。core 缺失/拨号失败走降级路径（NotWired）。
            kernel::bootstrap::bootstrap(app)?;

            // 系统浏览器认证开始后才准备证书和请求级Agent。
            app.manage(kernel::local_agent::LocalAgentState::default());

            // L4 `aTrust://` URL 协议注册（S4）：只在无人认领时注册到当前用户
            // HKCU\Software\Classes；官方客户端在运行/已注册（或认领状态未知）一律
            // 让位，只记录不阻断（见 atrust_scheme 模块注释的冲突守卫）。
            atrust_scheme::ensure_registered();

            // 启动可见性（静默启动 ∨ prefs）：默认窗口 visible:false，此处决定是否亮出。
            lifecycle::apply_startup_visibility(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if window.label() == "main" {
                        // 系统关闭与自绘关闭按钮共用 lifecycle 的单一关闭协调器。
                        api.prevent_close();
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
            commands::auth_preparation_respond,
            commands::auth_receiver_status,
            commands::restore_official_auth_receiver,
            commands::atrust_browser_fallback,
            commands::atrust_browser_fallback_available,
            commands::atrust_show_auth_window,
            commands::core_status,
            commands::agent_sim_ca_uninstall,
            atrust_scheme::atrust_scheme_unregister,
            commands::stop,
            commands::snapshot,
            commands::stats,
            commands::logs_list,
            commands::logs_clear,
            log_export::logs_export,
            commands::config_get,
            commands::config_set,
            config_acl_repair::config_repair_permissions,
            kernel::quick_start::quick_start_apply,
            commands::tunnel_address,
            commands::respond_interaction,
            commands::trigger_latency_refresh,
            commands::service_control,
            commands::open_external,
            commands::core_restart,
            lifecycle::app_restart,
            lifecycle::app_exit,
            ui_prefs::ui_prefs_get,
            ui_prefs::ui_prefs_set,
            quick_start_marker::quick_start_seen_get,
            quick_start_marker::quick_start_seen_mark,
            autostart::autostart_set,
            tray::tray_notify,
            window_chrome::window_chrome_set_mode,
            window_chrome::window_chrome_control,
            window_chrome::window_set_visible,
            window_chrome::window_is_foreground,
        ])
        .run(tauri::generate_context!())
        .expect("error while running EXV Tauri app");
}
