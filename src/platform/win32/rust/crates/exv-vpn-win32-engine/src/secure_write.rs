
//! T7 D1（v6 改判 b）：connection 载体写盘侧加固——显式 DACL + 原子写原语。
//!
//! 语义：`%ProgramData%\ExvVpn\connection` 目录与其中全部文件（trace、版本标记、
//! 原子写 temp）出生即带**显式保护 DACL**——发起用户（该 trace 的 user-sid）+
//! SYSTEM + BUILTIN\Administrators 允许；其余主体（BUILTIN\Users / Interactive /
//! Everyone）因保护 DACL 不继承父目录、且无 allow ACE 而被拒（不使用显式 deny
//! ACE——缺席 allow 即拒，且显式 deny 在 ACE 求值序上易误伤）。
//!
//! 机制选择（T7 说明理由）：
//! - **文件（temp / 版本标记 → 改名后的 trace 同 SD）用 `CreateFileW` +
//!   `SECURITY_ATTRIBUTES`**：对象**出生即带**正确 DACL，结构上不存在
//!   "先创建（默认/继承 DACL）后改 ACL"的可读窗口——temp 文件窗口期全局可读
//!   在此路径上不可能出现。`MoveFileExW` 改名携带源文件 SD，目标文件继承的
//!   即 temp 的显式 DACL。
//! - **目录用 `SetNamedSecurityInfoW`**：目录经 `create_dir_all` 建立后按名字
//!   应用，一次调用携带 `PROTECTED_DACL_SECURITY_INFORMATION`（与 resource
//!   `storage_security` 同款纪律：不加 PROTECTED 时父目录可继承 ACE 会被系统
//!   合并回 DACL，实测拒不掉）。
//!
//! 创建者自持：当前进程用户 SID 一并入册——`CreateFileW` 的创建者若不在 DACL
//! 内即自拒（ACCESS_DENIED），这是 OS 硬约束而非测试便利；生产形态下该 ACE 恒
//! 冗余（service=SYSTEM∈SY；oneshot=提权用户∈BA 或即 user-sid 本身）。
//!
//! 注：本模块**有意不改 SD owner 字段**（"owner 用户"落在 DACL 允许清单的显式
//! ACE；engine 是唯一写者，DACL 已足够，避免跨形态写 owner 的权限矩阵复杂化）。
//! journal 目录（W14）仍走 resource `storage_security`（BA 排除的另一种冻结
//! 语义），两纪律互不覆盖。

use std::fs;
use std::io::Write;
use std::os::windows::io::FromRawHandle;
use std::path::Path;

use exv_vpn_win32_ipc::pipe_names::validate_user_sid;
use exv_vpn_win32_resource::native_error::NativeError;
use windows::core::{BOOL, HSTRING};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HLOCAL, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    GetSecurityDescriptorDacl, ACL, PSECURITY_DESCRIPTOR, PROTECTED_DACL_SECURITY_INFORMATION,
    SECURITY_ATTRIBUTES, DACL_SECURITY_INFORMATION,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, MoveFileExW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, OPEN_EXISTING,
};

/// SYSTEM 账户的 SID 字符串（创建者去重用：SYSTEM 进程的 current sid 与 SY 同义）。
const SYSTEM_SID: &str = "S-1-5-18";

/// `MoveFileExW` 不带 `REPLACE_EXISTING` 时目标已存在的 Win32 错误码。
const ERROR_ALREADY_EXISTS: u32 = 183;

/// SID 文法非法的 Win32 参数错误码（ERROR_INVALID_PARAMETER）。
const ERROR_INVALID_PARAMETER: u32 = 87;

/// 安全写失败。
#[derive(Debug)]
pub(crate) enum SecureWriteError {
    /// 目标已存在且未请求覆盖（原子 create 语义，镜像
    /// `TraceStoreError::AlreadyExists`）。
    AlreadyExists,
    /// Win32 失败（描述符构造 / 创建 / 改名 / 目录刷盘）。
    Native(NativeError),
    /// 写入/刷盘的 std I/O 失败。
    Io(std::io::Error),
}

/// 构造 connection 载体的 DACL SDDL（纯函数；单测直接断言 ACE 集合）。
///
/// 形状：`D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;<user-sid>)…`——
/// 保护 DACL（`P`，不继承）+ 全权 allow ACE。`user_sid` 经 canonical SID 校验
/// （`ipc::pipe_names`，拼接注入防御：sid 段最终来自 RPC 侧，直接拼进 SDDL，
/// 非法字符可闭合 ACE 注入额外授权，必须先校验）。当前进程用户 SID 自动追加
/// （创建者自持，见模块文档；与已有条目去重）。
///
/// # Errors
///
/// `user_sid` 非 SID 文法 → [`NativeError`]（ERROR_INVALID_PARAMETER）。
pub(crate) fn connection_dacl_sddl(user_sid: Option<&str>) -> Result<String, NativeError> {
    let mut sids: Vec<String> = vec!["SY".to_owned(), "BA".to_owned()];
    let push_sid = |sid: &str, sids: &mut Vec<String>| {
        if !sids.iter().any(|existing| existing == sid) {
            sids.push(sid.to_owned());
        }
    };
    if let Some(sid) = user_sid {
        validate_user_sid(sid).map_err(|_| {
            NativeError::from_win32(
                ERROR_INVALID_PARAMETER,
                "secure_write: user sid 非 SID 文法，拒绝构造 DACL",
            )
        })?;
        // SYSTEM 进程的 user_sid 即 SYSTEM（与 SY 冗余）——去重。
        if sid != SYSTEM_SID {
            push_sid(sid, &mut sids);
        }
    }
    if let Some(creator) = exv_vpn_win32_ipc::peer_auth::current_user_sid()
        && creator != SYSTEM_SID
    {
        push_sid(&creator, &mut sids);
    }
    let aces = sids
        .iter()
        .map(|sid| format!("(A;OICI;FA;;;{sid})"))
        .collect::<Vec<_>>()
        .join("");
    Ok(format!("D:P{aces}"))
}

/// 原子写（T7：temp 出生即带显式 DACL）。
///
/// 调用序：`CreateFileW(temp, SECURITY_ATTRIBUTES)` → 写入 + `FlushFileBuffers`
/// → 关句柄 → `MoveFileExW`（`REPLACE_EXISTING` 视需要，恒 `WRITE_THROUGH`）
/// → 刷目录元数据。改名携带 temp 的 SD，目标文件即显式 DACL——无窗口期。
///
/// 尺寸上限归调用方（trace 64 KiB / 版本标记 64 B，各自语义不同）。
///
/// # Errors
///
/// 见 [`SecureWriteError`]。
pub(crate) fn write_atomic_scoped(
    temp: &std::path::Path,
    target: &std::path::Path,
    dir: &Path,
    bytes: &[u8],
    user_sid: Option<&str>,
    replace_existing: bool,
) -> Result<(), SecureWriteError> {
    let sddl = connection_dacl_sddl(user_sid).map_err(SecureWriteError::Native)?;
    let psd = sddl_to_descriptor(&sddl).map_err(SecureWriteError::Native)?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
        lpSecurityDescriptor: psd.0,
        bInheritHandle: BOOL(0),
    };

    // CREATE_ALWAYS：上次进程崩溃残留的同名 temp（进程内写者互斥下固定名）被
    // 原子截断复用——持有旧 DACL 的残留不会阻塞新写。
    let htemp = HSTRING::from(temp.to_string_lossy().as_ref());
    let created = unsafe {
        CreateFileW(
            &htemp,
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            Some(&raw const attributes),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    let write_result = (|| -> Result<(), SecureWriteError> {
        let handle = created.map_err(|e| {
            SecureWriteError::Native(NativeError::from_win32(
                win32_code(&e),
                "secure_write: create temp with scoped DACL",
            ))
        })?;
        // SAFETY: handle 由上方 CreateFileW 成功返回、归本闭包所有；File 接管后
        // 恰好在 drop 时关闭（MoveFileExW 要求改名前句柄已关）。
        let mut file = unsafe { fs::File::from_raw_handle(handle.0) };
        let io = file.write_all(bytes).and_then(|()| file.sync_all());
        let io = io.map_err(SecureWriteError::Io);
        drop(file);
        io
    })();
    // SAFETY: psd 由 SDDL 转换 API 分配；CreateFileW 已返回（复制了所需内容），
    // 所有路径都必须恰好释放一次。
    unsafe {
        let _ = LocalFree(Some(HLOCAL(psd.0)));
    }
    let _ = &mut attributes;
    write_result?;

    let src = HSTRING::from(temp.to_string_lossy().as_ref());
    let dst = HSTRING::from(target.to_string_lossy().as_ref());
    let flags = if replace_existing {
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    } else {
        MOVEFILE_WRITE_THROUGH
    };
    // SAFETY: src/dst 是存活于整个调用期间的合法路径 PCWSTR；temp 句柄已关闭，
    // 目标文件本模块从不长期持有句柄。
    let moved = unsafe { MoveFileExW(&src, &dst, flags) };
    if let Err(e) = moved {
        // 改名失败清理残留 temp（best-effort；清理二次失败并入原错误返回）。
        let _ = fs::remove_file(temp);
        let code = win32_code(&e);
        if !replace_existing && code == ERROR_ALREADY_EXISTS {
            return Err(SecureWriteError::AlreadyExists);
        }
        return Err(SecureWriteError::Native(NativeError::from_win32(
            code,
            "secure_write: atomic replace",
        )));
    }
    flush_directory(dir).map_err(SecureWriteError::Native)
}

/// 目录保护：`create_dir_all` + 显式保护 DACL（`SetNamedSecurityInfoW`，名字级、
/// 一次调用携带 PROTECTED——不加则父目录可继承 ACE 被合并回，见模块文档）。
///
/// # Errors
///
/// 目录建立或 DACL 应用失败 → [`NativeError`]。
pub(crate) fn protect_connection_dir(dir: &Path, user_sid: Option<&str>) -> Result<(), NativeError> {
    fs::create_dir_all(dir)
        .map_err(|e| io_native("secure_write: create_dir_all", &e))?;
    let sddl = connection_dacl_sddl(user_sid)?;
    let psd = sddl_to_descriptor(&sddl)?;
    let applied = apply_dacl_by_name(dir, psd);
    // SAFETY: psd 由 SDDL 转换 API 分配；SetNamedSecurityInfoW 返回后所有路径
    // 都必须恰好释放一次。
    unsafe {
        let _ = LocalFree(Some(HLOCAL(psd.0)));
    }
    applied
}

/// SDDL → 自相对安全描述符（LocalAlloc 分配，调用方负责 `LocalFree`）。
fn sddl_to_descriptor(sddl: &str) -> Result<PSECURITY_DESCRIPTOR, NativeError> {
    let hsddl = HSTRING::from(sddl);
    let mut psd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    // SAFETY: psd 是本函数的存活出参（API 分配）；hsddl 是合法 PCWSTR。
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &hsddl,
            SDDL_REVISION_1,
            std::ptr::addr_of_mut!(psd),
            None,
        )
    };
    if ok.is_err() {
        // SAFETY: 读取线程局部 LastError 仅为错误码取值，无内存安全性前提。
        let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
        return Err(NativeError::from_win32(
            code,
            "secure_write: invalid security descriptor",
        ));
    }
    Ok(psd)
}

/// 按名字应用描述符中的 DACL（PROTECTED；不动 owner/group/sacl）。
fn apply_dacl_by_name(path: &Path, psd: PSECURITY_DESCRIPTOR) -> Result<(), NativeError> {
    let mut present = BOOL(0);
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut defaulted = BOOL(0);
    // SAFETY: psd 是合法描述符；三个出参均为存活栈变量。
    let extracted =
        unsafe { GetSecurityDescriptorDacl(psd, &raw mut present, &raw mut dacl, &raw mut defaulted) };
    if extracted.is_err() || !present.as_bool() || dacl.is_null() {
        return Err(NativeError::from_win32(
            0,
            "secure_write: descriptor carries no usable DACL",
        ));
    }
    let hpath = HSTRING::from(path.to_string_lossy().as_ref());
    // SAFETY: hpath 是合法路径 PCWSTR；dacl 指向存活于本次调用的 ACL（psd 由调用
    // 方持有至本函数返回之后才释放）。
    let rc = unsafe {
        SetNamedSecurityInfoW(
            &hpath,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(dacl),
            None,
        )
    };
    if rc.0 != 0 {
        return Err(NativeError::from_win32(
            rc.0,
            "secure_write: SetNamedSecurityInfoW failed",
        ));
    }
    Ok(())
}

/// 刷目录元数据，使改名/创建结果持久（镜像 `journal_store.rs::flush_directory`）。
fn flush_directory(dir: &Path) -> Result<(), NativeError> {
    let hpath = HSTRING::from(dir.to_string_lossy().as_ref());
    // SAFETY: hpath 是合法目录路径 PCWSTR；目录句柄经 FILE_FLAG_BACKUP_SEMANTICS
    // 打开并在下方关闭。
    let handle: HANDLE = unsafe {
        CreateFileW(
            &hpath,
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map_err(|e| {
        NativeError::from_win32(win32_code(&e), "secure_write: open dir for flush")
    })?;

    // SAFETY: handle 是本函数持有的合法打开目录句柄。
    let flushed = unsafe { FlushFileBuffers(handle) };
    // SAFETY: handle 归本函数所有，必须恰好关闭一次。
    unsafe {
        let _ = CloseHandle(handle);
    }

    flushed.map_err(|e| {
        NativeError::from_win32(win32_code(&e), "secure_write: flush dir")
    })
}

/// 从 `windows` crate `Error`（facility-7 编码）提取 Win32 错误码。
fn win32_code(error: &windows::core::Error) -> u32 {
    windows::Win32::Foundation::WIN32_ERROR::from_error(error).map_or(0, |win32| win32.0)
}

/// I/O 错误 → [`NativeError`]（保留 Win32 码）。
fn io_native(context: &str, error: &std::io::Error) -> NativeError {
    let code = error
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        .unwrap_or(0);
    NativeError::from_win32(code, context)
}

// ---------------------------------------------------------------------------
// 单元测试：SDDL 纯函数断言 + 真文件/真目录的 DACL 集成断言（tempfile 于
// %TEMP%；不触碰真实 %ProgramData%）。
// ---------------------------------------------------------------------------
