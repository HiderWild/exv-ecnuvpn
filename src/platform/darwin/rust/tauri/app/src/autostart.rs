//! 登录自启动（MAC-SHELL-17 S4；win32 `autostart.rs` 的 macOS 完全重写，计划 §2）。
//!
//! ## 选型（计划 §0.3.3）
//!
//! 自管 **`LaunchAgent` plist**：写/删/读
//! `~/Library/LaunchAgents/com.exv.vpn.exv-vpn-darwin.plist`（Label = 文件名 stem、
//! `ProgramArguments` = 当前 exe 绝对路径、`RunAtLoad` = true）。纯用户态家目录文件
//! 操作——无提权、无 TCC、无签名要求；`tauri-plugin-autostart`（`AppleScript` System
//! Events）在本宿主因 TCC 不可用（TASKBOARD 事实「System Events -1712」），不采用。
//! 只碰用户 `~/Library/LaunchAgents/`，绝不触碰系统级 `/Library/LaunchDaemons`。
//!
//! ## 幂等与自愈语义（win32 同源）
//!
//!   * `set(true)`：渲染 plist → 原子写（tmp + rename）→ `plutil -lint` 校验；
//!     重复 set 写入同一内容，无害。lint 失败（渲染器 bug 级）删除坏文件并报错，
//!     不给 launchd 留坏 plist。
//!   * `set(false)`：删除文件；文件不存在即目标状态已达成，无害。
//!   * `status`：文件存在且 Label 段等于本应用 label 才算 enabled（被第三方内容
//!     占用视为未启用，下次 set(true) 自愈覆盖）。
//!
//! ## 「执行真相源 = 系统注册、显示态 = 偏好文件」分工（win32 同源保留）
//!
//! 前端持久开关记录在 `ui-preferences.json` 的 `launch_at_login`（显示态）；本模块
//! 写入的 plist 是执行真相源。两者由前端保存分流（ui-prefs.ts
//! `updateUiPreferences`）经 `autostart_set` 同步写：先执行层后偏好文件，失败整体
//! 回滚。
//!
//! 不带命令行参数启动：`RunAtLoad` 只拉起 exe 本体，静默与否由 `silent_startup`
//! 偏好在运行时读取（S2 `should_start_silent` 的 argv ∨ prefs OR 语义，两处不漂移）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// `LaunchAgent` 的 label（= plist 文件名 stem，macOS 惯例二者一致）。
pub(crate) const LAUNCH_AGENT_LABEL: &str = "com.exv.vpn.exv-vpn-darwin";

// ---- 路径推导（测试注入 home；生产读 $HOME）----

/// 用户 home（`$HOME`）；不可解析时自启动状态不可用（Command 报错，不猜测路径）。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `<home>/Library/LaunchAgents`（路径形状的纯函数视图，测试直接注入 home）。
#[must_use]
pub(crate) fn launch_agents_dir_for_home(home: &Path) -> PathBuf {
    home.join("Library").join("LaunchAgents")
}

/// 本应用 `LaunchAgent` plist 的完整路径（纯函数视图）。
#[must_use]
pub(crate) fn plist_path_for_home(home: &Path) -> PathBuf {
    launch_agents_dir_for_home(home).join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

/// 生产 plist 路径。
///
/// # Errors
/// `$HOME` 不可解析。
fn plist_path() -> Result<PathBuf, String> {
    home_dir()
        .map(|home| plist_path_for_home(&home))
        .ok_or_else(|| "HOME environment variable is not set".to_owned())
}

// ---- plist 内容（纯函数，单测覆盖）----

/// XML 特殊字符转义（plist 值文本；exe 路径可能含 `&` 等字符）。
#[must_use]
fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' => escaped.push_str("&apos;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 渲染 `LaunchAgent` plist 内容（XML plist 1.0；`ProgramArguments` 数组形式而非
/// 单 `Program` 键，避免含空格路径被 launchd 按 shell 切分——官方推荐写法）。
#[must_use]
pub(crate) fn render_plist(label: &str, program: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
</dict>
</plist>
"#,
        label = xml_escape(label),
        program = xml_escape(program),
    )
}

// ---- 读 / 写 / 删（路径注入核心逻辑；测试驱动临时目录真实文件）----

/// plist 是否代表本应用的启用态：文件存在且 Label 段等于 `label`。
/// 读取失败（权限等）按错误上报，不静默当作未启用。
///
/// # Errors
/// 文件存在但不可读。
pub(crate) fn is_autostart_enabled_at(plist: &Path, label: &str) -> Result<bool, String> {
    let text = match std::fs::read_to_string(plist) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("read LaunchAgent plist failed: {error}")),
    };
    Ok(label_marker_line(&text) == Some(label))
}

/// 提取 plist 文本中 `Label` 键紧随的字符串值行（只认我们自己的渲染形状；本文件
/// 只由本模块写出，形状稳定）。
fn label_marker_line(text: &str) -> Option<&str> {
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line == "<key>Label</key>" {
            let value = lines.next()?;
            return value.strip_prefix("<string>")?.strip_suffix("</string>");
        }
    }
    None
}

/// 设置/清除登录自启动。`enabled=true` 写 plist（含 `plutil -lint` 校验）；
/// false 删除文件（不存在视为成功）。
///
/// # Errors
/// exe 路径不可解析、文件 IO 失败或 lint 校验失败。
pub(crate) fn set_autostart_at(plist: &Path, label: &str, enabled: bool) -> Result<(), String> {
    if !enabled {
        match std::fs::remove_file(plist) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("remove LaunchAgent plist failed: {error}")),
        }
    }

    let program = std::env::current_exe()
        .map_err(|e| format!("current_exe failed: {e}"))?
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "exe path is not valid UTF-8".to_owned())?;

    let content = render_plist(label, &program);
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create LaunchAgents dir failed: {e}"))?;
    }
    // 原子写（tmp + rename）：launchd 不会读到半截 plist。
    let tmp = plist.with_extension("plist.tmp");
    std::fs::write(&tmp, &content).map_err(|e| format!("write LaunchAgent plist failed: {e}"))?;
    if let Err(error) = std::fs::rename(&tmp, plist) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("rename LaunchAgent plist failed: {error}"));
    }

    // 内容自检：plutil 是 macOS 自带的 plist 校验器（纯文件解析，无 TCC）。
    // 失败说明渲染器产出非法 plist——删除坏文件并报错，不给 launchd 留坏状态。
    let lint = Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(plist)
        .output()
        .map_err(|e| format!("plutil -lint spawn failed: {e}"))?;
    if !lint.status.success() {
        let _ = std::fs::remove_file(plist);
        return Err(format!(
            "plutil -lint rejected the generated plist: {}",
            String::from_utf8_lossy(&lint.stderr).trim()
        ));
    }
    Ok(())
}

// ---- 生产入口 ----

/// 设置/清除登录自启动（生产路径：`$HOME/Library/LaunchAgents`）。
///
/// # Errors
/// `$HOME` 不可解析或 [`set_autostart_at`] 的全部错误面。
pub(crate) fn set_autostart(enabled: bool) -> Result<(), String> {
    set_autostart_at(&plist_path()?, LAUNCH_AGENT_LABEL, enabled)
}

/// 查询登录自启动的执行真相源状态（生产路径）。
///
/// # Errors
/// `$HOME` 不可解析或 [`is_autostart_enabled_at`] 的全部错误面。
pub(crate) fn is_autostart_enabled() -> Result<bool, String> {
    is_autostart_enabled_at(&plist_path()?, LAUNCH_AGENT_LABEL)
}

// ---- Commands ----

/// `autostart_set`：设置登录自启动并回报结果（ok = 执行真相源与目标一致）。
/// 前端 `launch_at_login` 保存分流调用（ui-prefs.ts `setAutostart`）。
#[allow(clippy::needless_pass_by_value)] // Tauri command 宏的固定签名形状
#[tauri::command]
pub(crate) fn autostart_set(enabled: bool) -> Result<serde_json::Value, String> {
    set_autostart(enabled)?;
    let now_enabled = is_autostart_enabled().unwrap_or(!enabled);
    Ok(serde_json::json!({ "ok": now_enabled == enabled }))
}

// ---- 测试 ----
//
// 真实文件操作落在 std::env::temp_dir() 下的测试专用目录（等价 HOME 形状
// `<tmp>/Library/LaunchAgents/`），用测试专用 label，结束后清理——不触碰产品
// `~/Library/LaunchAgents/`，更不触碰系统级 /Library/LaunchDaemons（计划 §S4）。
