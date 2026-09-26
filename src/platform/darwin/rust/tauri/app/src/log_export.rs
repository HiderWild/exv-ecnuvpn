//! 日志导出属于本地桌面文件操作，不经过 VPN 控制链（win32 壳 `log_export.rs`
//! 同款边界，2026-09-17 吸收）。
//!
//! macOS 保存面板是 AppKit `NSSavePanel runModal`。AppKit 只能在主线程触碰，
//! 而 async Tauri 命令运行在 tokio worker 线程 → 先经
//! [`tauri::AppHandle::run_on_main_thread`] 派发再用 oneshot 回传结果
//! （[`crate::kernel::external_open`] 同款模式）；win32 对应实现是独立 STA
//! 线程上的 FileSaveDialog——同一「模态保存面板 + 真实落盘确认」问题的两个
//! 平台惯用法。只有选择路径并写入成功才返回成功；取消不报错。
//!
//! 守卫红线复核：不加新 crate（objc2-app-kit 同源既有依赖，仅打开
//! NSPanel/NSSavePanel feature，与 NSWorkspace 先例同款）；无 tauri-plugin、
//! 无进程/脚本执行面。文件写入在主线程闭包内完成——面板确认后立即落盘并
//! `sync_all`，路径与内容不离开本命令。

use std::{io::Write, path::Path};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
use objc2_foundation::NSString;

/// 在当前线程（调用方保证主线程）运行保存面板；`None` 表示用户取消。
fn choose_destination_on_main(
    mtm: MainThreadMarker,
    suggested_name: &str,
) -> Option<std::path::PathBuf> {
    let panel = NSSavePanel::savePanel(mtm);
    panel.setTitle(Some(&NSString::from_str("导出日志")));
    panel.setNameFieldStringValue(&NSString::from_str(suggested_name));
    panel.setCanCreateDirectories(true);
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    let url = panel.URL()?;
    let path = url.path()?;
    Some(std::path::PathBuf::from(path.to_string()))
}

fn write_log_file(path: &Path, contents: &str) -> Result<(), String> {
    let mut file =
        std::fs::File::create(path).map_err(|error| format!("无法创建日志文件：{error}"))?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("写入日志文件失败：{error}"))
}

/// false 仅表示用户取消；只有实际写入成功才返回 true。
#[tauri::command]
pub async fn logs_export(
    app: tauri::AppHandle,
    suggested_name: String,
    contents: String,
) -> Result<bool, String> {
    if suggested_name.is_empty() || suggested_name.contains(['/', '\\', '\0']) {
        return Err("日志文件名无效".into());
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        // AppKit 在主线程触碰；面板结束并确认落盘后经 oneshot 回传真实结果。
        let outcome = (|| {
            let Some(mtm) = MainThreadMarker::new() else {
                return Err("日志导出未运行在主线程".to_string());
            };
            let Some(path) = choose_destination_on_main(mtm, &suggested_name) else {
                return Ok(false);
            };
            write_log_file(&path, &contents)?;
            Ok(true)
        })();
        let _ = sender.send(outcome);
    })
    .map_err(|error| format!("日志导出派发主线程失败：{error}"))?;
    receiver
        .await
        .map_err(|_| "日志导出意外中止".to_string())?
}
