//! 日志导出属于本地桌面文件操作，不经过 VPN 控制链。

use std::{io::Write, path::Path};
use windows::{
    core::{w, HSTRING},
    Win32::{
        Foundation::{ERROR_CANCELLED, HWND},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        UI::Shell::{
            FileSaveDialog, IFileSaveDialog, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR,
            FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
        },
    },
};

struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: 仅在本线程 CoInitializeEx 成功之后创建，所有 COM 对象先于它释放。
        unsafe { CoUninitialize() };
    }
}

fn choose_destination(
    owner: isize,
    suggested_name: &str,
) -> windows::core::Result<Option<std::path::PathBuf>> {
    // SAFETY: 此函数只在导出专用线程运行；没有跨线程传递 COM 接口。
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _apartment = ComApartment;
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetTitle(w!("导出日志"))?;
        dialog.SetFileName(&HSTRING::from(suggested_name))?;
        dialog.SetDefaultExtension(w!("jsonl"))?;
        dialog.SetOptions(
            dialog.GetOptions()?
                | FOS_FORCEFILESYSTEM
                | FOS_PATHMUSTEXIST
                | FOS_OVERWRITEPROMPT
                | FOS_NOCHANGEDIR,
        )?;
        if let Err(error) = dialog.Show(Some(HWND(owner as *mut _))) {
            if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(error);
        }
        let item = dialog.GetResult()?;
        let raw_path = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = raw_path.to_string();
        CoTaskMemFree(Some(raw_path.0.cast()));
        Ok(Some(std::path::PathBuf::from(path?)))
    }
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
    window: tauri::WebviewWindow,
    suggested_name: String,
    contents: String,
) -> Result<bool, String> {
    if suggested_name.is_empty() || suggested_name.contains(['/', '\\', '\0']) {
        return Err("日志文件名无效".into());
    }
    let owner = window
        .hwnd()
        .map_err(|error| format!("无法获取日志窗口：{error}"))?
        .0 as isize;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("exv-log-export".into())
        .spawn(move || {
            let result = (|| {
                let Some(path) = choose_destination(owner, &suggested_name)
                    .map_err(|error| format!("无法打开日志保存窗口：{error}"))?
                else {
                    return Ok(false);
                };
                write_log_file(&path, &contents)?;
                Ok(true)
            })();
            let _ = sender.send(result);
        })
        .map_err(|error| format!("无法启动日志导出：{error}"))?;
    receiver
        .await
        .map_err(|error| format!("日志导出意外中止：{error}"))?
}
