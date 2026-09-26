//! 前端自有设置（UI preferences）的持久化与 Tauri Command。
//!
//! ## 边界切割（MAC-SHELL-17 S1；与 win32 `ui_prefs.rs` 同源）
//!
//! 设置分两域，互不越界：
//!   * **core 配置**（`server/username/password/remember_password/routes/user_agent/mtu`）：
//!     走既有 `config_get/config_set`（core 权威），本模块绝不触碰。
//!   * **前端自有设置**（纯 UI 行为偏好）：存本模块文件，修改**不走 core config**。
//!
//! 字段名与键集照抄 win32 宿主偏好契约（七键：`close_preference /
//! minimize_to_tray_on_connect / launch_at_login / silent_startup /
//! connection_state_notifications / auto_connect_on_launch / show_latency`），
//! 键名、默认值与非法值回落语义同源；保证跨平台语义连续。win32 前端的通知分事件键
//!（connect_notify 等）不在 win32 后端持久化面内，darwin 同样不落盘。
//!
//! ## 存储位置（MAC-SHELL-17 计划 §S1 拍板）
//!
//! `~/Library/Application Support/EXV/ui-preferences.json` —— macOS 用户态应用数据惯例
//! 目录；与 core 的连接配置分文件、分域（core 配置仍是 core 权威，本文件只承载壳/UI 行为）。
//! win32 版的一次性 legacy 导入（C++ 壳 `close-preference.json`）是 Windows 特有历史，
//! macOS 无 C++ 壳先例，不迁移。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::State;

/// 应用数据根子目录：`~/Library/Application Support/EXV`。
const APP_SUPPORT_ROOT: &str = "EXV";
/// 本模块持久化文件名。
const PREFS_FILE: &str = "ui-preferences.json";

/// 关闭按钮行为的合法值（与契约 `close_preference` 枚举一致）。
pub const CLOSE_PREFERENCE_VALUES: [&str; 3] = ["smart", "tray", "quit"];
/// `close_preference` 缺省值：智能最小化（误关保护宽限）。
pub const DEFAULT_CLOSE_PREFERENCE: &str = "smart";

// ---- wire 结构（snake_case 键照抄契约；全 Option 以区分「未设置」与默认值，
//      序列化时 None 跳过，读入时缺键回落默认——向前兼容旧文件。） ----

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct UiPreferences {
    /// 关闭按钮行为："smart" | "tray" | "quit"。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_preference: Option<String>,
    /// 连接成功后自动隐藏窗口到托盘。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimize_to_tray_on_connect: Option<bool>,
    /// 登录后自动运行（LaunchAgent 的持久开关记录；执行真相源由 S4 autostart 写）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_at_login: Option<bool>,
    /// 启动静默：所有启动方式都不弹窗，仅托盘驻留。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silent_startup: Option<bool>,
    /// 连接/断开时发系统通知。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_state_notifications: Option<bool>,
    /// 应用启动且空闲时自动发起连接。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_connect_on_launch: Option<bool>,
    /// 「安装服务后连接」意图（连接页 checkbox / 快速入门「安装服务」默认勾选）。
    ///
    /// **纯前端偏好**：只落偏好文件，无宿主副作用——绝不进入 `launch_at_login` 的
    /// `LaunchAgent` 执行分支。有效值默认 `true`（与前端 `DEFAULT_UI_PREFERENCES` 一致）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_service_on_connect: Option<bool>,
    /// 连接后显示实时时延（毫秒）；纯前端显示偏好，默认关闭。
    ///（win32 全量接入：键集对齐 win32 `app/src/ui_prefs.rs` 的 `show_latency`；
    /// 存储 path 维持 darwin 既有 `~/Library/Application Support/EXV`。）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_latency: Option<bool>,
}

impl UiPreferences {
    /// 读出时的有效值视图（None → 默认；非法 `close_preference` → 默认 smart）。
    fn effective(&self) -> Self {
        Self {
            close_preference: Some(
                self.close_preference
                    .as_deref()
                    .filter(|v| CLOSE_PREFERENCE_VALUES.contains(v))
                    .unwrap_or(DEFAULT_CLOSE_PREFERENCE)
                    .to_string(),
            ),
            minimize_to_tray_on_connect: Some(self.minimize_to_tray_on_connect.unwrap_or(false)),
            launch_at_login: Some(self.launch_at_login.unwrap_or(false)),
            silent_startup: Some(self.silent_startup.unwrap_or(false)),
            connection_state_notifications: Some(
                self.connection_state_notifications.unwrap_or(false),
            ),
            auto_connect_on_launch: Some(self.auto_connect_on_launch.unwrap_or(false)),
            // 默认勾选（R2/R3）：缺键即 true，与前端默认值一致。
            install_service_on_connect: Some(self.install_service_on_connect.unwrap_or(true)),
            show_latency: Some(self.show_latency.unwrap_or(false)),
        }
    }
}

// ---- 路径推导（测试可注入目录）----

/// 用户 home（`$HOME`）；解析失败时产品状态目录不可用（Command 报错，不猜测路径）。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 产品状态目录：`~/Library/Application Support/EXV`。
///
/// 测试经 [`ui_prefs_path_for`] 注入任意根目录；生产调用方用本函数。
#[must_use]
pub fn ui_state_dir() -> Option<PathBuf> {
    Some(ui_state_dir_for_home(&home_dir()?))
}

/// `<home>/Library/Application Support/EXV`（路径形状的纯函数视图，测试直接注入 home）。
#[must_use]
pub fn ui_state_dir_for_home(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join(APP_SUPPORT_ROOT)
}

/// `<dir>/ui-preferences.json`。
#[must_use]
pub fn ui_prefs_path_for(dir: &Path) -> PathBuf {
    dir.join(PREFS_FILE)
}

// ---- 读写 ----

/// 读磁盘上的原始 JSON 对象（坏 JSON / 不存在 → 空对象；未知键原样保留）。
fn load_raw_object(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        },
        Err(_) => serde_json::Map::new(),
    }
}

/// 原子写（tmp + rename），失败静默降级为直接写（rename 跨设备等场景兜底）。
fn store_atomic(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    if fs::rename(&tmp, path).is_err() {
        fs::copy(&tmp, path).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&tmp);
    }
    Ok(())
}

/// 原始对象 → 类型化视图（缺键 None）。
fn typed_view(map: &serde_json::Map<String, serde_json::Value>) -> UiPreferences {
    serde_json::from_value(serde_json::Value::Object(map.clone())).unwrap_or_default()
}

/// 加载有效偏好。
fn load_effective(dir: &Path) -> UiPreferences {
    let map = load_raw_object(&ui_prefs_path_for(dir));
    typed_view(&map).effective()
}

/// 合并补丁并持久化（只写补丁涉及的键；未知键保留），返回有效值视图。
fn apply_patch_and_store(dir: &Path, patch: &UiPreferences) -> Result<UiPreferences, String> {
    let prefs_path = ui_prefs_path_for(dir);
    let mut map = load_raw_object(&prefs_path);

    let patch_value = serde_json::to_value(patch).map_err(|e| e.to_string())?;
    if let serde_json::Value::Object(patch_map) = patch_value {
        for (key, value) in patch_map {
            map.insert(key, value);
        }
    }

    let effective = typed_view(&map).effective();
    store_atomic(&prefs_path, &serde_json::Value::Object(map))?;
    Ok(effective)
}

// ---- Tauri 托管状态（setup 时按真实目录构造；测试直接构造注入目录）----

/// 进程内共享的偏好存储（持锁串行化读写，避免并发写交错）。
pub struct UiPrefsStore {
    dir: PathBuf,
    inner: Mutex<()>,
}

impl UiPrefsStore {
    /// 生产构造：产品状态目录不可解析时不建 store（Command 返回错误）。
    #[must_use]
    pub fn from_product_dir() -> Option<Self> {
        Some(Self {
            dir: ui_state_dir()?,
            inner: Mutex::new(()),
        })
    }

    /// 读有效偏好。
    ///
    /// # Errors
    /// 内部锁中毒（理论不可达：锁内无 panic 点）。
    pub fn get(&self) -> Result<UiPreferences, String> {
        let _guard = self.inner.lock().map_err(|e| e.to_string())?;
        Ok(load_effective(&self.dir))
    }

    /// 合并补丁、持久化并返回有效偏好。
    ///
    /// # Errors
    /// 持久化 IO 失败或内部锁中毒。
    pub fn set(&self, patch: &UiPreferences) -> Result<UiPreferences, String> {
        let _guard = self.inner.lock().map_err(|e| e.to_string())?;
        apply_patch_and_store(&self.dir, patch)
    }
}

// ---- Commands ----

/// `ui_prefs_get`：读前端自有设置（有效值视图）。
// State 按值提取是 Tauri command 宏的固定形状（官方惯例），非本模块可选择的签名。
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn ui_prefs_get(store: State<'_, UiPrefsStore>) -> Result<UiPreferences, String> {
    store.get()
}

/// `ui_prefs_set`：合并补丁保存前端自有设置（不走 core config）。
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn ui_prefs_set(
    store: State<'_, UiPrefsStore>,
    patch: UiPreferences,
) -> Result<UiPreferences, String> {
    store.set(&patch)
}

// ---- 测试 ----
