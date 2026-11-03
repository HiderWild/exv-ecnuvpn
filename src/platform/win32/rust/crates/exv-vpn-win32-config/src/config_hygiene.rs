//! Startup classification and idempotent repair for `config.json`.

use std::{
    io::Write,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value};

use crate::{
    ConfigError, ConfigReadFailure, ExvConfig, Protocol,
    config::{DEFAULT_ATRUST_SERVER, DEFAULT_SERVER, write_config_json_atomically},
    paths::config_path,
};

/// T6（D2）：损坏 config 修复性覆盖前的备份文件名前缀（与 `config.json` 同目录）。
/// 命名纪律复用 [`crate::PROTOCOL_SWITCH_BACKUP_FILE`]（`config.*-backup*.json`）；
/// 后缀为 Unix 毫秒时间戳（纯数字，卸载清理的通配据此收紧）。
const CORRUPT_BACKUP_FILE_PREFIX: &str = "config.corrupt-backup-";

/// R-D3-C：当次启动损坏修复的一次性报告。仅在 `load_for_startup` 的损坏分支
/// 产出（不持久化——修复落盘后下次启动 config 已合法，天然不再触发），随
/// `requires_quick_start` 同路透传到前端 quiet 分支显示一次性通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRecovery {
    /// 本次启动是否发生了「已存在的 config 损坏被修复」。
    pub corrupted: bool,
    /// 覆盖前的整字节备份文件名（与 `config.json` 同目录，仅文件名不含路径）。
    pub backup_file: String,
}

/// The configuration made available to startup callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupConfig {
    /// Fully hydrated configuration with every known field populated.
    pub config: ExvConfig,
    /// Whether startup had to replace a missing or unusable config with defaults,
    /// or the bootstrap-provisioned flag has not been cleared by a user submission
    /// yet (T6/D1).
    pub requires_quick_start: bool,
    /// R-D3-C：损坏修复报告；缺失文件 bootstrap 与合法启动均为 `None`。
    pub config_recovery: Option<ConfigRecovery>,
}

/// Load, classify, and repair configuration for application startup.
///
/// Missing, blank, malformed, non-object, and known-schema-invalid JSON are
/// replaced with a complete default configuration and require Quick Start;
/// an existing unusable file is first preserved byte-for-byte as
/// `config.corrupt-backup-<unix_millis>.json` (T6/D2). Every valid JSON object
/// is hydrated with known defaults while retaining its unknown fields. The
/// repaired representation is atomically persisted.
///
/// T6（D1）：`requires_quick_start` 的判定是「文件缺失/损坏**或**
/// `provisioned_by_bootstrap` 标记为 true」——bootstrap 播种的配置在用户显式提交
/// 之前，每次启动都持续要求快速入门，首启中断的新用户不再永久错过。
pub fn load_for_startup(dir: &Path) -> Result<StartupConfig, ConfigError> {
    let path = config_path(dir);
    let text = match read_config_text(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return bootstrap_default(dir);
        }
        // S4 第三档：占用重试与 ACL 自修（见 [`read_config_text`]）都救不回来时，
        // 以结构化信息上抛。禁令：这条路径**绝不**进入 bootstrap_default 或任何
        // 覆盖写——读不了就无法先备份，覆盖等于丢配置（测试断言文件字节不变）。
        Err(error) => {
            return Err(ConfigError::ReadFailed(ConfigReadFailure {
                raw_os_error: error.raw_os_error(),
                path,
            }));
        }
    };

    let object = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(object)) => object,
        Ok(_) | Err(_) => return repair_corrupt_config(dir, &path),
    };
    let mut object = object;
    crate::config::normalize_atrust_auth_fields(&mut object);
    hydrate_legacy_server_fields(&mut object);
    let mut config = match serde_json::from_value::<ExvConfig>(Value::Object(object.clone())) {
        Ok(config) => config,
        Err(_) => return repair_corrupt_config(dir, &path),
    };
    // S5a：旧 config.json 缺 password_owner_username 键时按 owner=username 迁移
    //（与 `ExvConfig::load_from_dir` 同一条迁移；hydrate 写回让迁移落盘）。
    config.reconcile_password_owner();

    let hydrated = hydrate_known_fields(object, &config)?;
    write_config_json_atomically(dir, &serde_json::to_string_pretty(&hydrated)?)?;
    let requires_quick_start = config.provisioned_by_bootstrap;
    Ok(StartupConfig {
        config,
        requires_quick_start,
        config_recovery: None,
    })
}

// ---------------------------------------------------------------------------
// S4（2026-10）：config.json 读失败三档自愈（计划 2026-10-10 §四 v2）
// ---------------------------------------------------------------------------

/// `ERROR_SHARING_VIOLATION`：文件被其他进程以不兼容共享模式占用。
/// 判定以 `raw_os_error()` 为准（复审 MINOR-2：`ErrorKind` 映射不保证覆盖 32）。
const ERROR_SHARING_VIOLATION: i32 = 32;
/// `ERROR_ACCESS_DENIED`：权限类读失败（坏 DACL 最常见的形态）。
const ERROR_ACCESS_DENIED: i32 = 5;
/// 第一档：占用重试的总尝试次数（含首次）。
const SHARING_RETRY_ATTEMPTS: u32 = 8;
/// 第一档：两次尝试之间的退避间隔。
const SHARING_RETRY_DELAY: Duration = Duration::from_millis(50);

/// 读 `config.json` 全文，按三档自愈：
///
/// 1. **占用重试**——`raw_os_error()==32`（sharing violation，通常是杀软/备份
///    软件瞬时空抓）有界重试（[`SHARING_RETRY_ATTEMPTS`]×[`SHARING_RETRY_DELAY`]）；
/// 2. **ACL 自修**——读错误为 `ERROR_ACCESS_DENIED` 且文件 **Owner==当前用户**
///    （Owner 隐式持有 READ_CONTROL|WRITE_DAC，非提权进程可改自己的 DACL）时，
///    `SetNamedSecurityInfoW` 重写为干净 DACL 后重读一次；Owner 不是自己
///    （典型：提权进程写入后 Owner=Administrators）则**不尝试**，静默自愈不可达；
/// 3. **上抛**——两档都救不回来时原样返回最后一次错误，由
///    [`load_for_startup`] 包装为 [`ConfigError::ReadFailed`]（禁令：不重建/覆盖）。
fn read_config_text(path: &Path) -> std::io::Result<String> {
    read_config_text_with(path, |path| std::fs::read_to_string(path))
}

/// [`read_config_text`] 的可注入内核（测试注入假读函数验证重试边界）。
fn read_config_text_with(
    path: &Path,
    mut read: impl FnMut(&Path) -> std::io::Result<String>,
) -> std::io::Result<String> {
    let mut attempts_left = SHARING_RETRY_ATTEMPTS;
    loop {
        let error = match read(path) {
            Ok(text) => return Ok(text),
            Err(error) => error,
        };
        // 第一档：占用有界重试。
        if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) && attempts_left > 1 {
            attempts_left -= 1;
            std::thread::sleep(SHARING_RETRY_DELAY);
            continue;
        }
        // 第二档：ACL 异常 + Owner=self → 自修 DACL 后重读一次。
        if error.raw_os_error() == Some(ERROR_ACCESS_DENIED)
            && repair_dacl_as_owner(path)
            && let Ok(text) = read(path)
        {
            return Ok(text);
        }
        // 第三档：不可自愈，原样上抛（最后一次真实错误）。
        return Err(error);
    }
}

/// 若 `path` 的 Owner 是当前进程用户，则把 DACL 重写为「当前用户/SYSTEM/
/// Administrators 完全控制」并返回是否成功。任何查询或写入失败都返回
/// `false`（调用方回落第三档上抛，不做无据猜测）。
///
/// 非 Windows 构建无 ACL 语义，恒 `false`（三档退化为「占用重试 + 上抛」）。
fn repair_dacl_as_owner(path: &Path) -> bool {
    #[cfg(windows)]
    {
        windows_acl::repair_dacl_as_owner(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

/// Windows ACL 自修实现（`cfg(windows)` 独立模块，unsafe 全部带 SAFETY 注释）。
#[cfg(windows)]
mod windows_acl {
    use std::path::Path;

    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, HLOCAL, LocalFree, WIN32_ERROR,
    };
    use windows::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GetNamedSecurityInfoW, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE,
        SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
        TRUSTEE_W,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, EqualSid, GetLengthSid, GetTokenInformation,
        NO_INHERITANCE, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::PCWSTR;

    /// 见 [`super::repair_dacl_as_owner`]。
    pub(super) fn repair_dacl_as_owner(path: &Path) -> bool {
        let Some(path_w) = windows_path(path) else {
            return false;
        };
        // 1) 查 Owner（失败 = 拿不到判断依据，不修）。
        let Ok((owner_sid, descriptor)) = query_owner_sid(&path_w) else {
            return false;
        };
        // 2) Owner 必须是当前进程用户（Owner 隐式持有 WRITE_DAC；否则普通令牌
        //    改不了 DACL，静默自愈不可达，交第三档 runas 入口）。
        let token = match open_own_token() {
            Ok(token) => token,
            Err(()) => {
                // SAFETY: descriptor 由 GetNamedSecurityInfoW 分配，需 LocalFree 释放。
                unsafe { let _ = LocalFree(Some(HLOCAL(descriptor.0))); }
                return false;
            }
        };
        let user_sid = token_user_sid_bytes(token);
        // SAFETY: token 是本函数打开的句柄，用完即关。
        unsafe {
            let _ = CloseHandle(token);
        }
        let Some(user_sid) = user_sid else {
            // SAFETY: descriptor 由 GetNamedSecurityInfoW 分配，需 LocalFree 释放。
            unsafe { let _ = LocalFree(Some(HLOCAL(descriptor.0))); }
            return false;
        };
        // SAFETY: owner_sid 指向 descriptor 内合法 SID；user_sid 指向本地 Vec 内
        // 副本，两者在调用期间均存活。
        let owns = unsafe { EqualSid(owner_sid, PSID(user_sid.as_ptr().cast_mut().cast())) }.is_ok();
        // SAFETY: descriptor 由 GetNamedSecurityInfoW 分配，需 LocalFree 释放。
        unsafe { let _ = LocalFree(Some(HLOCAL(descriptor.0))); }
        if !owns {
            return false;
        }
        // 3) 重写 DACL：当前用户 + SYSTEM + Administrators 完全控制（整条替换，
        //    不合并既有 ACE——既有 DACL 正是故障源）。
        rebuild_dacl(&path_w, &user_sid)
    }

    /// 路径转 Windows 宽字符串（超长/不可 UTF-8 表达时 `None`，不修）。
    fn windows_path(path: &Path) -> Option<windows::core::HSTRING> {
        let text = path.to_str()?;
        if text.contains('\0') {
            return None;
        }
        Some(windows::core::HSTRING::from(text))
    }

    /// `GetNamedSecurityInfoW(OWNER)`：返回 (Owner SID 指针, 待释放的描述符)。
    /// SID 指向描述符内部，两者生命周期绑定；描述符由调用方 `LocalFree`。
    fn query_owner_sid(path: &windows::core::HSTRING) -> Result<(PSID, PSECURITY_DESCRIPTOR), ()> {
        let mut owner = PSID::default();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: path 是合法宽字符串；两个输出参数均为活存储；成功（WIN32_ERROR 0）
        // 时 owner/descriptor 由 API 写入，失败值在下方检查后丢弃。
        let status = unsafe {
            GetNamedSecurityInfoW(
                PCWSTR(path.as_ptr()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(&raw mut owner),
                None,
                None,
                None,
                &raw mut descriptor,
            )
        };
        if status != WIN32_ERROR(0) || owner.is_invalid() || descriptor.is_invalid() {
            return Err(());
        }
        Ok((owner, descriptor))
    }

    /// 打开自身进程 token（TOKEN_QUERY）。
    fn open_own_token() -> Result<HANDLE, ()> {
        let mut token = HANDLE::default();
        // SAFETY: GetCurrentProcess 返回伪句柄无需关闭；token 是活输出参数；
        // 返回 Result 在下方检查。
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.is_err() {
            return Err(());
        }
        Ok(token)
    }

    /// token 用户 SID 的字节副本（`GetTokenInformation(TokenUser)` → `GetLengthSid`
    /// → 复制）。任何查询失败 → `None`。
    fn token_user_sid_bytes(token: HANDLE) -> Option<Vec<u8>> {
        let mut len = 0u32;
        // SAFETY: 只查尺寸（null 缓冲）不会写入；len 是有效输出参数；返回值忽略
        // ——失败时 len 保持 0，下方短路。
        unsafe {
            let _ = GetTokenInformation(token, TokenUser, None, 0, &raw mut len);
        }
        if len == 0 {
            return None;
        }
        // 对齐：TOKEN_USER 需指针对齐，u64 缓冲保证（与 ipc peer_auth 同法）。
        let mut buffer = vec![0u64; usize::try_from(len).unwrap_or_default().div_ceil(8)];
        // SAFETY: buffer 至少 len 字节活存储；API 写入结构并回报同长度。
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                len,
                &raw mut len,
            )
        }
        .is_err()
        {
            return None;
        }
        // SAFETY: buffer 以合法 TOKEN_USER 开头（User.Sid 指向缓冲内）。
        let user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
        let sid = user.User.Sid;
        if sid.is_invalid() {
            return None;
        }
        // SAFETY: sid 指向同一缓冲内的合法 SID；GetLengthSid 只读取其长度。
        let sid_len = usize::try_from(unsafe { GetLengthSid(sid) }).ok()?;
        // SAFETY: sid_len 由 GetLengthSid 报告，读取范围在 buffer 内。
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(sid.0.cast::<u8>(), sid_len)
        };
        Some(bytes.to_vec())
    }

    /// 用「当前用户 / SYSTEM / Administrators 完全控制」三条 GRANT ACE 整条重建
    /// `path` 的 DACL（`SetEntriesInAclW(oldacl=None)` + `SetNamedSecurityInfoW`）。
    fn rebuild_dacl(path: &windows::core::HSTRING, user_sid: &[u8]) -> bool {
        // Well-known SID 二进制形式：SYSTEM=S-1-5-18、Administrators=S-1-5-32-544。
        const SID_SYSTEM: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
        const SID_ADMINISTRATORS: [u8; 16] =
            [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 32, 2, 0, 0];
        let sids: Vec<Vec<u8>> =
            vec![user_sid.to_vec(), SID_SYSTEM.to_vec(), SID_ADMINISTRATORS.to_vec()];
        let entries: Vec<EXPLICIT_ACCESS_W> = sids.iter().map(|sid| explicit_grant(sid)).collect();
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: entries 是合法切片；new_dacl 是活输出参数，成功时由 API 分配。
        let status = unsafe { SetEntriesInAclW(Some(&entries), None, &raw mut new_dacl) };
        if status != WIN32_ERROR(0) {
            return false;
        }
        // SAFETY: path 合法；DACL_SECURITY_INFORMATION 只改 DACL；new_dacl 是
        // SetEntriesInAclW 刚产出的合法 ACL。
        let status = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(path.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(new_dacl),
                None,
            )
        };
        // SAFETY: new_dacl 由 SetEntriesInAclW 分配（LocalFree 家族），用后释放。
        unsafe {
            let _ = LocalFree(Some(HLOCAL(new_dacl.cast())));
        }
        status == WIN32_ERROR(0)
    }

    /// 单条「SID 受托人 + FILE_ALL_ACCESS + GRANT」的 EXPLICIT_ACCESS_W。
    fn explicit_grant(sid: &[u8]) -> EXPLICIT_ACCESS_W {
        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: windows::core::PWSTR(sid.as_ptr() as *mut u16),
        };
        EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS.0,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: trustee,
        }
    }
}

/// Persist an explicit user submission after removing unknown fields.
///
/// Unlike startup hydration, an explicit submission is the hygiene boundary:
/// the resulting document is exactly the current known [`ExvConfig`] schema.
///
/// T6（D1）：用户显式提交成功即清除 bootstrap 播种标记——`config_set` 与 UI
/// 凭据保存（`persist_ui_credentials`）都经此边界落盘，此后启动不再判定
/// requires_quick_start。
pub fn save_after_user_submission(dir: &Path, config: &ExvConfig) -> Result<(), ConfigError> {
    let mut submitted = config.clone();
    submitted.provisioned_by_bootstrap = false;
    submitted.save_to_dir(dir)
}

/// 损坏 config 的修复路径（T6/D2）：先整字节备份，再写默认值。
///
/// 备份失败则不覆盖原文件（宁可让本次启动报错，也不销毁用户可能手工修复的
/// 材料——与 protocol-switch 备份「宁可拒绝」同一条纪律）。R-D3-C：备份文件名
/// 经 [`StartupConfig::config_recovery`] 如实上报（一次性，不持久化）。
fn repair_corrupt_config(dir: &Path, source: &Path) -> Result<StartupConfig, ConfigError> {
    let backup_file = back_up_corrupt_config(source)?;
    let mut startup = bootstrap_default(dir)?;
    startup.config_recovery = Some(ConfigRecovery {
        corrupted: true,
        backup_file,
    });
    Ok(startup)
}

/// 把损坏的 config 整字节复制为 `config.corrupt-backup-<unix_millis>.json`，
/// 返回实际写入的备份文件名（仅文件名，与 `config.json` 同目录）。
///
/// 时间戳后缀保持纯数字：毫秒起点取当前时钟，同名冲突时向后顺延（有界），
/// 卸载清理侧据此用严格通配枚举。
fn back_up_corrupt_config(source: &Path) -> Result<String, ConfigError> {
    let bytes = std::fs::read(source)?;
    let dir = source.parent().unwrap_or_else(|| Path::new("."));
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0);
    for offset in 0..1000u64 {
        let destination =
            dir.join(format!("{CORRUPT_BACKUP_FILE_PREFIX}{}.json", base + offset));
        let write_result = (|| -> Result<(), std::io::Error> {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&destination)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        match write_result {
            Ok(()) => {
                return Ok(destination
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string());
            }
            // 时间戳撞名（时钟停滞/同毫秒双损坏）：顺延重试，不覆盖既有备份。
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                let _ = std::fs::remove_file(&destination);
                return Err(error.into());
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique corrupt-config backup file",
    )
    .into())
}

fn bootstrap_default(dir: &Path) -> Result<StartupConfig, ConfigError> {
    // T6（D1/D2）：全新 provision（NotFound）与损坏修复两条 bootstrap 路径都会
    // 置位播种标记——这是 requires_quick_start 跨启动存续的唯一载体，直到用户
    // 显式提交（save_after_user_submission）才清除。
    let config = ExvConfig {
        provisioned_by_bootstrap: true,
        ..ExvConfig::default()
    };
    config.save_to_dir(dir)?;
    Ok(StartupConfig {
        config,
        requires_quick_start: true,
        config_recovery: None,
    })
}

/// 将旧活动 server 迁移为协议专属服务器字段，与 `load_from_dir` 保持一致。
fn hydrate_legacy_server_fields(object: &mut Map<String, Value>) {
    let protocol = object
        .get("protocol")
        .and_then(Value::as_str)
        .and_then(|raw| raw.parse::<Protocol>().ok())
        .unwrap_or(Protocol::Cisco);
    if !object.contains_key("protocol") {
        object.insert("protocol".into(), Value::String(Protocol::Cisco.as_str().into()));
    }
    if !object.contains_key("server") {
        let default = match protocol {
            Protocol::Cisco => DEFAULT_SERVER,
            Protocol::Atrust => DEFAULT_ATRUST_SERVER,
        };
        object.insert("server".into(), Value::String(default.into()));
    }
    let legacy_server = object
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !object.contains_key("cisco_server") {
        let value = if protocol == Protocol::Cisco {
            legacy_server.as_str()
        } else {
            DEFAULT_SERVER
        };
        object.insert("cisco_server".into(), Value::String(value.into()));
    }
    if !object.contains_key("atrust_server") {
        let value = if protocol == Protocol::Atrust {
            legacy_server.as_str()
        } else {
            DEFAULT_ATRUST_SERVER
        };
        object.insert("atrust_server".into(), Value::String(value.into()));
    }
}

fn hydrate_known_fields(
    mut original: Map<String, Value>,
    config: &ExvConfig,
) -> Result<Value, ConfigError> {
    let Value::Object(known_fields) = serde_json::to_value(config)? else {
        unreachable!("ExvConfig always serializes to an object");
    };
    original.extend(known_fields);
    Ok(Value::Object(original))
}

