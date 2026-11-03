//! L4 `aTrust://` URL 协议注册（用户级 `HKCU\Software\Classes\aTrust`，冲突让位）。
//!
//! 目的（S4 伪装表层）：门户/网页可用 `aTrust://browserStart` 一类链接唤起「客户端」；
//! 官方客户端的注册形态（本机实测，2026-10-05）：
//!
//! ```text
//! HKCR\aTrust                      默认值 = "aTrust"，"URL Protocol" = "aTrust"
//! HKCR\aTrust\DefaultIcon          默认值 = …\aTrustTray\aTrustTray.exe
//! HKCR\aTrust\Shell\Open\command   默认值 = "…\aTrustTray.exe" "-- -s browserstart"
//! ```
//!
//! （本机该注册位于 `HKLM\SOFTWARE\Classes\aTrust`，机器级。）
//!
//! 冲突守卫（计划 §六 风险 5）：**只在无人认领时注册**——
//! * 官方客户端进程在运行（复用 L3 探针名单，含 `aTrustXtunnel.exe`）→ 让位；
//! * `HKCR\aTrust\Shell\Open\command` 已被任何非本应用的 exe 认领（官方安装或其他程序）
//!   → 让位，绝不抢占；读取失败（状态未知）同样不注册；
//! * 已由本应用认领（命令指向的 exe 与当前 exe 同名，容错升级/换目录）→ 幂等刷新。
//!
//! 只写**当前用户** `HKCU\Software\Classes`（无提权），卸载可整体回退（卸载器
//! `RemoveUrlSchemeRegistration` + 运行时 [`unregister_best_effort`] 双通道）。
//!
//! 激活行为：注册命令为 `"<本应用 exe>" "%1"`，Windows 会以 URL 为参数启动/激活本应用；
//! 应用侧最小处理 = 唤出主窗口（第二实例经单实例回调 [`is_scheme_activation`] 识别并
//! 记录，冷启动经默认可见性逻辑弹窗）——不自动发起连接，不伪造官方 `-s browserstart`
//! 的内部语义，如实保持「打开 EXV 主界面」。

use exv_vpn_win32_agent_sim::HostProbe;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE, REG_SZ,
};

/// 目标 URL 协议名。
pub const SCHEME: &str = "aTrust";

/// 真实注册决策的处置动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemeDecision {
    /// 无人认领 → 注册我们的用户级协议。
    Register,
    /// 已是本应用认领（exe 同名）→ 幂等刷新（容错升级换路径）。
    RefreshOurs,
    /// 官方客户端进程在运行 → 让位（不注册、不抢占）。
    SkipOfficialProcess(Vec<String>),
    /// 已被其他程序（含官方安装）认领 → 让位；`official` 标记命令是否像官方客户端。
    SkipClaimedByOther { command: String, official: bool },
}

/// argv 中是否携带 `aTrust://` 激活 URL（单实例回调识别协议激活用，大小写不敏感）。
#[must_use]
pub fn is_scheme_activation(argv: &[String]) -> bool {
    let prefix = format!("{SCHEME}://").to_ascii_lowercase();
    argv.iter().any(|arg| arg.to_ascii_lowercase().starts_with(&prefix))
}

/// 纯决策：给定当前认领命令、本应用 exe 与官方进程名单，决定注册处置。
///
/// 顺序：官方进程优先（运行中一律让位）；其次看认领——无人认领才注册；
/// 他人认领（无论官方还是第三方）一律让位。
#[must_use]
pub fn decide(
    claimed_command: Option<&str>,
    our_exe: &str,
    official_processes: &[String],
) -> SchemeDecision {
    if !official_processes.is_empty() {
        return SchemeDecision::SkipOfficialProcess(official_processes.to_vec());
    }
    match claimed_command.filter(|command| !command.trim().is_empty()) {
        None => SchemeDecision::Register,
        Some(command) => {
            if command_invokes_our_exe(command, our_exe) {
                SchemeDecision::RefreshOurs
            } else {
                SchemeDecision::SkipClaimedByOther {
                    command: command.to_owned(),
                    official: looks_like_official_client(command),
                }
            }
        }
    }
}

/// 认领命令的目标 exe 是否与本应用同名（Windows 路径大小写不敏感）。
///
/// 只比对**文件名**而非全路径：本应用升级/换目录（debug → 安装版）后命令路径会变，
/// 同名（`exv-ui.exe`）即视为同一产品认领，允许刷新；官方 `aTrustTray.exe` 天然不同名。
fn command_invokes_our_exe(command: &str, our_exe: &str) -> bool {
    let Some(target) = first_command_token(command) else {
        return false;
    };
    let (Some(target_name), Some(our_name)) = (exe_file_name(&target), exe_file_name(our_exe))
    else {
        return false;
    };
    target_name == our_name
}

/// 取命令串的首个 token：优先带引号的路径，其次空白分隔首段。
fn first_command_token(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        rest.split_once('"').map(|(token, _)| token.to_owned())
    } else {
        trimmed.split_whitespace().next().map(str::to_owned)
    }
}

/// 路径的文件名（小写，便于大小写不敏感比较）。
fn exe_file_name(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
}

/// 认领命令是否像官方客户端（官方安装路径含 `Sangfor`，仅用于日志标注）。
fn looks_like_official_client(command: &str) -> bool {
    command.to_ascii_lowercase().contains("sangfor")
}

fn current_exe_path() -> Result<String, String> {
    std::env::current_exe()
        .map_err(|e| format!("current_exe failed: {e}"))?
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "exe path is not valid UTF-8".to_owned())
}

// ---- 注册表 I/O（仅 HKCU\Software\Classes；读取经 HKCR 合并视图看有效认领）----

fn wide(value: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    wide.push(0);
    wide
}

/// 读取 `HKCR\<scheme>\Shell\Open\command` 默认值（合并视图 = 当前有效认领）。
fn read_claim_at(scheme: &str) -> Result<Option<String>, String> {
    let subkey = wide(&format!("{scheme}\\Shell\\Open\\command"));
    let mut hkey = HKEY::default();
    // 安全注释：只读查询 HKCR，不写任何键。
    let status = unsafe {
        RegOpenKeyExW(HKEY_CLASSES_ROOT, PCWSTR::from_raw(subkey.as_ptr()), Some(0), KEY_QUERY_VALUE, &mut hkey)
    };
    match status {
        ERROR_SUCCESS => {}
        WIN32_ERROR(2) | WIN32_ERROR(3) => return Ok(None), // 键/路径不存在 = 无人认领
        other => return Err(format!("RegOpenKeyExW(HKCR {scheme}) failed: {}", other.0)),
    }
    let mut kind = REG_VALUE_TYPE::default();
    let mut buffer = [0u16; 1024];
    let mut bytes = (buffer.len() * std::mem::size_of::<u16>()) as u32;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR::null(), // 默认值
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr().cast::<u8>()),
            Some(&mut bytes),
        )
    };
    let _ = unsafe { RegCloseKey(hkey) };
    match status {
        ERROR_SUCCESS if kind == REG_SZ => {
            let length = (bytes as usize) / std::mem::size_of::<u16>();
            let value = String::from_utf16_lossy(&buffer[..length])
                .trim_end_matches('\0')
                .trim()
                .to_owned();
            Ok((!value.is_empty()).then_some(value))
        }
        WIN32_ERROR(2) => Ok(None),
        other => Err(format!("RegQueryValueExW(HKCR {scheme} command) failed: {}", other.0)),
    }
}

fn set_default_string(hkey: HKEY, value: &str) -> Result<(), String> {
    let wide_value = wide(value);
    let bytes = wide_value.len() * std::mem::size_of::<u16>();
    // 安全注释：REG_SZ 值为含结尾 NUL 的 UTF-16。
    let status = unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR::null(),
            Some(0),
            REG_SZ,
            Some(std::slice::from_raw_parts(wide_value.as_ptr().cast::<u8>(), bytes)),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("RegSetValueExW failed: {}", status.0));
    }
    Ok(())
}

fn create_and_set_default(subkey: &str, value: &str) -> Result<(), String> {
    let hkey = open_or_create(subkey)?;
    let result = set_default_string(hkey, value);
    let _ = unsafe { RegCloseKey(hkey) };
    result
}

/// 打开/创建 `HKCU\<subkey>`（返回的句柄由调用方关闭）。
fn open_or_create(subkey: &str) -> Result<HKEY, String> {
    let wide_subkey = wide(subkey);
    let mut hkey = HKEY::default();
    // 安全注释：只创建/打开本应用自己的 HKCU\Software\Classes 子键，用户权限即可。
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide_subkey.as_ptr()),
            Some(0),
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("RegCreateKeyExW({subkey}) failed: {}", status.0));
    }
    Ok(hkey)
}

/// 写入用户级注册（镜像官方键形：默认值/`URL Protocol`/DefaultIcon/Shell\Open\command）。
///
/// 命令模板 `"<exe>" "%1"`：Windows 激活 `aTrust://…` 链接时以 URL 为参数启动本应用。
fn write_registration_at(scheme: &str, exe: &str) -> Result<(), String> {
    let base = format!("Software\\Classes\\{scheme}");
    create_and_set_default(&base, scheme)?;
    // “URL Protocol” 是 base 键上的命名值（值名存在即声明 URL 协议；官方把值内容也
    // 写成协议名，跟随）。
    let hkey = open_or_create(&base)?;
    let name = wide("URL Protocol");
    let value = wide(scheme);
    let bytes = value.len() * std::mem::size_of::<u16>();
    let status = unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR::from_raw(name.as_ptr()),
            Some(0),
            REG_SZ,
            Some(std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), bytes)),
        )
    };
    let _ = unsafe { RegCloseKey(hkey) };
    if status != ERROR_SUCCESS {
        return Err(format!("RegSetValueExW(URL Protocol) failed: {}", status.0));
    }
    create_and_set_default(&format!("{base}\\DefaultIcon"), exe)?;
    create_and_set_default(
        &format!("{base}\\Shell\\Open\\command"),
        &format!("\"{exe}\" \"%1\""),
    )
}

/// 删除用户级注册树；返回键是否原本存在。
fn delete_registration_at(scheme: &str) -> Result<bool, String> {
    let subkey = wide(&format!("Software\\Classes\\{scheme}"));
    // 安全注释：只删除 HKCU 下本模块创建的键树，不触碰 HKLM/官方注册。
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR::from_raw(subkey.as_ptr())) };
    match status {
        ERROR_SUCCESS => Ok(true),
        WIN32_ERROR(2) => Ok(false),
        other => Err(format!("RegDeleteTreeW({scheme}) failed: {}", other.0)),
    }
}

// ---- 编排 ----

/// 启动时执行：按冲突守卫决定是否注册 `aTrust://`，结果只记录不阻断应用启动。
pub fn ensure_registered() {
    let our_exe = match current_exe_path() {
        Ok(exe) => exe,
        Err(error) => {
            tracing::warn!(target: "exv.atrust_scheme", %error, "无法解析本应用 exe 路径，跳过 aTrust:// 注册");
            return;
        }
    };
    let official_processes =
        exv_vpn_win32_agent_sim::SystemProbe.running_official_client_processes();
    let claimed = match read_claim_at(SCHEME) {
        Ok(claimed) => claimed,
        Err(error) => {
            // 认领状态未知 ≠ 无人认领：按让位处理，避免遮蔽既有注册。
            tracing::warn!(target: "exv.atrust_scheme", %error, "无法读取 aTrust:// 认领状态，跳过注册（不抢占）");
            return;
        }
    };
    match decide(claimed.as_deref(), &our_exe, &official_processes) {
        SchemeDecision::Register => match write_registration_at(SCHEME, &our_exe) {
            Ok(()) => tracing::info!(target: "exv.atrust_scheme", scheme = SCHEME, "aTrust:// 协议已注册（此前无人认领）"),
            Err(error) => tracing::warn!(target: "exv.atrust_scheme", %error, "aTrust:// 注册失败（不影响其余启动）"),
        },
        SchemeDecision::RefreshOurs => match write_registration_at(SCHEME, &our_exe) {
            Ok(()) => tracing::info!(target: "exv.atrust_scheme", scheme = SCHEME, "aTrust:// 协议认领仍是本应用，已幂等刷新"),
            Err(error) => tracing::warn!(target: "exv.atrust_scheme", %error, "aTrust:// 刷新失败（不影响其余启动）"),
        },
        SchemeDecision::SkipOfficialProcess(processes) => {
            tracing::info!(target: "exv.atrust_scheme", processes = ?processes, "官方客户端进程在运行，aTrust:// 让位（不注册）");
        }
        SchemeDecision::SkipClaimedByOther { command, official } => {
            tracing::info!(target: "exv.atrust_scheme", command, official, "aTrust:// 已被其他程序认领，让位（不注册、不抢占）");
        }
    }
}

/// best-effort 运行时回退：仅当 `aTrust://` 认领指向本应用 exe 时删除用户级注册树。
///
/// 返回是否实际删除；他人认领（含官方）一律不动。
///
/// # Errors
/// exe 路径不可解析、认领读取或注册树删除失败。
pub fn unregister_best_effort() -> Result<bool, String> {
    let our_exe = current_exe_path()?;
    match read_claim_at(SCHEME)? {
        Some(command) if command_invokes_our_exe(&command, &our_exe) => {
            delete_registration_at(SCHEME)
        }
        _ => Ok(false),
    }
}

/// `atrust_scheme_unregister`：显式回退本应用的 `aTrust://` 用户级注册（返回是否删除）。
#[tauri::command]
pub fn atrust_scheme_unregister() -> Result<bool, String> {
    unregister_best_effort()
}
