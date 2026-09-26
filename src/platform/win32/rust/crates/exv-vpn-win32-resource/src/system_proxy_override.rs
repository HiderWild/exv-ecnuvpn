
//! 系统代理豁免合并写入（设计 §5.2 / 拍板结论 2，T2）。
//!
//! 职责边界（对齐设计文档）：
//!
//! - [`cidr_to_wildcard`]：CIDR → `WinInet` 通配豁免条目的纯函数派生。仅整段
//!   对齐的 /8、/16、/24 转 `a.*` / `a.b.*` / `a.b.c.*`；其余前缀长度与主机位
//!   非零的网络地址一律保持 IP 精确条目（不猜、不归一）。
//! - [`merge_bypass_entries`]：原 `ProxyOverride` + EXV 目标条目 → 合并结果
//!   （去重 / ASCII 大小写归一 / 512 上限 / 禁分号 / 单条 ≤1024 字节）。非法
//!   输入是类型化错误，绝不静默丢弃。
//! - [`write_override_for_user`]：按发起用户 SID 写
//!   `HKU\<sid>\...\Internet Settings\ProxyOverride` 并经 wininet 广播生效；
//!   DLL 缺失是 typed skip（返回 `Ok(false)`），不是 panic。
//!
//! 本模块不做 prestate 捕获与还原（TK1 journal 扩展职责）；engine 每用户
//! fail-closed 语义（无 SID 不写 HKCU）由调用方保证。

use windows::core::{HSTRING, PCSTR, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE, WIN32_ERROR};
use windows::Win32::Networking::WinInet::{
    INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_USERS, KEY_READ,
    KEY_SET_VALUE, KEY_WRITE, REG_DWORD, REG_SZ,
};

use crate::native_error::NativeError;
use crate::system_proxy::RawValue;
use crate::system_proxy_family::ValueName;

/// `ERROR_INVALID_PARAMETER`（87）：条目级非法输入的 Win32 惯用错误码。
const ERROR_INVALID_PARAMETER: u32 = 87;
/// `ERROR_MORE_DATA`（234）：容量超限类错误（512 上限）的 Win32 惯用错误码。
const ERROR_MORE_DATA: u32 = 234;
/// `ERROR_SUCCESS`（0）：Win32 成功码。
const ERROR_SUCCESS: u32 = 0;
/// `ERROR_PROC_NOT_FOUND`（127）：动态模块缺导出。
const ERROR_PROC_NOT_FOUND: u32 = 127;

/// `HKU\<sid>\Software\Microsoft\Windows\CurrentVersion\Internet Settings`
/// 的相对子键路径（相对 `HKEY_USERS` 打开；SID 由调用方拼在最前）。
const INTERNET_SETTINGS_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// 合并结果总条数上限（设计 §5.2 冻结值 512；超限报错不截断）。
pub const MAX_MERGE_ENTRIES: usize = 512;

/// 单条豁免条目的字节长度上限（设计 §5.2 冻结值 1024；UTF-8 计）。
pub const MAX_ENTRY_LEN: usize = 1024;

/// 一条派生后的豁免条目：IP 精确条目或 `WinInet` 通配条目。
///
/// 通配形式仅由整段对齐的 /8、/16、/24 CIDR 派生；其余输入保持精确条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WildcardEntry {
    /// IP 精确条目（原样保留，如 `10.1.2.3`、不对齐的 `10.1.2.3/24`）。
    Exact(String),
    /// `WinInet` 通配条目（`a.*` / `a.b.*` / `a.b.c.*` 形式）。
    Wildcard(String),
}

impl WildcardEntry {
    /// 条目文本（通配或精确形式本身）。
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(text) | Self::Wildcard(text) => text,
        }
    }
}

/// 校验一条期望写入的豁免条目并返回其 ASCII 小写归一形式。
fn normalized_desired(entry: &str) -> Result<String, NativeError> {
    if entry.contains(';') {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "system_proxy_entry_contains_semicolon",
        ));
    }
    if entry.len() > MAX_ENTRY_LEN {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "system_proxy_entry_too_long",
        ));
    }
    if entry.is_empty() {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "system_proxy_entry_empty",
        ));
    }
    Ok(entry.to_ascii_lowercase())
}

/// 纯函数：原 `ProxyOverride` + EXV 目标条目 → 合并结果。
///
/// 语义（设计 §5.2 冻结契约）：
///
/// - `original` 为空串/纯空白：结果就是去重后的 `desired`；
/// - 否则按 `;` 拆分 `original`（空段跳过），ASCII case-insensitive 去重，
///   再按 `desired` 给定顺序追加未命中的条目；
/// - 总条数超过 [`MAX_MERGE_ENTRIES`] 报类型化错误（不是截断）；
/// - 任一条目含分号或长度 >[`MAX_ENTRY_LEN`] 报类型化错误；
/// - 非法输入是类型化错误，绝不静默丢弃。
///
/// # Errors
///
/// 任一期望条目含分号/为空/超长，或合并后总数超上限时返回 [`NativeError`]。
pub fn merge_bypass_entries(original: &str, desired: &[String]) -> Result<String, NativeError> {
    let mut seen: Vec<String> = Vec::new();
    let mut merged: Vec<String> = Vec::new();

    // original 是既有机器状态：拆段后走与 desired 同一条校验路径（除「空段跳过」
    // 与「空 original 整体合法」外，禁分号在拆分后天然不可能，超长仍拒绝）。
    let trimmed_original = original.trim();
    for segment in trimmed_original.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue; // 空段跳过（冻结语义）
        }
        if segment.len() > MAX_ENTRY_LEN {
            return Err(NativeError::from_win32(
                ERROR_INVALID_PARAMETER,
                "system_proxy_original_entry_too_long",
            ));
        }
        let key = segment.to_ascii_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            merged.push(segment.to_string());
        }
    }

    for entry in desired {
        let key = normalized_desired(entry)?;
        if !seen.contains(&key) {
            seen.push(key);
            merged.push(entry.clone());
        }
    }

    if merged.len() > MAX_MERGE_ENTRIES {
        return Err(NativeError::from_win32(
            ERROR_MORE_DATA,
            "system_proxy_merge_limit_exceeded",
        ));
    }

    Ok(merged.join(";"))
}

/// 纯函数：IPv4 CIDR → `WinInet` 通配豁免条目（拍板结论 2）。
///
/// 转换规则：
///
/// - `/8` 且网络地址对齐 → `Wildcard("a.*")`；
/// - `/16` 且网络地址对齐 → `Wildcard("a.b.*")`；
/// - `/24` 且网络地址对齐 → `Wildcard("a.b.c.*")`；
/// - 其余前缀长度（含 /0-/7、/9-/15、/17-/23、/25-/31、/32）→ 保持 IP 精确条目；
/// - 主机位非零（非对齐，如 `10.1.2.3/24`）→ 保持精确条目（不猜、不归一）。
///
/// # Errors
///
/// 输入不含 `/`、地址段或前缀长度解析失败时返回 [`NativeError`]。
pub fn cidr_to_wildcard(cidr: &str) -> Result<WildcardEntry, NativeError> {
    cidr_to_wildcard_opt(cidr).ok_or_else(|| {
        NativeError::from_win32(ERROR_INVALID_PARAMETER, "system_proxy_cidr_unparseable")
    })
}

/// [`cidr_to_wildcard`] 的 `Option` 变体。
///
/// 「格式无法按 IPv4 CIDR 解析」（缺 `/`、段溢出、前缀 >32 等）返回 `None`：
/// 本派生函数只负责转换，端到端条目校验由更早的 admission 层负责。
#[must_use]
pub fn cidr_to_wildcard_opt(cidr: &str) -> Option<WildcardEntry> {
    let (addr_part, prefix_part) = cidr.split_once('/')?;
    let prefix: u8 = prefix_part.parse().ok()?;
    if prefix > 32 {
        return None; // IPv4 前缀上限 32（routes.rs facts §3 同款边界）
    }
    let octets = parse_ipv4_octets(addr_part)?;
    let addr = u32::from_be_bytes(octets);
    if prefix < 32 {
        let host_bits = 32u32 - u32::from(prefix); // prefix<32 ⇒ host_bits∈1..=32
        // /0 的 32 位主机掩码是全 1（1u32<<32 溢出，checked_shl 取 None 后按 0 处理）。
        let mask = (1u32.checked_shl(host_bits).unwrap_or(0)).wrapping_sub(1);
        // 主机位非零 → 非对齐网络地址：保持精确条目，不猜、不归一。
        if addr & mask != 0 {
            return Some(WildcardEntry::Exact(cidr.to_string()));
        }
    }
    match prefix {
        8 => Some(WildcardEntry::Wildcard(format!("{}.*", octets[0]))),
        16 => Some(WildcardEntry::Wildcard(format!(
            "{}.{}.*",
            octets[0], octets[1]
        ))),
        24 => Some(WildcardEntry::Wildcard(format!(
            "{}.{}.{}.*",
            octets[0], octets[1], octets[2]
        ))),
        _ => Some(WildcardEntry::Exact(cidr.to_string())),
    }
}

/// 解析点分 IPv4 地址为四个八位组（恰四段，每段十进制 0-255，拒绝前导零，
/// 与 `Ipv4Addr::parse` 的严格性一致）。独立实现以区分「纯 IP 无 `/`」输入。
fn parse_ipv4_octets(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut parts = s.split('.');
    for slot in &mut octets {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        *slot = part.parse::<u8>().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(octets)
}

/// 以 NUL 结尾的宽字符指针（HSTRING 保证内部 NUL 结尾）。
fn wide_ptr(value: &HSTRING) -> PCWSTR {
    PCWSTR(value.as_ptr())
}

/// 打开发起用户的 `Internet Settings` 键并写入 `ProxyOverride`（`REG_SZ`）。
///
/// 打开 `HKU\<sid>\Software\Microsoft\Windows\CurrentVersion\Internet Settings`
/// （`KEY_READ|KEY_WRITE|KEY_SET_VALUE`），写完即关闭句柄。成功返回写入的
/// UTF-16 字节数（供上层日志），失败返回类型化错误。
fn write_registry_value(sid: &str, merged: &str) -> Result<usize, NativeError> {
    let subkey_path = format!("{sid}\\{INTERNET_SETTINGS_SUBKEY}");
    let subkey = HSTRING::from(subkey_path.as_str());
    let value_name = HSTRING::from("ProxyOverride");
    // REG_SZ 数据以双 NUL 结尾（注册表存储契约）；RegSetValueExW 按字节数写入。
    let mut data: Vec<u16> = merged.encode_utf16().collect();
    data.push(0);

    let mut key = HKEY::default();
    // SAFETY: subkey 是合法 NUL 结尾宽字符串；key 由系统填充；权限掩码按任务书
    // 冻结组合（KEY_READ|KEY_WRITE|KEY_SET_VALUE）。
    let open_rc = unsafe {
        RegOpenKeyExW(
            HKEY_USERS,
            wide_ptr(&subkey),
            None,
            KEY_READ | KEY_WRITE | KEY_SET_VALUE,
            &raw mut key,
        )
    };
    if open_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(
            open_rc.0,
            "system_proxy_reg_open_failed(HKU\\<sid>\\...\\Internet Settings)",
        ));
    }

    let data_bytes = data.len() * 2;
    // SAFETY: key 来自上方成功的 RegOpenKeyExW；value_name 是 NUL 结尾宽字符串；
    // data 生命周期覆盖本调用且长度与其真实字节数一致；REG_SZ 类型化写入。
    let set_rc = unsafe {
        RegSetValueExW(
            key,
            wide_ptr(&value_name),
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                data.as_ptr().cast::<u8>(),
                data_bytes,
            )),
        )
    };
    // 无论写成功与否都要关闭句柄；两者都失败时报先发生的 set 错误。
    // SAFETY: key 是打开中的注册表句柄，关闭后不再使用。
    let close_rc = unsafe { RegCloseKey(key) };
    if set_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(set_rc.0, "system_proxy_reg_set_failed(ProxyOverride)"));
    }
    if close_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(close_rc.0, "system_proxy_reg_close_failed"));
    }
    Ok(data_bytes)
}

/// 动态加载 wininet.dll 并发 `SETTINGS_CHANGED` + `REFRESH` 广播（设计 §5.2）。
///
/// DLL 或导出缺失 → `Ok(false)`（typed skip：无 `WinInet` 的环境设置在重启后由
/// 系统读取生效）；广播调用本身失败 → 类型化错误。
fn broadcast_wininet() -> Result<bool, NativeError> {
    let dll_name = HSTRING::from("wininet.dll");
    // SAFETY: dll_name 是合法 NUL 结尾宽字符串模块名；取得的模块引用在本函数内
    // 经 FreeLibrary 配对释放。
    let Ok(module) = (unsafe { LoadLibraryW(wide_ptr(&dll_name)) }) else {
        return Ok(false); // typed skip：DLL 缺失不是错误
    };
    let result = broadcast_via_module(module);
    // SAFETY: module 是本函数 LoadLibraryW 取得的引用，最后一次使用之后释放
    // （LoadLibraryW/FreeLibrary 引用计数配对）。
    unsafe { let _ = FreeLibrary(module); }
    result
}

/// 广播刷新（apply/restore 后让应用立即感知；wininet 缺失是 typed skip）。
pub fn broadcast_wininet_refresh() {
    let _ = broadcast_wininet();
}

/// 在已加载模块上解析 `InternetSetOptionW` 导出并发两条广播选项。
fn broadcast_via_module(module: HMODULE) -> Result<bool, NativeError> {
    const EXPORT_NAME: &[u8] = b"InternetSetOptionW\0";
    // SAFETY: module 是有效已加载模块句柄；EXPORT_NAME 是 NUL 结尾 ANSI 导出名。
    let far_proc = unsafe { GetProcAddress(module, PCSTR::from_raw(EXPORT_NAME.as_ptr())) }
        .ok_or_else(|| {
            NativeError::from_win32(ERROR_PROC_NOT_FOUND, "system_proxy_wininet_export_missing")
        })?;
    // SAFETY: wininet!InternetSetOptionW 的真实签名即此四参 BOOL 返回形式；
    // NULL hint + 两个通知选项均无需缓冲区（lpbuffer=NULL, length=0）。
    let internet_set_option_w: unsafe extern "system" fn(
        hinternet: *const core::ffi::c_void,
        dwoption: u32,
        lpbuffer: *const core::ffi::c_void,
        dwbufferlength: u32,
    ) -> i32 = unsafe { core::mem::transmute_copy(&far_proc) };

    // SAFETY: 广播调用约定为 NULL 句柄 + NULL 缓冲区（系统文档定义的通知语义）。
    unsafe { internet_set_option_w(core::ptr::null(), INTERNET_OPTION_SETTINGS_CHANGED, core::ptr::null(), 0) };
    // SAFETY: 同上；REFRESH 让 `WinInet` 立即重读设置。
    unsafe { internet_set_option_w(core::ptr::null(), INTERNET_OPTION_REFRESH, core::ptr::null(), 0) };
    Ok(true)
}

/// 写入发起用户的 `ProxyOverride` 并广播 `WinInet` 生效（设计 §5.2）。
///
/// 步骤：
///
/// 1. `RegOpenKeyExW(HKEY_USERS, "<sid>\Software\...\Internet Settings",
///    KEY_READ|KEY_WRITE|KEY_SET_VALUE)`；
/// 2. `RegSetValueExW(.., "ProxyOverride", REG_SZ, ..UTF-16 双 NUL 结尾..)`；
/// 3. 动态 `LoadLibraryW("wininet.dll")` → `InternetSetOptionW(NULL,
///    INTERNET_OPTION_SETTINGS_CHANGED)` + `(NULL, INTERNET_OPTION_REFRESH)`。
///
/// 返回 `Ok(true)` 表示注册表写入 + 广播都执行；`Ok(false)` 表示注册表已写入但
/// wininet.dll 或其导出缺失（typed skip，设置重启后生效）；广播 API 失败是错误。
///
/// 注意：本任务不做 prestate 捕获（TK1 journal 扩展职责），只写与广播。
///
/// # Errors
///
/// 注册表打开/写入/关闭失败、SID 含 NUL、或广播调用失败时返回 [`NativeError`]。
pub fn write_override_for_user(sid: &str, merged: &str) -> Result<bool, NativeError> {
    if sid.contains('\0') {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "system_proxy_sid_contains_nul",
        ));
    }
    write_registry_value(sid, merged)?;
    broadcast_wininet()
}

/// 按 `RawValue` 的 type+data 字节级写回发起用户的任一受管值（设计 §5.3
/// 「存在 → 按 type+data 字节级写回」）。
///
/// [`RawValue::Sz`] 以 UTF-16LE 单元 + 尾部 NUL（REG_SZ 存储契约）；
/// [`RawValue::Dword`] 以 4 字节小端；[`RawValue::Other`] 原样写回原始
/// type + 数据字节。值打开/写入失败 → 类型化错误。
///
/// # Errors
///
/// 注册表打开/写入失败、SID 含 NUL 时返回 [`NativeError`]。
pub fn write_back_value_for_user(sid: &str, value: ValueName, data: &RawValue) -> Result<(), NativeError> {
    if sid.contains('\0') {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "system_proxy_sid_contains_nul",
        ));
    }
    let subkey_path = format!("{sid}\\{INTERNET_SETTINGS_SUBKEY}");
    let subkey = HSTRING::from(subkey_path.as_str());
    let value_name = HSTRING::from(value.as_str());

    let mut key = HKEY::default();
    // SAFETY: subkey 是合法 NUL 结尾宽字符串；key 由系统填充；权限按冻结组合。
    let open_rc = unsafe {
        RegOpenKeyExW(
            HKEY_USERS,
            wide_ptr(&subkey),
            None,
            KEY_READ | KEY_WRITE | KEY_SET_VALUE,
            &raw mut key,
        )
    };
    if open_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(
            open_rc.0,
            "system_proxy_reg_open_failed(HKU\\<sid>\\...\\Internet Settings)",
        ));
    }

    // 构造 (类型码, 数据字节)：Sz → REG_SZ + UTF-16 双 NUL；Dword → REG_DWORD + 4B LE；
    // Other → 原 type + 原数据。
    let (kind, bytes) = match data {
        RawValue::Absent => {
            // 调用方不该以 Absent 调写回（删除走 delete_value_for_user）；防御返回成功。
            let _ = unsafe { RegCloseKey(key) };
            return Ok(());
        }
        RawValue::Dword(v) => (REG_DWORD, v.to_le_bytes().to_vec()),
        RawValue::Sz(text) => {
            let mut units: Vec<u16> = text.encode_utf16().collect();
            units.push(0);
            (REG_SZ, units.iter().flat_map(|u| u.to_le_bytes()).collect())
        }
        RawValue::Other { r#type, data } => (
            windows::Win32::System::Registry::REG_VALUE_TYPE(*r#type),
            data.clone(),
        ),
    };

    // SAFETY: key 来自上方成功的 RegOpenKeyExW；value_name 是 NUL 结尾宽字符串；
    // bytes 生命周期覆盖本调用且长度与其真实字节数一致。
    let set_rc = unsafe {
        RegSetValueExW(
            key,
            wide_ptr(&value_name),
            None,
            kind,
            Some(bytes.as_slice()),
        )
    };
    // SAFETY: key 是打开中的注册表句柄。
    let close_rc = unsafe { RegCloseKey(key) };
    if set_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(
            set_rc.0,
            "system_proxy_reg_set_failed",
        ));
    }
    if close_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(close_rc.0, "system_proxy_reg_close_failed"));
    }
    Ok(())
}

/// 删除发起用户的受管值（设计 §5.3「原本不存在 → 还原时删除该值」）。
///
/// 值已不存在（ERROR_FILE_NOT_FOUND）→ 幂等成功（删除意图已满足）。
///
/// # Errors
///
/// 打开失败、删除非「不存在」失败时返回类型化错误。
pub fn delete_value_for_user(sid: &str, value: ValueName) -> Result<(), NativeError> {
    if sid.contains('\0') {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER,
            "system_proxy_sid_contains_nul",
        ));
    }
    let subkey_path = format!("{sid}\\{INTERNET_SETTINGS_SUBKEY}");
    let subkey = HSTRING::from(subkey_path.as_str());
    let value_name = HSTRING::from(value.as_str());

    let mut key = HKEY::default();
    // SAFETY: subkey 合法；key 系统填充；删除只需 KEY_SET_VALUE。
    let open_rc = unsafe {
        RegOpenKeyExW(
            HKEY_USERS,
            wide_ptr(&subkey),
            None,
            KEY_SET_VALUE,
            &raw mut key,
        )
    };
    if open_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(
            open_rc.0,
            "system_proxy_reg_open_failed_for_delete",
        ));
    }
    // SAFETY: key 有效；value_name NUL 结尾。
    let del_rc = unsafe { RegDeleteValueW(key, wide_ptr(&value_name)) };
    // SAFETY: key 是打开中的注册表句柄。
    let close_rc = unsafe { RegCloseKey(key) };
    if del_rc != WIN32_ERROR(ERROR_SUCCESS) {
        if del_rc.0 == ERROR_FILE_NOT_FOUND {
            return Ok(()); // 幂等：值已不存在，删除意图已满足。
        }
        return Err(NativeError::from_win32(del_rc.0, "system_proxy_reg_delete_failed"));
    }
    if close_rc != WIN32_ERROR(ERROR_SUCCESS) {
        return Err(NativeError::from_win32(close_rc.0, "system_proxy_reg_close_failed"));
    }
    Ok(())
}

/// `ERROR_FILE_NOT_FOUND`（2）：删除时值不存在的幂等跳过。
const ERROR_FILE_NOT_FOUND: u32 = 2;

// PROBE_MARKER_xyz
