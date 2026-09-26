//! 按住眼睛才短暂读取 Darwin 已保存密码；常规配置、快照和日志不返回明文。
use exv_vpn_darwin_core::DarwinUiConfig;
use serde::{Serialize, Serializer};
use zeroize::Zeroizing;

/// 不实现 Debug/Clone，命令返回值序列化完成后清零 Rust 侧明文。
pub(crate) struct PasswordForDisplay(Zeroizing<String>);
impl Serialize for PasswordForDisplay {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_str())
    }
}
const UNAVAILABLE: &str = "无法读取当前配置的已保存密码";
pub(crate) fn read_current_for_display(
    username: &str,
    server: &str,
) -> Result<Option<PasswordForDisplay>, String> {
    // DarwinUiConfig::load 与 Core 共用 EXV_CONFIG_DIR / 本机配置目录，不使用 Windows 路径。
    let config = DarwinUiConfig::load().map_err(|_| UNAVAILABLE.to_string())?;
    read_for_display(&config, username, server)
}
fn read_for_display(
    config: &DarwinUiConfig,
    username: &str,
    server: &str,
) -> Result<Option<PasswordForDisplay>, String> {
    if config.username() != username || config.server() != server {
        return Err(UNAVAILABLE.into());
    }
    let password = Zeroizing::new(
        config
            .load_saved_password()
            .map_err(|_| UNAVAILABLE.to_string())?,
    );
    if password.is_empty() {
        return Ok(None);
    }
    Ok(Some(PasswordForDisplay(password)))
}
