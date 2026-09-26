

use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_USERS, KEY_READ, REG_VALUE_TYPE,
    REG_DWORD, REG_SZ,
};

use crate::native_error::NativeError;

/// 错误码 token：注册表原始值自洽性失败（如 ProxyEnable=false 却解析出端点）。
pub const ERROR_SYSTEM_PROXY_MALFORMED: &str = "system_proxy_malformed";

/// 错误码 token：单个端点条目无法接受（空 host / 端口越界 / 带 `user@host` 凭据）。
pub const ERROR_SYSTEM_PROXY_ENDPOINT_INVALID: &str = "system_proxy_endpoint_invalid";

/// typed 错误码编码：`NativeError.code` 保留高位标记（非 Win32 原生码空间），
/// 低 16 位放模块内子码。对齐 `native_error.rs` 的 code 字段惯例（原生码直存），
/// 高位标记保证与任何 `GetLastError` 现役码不冲突。
const NATIVE_CODE_FLAG: u32 = 0x8000_0000;

/// 子码：`system_proxy_malformed`。
const NATIVE_CODE_SYSTEM_PROXY_MALFORMED: u32 = 0x0000_0001;

/// 子码：`system_proxy_endpoint_invalid`。
const NATIVE_CODE_SYSTEM_PROXY_ENDPOINT_INVALID: u32 = 0x0000_0002;

/// 本模块使用的参数类错误 raw code（`ERROR_INVALID_PARAMETER` = 87，Unknown 类）。
const ERROR_INVALID_PARAMETER_CODE: u32 = 87;

/// REG_DWORD 数据损坏时的占位 raw code（`ERROR_NO_SYSTEM_RESOURCES` = 1450，Unknown 类）。
const ERROR_NO_SYSTEM_RESOURCES_CODE: u32 = 1450;

/// 单个代理端点（规范化后，无凭据；设计 §5.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyEndpoint {
    /// 协议类别（socks5/socks4 归一为 socks）。
    pub kind: ProxyKind,
    /// 主机（IPv6 为去括号后的裸地址，如 `::1`）。
    pub host: String,
    /// 端口。
    pub port: u16,
}

/// 代理协议类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyKind {
    /// `http=…` 条目（以及无 `=` 单值形式的隐含 http/secure 双挂）。
    Http,
    /// `https=…` 条目。
    Secure,
    /// `socks=…` 条目（socks5/socks4 归一到此）。
    Socks,
}

/// 注册表单值语义（存在性 + 类型 + 数据），供 journal prestate 复用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawValue {
    /// 值不存在。
    Absent,
    /// `REG_DWORD`（Win32 API 返回的小端字节序列按原生 u32 读回）。
    Dword(u32),
    /// `REG_SZ`（UTF-16，去尾部 NUL）。
    Sz(String),
    /// 存在但类型不在本模块常规读取范围（`REG_EXPAND_SZ` / `REG_BINARY` /
    /// `REG_MULTI_SZ` …）：记录原始类型码与原始数据字节，供 prestate 对账与
    /// 字节级精确还原（type+data 写回），不解释内容。
    Other {
        /// 原始注册表类型码。
        r#type: u32,
        /// 原始数据字节（原样保留，还原时原样写回）。
        data: Vec<u8>,
    },
}

impl RawValue {
    /// 是否存在（任意类型）。
    #[must_use]
    pub fn exists(&self) -> bool {
        !matches!(self, Self::Absent)
    }

    /// REG_SZ 文本（仅 [`RawValue::Sz`] 有值；其余 None）。
    #[must_use]
    pub fn as_sz(&self) -> Option<&str> {
        match self {
            Self::Sz(text) => Some(text.as_str()),
            _ => None,
        }
    }

    /// `DWORD` 数值（仅 [`RawValue::Dword`] 有值；其余 None）。
    #[must_use]
    pub fn as_dword(&self) -> Option<u32> {
        match self {
            Self::Dword(value) => Some(*value),
            _ => None,
        }
    }
}

/// `Internet Settings` 五原始值 + 每值 exists/type/data 语义（可注入，无 Win32 依赖；
/// journal prestate 直接复用本结构）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawInternetSettings {
    /// `ProxyEnable`（`REG_DWORD`；非 0 视为 true）。
    pub proxy_enable: RawValue,
    /// `ProxyServer`（`REG_SZ`）。
    pub proxy_server: RawValue,
    /// `ProxyOverride`（`REG_SZ`，分号分隔豁免列表）。
    pub proxy_override: RawValue,
    /// `AutoConfigURL`（`REG_SZ`，PAC 地址）。
    pub auto_config_url: RawValue,
    /// `AutoDetect`（`REG_DWORD`；非 0 视为 WPAD 开启）。
    pub auto_detect: RawValue,
}

/// 系统代理模式（设计 §5.1 mode 判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemProxyMode {
    /// 无端点且无 PAC/AutoDetect。
    Disabled,
    /// 有手动端点。
    Manual,
    /// 有 PAC 或 AutoDetect=true。
    Automatic,
    /// 手动与自动并存。
    Mixed,
}

/// 规范化系统代理快照（present / mode / endpoints / bypass_entries / pac_url /
/// auto_discovery）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxySnapshot {
    /// 四态模式。
    pub mode: SystemProxyMode,
    /// 规范化端点列表（Disabled 态必须为空，否则 [`snapshot_from_raw`] 报 malformed）。
    pub endpoints: Vec<ProxyEndpoint>,
    /// `ProxyOverride` 按分号拆分的豁免条目（空段跳过）。
    pub bypass_entries: Vec<String>,
    /// PAC 地址（`AutoConfigURL` 非空时存在）。
    pub pac_url: Option<String>,
    /// WPAD 自动发现（`AutoDetect != 0`）。
    pub auto_discovery: bool,
}

impl SystemProxySnapshot {
    /// 是否存在系统代理（Manual / Automatic / Mixed 任一）。
    ///
    /// 注意：这是**语义判定**而非布尔直读——即使 `ProxyEnable=0`，只要 `PAC` /
    /// `AutoDetect` 在位即视为 automatic present（设计 §2「分类是纯函数」的自洽性由
    /// [`snapshot_from_raw`] 保证，这里只看结果模式）。
    #[must_use]
    pub fn is_present(&self) -> bool {
        self.mode != SystemProxyMode::Disabled
    }
}

/// 四态拓扑分类（设计 §2 表格；TUN 维度来自 `proxy_tun.rs`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyKind {
    /// 无系统代理、无第三方 TUN。
    T0,
    /// 有系统代理、无第三方 TUN。
    T1,
    /// 无系统代理、有第三方 TUN。
    T2,
    /// 双有。
    T3,
}

// ---------------------------------------------------------------------------
// 纯函数（注入单测；不依赖真机注册表）。
// ---------------------------------------------------------------------------

/// 注册表原始值 → 规范化快照（设计 §5.1 解析语义）。
///
/// - `ProxyEnable=false` 或无端点且无 PAC → Disabled；此时若解析出端点则报
///   malformed（fail-closed 不猜）。
/// - `ProxyServer` 无 `=`：单值同时作为 http 与 secure 端点；含 `=`：按
///   `http=…;https=…;socks=…` 分号拆分，socks5/socks4 归一为 socks。
/// - IPv6 括号形式 `[::1]:7890` 支持（host 去括号存储）。
/// - 带 `user@host` 凭据形式的条目拒绝报 `system_proxy_endpoint_invalid`
///   （fail-closed，不静默剥离）。
/// - mode 判定：manual=有端点；automatic=有 pac_url 或 `AutoDetect=true`；
///   两者皆有=Mixed；皆无=Disabled。
/// - `ProxyOverride` 按分号拆分为 bypass_entries（空段跳过）。
///
/// # Errors
///
/// 自洽性破坏 → [`ERROR_SYSTEM_PROXY_MALFORMED`]；端点条目不可接受 →
/// [`ERROR_SYSTEM_PROXY_ENDPOINT_INVALID`]；均为 typed [`NativeError`]。
pub fn snapshot_from_raw(raw: &RawInternetSettings) -> Result<SystemProxySnapshot, NativeError> {
    let proxy_enabled = raw.proxy_enable.as_dword().unwrap_or(0) != 0;
    let pac_url = pac_url_from(raw);
    let auto_discovery = raw.auto_detect.as_dword().unwrap_or(0) != 0;
    let bypass_entries = bypass_from(raw);
    let endpoints = endpoints_from(raw)?;

    // 自洽性（fail-closed）：ProxyEnable=false 却解析出端点 → malformed，不猜。
    if !proxy_enabled && !endpoints.is_empty() {
        return Err(malformed(
            "ProxyEnable=false 但 ProxyServer 解析出端点（fail-closed 不猜）",
        ));
    }

    let mode = match (!endpoints.is_empty(), pac_url.is_some() || auto_discovery) {
        (true, true) => SystemProxyMode::Mixed,
        (true, false) => SystemProxyMode::Manual,
        (false, true) => SystemProxyMode::Automatic,
        (false, false) => SystemProxyMode::Disabled,
    };

    Ok(SystemProxySnapshot {
        mode,
        endpoints,
        bypass_entries,
        pac_url,
        auto_discovery,
    })
}

/// `(proxy_present, tunnel_present) -> TopologyKind`（对齐设计 §2 表格：
/// 无无=T0，有代理无TUN=T1，无代理有TUN=T2，双有=T3）。纯函数。
#[must_use]
pub fn classify(proxy_present: bool, tunnel_present: bool) -> TopologyKind {
    match (proxy_present, tunnel_present) {
        (false, false) => TopologyKind::T0,
        (true, false) => TopologyKind::T1,
        (false, true) => TopologyKind::T2,
        (true, true) => TopologyKind::T3,
    }
}

/// `AutoConfigURL` 非空（去空白后）→ PAC 地址。
fn pac_url_from(raw: &RawInternetSettings) -> Option<String> {
    let text = raw.auto_config_url.as_sz()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `ProxyOverride` 按分号拆分，空段跳过（保留段内原有大小写与空白之外的原样字符，
/// 仅去除段两端空白）。
fn bypass_from(raw: &RawInternetSettings) -> Vec<String> {
    raw.proxy_override
        .as_sz()
        .map(|text| {
            text.split(';')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `ProxyServer` → 规范化端点列表。
///
/// - 无 `=`：单值形式，同一 host:port 同时作为 http 与 secure 端点（C++ 参考契约）。
/// - 含 `=`：分号拆分逐条 `<scheme>=<host:port>`；scheme 大小写不敏感，
///   socks5/socks4 归一为 socks。
/// - IPv6 括号形式 `[::1]:7890` 支持。
/// - 空 scheme 段（如 `;http=…`）跳过；未知 scheme 报 malformed。
fn endpoints_from(raw: &RawInternetSettings) -> Result<Vec<ProxyEndpoint>, NativeError> {
    let server = match raw.proxy_server.as_sz() {
        Some(text) if !text.trim().is_empty() => text.trim(),
        _ => return Ok(Vec::new()),
    };

    if !server.contains('=') {
        // 单值形式：同一端点同时充当 http 与 secure（C++ 参考实现行为契约）。
        let endpoint = parse_endpoint(server)?;
        return Ok(vec![
            ProxyEndpoint { kind: ProxyKind::Http, host: endpoint.host.clone(), port: endpoint.port },
            ProxyEndpoint { kind: ProxyKind::Secure, host: endpoint.host, port: endpoint.port },
        ]);
    }

    let mut out = Vec::new();
    for entry in server.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((scheme_text, endpoint_text)) = entry.split_once('=') else {
            return Err(malformed("ProxyServer 含 '=' 但某分段缺少 '='"));
        };
        let scheme = scheme_text.trim().to_ascii_lowercase();
        let kind = match scheme.as_str() {
            "http" => ProxyKind::Http,
            "https" | "secure" => ProxyKind::Secure,
            "socks" | "socks5" | "socks4" => ProxyKind::Socks,
            "" => continue, // 空 scheme 段跳过（防御式，正常注册表不产生）
            other => {
                return Err(malformed(&format!(
                    "ProxyServer 含未知 scheme '{other}'"
                )))
            }
        };
        out.push(ProxyEndpoint { kind, ..parse_endpoint(endpoint_text)? });
    }
    Ok(out)
}

/// 解析单个 `<host>:<port>` 条目为端点（支持 IPv6 括号形式；拒绝凭据形式）。
fn parse_endpoint(entry: &str) -> Result<ProxyEndpoint, NativeError> {
    // 凭据形式 user@host[:port]：fail-closed 拒绝，不静默剥离。
    if entry.contains('@') {
        return Err(endpoint_invalid("端点带凭据形式 user@host"));
    }
    // IPv6 括号形式 [::1]:7890。
    if let Some(rest) = entry.strip_prefix('[') {
        let Some((host, port_text)) = rest.split_once(']') else {
            return Err(endpoint_invalid("IPv6 括号形式缺少 ']'"));
        };
        if host.is_empty() {
            return Err(endpoint_invalid("IPv6 括号内 host 为空"));
        }
        let Some(port_text) = port_text.strip_prefix(':') else {
            return Err(endpoint_invalid("IPv6 括号形式缺少 ':port'"));
        };
        let port = parse_port(port_text)?;
        return Ok(ProxyEndpoint { kind: ProxyKind::Http, host: host.to_string(), port });
    }

    let Some((host, port_text)) = entry.rsplit_once(':') else {
        return Err(endpoint_invalid("端点缺少 ':port'"));
    };
    // 最后一个冒号之后还应在 host 段残留冒号 / 括号 → 裸 IPv6，要求括号形式。
    if host.contains(':') || host.contains('[') || host.contains(']') {
        return Err(endpoint_invalid("裸 IPv6 必须使用 [host]:port 括号形式"));
    }
    if host.is_empty() {
        return Err(endpoint_invalid("端点 host 为空"));
    }
    let port = parse_port(port_text)?;
    Ok(ProxyEndpoint { kind: ProxyKind::Http, host: host.to_string(), port })
}

/// 解析十进制端口（u16 范围）。
fn parse_port(text: &str) -> Result<u16, NativeError> {
    text.parse::<u16>()
        .map_err(|_| endpoint_invalid("端口不是 0..=65535 十进制数"))
}

/// malformed typed 错误（kind 走既有 Storage 分类；code 用保留高位标记）。
fn malformed(message: &str) -> NativeError {
    NativeError {
        kind: crate::native_error::NativeErrorKind::Storage,
        code: NATIVE_CODE_FLAG | NATIVE_CODE_SYSTEM_PROXY_MALFORMED,
        message: format!("{ERROR_SYSTEM_PROXY_MALFORMED}: {message}"),
    }
}

/// endpoint invalid typed 错误（同上编码惯例）。
fn endpoint_invalid(message: &str) -> NativeError {
    NativeError {
        kind: crate::native_error::NativeErrorKind::Storage,
        code: NATIVE_CODE_FLAG | NATIVE_CODE_SYSTEM_PROXY_ENDPOINT_INVALID,
        message: format!("{ERROR_SYSTEM_PROXY_ENDPOINT_INVALID}: {message}"),
    }
}

// ---------------------------------------------------------------------------
// Win32 直调路径（薄封装；真机路径不要求单测）。
// ---------------------------------------------------------------------------

const INTERNET_SETTINGS_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// SID 形状门禁：`S-1-<十进制子授权…>`（至少一个子授权，全段十进制）。
/// 仅做形状校验（在触碰注册表前拒绝明显非法输入），不验证 SID 真实存在。
fn is_well_shaped_sid(sid: &str) -> bool {
    let Some(rest) = sid.strip_prefix("S-1-") else {
        return false;
    };
    !rest.is_empty()
        && rest.split('-').all(|part| {
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// 读取指定用户 `SID` 的 `Internet Settings` 五原始值
/// （`RegOpenKeyExW(HKU\<sid>\…)` 五次 `RegQueryValueExW`，处理
/// `REG_SZ` / `REG_DWORD` 与值不存在）。
///
/// 键不存在返回 typed 错误（Storage 类，raw code 2/3 经 `from_win32` 映射），
/// 不 panic。detect-and-report only：只读。
///
/// # Errors
///
/// SID 形状不合法 → typed 参数错误；键打不开或单值查询遇到非
/// `ERROR_FILE_NOT_FOUND` 失败 → [`NativeError`]。
pub fn capture_for_user(sid: &str) -> Result<RawInternetSettings, NativeError> {
    if !is_well_shaped_sid(sid) {
        return Err(NativeError::from_win32(
            ERROR_INVALID_PARAMETER_CODE,
            "SID 形状不合法（期望 S-1-… 十进制子授权）",
        ));
    }
    let path = HSTRING::from(format!(r"{sid}\{INTERNET_SETTINGS_SUBKEY}"));
    let mut key = HKEY::default();
    // SAFETY: path 有效；key 输出句柄，用后关闭（下方统一 RegCloseKey）。
    let rc = unsafe { RegOpenKeyExW(HKEY_USERS, &path, Some(0), KEY_READ, &raw mut key) };
    if rc.0 != 0 {
        return Err(NativeError::from_win32(rc.0, "RegOpenKeyExW HKU\\<sid>\\Internet Settings 失败"));
    }
    let result = query_values(key);
    // SAFETY: key 是本函数打开的句柄；无论成败都要关闭。
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

/// 逐值 `RegQueryValueExW`（键已打开）。值不存在（ERROR_FILE_NOT_FOUND）→ Absent；
/// 其他失败 → typed 错误。
fn query_values(key: HKEY) -> Result<RawInternetSettings, NativeError> {
    Ok(RawInternetSettings {
        proxy_enable: query_value(key, "ProxyEnable")?,
        proxy_server: query_value(key, "ProxyServer")?,
        proxy_override: query_value(key, "ProxyOverride")?,
        auto_config_url: query_value(key, "AutoConfigURL")?,
        auto_detect: query_value(key, "AutoDetect")?,
    })
}

/// 单值查询：REG_DWORD → [`RawValue::Dword`]；REG_SZ → [`RawValue::Sz`]（去 NUL）；
/// 其他类型 → [`RawValue::Other`]（type + 原始数据字节）；不存在 → [`RawValue::Absent`]。
fn query_value(key: HKEY, name: &str) -> Result<RawValue, NativeError> {
    let name_w = HSTRING::from(name);
    let mut kind = REG_VALUE_TYPE(0);
    let mut len = 0u32;
    // SAFETY: 第一遍只取所需字节数（buffer 传 None）；kind/len 由系统填充。
    let rc = unsafe { RegQueryValueExW(key, &name_w, None, Some(&raw mut kind), None, Some(&raw mut len)) };
    if rc.0 == ERROR_FILE_NOT_FOUND.0 || rc.0 == ERROR_PATH_NOT_FOUND.0 {
        return Ok(RawValue::Absent);
    }
    if rc.0 != 0 {
        return Err(NativeError::from_win32(rc.0, "RegQueryValueExW 取值大小失败"));
    }
    if len == 0 {
        return Ok(match kind {
            x if x == REG_SZ => RawValue::Sz(String::new()),
            x if x == REG_DWORD => RawValue::Dword(0),
            other => RawValue::Other {
                r#type: other.0,
                data: Vec::new(),
            },
        });
    }

    let mut buff = vec![0u8; len as usize];
    let mut kind2 = REG_VALUE_TYPE(0);
    let mut len2 = len;
    // SAFETY: buff 按 len 分配；len2 输入输出。
    let rc = unsafe {
        RegQueryValueExW(
            key,
            &name_w,
            None,
            Some(&raw mut kind2),
            Some(buff.as_mut_ptr()),
            Some(&raw mut len2),
        )
    };
    if rc.0 == ERROR_FILE_NOT_FOUND.0 || rc.0 == ERROR_PATH_NOT_FOUND.0 {
        return Ok(RawValue::Absent); // TOCTOU：两次调用之间值被删除，按不存在处理
    }
    if rc.0 != 0 {
        return Err(NativeError::from_win32(rc.0, "RegQueryValueExW 读值失败"));
    }
    buff.truncate(len2 as usize);
    match kind2 {
        x if x == REG_SZ => {
            let utf16: Vec<u16> = buff
                .chunks_exact(2)
                .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
                .take_while(|ch| *ch != 0)
                .collect();
            Ok(RawValue::Sz(String::from_utf16(&utf16).unwrap_or_default()))
        }
        x if x == REG_DWORD => {
            if buff.len() < 4 {
                return Err(NativeError::from_win32(
                    ERROR_NO_SYSTEM_RESOURCES_CODE,
                    "REG_DWORD 数据不足 4 字节",
                ));
            }
            let bytes = [buff[0], buff[1], buff[2], buff[3]];
            Ok(RawValue::Dword(u32::from_le_bytes(bytes)))
        }
        other => Ok(RawValue::Other {
            r#type: other.0,
            data: buff,
        }),
    }
}

