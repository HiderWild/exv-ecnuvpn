//! 快速入门「已见」标记（win32 `quick_start_marker.rs` 的 darwin 移植，KI-13 语义）。
//!
//! ## 语义（用户铁律）
//!
//! 快速入门**有且仅有首次运行弹出**：干净安装（无历史数据）首运行必须弹；用户完成
//! 或跳过之后绝不再弹第二次；卸载必须清除标记，保证重装 = 再次首运行。
//!
//! ## 存储位置（与 core config.json 同目录强耦合）
//!
//! `$HOME/.exv/quick-start.json`——与 core 配置 `config.json` 同目录。理由（照 win32
//! KI-13 的事故教训）：
//!
//! 1. **清理耦合**：卸载清理（`pre_uninstall`）以 `~/.exv` 整树为单位（config.json/
//!    key.bin/会话缓存/本标记），标记与「隐式首运行信号」config.json 同生共死，不存在
//!    只清其一的拆分态（那会让重装出现「无配置却已见过」或「有配置却未见过」的脑裂）。
//! 2. **不进 `~/Library/Application Support/EXV`**：该目录（ui-preferences.json 所在）
//!    与 config.json 分属两个清理目标；标记放那里会重新引入「标记幸存而配置已清 →
//!    重装不弹」的事故形态。
//! 3. **路径推导纪律**：复用Core的配置目录入口，保留EXV_CONFIG_DIR、USERPROFILE、HOME
//!    的既有优先级；只取路径，不读取或改写正式配置。卸载仍遵原有删除路径限制。
//!
//! ## 文件契约（单一键）
//!
//! ```json
//! { "quick_start_seen": true }
//! ```
//!
//! - 无文件 / 键缺失 / 值非 true / JSON 损坏 → 视为未见过（诚实降级，交由判定层
//!   结合 core 的 `requires_quick_start` 决定）；
//! - `quick_start_seen == true` → 用户已见过（完成或跳过），永不再弹。
//!
//! ## 写入时机（判定层 `App.vue` + `product/quick-start.ts`）
//!
//! - 用户点击「暂时跳过」→ 立即写标记；
//! - 快速入门完成（core 配置保存 + 运行时刷新成功）→ 写标记；
//! - 启动迁移播种：已有合法配置（`requires_quick_start == false`）但无标记
//!   （历史安装从未写过标记）→ 补写标记，避免升级后二次弹出。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;
use tauri::State;

/// 标记文件名（路径契约：卸载清理必须连同 `~/.exv` 整树覆盖本文件）。
pub(crate) const MARKER_FILE: &str = "quick-start.json";
/// 标记文件内的唯一键。
pub(crate) const MARKER_KEY: &str = "quick_start_seen";

/// `<dir>/quick-start.json`。
#[must_use]
pub(crate) fn marker_path_for(dir: &Path) -> PathBuf {
    dir.join(MARKER_FILE)
}

/// 读标记（坏文件一律视为未见过；不修复、不删除，留给写入时机覆盖）。
fn is_seen_at(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    parsed.get(MARKER_KEY) == Some(&Value::Bool(true))
}

/// 原子写（tmp + rename），失败静默降级为直接写（与 ui_prefs 同策略）。
fn store_marker(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = format!("{{\n  \"{MARKER_KEY}\": true\n}}\n");
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    if fs::rename(&tmp, path).is_err() {
        fs::copy(&tmp, path).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&tmp);
    }
    Ok(())
}

// ---- Tauri 托管状态（setup 时按真实 config 目录构造；测试注入路径） ----

/// 进程内共享的标记存储（持锁串行化，避免读写交错）。
pub(crate) struct QuickStartMarkerStore {
    path: PathBuf,
    inner: Mutex<()>,
}

impl QuickStartMarkerStore {
    /// 生产构造：与Core配置相同目录，Default只组装路径、不读写配置。
    ///
    /// 保留Core的既有环境覆盖和缺省相对目录，不另建路径解析规则。
    #[must_use]
    pub(crate) fn from_product_dir() -> Option<Self> {
        let config = exv_vpn_darwin_core::DarwinUiConfig::default();
        let dir = config.config_dir();
        Some(Self {
            path: marker_path_for(dir),
            inner: Mutex::new(()),
        })
    }

    /// 用户是否已见过快速入门（完成或跳过）。
    ///
    /// # Errors
    /// 内部锁中毒（理论不可达：锁内无 panic 点）。
    pub(crate) fn seen(&self) -> Result<bool, String> {
        let _guard = self.inner.lock().map_err(|e| e.to_string())?;
        Ok(is_seen_at(&self.path))
    }

    /// 写入「已见」标记（幂等：重复写结果不变）。
    ///
    /// # Errors
    /// 持久化 IO 失败或内部锁中毒。
    pub(crate) fn mark_seen(&self) -> Result<(), String> {
        let _guard = self.inner.lock().map_err(|e| e.to_string())?;
        store_marker(&self.path)
    }
}

// ---- Commands ----

/// `quick_start_seen_get`：读「已见」标记（外壳外/后端不可用由前端诚实降级为 false）。
#[tauri::command]
pub(crate) fn quick_start_seen_get(
    store: State<'_, QuickStartMarkerStore>,
) -> Result<bool, String> {
    store.seen()
}

/// `quick_start_seen_mark`：写「已见」标记（跳过/完成/迁移播种三处时机）。
#[tauri::command]
pub(crate) fn quick_start_seen_mark(
    store: State<'_, QuickStartMarkerStore>,
) -> Result<(), String> {
    store.mark_seen()
}

// ---- 测试 ----
