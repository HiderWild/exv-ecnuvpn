
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, WriteFile, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
};

use crate::pipe_security::PipeSecurity;

/// `%ProgramData%` 下服务密钥相对路径。
pub const SERVICE_KEY_REL: &str = r"exv\service.key";

/// HMAC-SHA256（RFC 2104）——用 `sha2` 实现，零额外依赖。
///
/// 用于 pre-gRPC 挑战的应答计算；`key` 为 32 字节 PSK（或任意长度）。
#[must_use]
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key = key.to_vec();
    if key.len() > BLOCK {
        key = Sha256::digest(&key).to_vec();
    }
    key.resize(BLOCK, 0);
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= key[i];
        opad[i] ^= key[i];
    }
    let inner = Sha256::digest(&[&ipad[..], msg].concat());
    let out = Sha256::digest(&[&opad[..], &inner[..]].concat());
    let mut o = [0u8; 32];
    o.copy_from_slice(&out);
    o
}

/// 恒时比较（长度不等 → false；否则 XOR 折叠）。
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 32 字节随机数（getrandom；失败 → typed 错误，调用方 fail closed）。
///
/// # Errors
/// getrandom 失败 → 携带原因的字符串。
pub fn random_32() -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    getrandom::fill(&mut out).map_err(|e| format!("getrandom: {e}"))?;
    Ok(out)
}

/// `%ProgramData%\exv\service.key` 的完整路径。
///
/// # Errors
/// `ProgramData` 环境变量缺失 → typed 错误。
pub fn service_key_path() -> Result<PathBuf, String> {
    let pd = std::env::var_os("ProgramData")
        .ok_or_else(|| "ProgramData env missing (service PSK path)".to_string())?;
    Ok(PathBuf::from(pd).join(SERVICE_KEY_REL))
}

/// 生成并写服务 PSK 到 `%ProgramData%\exv\service.key`（DACL = SYSTEM + 安装用户）。
///
/// 安装/修复路径调用（engine 子命令，runas 提权边界内）：每次安装都轮换密钥（M14）。
/// 文件以 `CREATE_ALWAYS` 覆盖写；DACL 经 `PipeSecurity`（WSP1 §4 冻结形状：
/// `D:(A;;GA;;;SY)(A;;GA;;;<user_sid>)`）随 `CreateFileW` 的 `SECURITY_ATTRIBUTES` 生效。
///
/// # Errors
/// 随机数 / 路径 / `CreateFileW` / `WriteFile` 失败 → 携带原因的字符串。
pub fn write_service_psk(installer_sid: &str) -> Result<[u8; 32], String> {
    let psk = random_32()?;
    let path = service_key_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create service key dir {}: {e}", parent.display()))?;
    }
    let security = PipeSecurity::new(installer_sid, true)
        .map_err(|code| format!("service key DACL build failed (code {code})"))?;
    let attributes = security.as_attributes();
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` 是活的 NUL 结尾宽字符串；`attributes` 是活 `SECURITY_ATTRIBUTES`，
    // 其 `lpSecurityDescriptor` 指向 `security` 拥有的活 descriptor（同作用域存活）。
    let handle = unsafe {
        CreateFileW(
            windows::core::PCWSTR(wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_READ,
            Some(&raw const attributes as *const _),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|e| format!("create service key file: {e}"))?;
    let result = write_all_to_handle(handle, &psk);
    // SAFETY: `handle` 是 `CreateFileW` 返回的已打开句柄，使用后关闭。
    unsafe {
        let _ = CloseHandle(handle);
    }
    result?;
    Ok(psk)
}

/// 读服务 PSK（core 与服务 engine 都读；oneshot 无文件 → 调用方判缺）。
///
/// # Errors
/// 路径不可得 / 文件缺失 / 读取失败 → 携带原因的字符串。
pub fn read_service_psk() -> Result<[u8; 32], String> {
    let path = service_key_path()?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read service key {}: {e}", path.display()))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("service key must be exactly 32 bytes, got {}", bytes.len()))?;
    Ok(arr)
}

/// PSK 指纹摘要（审计专用）：`hex(SHA-256(psk)[0..8])`，恒定 16 hex chars。
///
/// 2026-09-05 轮换（撤销）计划的审计契约：轮换事件（批量步骤结果 / host 回复与日志 /
/// engine accept 日志）只携带该摘要做关联，**任何日志/错误/证据中禁止出现 PSK 原文或
/// HMAC**。摘要取 SHA-256 前 8 字节、不可逆，不泄露密钥材料（与认证面恒时比较无关，
/// 仅日志关联面）。
#[must_use]
pub fn fingerprint(psk: &[u8; 32]) -> String {
    let digest = Sha256::digest(psk);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// 删除服务 PSK 文件（`%ProgramData%\exv\service.key`）——卸载时清理孤儿载荷。
///
/// R3 卸载对策：`uninstall_service` 在 `DeleteService` 成功后调用。卸载面对「半装误判
/// 已装/上次卸载不彻底」——SCM 条目已删但 `service.key` 残留即孤儿载荷（host 健康模型
/// `HealthState::PayloadOrphan` 据此检测：SCM 未注册但 PSK 可读）。**幂等**：文件缺失
/// → `Ok(())`（首次卸载 / 无服务安装 / 已删）。
///
/// 与 [`write_service_psk`] 同路径（`service_key_path`），不重复建目录——删除无需目录。
///
/// # Errors
/// 路径不可得 / 删除失败（非「文件不存在」）→ 携带原因的字符串。
pub fn delete_service_psk() -> Result<(), String> {
    let path = service_key_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("delete service key {}: {e}", path.display())),
    }
}

/// 一次性写满 `buf` 到句柄（`WriteFile`；短写视为失败——密钥必须完整落盘）。
fn write_all_to_handle(handle: windows::Win32::Foundation::HANDLE, buf: &[u8]) -> Result<(), String> {
    let mut written = 0u32;
    // SAFETY: handle 是已打开可写句柄；written 是活 out-param。
    unsafe {
        WriteFile(handle, Some(buf), Some(&raw mut written), None)
    }
    .map_err(|e| format!("write service key: {e}"))?;
    if written as usize != buf.len() {
        return Err(format!(
            "short write: wrote {written}, expected {}",
            buf.len()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 单元测试：HMAC 已知答案 + 恒时比较 + 随机性 + 密钥文件 DACL 往返。
// ---------------------------------------------------------------------------
