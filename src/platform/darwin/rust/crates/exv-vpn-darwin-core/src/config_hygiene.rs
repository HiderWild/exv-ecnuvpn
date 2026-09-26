//! Darwin `config.json` 启动期分类与幂等修复（W3-3/P2b）。
//!
//! win32 `exv-vpn-win32-config/src/config_hygiene.rs` 的 darwin 对应：`load_for_startup`
//! 三段语义——缺失 → 默认 bootstrap 落盘；解析失败 / 非 JSON 对象 / 已知字段反序列化
//! 失败 → 默认 bootstrap 落盘；两者都置 `requires_quick_start = true`（`QuickStart`
//! 触发链的真实产码点）。合法 JSON 对象 → 已知字段用默认值 hydrate、未知字段原样保留
//! （`serde_json::Map::extend`），并原子写回（幂等、byte-stable），不触发 `QuickStart`。
//!
//! 显式用户提交的卫生边界（win32 `save_after_user_submission`）在 darwin 由
//! [`crate::DarwinUiConfig::apply_and_save`] 天然承担：typed 序列化只落已知字段，
//! 读取期保留的未知字段在提交成功后剥除（对应关系已在 `config.rs` 注释指认）。
//! 本模块只做纯 JSON 分类与修复，不触碰加密（`key.bin` 由既有 apply/persist 边界按需
//! 生成），路径与原子替换复用 `config_paths`。

use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    DarwinConfigError, DarwinUiConfig,
    config::DarwinStoredConfig,
    config_paths::{atomic_write, config_path},
};

/// 启动调用方可用的配置分类结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupConfig {
    /// 已 hydrate 全部已知字段的完整配置（绑定 `dir`）。
    pub(crate) config: DarwinUiConfig,
    /// 启动期是否以默认配置替换了缺失或不可用的 `config.json`（`QuickStart` 触发链）。
    pub(crate) requires_quick_start: bool,
}

/// 为应用启动加载、分类并修复配置。
///
/// 缺失、空白、畸形、非对象与已知 schema 非法的 JSON 被替换为完整默认配置并要求
/// Quick Start；每个合法 JSON 对象被已知默认值 hydrate 且保留未知字段，修复后的
/// 表示原子落盘（幂等）。
///
/// # Errors
///
/// `config.json` 存在但不可读取，或修复结果不可序列化 / 不可落盘时返回
/// [`DarwinConfigError::Storage`]（typed `Internal`）。
pub(crate) fn load_for_startup(dir: &Path) -> Result<StartupConfig, DarwinConfigError> {
    let path = config_path(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return bootstrap_default(dir);
        }
        Err(_) => return Err(DarwinConfigError::Storage),
    };

    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(&text) else {
        return bootstrap_default(dir);
    };
    let Ok(stored) = serde_json::from_value::<DarwinStoredConfig>(Value::Object(object.clone()))
    else {
        return bootstrap_default(dir);
    };

    let hydrated = hydrate_known_fields(object, &stored)?;
    let json = serde_json::to_string_pretty(&hydrated).map_err(|_| DarwinConfigError::Storage)?;
    atomic_write(&path, json.as_bytes()).map_err(|_| DarwinConfigError::Storage)?;
    Ok(StartupConfig {
        config: DarwinUiConfig::from_stored(stored, dir.to_path_buf()),
        requires_quick_start: false,
    })
}

/// 以默认配置 bootstrap：完整默认 `DarwinStoredConfig` 原子落盘 + 要求 `QuickStart`。
fn bootstrap_default(dir: &Path) -> Result<StartupConfig, DarwinConfigError> {
    let stored = DarwinStoredConfig::default();
    stored.save_to_dir_atomically(dir)?;
    Ok(StartupConfig {
        config: DarwinUiConfig::from_stored(stored, dir.to_path_buf()),
        requires_quick_start: true,
    })
}

/// 用已知字段的当前值覆盖同名键、补齐缺失键，未知键原样保留。
fn hydrate_known_fields(
    mut original: Map<String, Value>,
    stored: &DarwinStoredConfig,
) -> Result<Value, DarwinConfigError> {
    let Value::Object(known_fields) =
        serde_json::to_value(stored).map_err(|_| DarwinConfigError::Storage)?
    else {
        unreachable!("DarwinStoredConfig 总是序列化为 JSON 对象")
    };
    original.extend(known_fields);
    Ok(Value::Object(original))
}
