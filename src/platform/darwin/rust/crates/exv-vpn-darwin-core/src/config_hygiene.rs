//! Darwin `config.json` 启动期分类与幂等修复（W3-3/P2b）。
//!
//! 缺失配置沿新装默认落盘；可读但损坏的配置先完整备份原字节，再原子修复。
//! 不可读配置返回结构化失败，不覆盖磁盘或内存。合法 JSON 对象 hydrate 已知字段，
//! 保留未知字段并幂等原子写回。恢复报告仅由发生修复的那次调用返回。
//!
//! 显式用户提交的卫生边界（win32 `save_after_user_submission`）在 darwin 由
//! [`crate::DarwinUiConfig::apply_and_save`] 天然承担：typed 序列化只落已知字段，
//! 读取期保留的未知字段在提交成功后剥除（对应关系已在 `config.rs` 注释指认）。
//! 本模块只做纯 JSON 分类与修复，不触碰加密（`key.bin` 由既有 apply/persist 边界按需
//! 生成），路径与原子替换复用 `config_paths`。

use std::{
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value};

use crate::{
    DarwinConfigError, DarwinUiConfig,
    config::DarwinStoredConfig,
    config_paths::{atomic_write, config_path},
};

/// 本次损坏修复的恢复资料；仅含同目录备份文件名，不携带原文或完整路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigRecovery {
    pub(crate) corrupted: bool,
    pub(crate) backup_file: String,
}

/// 启动调用方可用的配置分类结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupConfig {
    /// 已 hydrate 全部已知字段的完整配置（绑定 `dir`）。
    pub(crate) config: DarwinUiConfig,
    /// 启动期是否以默认配置替换了缺失或不可用的 `config.json`（`QuickStart` 触发链）。
    pub(crate) requires_quick_start: bool,
    pub(crate) config_recovery: Option<ConfigRecovery>,
}

/// 为应用启动加载、分类并修复配置。
///
/// 缺失配置直接创建；空白、畸形、非对象与已知 schema 非法的 JSON 先备份再默认修复并要求
/// Quick Start；每个合法 JSON 对象被已知默认值 hydrate 且保留未知字段，修复后的
/// 表示原子落盘（幂等）。
///
/// # Errors
///
/// 不可读取返回 [`DarwinConfigError::ReadFailed`]；备份、序列化或落盘失败返回
/// [`DarwinConfigError::Storage`]。备份完成之前不替换原配置。
pub(crate) fn load_for_startup(dir: &Path) -> Result<StartupConfig, DarwinConfigError> {
    let path = config_path(dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return bootstrap_default(dir);
        }
        Err(error) => {
            return Err(DarwinConfigError::ReadFailed {
                path,
                raw_os_error: error.raw_os_error(),
            });
        }
    };

    let Ok(Value::Object(mut object)) = serde_json::from_slice::<Value>(&bytes) else {
        return repair_corrupt_config(dir, &bytes);
    };
    crate::config::normalize_atrust_auth_fields(&mut object);
    let Ok(mut stored) = serde_json::from_value::<DarwinStoredConfig>(Value::Object(object.clone()))
    else {
        return repair_corrupt_config(dir, &bytes);
    };
    // S5a：旧配置缺 password_owner_username 键时按 owner=username 迁移（与
    // `DarwinStoredConfig::load_from_dir` 同一条迁移；hydrate 写回让迁移落盘）。
    stored.reconcile_password_owner();

    let hydrated = hydrate_known_fields(object, &stored)?;
    let json = serde_json::to_string_pretty(&hydrated).map_err(|_| DarwinConfigError::Storage)?;
    atomic_write(&path, json.as_bytes()).map_err(|_| DarwinConfigError::Storage)?;
    Ok(StartupConfig {
        config: DarwinUiConfig::from_stored(stored, dir.to_path_buf()),
        requires_quick_start: false,
        config_recovery: None,
    })
}

/// 以默认配置 bootstrap：完整默认 `DarwinStoredConfig` 原子落盘 + 要求 `QuickStart`。
///
/// 2026-09-30 用户裁定：新装缺省协议为 aTrust（`bootstrap_default`——翻转设计 §8
/// 的「默认不翻转」；只影响 config.json 缺失/不可用的新装与修复路径，反序列化缺键
/// 仍 Cisco，安装基数不被隐式改协议）。
fn bootstrap_default(dir: &Path) -> Result<StartupConfig, DarwinConfigError> {
    let stored = DarwinStoredConfig::bootstrap_default();
    stored.save_to_dir_atomically(dir)?;
    Ok(StartupConfig {
        config: DarwinUiConfig::from_stored(stored, dir.to_path_buf()),
        requires_quick_start: true,
        config_recovery: None,
    })
}

/// 备份属于用户恢复资料，普通 Stop 与构建清理不删除；卸载沿既有目录处置。
fn repair_corrupt_config(dir: &Path, bytes: &[u8]) -> Result<StartupConfig, DarwinConfigError> {
    repair_with_backup(dir, bytes, |dir, bytes| {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| DarwinConfigError::Storage)?
            .as_millis();
        backup_corrupt_bytes(dir, bytes, millis)
    })
}

fn repair_with_backup(
    dir: &Path,
    bytes: &[u8],
    backup: impl FnOnce(&Path, &[u8]) -> Result<String, DarwinConfigError>,
) -> Result<StartupConfig, DarwinConfigError> {
    let backup_file = backup(dir, bytes)?;
    let mut startup = bootstrap_default(dir)?;
    startup.config_recovery = Some(ConfigRecovery {
        corrupted: true,
        backup_file,
    });
    Ok(startup)
}

fn backup_corrupt_bytes(
    dir: &Path,
    bytes: &[u8],
    mut millis: u128,
) -> Result<String, DarwinConfigError> {
    loop {
        let filename = format!("config.corrupt-backup-{millis}.json");
        let path = dir.join(&filename);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                millis = millis.checked_add(1).ok_or(DarwinConfigError::Storage)?;
                continue;
            }
            Err(_) => return Err(DarwinConfigError::Storage),
        };
        if write_backup(&mut file, bytes)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(DarwinConfigError::Storage);
        }
        return Ok(filename);
    }
}

fn write_backup(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes)?;
    writer.flush()
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
