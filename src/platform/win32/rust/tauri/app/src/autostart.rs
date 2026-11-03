//! 开机自动运行（HKCU `Run` key）。
//!
//! 写当前用户侧注册表（无提权）：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! 值名 `EXV VPN` = 带引号的当前 exe 绝对路径；兼容清除旧 `EXV VPN Client`。不带命令行参数——静默与否由 ui_prefs 的
//! `silent_startup` 决定（运行时读取），避免 Run key 与偏好文件两处漂移。
//!
//! 前端持久开关记录在 `ui-preferences.json` 的 `launch_at_login`（显示态）；
//! 注册表是执行真相源，两者由前端 toggle 同步写。

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE,
};

const RUN_SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
pub const RUN_VALUE_NAME: PCWSTR = w!("EXV VPN");
// ae21a4e9: src/core/use_cases/config_use_cases.cpp::apply_launch_at_login。
const LEGACY_RUN_VALUE_NAME: PCWSTR = w!("EXV VPN Client");

/// 仅缺失表示目标不存在；权限等真实失败必须向调用方报告。
fn present(status: WIN32_ERROR, operation: &str) -> Result<bool, String> {
    match status {
        ERROR_SUCCESS => Ok(true),
        ERROR_FILE_NOT_FOUND => Ok(false),
        other => Err(format!("{operation} failed: {}", other.0)),
    }
}

struct RunKey(HKEY);
impl Drop for RunKey {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open_run_key(write: bool, create: bool) -> Result<Option<RunKey>, String> {
    let mut hkey = HKEY::default();
    let access = if write {
        KEY_SET_VALUE | KEY_QUERY_VALUE
    } else {
        KEY_QUERY_VALUE
    };
    let status = unsafe {
        if create {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN_SUBKEY,
                Some(0),
                None,
                REG_OPTION_NON_VOLATILE,
                access,
                None,
                &mut hkey,
                None,
            )
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, Some(0), access, &mut hkey)
        }
    };
    if present(status, "open HKCU Run")? {
        Ok(Some(RunKey(hkey)))
    } else {
        Ok(None)
    }
}

fn current_exe_path() -> Result<String, String> {
    std::env::current_exe()
        .map_err(|e| format!("current_exe failed: {e}"))?
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "exe path is not valid UTF-8".to_string())
}

fn write_command(key: &RunKey, name: PCWSTR, command: &str) -> Result<(), String> {
    let wide: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name,
            Some(0),
            REG_SZ,
            Some(std::slice::from_raw_parts(
                wide.as_ptr().cast::<u8>(),
                wide.len() * 2,
            )),
        )
    };
    present(status, "write EXV Run value")?
        .then_some(())
        .ok_or_else(|| "write EXV Run value unexpectedly missing".to_string())
}

fn remove_value(key: &RunKey, name: PCWSTR) -> Result<(), String> {
    present(
        unsafe { RegDeleteValueW(key.0, name) },
        "delete EXV Run value",
    )?;
    Ok(())
}

fn value_enabled(key: &RunKey, name: PCWSTR) -> Result<bool, String> {
    let mut kind = REG_VALUE_TYPE::default();
    let mut bytes = 0u32;
    let status =
        unsafe { RegQueryValueExW(key.0, name, None, Some(&mut kind), None, Some(&mut bytes)) };
    if !present(status, "query EXV Run value")? {
        return Ok(false);
    }
    // 两版写入均为 REG_SZ；仅终止符的空字符串不能代表有效命令。
    Ok(kind == REG_SZ && bytes > 2)
}

fn is_enabled_with_names(names: [PCWSTR; 2]) -> Result<bool, String> {
    let Some(key) = open_run_key(false, false)? else {
        return Ok(false);
    };
    let current = value_enabled(&key, names[0])?;
    let legacy = value_enabled(&key, names[1])?;
    Ok(current || legacy)
}

fn set_with_names(names: [PCWSTR; 2], enabled: bool) -> Result<(), String> {
    let Some(key) = open_run_key(true, enabled)? else {
        return Ok(());
    };
    if enabled {
        // Run 数据是命令行；为包含空格的 exe 路径加引号，不附带参数。
        write_command(&key, names[0], &format!("\"{}\"", current_exe_path()?))?;
        remove_value(&key, names[1])?;
    } else {
        remove_value(&key, names[0])?;
        remove_value(&key, names[1])?;
    }
    drop(key);
    if is_enabled_with_names(names)? != enabled {
        return Err("EXV 登录自启注册与请求状态不一致".to_string());
    }
    Ok(())
}

/// 只控制现役及有历史源码证据的 EXV 注册；不改变 Engine 服务启动类型。
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    set_with_names([RUN_VALUE_NAME, LEGACY_RUN_VALUE_NAME], enabled)
}

pub fn is_autostart_enabled() -> Result<bool, String> {
    is_enabled_with_names([RUN_VALUE_NAME, LEGACY_RUN_VALUE_NAME])
}

#[tauri::command]
pub fn autostart_set(enabled: bool) -> Result<serde_json::Value, String> {
    set_autostart(enabled)?;
    Ok(serde_json::json!({ "ok": true }))
}
