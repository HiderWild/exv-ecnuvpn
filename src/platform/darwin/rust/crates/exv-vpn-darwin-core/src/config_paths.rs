//! Darwin 配置文件路径与同目录原子替换。

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) const CONFIG_FILE: &str = "config.json";
pub(crate) const KEY_FILE: &str = "key.bin";
const DEFAULT_CONFIG_SUBDIR: &str = ".exv";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 与 Windows 配置层保持相同的目录优先级和文件名。
#[must_use]
pub(crate) fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("EXV_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return PathBuf::from(profile).join(DEFAULT_CONFIG_SUBDIR);
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(DEFAULT_CONFIG_SUBDIR);
    }
    PathBuf::from(DEFAULT_CONFIG_SUBDIR)
}

#[must_use]
pub(crate) fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

#[must_use]
pub(crate) fn key_path(dir: &Path) -> PathBuf {
    dir.join(KEY_FILE)
}

/// 写入同目录临时文件，再以 rename 替换目标文件。
///
/// M6（M5 复审移交 B-3）：产物统一显式 0600——umask 不可靠（默认 022 下
/// `fs::write` 产物是 0644），而 `~/.exv` 的 key.bin（AES 密钥）/config.json
/// （加密凭据+配置）/aTrust 会话缓存（密文）对同机其他用户必须不可读。权限在
/// rename **之前**落到临时文件（rename 不改变权限位，落盘窗口内同样收紧）。
pub(crate) fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("configuration path has no parent"))?;
    std::fs::create_dir_all(directory)?;
    let filename = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("configuration path has no filename"))?
        .to_string_lossy();
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(
        ".{filename}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));

    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = file
            .write_all(contents)
            .and_then(|()| file.sync_all())
            .and_then(|()| {
                std::fs::set_permissions(
                    &temporary,
                    std::fs::Permissions::from_mode(0o600),
                )
            });
        if let Err(error) = result {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    };
    write()?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}
