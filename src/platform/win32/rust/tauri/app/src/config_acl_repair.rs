

use std::path::Path;

use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOCLOSEPROCESS, ShellExecuteExW, SHELLEXECUTEINFOW,
};

/// icacls（System32 内置 ACL 工具；提权承载）。
const ICACLS_EXE: &str = "icacls.exe";
/// UAC 确认 + icacls 执行的最坏等待上界。
const ELEVATED_REPAIR_TIMEOUT_MS: u32 = 120_000;

/// 构造 icacls 参数：对 `config_path` 整条授予三受托人完全控制。
///
/// 与 core 第二档自修（config_hygiene `rebuild_dacl`）同一 ACE 集；SID 用
/// `*S-1-…` 数字形式（`*` 前缀告知 icacls 按 SID 而非账户名解析），路径加引号
/// 防空格拆词。纯函数（可测）。
fn icacls_arguments(config_path: &Path, user_sid: &str) -> String {
    format!(
        "\"{}\" /grant \"*{user_sid}:F\" \"*S-1-5-18:F\" \"*S-1-5-32-544:F\"",
        config_path.display()
    )
}

/// runas 提权执行 icacls 重置 `config_path` 的 ACL，等待其退出（有界）。
///
/// # Errors
/// - 当前用户 SID 不可解析（无法构造受托人）；
/// - 用户取消 UAC（ShellExecuteExW ERROR_CANCELLED）；
/// - 提权启动失败 / 等待超时 / icacls 非零退出（ACL 未修复，前端保持错误态）。
fn run_elevated_acl_repair(config_path: &Path) -> Result<(), String> {
    let user_sid = crate::kernel::core_transport::current_user_sid()
        .ok_or_else(|| "无法解析当前用户 SID（不能构造修复受托人）".to_string())?;
    let arguments = icacls_arguments(config_path, &user_sid);
    let verb = windows::core::HSTRING::from("runas");
    let file = windows::core::HSTRING::from(ICACLS_EXE);
    let parameters = windows::core::HSTRING::from(&arguments);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>())
            .map_err(|_| "SHELLEXECUTEINFOW 尺寸异常".to_string())?,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: 0, // SW_HIDE：icacls 无 UI，避免闪控制台
        ..SHELLEXECUTEINFOW::default()
    };
    // SAFETY: info 是本函数栈上的活结构（cbSize 已按实际尺寸初始化）；失败时
    // Result 携带真实 Win32 错误码（UAC 取消 = HRESULT_FROM_WIN32(ERROR_CANCELLED)）。
    if let Err(code) = unsafe { ShellExecuteExW(&raw mut info) } {
        let hresult = code.code();
        return Err(if hresult == HRESULT::from_win32(ERROR_CANCELLED.0) {
            "已取消管理员授权；配置文件权限未修复".to_string()
        } else {
            format!("管理员授权启动失败（Windows 错误 {hresult}）")
        });
    }
    if info.hProcess.is_invalid() {
        return Err("管理员授权未返回修复进程".to_string());
    }
    // SAFETY: hProcess 由 ShellExecuteExW(NOCLOSEPROCESS) 产出，归本函数所有。
    let wait = unsafe { WaitForSingleObject(info.hProcess, ELEVATED_REPAIR_TIMEOUT_MS) };
    let mut exit_code = 0u32;
    // SAFETY: exit_code 是活输出参数；hProcess 仍有效。
    let _ = unsafe { GetExitCodeProcess(info.hProcess, &raw mut exit_code) };
    // SAFETY: hProcess 用后关闭（无论等待结果如何都释放句柄）。
    unsafe {
        let _ = CloseHandle(info.hProcess);
    }
    match wait {
        WAIT_OBJECT_0 if exit_code == 0 => Ok(()),
        WAIT_OBJECT_0 => Err(format!("权限修复执行失败（icacls 退出码 {exit_code}）")),
        _ => Err("权限修复等待超时".to_string()),
    }
}

/// `config_repair_permissions`：前端「以管理员身份修复权限」按钮入口。
///
/// 目标固定为 core 的 `config_dir()/config.json`（与 core 读路径同一真相源）。
/// 成功返回后由**前端**重发 `config_get` 重试读取——本命令只修 ACL，不读配置。
#[tauri::command]
pub async fn config_repair_permissions() -> Result<(), String> {
    let path =
        exv_vpn_win32_config::config_path(&exv_vpn_win32_config::config_dir());
    tokio::task::spawn_blocking(move || run_elevated_acl_repair(&path))
        .await
        .map_err(|e| format!("权限修复任务失败：{e}"))?
}
