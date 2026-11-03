
//! 上次连接版本标记（T7 §三：版本升级首连强制预检；计划验收 13）。
//!
//! 载体：与 trace 同目录的 `last-connected-version-<user-sid>.txt`——**每用户
//! 一份**而非机器级单文件：预检的系统代理域按用户探测（HKU\<sid>），机器级单
//! 文件会让升级后第一个连接的用户"代消"其他用户的升级首连预检（B 用户自己的
//! WinINET 残留永不触发）；每用户一份与 `trace-<user-sid>.json` 命名对称，且
//! D1 ACL 的"owner 用户 = 该 trace 的 user-sid"对所有文件统一。
//!
//! 单写者纪律（D1-b）：engine 是唯一读写者——读侧任何异常（缺失/损坏/超限/
//! sid 非法/IO 失败）一律折算为"版本未知"→ 触发预检（保守方向：升级、全新
//! 安装、标记被外物破坏三种情形都强制预检，全新安装预检无害）。写侧走
//! [`crate::connection_trace::write_lock`]（connection 目录全部写者在 engine
//! 进程内串行）与 [`crate::secure_write`] 原语（原子替换 + 显式 DACL）。
//!
//! 写入锚点 = 连接成功建立（`connect_gate` 在 `mark_live`，即 `install_live`
//! 之后调用；重连成功的 `update_established_at` 不是锚点——重连是既有连接代次
//! 的重新建立，版本已在首连时记录）。

use std::fs;
use std::path::{Path, PathBuf};

use exv_vpn_win32_ipc::pipe_names::validate_user_sid;
use exv_vpn_win32_resource::native_error::NativeError;

/// 当前 engine 版本（T7 版本触发源的比较基准；Cargo manifest 为既有版本常量
/// 来源——engine crate 无独立 version 报告路径）。
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 版本标记内容尺寸上限（防外物撑大；正常版本串 ≪ 此值）。
const MAX_VERSION_BYTES: u64 = 64;

/// 版本标记写失败。
#[derive(Debug)]
pub(crate) enum MarkerWriteError {
    /// user-sid 非 SID 文法（路径注入防御）。
    InvalidSid,
    /// 版本串含非法字符/超限（防把任意内容写进标记文件）。
    InvalidVersion,
    /// DACL 构造 / 原子写 / 目录刷盘失败。
    Native(NativeError),
    /// I/O 失败。
    Io(std::io::Error),
}

impl std::fmt::Display for MarkerWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSid => write!(f, "user-sid 非 SID 文法"),
            Self::InvalidVersion => write!(f, "版本串非法（字符集/长度）"),
            Self::Native(native) => write!(f, "Win32 失败：{native:?}"),
            Self::Io(io) => write!(f, "I/O 失败：{io}"),
        }
    }
}

/// 标记文件路径：`<dir>/last-connected-version-<user-sid>.txt`。
///
/// # Errors
///
/// user-sid 非 SID 文法 → [`MarkerWriteError::InvalidSid`]。
pub(crate) fn marker_path(dir: &Path, user_sid: &str) -> Result<PathBuf, MarkerWriteError> {
    validate_user_sid(user_sid).map_err(|_| MarkerWriteError::InvalidSid)?;
    Ok(dir.join(format!("last-connected-version-{user_sid}.txt")))
}

/// 读上次连接版本：`Some(version)` = 已知；`None` = 版本未知（缺失/损坏/超限/
/// 非法内容/sid 非法/IO 失败——全部保守折算为未知，调用方据此触发预检）。
#[must_use]
pub(crate) fn read(dir: &Path, user_sid: &str) -> Option<String> {
    let path = marker_path(dir, user_sid).ok()?;
    let meta = fs::metadata(&path).ok()?;
    if meta.len() > MAX_VERSION_BYTES {
        return None;
    }
    let text = fs::read_to_string(&path).ok()?;
    let version = text.trim();
    valid_version(version).then(|| version.to_owned())
}

/// 原子写版本标记（temp + 改名 + 显式 DACL；覆盖写——标记是"最新一次成功
/// 连接的版本"，无 create 竞态语义）。经 [`crate::connection_trace::write_lock`]
/// 与 trace 写操作串行。
///
/// # Errors
///
/// 见 [`MarkerWriteError`]。
pub(crate) fn write(dir: &Path, user_sid: &str, version: &str) -> Result<(), MarkerWriteError> {
    let target = marker_path(dir, user_sid)?;
    if !valid_version(version) {
        return Err(MarkerWriteError::InvalidVersion);
    }
    let temp = dir.join(format!(
        "last-connected-version-{user_sid}.txt.tmp"
    ));
    let bytes = version.as_bytes();
    let _guard = crate::connection_trace::write_lock();
    crate::secure_write::write_atomic_scoped(
        &temp,
        &target,
        dir,
        bytes,
        Some(user_sid),
        true,
    )
    .map_err(|error| match error {
        crate::secure_write::SecureWriteError::AlreadyExists => {
            // 覆盖写不产生该错误；保守归并为 Native 语义。
            MarkerWriteError::Native(NativeError::from_win32(
                183,
                "version_marker: unexpected already-exists on overwrite",
            ))
        }
        crate::secure_write::SecureWriteError::Native(native) => {
            MarkerWriteError::Native(native)
        }
        crate::secure_write::SecureWriteError::Io(io) => MarkerWriteError::Io(io),
    })
}

/// 版本串文法：非空、≤上限、仅 ASCII 字母数字与 `.` `+` `-` `_`
/// （`CARGO_PKG_VERSION` 语义字符集；同时是标记文件内容的输入校验）。
fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() as u64 <= MAX_VERSION_BYTES
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-' | b'_'))
}

// ---------------------------------------------------------------------------
// 单元测试：temp-dir 真文件（不触碰真实 %ProgramData%）。
// ---------------------------------------------------------------------------
