//! aTrust 学校凭据与 SSO 档案的小端口。
//!
//! 存储位置及解析规则沿用 `DarwinUiConfig`：调用者传入冻结配置的同一目录。
//! 新增 `atrust-sso-<host完整SHA256>.bin`，沿原 key.bin/AES/同目录原子替换；
//! 不迁移网关候选或浏览器 profile，普通 Stop 不调用删除。

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

use crate::config::DarwinStoredConfig;
use crate::config_crypto::{decrypt_bytes, encrypt_bytes};
use crate::config_paths::atomic_write;
use crate::sso_session_jar::{SSO_HOST, SsoJarError, SsoSessionJar};
use crate::{DarwinConfigError, DarwinUiConfig};

/// 本次显式完整对；未提供与提供半对具有不同语义。
/// 不实现 Clone、Serialize 或明文 Debug，交付后随所有权释放而零化。
pub struct SchoolCredentialInput {
    pub username: Zeroizing<String>,
    pub password: Zeroizing<String>,
}

/// 只供当前 Owned 通道一次性消费的材料。
pub struct SchoolCredentials {
    pub username: Zeroizing<String>,
    pub password: Zeroizing<String>,
}

/// 显式完整对优先；显式半对不混配保存密码。未提供显式材料才读取保存对。
/// 本函数只读，读取保存密码不会生成密钥，也不经 ConfigGet 空密码字段。
///
/// # Errors
/// 保存密码实际解密/读取失败时透传配置错误。
pub fn school_credentials(
    config: &DarwinUiConfig,
    explicit: Option<SchoolCredentialInput>,
) -> Result<Option<SchoolCredentials>, DarwinConfigError> {
    if let Some(input) = explicit {
        return Ok(
            (!input.username.trim().is_empty() && !input.password.is_empty()).then_some(
                SchoolCredentials {
                    username: input.username,
                    password: input.password,
                },
            ),
        );
    }
    if config.username().trim().is_empty() {
        return Ok(None);
    }
    let password = Zeroizing::new(config.load_saved_password()?);
    if password.is_empty() {
        return Ok(None);
    }
    Ok(Some(SchoolCredentials {
        username: Zeroizing::new(config.username().to_owned()),
        password,
    }))
}

/// 实际存储错误分类；不保存路径、Cookie、设备标识或明文载荷。
#[derive(Debug)]
pub enum SsoStorageError {
    Host,
    Read,
    Key(DarwinConfigError),
    Crypto(DarwinConfigError),
    Jar(SsoJarError),
    Write,
}

/// 当前实现只读写本校 SSO，大小写和 DNS 尾点归一到同一档案。
fn normalized_host(host: &str) -> Result<&'static str, SsoStorageError> {
    if host.trim_end_matches('.').eq_ignore_ascii_case(SSO_HOST) {
        Ok(SSO_HOST)
    } else {
        Err(SsoStorageError::Host)
    }
}

fn sso_path(dir: &Path, host: &str) -> Result<PathBuf, SsoStorageError> {
    let digest = Sha256::digest(normalized_host(host)?.as_bytes());
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let hash: String = digest
        .iter()
        .flat_map(|byte| {
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 0xf)]),
            ]
        })
        .collect();
    Ok(dir.join(format!("atrust-sso-{hash}.bin")))
}

/// 缺失返回 None；损坏返回具体分类并保留原文件，读取不新建 key.bin。
///
/// # Errors
/// 主机不匹配、文件/密钥读取、解密或版本化数据解析失败时返回对应分类。
pub fn load_sso_jar(dir: &Path, host: &str) -> Result<Option<SsoSessionJar>, SsoStorageError> {
    let path = sso_path(dir, host)?;
    let sealed = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(SsoStorageError::Read),
    };
    let key = Zeroizing::new(
        DarwinStoredConfig::load_key(dir)
            .map_err(SsoStorageError::Key)?
            .ok_or(SsoStorageError::Key(DarwinConfigError::Storage))?,
    );
    let bytes = decrypt_bytes(&sealed, &key).map_err(SsoStorageError::Crypto)?;
    SsoSessionJar::decode(&bytes)
        .map(Some)
        .map_err(SsoStorageError::Jar)
}

/// 非空 Cookie 档案沿原密钥和 AES 原子写入；空采集不覆盖已有档案，返回是否写入。
///
/// # Errors
/// 主机不匹配、密钥生成、编码、加密或实际写入失败时返回对应分类。
pub fn save_sso_jar(dir: &Path, host: &str, jar: &SsoSessionJar) -> Result<bool, SsoStorageError> {
    let path = sso_path(dir, host)?;
    if jar.is_empty() {
        return Ok(false);
    }
    let wire = jar.encode().map_err(SsoStorageError::Jar)?;
    // 可选 SSO 写入不能把既有损坏密钥当作首次安装，避免替换密码/网关共用密钥。
    let key = Zeroizing::new(
        match DarwinStoredConfig::load_key(dir).map_err(SsoStorageError::Key)? {
            Some(key) => key,
            None => DarwinStoredConfig::ensure_key(dir).map_err(SsoStorageError::Key)?,
        },
    );
    let sealed = encrypt_bytes(&wire, &key).map_err(SsoStorageError::Crypto)?;
    atomic_write(&path, &sealed).map_err(|_| SsoStorageError::Write)?;
    Ok(true)
}

/// 沿原会话缓存写入此次实际认证账户。
pub fn save_session_with_username(dir: &Path, session: &exv_vpn_darwin_ipc::browser_session::BrowserSession, username: &str) -> Result<(), DarwinConfigError> {
    crate::atrust_session_cache::save_with_username(dir, session, username)
}

/// 读取与指定候选相同的认证账户绑定。
pub fn username_for_session(dir: &Path, session: &exv_vpn_darwin_ipc::browser_session::BrowserSession) -> Option<String> {
    crate::atrust_session_cache::username_for(dir, session)
}
