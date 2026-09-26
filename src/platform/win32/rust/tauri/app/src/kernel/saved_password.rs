//! 用户主动按住眼睛时短暂读取已保存密码。常规配置、快照和日志绝不包含明文。
use std::path::Path;

use exv_vpn_win32_config::ExvConfig;
use serde::{Serialize, Serializer};
use zeroize::Zeroizing;

/// 不实现 Debug/Clone；Tauri 序列化后即零化 Rust 侧明文。
pub struct PasswordForDisplay(Zeroizing<String>);

impl Serialize for PasswordForDisplay {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_str())
    }
}

pub fn read_for_display(
    directory: &Path,
    expected_username: &str,
    expected_server: &str,
) -> Result<Option<PasswordForDisplay>, String> {
    const UNAVAILABLE: &str = "无法读取当前配置的已保存密码";
    let config = ExvConfig::load_from_dir(directory).map_err(|_| UNAVAILABLE.to_string())?;
    // Core 和 UI 使用同一个 EXV_CONFIG_DIR。额外绑定 Core 当前身份，避免路径或
    // 配置在异步读取期间发生变化时回显另一账户的密码。
    if config.username != expected_username || config.server != expected_server {
        return Err(UNAVAILABLE.to_string());
    }
    if !config.remember_password || config.password.is_empty() {
        return Ok(None);
    }
    let key = Zeroizing::new(
        ExvConfig::load_key(directory)
            .map_err(|_| UNAVAILABLE.to_string())?
            .ok_or_else(|| UNAVAILABLE.to_string())?,
    );
    let plaintext = Zeroizing::new(config.decrypt_password(&key).map_err(|_| UNAVAILABLE.to_string())?);
    if plaintext.is_empty() { return Ok(None); }
    Ok(Some(PasswordForDisplay(plaintext)))
}
