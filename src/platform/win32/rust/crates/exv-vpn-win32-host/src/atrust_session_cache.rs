//! 当前 Windows 用户的 aTrust 会话缓存；有效性始终由网关重新判定。

use exv_vpn_win32_ipc::browser_session::BrowserSession;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use windows::Win32::{
    Foundation::{HLOCAL, LocalFree},
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
use zeroize::{Zeroize, Zeroizing};

fn cache_path(profile: &Path, gateway: &str) -> PathBuf {
    let digest: String = Sha256::digest(gateway.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    profile.join(format!("session-{digest}.dpapi"))
}

fn crypt(input: &[u8], protect: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: input.len().try_into().map_err(|_| "会话缓存过大")?,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: 输入 blob 在调用期间有效；DPAPI 分配输出，由本函数清零并 LocalFree。
    unsafe {
        let result = if protect {
            CryptProtectData(
                &raw const input,
                windows::core::PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        } else {
            CryptUnprotectData(
                &raw const input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        result.map_err(|_| "无法保护或恢复当前用户的 aTrust 会话".to_owned())?;
        if output.cbData == 0 {
            if !output.pbData.is_null() {
                let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            }
            return Ok(Zeroizing::new(Vec::new()));
        }
        let buffer = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let bytes = Zeroizing::new(buffer.to_vec());
        buffer.zeroize();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(bytes)
    }
}

/// 缓存候选，不延长服务端有效期；不保存学校密码。
pub fn save(profile: &Path, session: &BrowserSession) -> Result<(), String> {
    let bytes = session.encode().map_err(|_| "会话缓存编码失败")?;
    let sealed = crypt(&bytes, true)?;
    std::fs::create_dir_all(profile).map_err(|_| "无法创建会话缓存目录")?;
    std::fs::write(cache_path(profile, session.gateway()), &*sealed)
        .map_err(|_| "无法保存加密会话".to_owned())
}

/// 只恢复指定网关的候选；调用方必须重新向该网关验证。
pub fn load(profile: &Path, gateway: &str) -> Result<Option<BrowserSession>, String> {
    let sealed = match std::fs::read(cache_path(profile, gateway)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取加密会话".into()),
    };
    let bytes = crypt(&sealed, false)?;
    let session = BrowserSession::decode(&bytes).map_err(|_| "缓存会话格式无效")?;
    if session.gateway() != gateway {
        return Err("缓存会话网关不匹配".into());
    }
    Ok(Some(session))
}

/// 尽力删除失效候选；删除失败不阻止重新登录，后续恢复仍必须经过网关验证。
pub fn forget(profile: &Path, gateway: &str) {
    let _ = std::fs::remove_file(cache_path(profile, gateway));
}
