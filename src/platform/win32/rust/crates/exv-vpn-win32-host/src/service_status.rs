
use std::path::{Path, PathBuf};

use exv_vpn_wire::generated::ServiceSelfReport;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceConfigW, QueryServiceStatus,
    SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_CONTINUE_PENDING, SERVICE_PAUSED,
    SERVICE_PAUSE_PENDING, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
    SERVICE_START_PENDING, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_STATUS,
};

/// 服务生命周期状态（`SERVICE_STATUS.dwCurrentState` + 安装存在性的领域形态）。
///
/// 单一权威状态：`NotInstalled` 是一等状态（`OpenServiceW` → DOES_NOT_EXIST），不再有
/// 独立的 `installed: bool` 与 `state` 并列——消除「未安装但 Running」类矛盾组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// 未安装（`OpenServiceW` → `ERROR_SERVICE_DOES_NOT_EXIST`）。
    NotInstalled,
    /// `SERVICE_STOPPED`（已安装但未运行）。
    Stopped,
    /// `SERVICE_START_PENDING`（启动中）。
    StartPending,
    /// `SERVICE_RUNNING`（运行中）。
    Running,
    /// `SERVICE_STOP_PENDING`（停止中）。
    StopPending,
    /// `SERVICE_PAUSE_PENDING`（暂停中）。
    PausePending,
    /// `SERVICE_PAUSED`（已暂停）。
    Paused,
    /// `SERVICE_CONTINUE_PENDING`（继续中）。
    ContinuePending,
    /// 其它/未知原始状态码（fail-closed 兜底）。
    Other(u32),
}

impl ServiceState {
    /// 从 SCM 原始事实构造（安装存在性 + `dwCurrentState`；纯函数）。
    #[must_use]
    pub fn from_scm(installed: bool, raw_state: u32) -> Self {
        if !installed {
            return Self::NotInstalled;
        }
        map_raw_state(raw_state)
    }

    /// 是否已安装（`state != NotInstalled`）。
    #[must_use]
    pub fn is_installed(&self) -> bool {
        !matches!(self, Self::NotInstalled)
    }

    /// 是否处于 SCM 过渡态（start/stop/pause/continue pending——操作在途，未达终态）。
    #[must_use]
    pub fn is_transitioning(&self) -> bool {
        matches!(
            self,
            Self::StartPending | Self::StopPending | Self::PausePending | Self::ContinuePending
        )
    }

    /// 是否已达转移终态（安装/卸载/启停完成的可靠判据）。
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::NotInstalled | Self::Stopped)
    }

    /// 稳定 wire 字符串（`ServiceStatus.state` 取值）。`NotInstalled` 由 `installed=false`
    /// 表达，此处返回占位 `"stopped"`，与既有 wire 行为一致（前端在 `!installed` 时不读
    /// `state`）。
    #[must_use]
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            Self::NotInstalled => "stopped",
            Self::Stopped => "stopped",
            Self::StartPending => "start_pending",
            Self::Running => "running",
            Self::StopPending => "stop_pending",
            Self::PausePending => "pause_pending",
            Self::Paused => "paused",
            Self::ContinuePending => "continue_pending",
            Self::Other(_) => "other",
        }
    }
}

/// 服务状态快照（host 非提权查询的结果；UI 服务感知 / D6 的输入）。`state` 是单一权威
/// （含 `NotInstalled`），不再有并列的 `installed` 布尔。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatusSnapshot {
    /// 当前服务生命周期状态（`NotInstalled` = 未安装）。
    pub state: ServiceState,
    /// 服务二进制路径（`QUERY_SERVICE_CONFIGW.lpBinaryPathName`；含启动参数）。
    pub binary_path: Option<String>,
}

/// 原始 SCM 查询事实（从 windows API 读出；fake 测试注入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawServiceQuery {
    /// 服务是否已安装（`OpenServiceW` 成功）。
    pub installed: bool,
    /// `SERVICE_STATUS.dwCurrentState` 原始值。
    pub raw_state: u32,
    /// 二进制路径（`lpBinaryPathName`）。
    pub binary_path: Option<String>,
}

/// SCM 状态查询原语（测试注入 fake；生产 = [`RealServiceStatusSource`]）。
pub trait ServiceStatusSource: Send + Sync {
    /// 查询服务的原始 SCM 事实。
    ///
    /// # Errors
    /// SCM 打开 / 查询失败（除「服务未安装」外）→ 携带原因的字符串。
    fn query_raw(&self, service_name: &str) -> Result<RawServiceQuery, String>;
}

/// 真实 SCM 源：`OpenSCManagerW(SC_MANAGER_CONNECT)` → `OpenServiceW(SERVICE_QUERY_STATUS |
/// SERVICE_QUERY_CONFIG)` → `QueryServiceStatus` + `QueryServiceConfigW`。**非提权**。
pub struct RealServiceStatusSource;

impl ServiceStatusSource for RealServiceStatusSource {
    fn query_raw(&self, service_name: &str) -> Result<RawServiceQuery, String> {
        query_raw_scm(service_name)
    }
}

/// 查询服务状态快照（注入源；生产用 [`RealServiceStatusSource`]）。
///
/// # Errors
/// SCM 打开 / 查询失败（除「服务未安装」→ `Ok(installed=false)` 外）。
pub fn query_service_status(
    source: &dyn ServiceStatusSource,
    service_name: &str,
) -> Result<ServiceStatusSnapshot, String> {
    Ok(snapshot_from_raw(&source.query_raw(service_name)?))
}

/// 原始 SCM 事实 → 领域快照（纯函数，可单测）。
fn snapshot_from_raw(raw: &RawServiceQuery) -> ServiceStatusSnapshot {
    ServiceStatusSnapshot {
        state: ServiceState::from_scm(raw.installed, raw.raw_state),
        binary_path: raw.binary_path.clone(),
    }
}

/// 原始状态码 → 领域状态（纯函数；安装存在性由 [`ServiceState::from_scm`] 处理——本函数
/// 只把 `dwCurrentState` 原始码映射到已安装态的领域状态）。
fn map_raw_state(raw: u32) -> ServiceState {
    if raw == SERVICE_STOPPED.0 {
        ServiceState::Stopped
    } else if raw == SERVICE_START_PENDING.0 {
        ServiceState::StartPending
    } else if raw == SERVICE_STOP_PENDING.0 {
        ServiceState::StopPending
    } else if raw == SERVICE_RUNNING.0 {
        ServiceState::Running
    } else if raw == SERVICE_PAUSE_PENDING.0 {
        ServiceState::PausePending
    } else if raw == SERVICE_PAUSED.0 {
        ServiceState::Paused
    } else if raw == SERVICE_CONTINUE_PENDING.0 {
        ServiceState::ContinuePending
    } else {
        ServiceState::Other(raw)
    }
}

/// 判断 windows `Result` 错误是否为指定 Win32 错误码（`HRESULT::from_win32` 编码：
/// `0x8007 << 16 | code`，取低 16 位）。
fn is_win32_error(e: &windows::core::Error, code: u32) -> bool {
    // HRESULT 是 i32；Win32 错误码编码在低 16 位（HRESULT_FROM_WIN32）。
    (e.code().0 as u32) & 0xFFFF == code
}

/// `ERROR_SERVICE_DOES_NOT_EXIST`（1060）：服务未安装。
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;

fn query_raw_scm(service_name: &str) -> Result<RawServiceQuery, String> {
    // SAFETY: OpenSCManagerW 打开本机 SCM，仅请求 SC_MANAGER_CONNECT（非提权查询）；
    // 返回句柄随后由 CloseServiceHandle 关闭（RAII 手动）。
    let manager = unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
    }
    .map_err(|e| format!("open service manager (SC_MANAGER_CONNECT): {e}"))?;
    let result = query_raw_under_manager(manager, service_name);
    // SAFETY: manager 是打开的 SCM 句柄，使用后关闭。
    unsafe {
        let _ = CloseServiceHandle(manager);
    }
    result
}

fn query_raw_under_manager(
    manager: SC_HANDLE,
    service_name: &str,
) -> Result<RawServiceQuery, String> {
    let name = HSTRING::from(service_name);
    // SAFETY: `&name` 是活 HSTRING；OpenServiceW 只请求 QUERY_* 权限（非提权）。
    let service = unsafe {
        OpenServiceW(manager, &name, SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG)
    };
    let service = match service {
        Ok(h) => h,
        Err(e) if is_win32_error(&e, ERROR_SERVICE_DOES_NOT_EXIST) => {
            return Ok(RawServiceQuery {
                installed: false,
                raw_state: SERVICE_STOPPED.0,
                binary_path: None,
            });
        }
        Err(e) => return Err(format!("open service: {e}")),
    };
    let result = read_service_facts(service);
    // SAFETY: service 是打开的句柄，使用后关闭。
    unsafe {
        let _ = CloseServiceHandle(service);
    }
    result
}

fn read_service_facts(service: SC_HANDLE) -> Result<RawServiceQuery, String> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `status` 是活 out-param 缓冲。
    unsafe { QueryServiceStatus(service, &mut status) }
        .map_err(|e| format!("query service status: {e}"))?;
    let raw_state = status.dwCurrentState.0;

    let binary_path = query_binary_path(service)?;

    Ok(RawServiceQuery {
        installed: true,
        raw_state,
        binary_path,
    })
}

/// 查询服务二进制路径（`QueryServiceConfigW`；两次调用——先查大小再读）。
fn query_binary_path(service: SC_HANDLE) -> Result<Option<String>, String> {
    let mut bytes_needed = 0u32;
    // 第一次调用（空缓冲）返回 ERROR_INSUFFICIENT_BUFFER 并写入需要的大小——忽略错误。
    let _ = unsafe { QueryServiceConfigW(service, None, 0, &mut bytes_needed) };
    let mut buffer = vec![0u8; bytes_needed.max(8_192) as usize];
    let mut written = 0u32;
    // SAFETY: buffer 是活缓冲，大小正确；随后从同一缓冲读 QUERY_SERVICE_CONFIGW。
    unsafe {
        QueryServiceConfigW(
            service,
            Some(buffer.as_mut_ptr().cast()),
            buffer.len() as u32,
            &mut written,
        )
    }
    .map_err(|e| format!("query service config: {e}"))?;

    // SAFETY: QueryServiceConfigW 成功后 buffer 头部分是完整的 QUERY_SERVICE_CONFIGW。
    let config = unsafe { &*(buffer.as_ptr().cast::<windows::Win32::System::Services::QUERY_SERVICE_CONFIGW>()) };
    if config.lpBinaryPathName.is_null() {
        return Ok(None);
    }
    // SAFETY: lpBinaryPathName 是 NUL 结尾宽字符串（QueryServiceConfigW 契约）。
    let path = unsafe { config.lpBinaryPathName.to_string() }
        .map_err(|e| format!("read binary path: {e}"))?;
    Ok(Some(path))
}

// ---------------------------------------------------------------------------
// R3 统一服务健康模型：廉价事实 + 派生健康状态（5 态）。
//
// 根因修（2026-08-20-rootcause-service-connect-split-plan §3 R3）：安装/卸载与连接
// 拆独立路由后，UI/host 需区分安装/卸载完整度，避免「半装/半卸」被误判为「已装/未装」。
// 廉价字段（scm_registered / binary_path_present / engine_binary_exists /
// psk_readable）每快照计算；昂贵探针（control_pipe_reachable / keepalive_replies）
// **on-demand**——GetSnapshot 热路径绝不触发。
// ---------------------------------------------------------------------------

/// 统一服务健康状态（R3 状态机；wire `ServiceStatus.health_state` 取值）。
///
/// 五种安装/卸载完整度状态：
/// - **Healthy**：SCM 注册 + running + 引擎二进制存在且指向 engine。
/// - **ScmOrphan**：SCM 注册但引擎二进制缺失/载荷缺席（上次卸载不彻底；或半装被识别
///   为已装）。
/// - **InstalledUnavailable**：SCM 注册 + 二进制存在但未运行（stopped/start_pending）。
/// - **PayloadOrphan**：SCM 未注册但载荷/引擎二进制存在（半装被识别为未装；上次卸载
///   留下载荷——SCM 条目已删但 `service.key` 残留）。
/// - **NotInstalled**：SCM 未注册 + 无载荷。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    Healthy,
    ScmOrphan,
    InstalledUnavailable,
    PayloadOrphan,
    NotInstalled,
}

impl HealthState {
    /// 稳定 wire 字符串（`ServiceStatus.health_state` 取值；host 总是填充）。
    #[must_use]
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::ScmOrphan => "scm_orphan",
            Self::InstalledUnavailable => "installed_unavailable",
            Self::PayloadOrphan => "payload_orphan",
            Self::NotInstalled => "not_installed",
        }
    }
}

/// 服务健康事实（R3）：廉价字段每快照计算（GetSnapshot 热路径），昂贵探针 on-demand
/// 仅在需要时填充——**永不进入快照热路径**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceHealth {
    /// SCM 是否注册该服务（`OpenServiceW` 成功）。
    pub scm_registered: bool,
    /// SCM 配置携带非空二进制路径。
    pub binary_path_present: bool,
    /// 二进制路径指向的可执行文件存在。
    pub engine_binary_exists: bool,
    /// 二进制路径的文件名匹配 engine 可执行名（`exv-engine` / `exv-vpn-win32-engine`）。
    pub binary_targets_engine: bool,
    /// 服务 PSK（`%ProgramData%\exv\service.key`）可读——载荷残留/完整性信号。
    pub psk_readable: bool,
    /// 当前 SCM 状态。
    pub state: ServiceState,
    /// on-demand：控制面管道可连（服务 engine 已 bind 控制面）。
    pub control_pipe_reachable: Option<bool>,
    /// on-demand：keepalive 有响应（服务程序具备业务响应能力）。
    pub keepalive_replies: Option<bool>,
}

/// 从廉价事实派生健康状态（纯函数；R3 状态机，可单测）。
#[must_use]
pub fn derive_health(h: &ServiceHealth) -> HealthState {
    if !h.scm_registered {
        if h.psk_readable || (h.engine_binary_exists && h.binary_targets_engine) {
            HealthState::PayloadOrphan
        } else {
            HealthState::NotInstalled
        }
    } else {
        let binary_ok =
            h.binary_path_present && h.engine_binary_exists && h.binary_targets_engine;
        match h.state {
            ServiceState::Running => {
                if binary_ok {
                    HealthState::Healthy
                } else {
                    HealthState::ScmOrphan
                }
            }
            _ => {
                if binary_ok {
                    HealthState::InstalledUnavailable
                } else {
                    HealthState::ScmOrphan
                }
            }
        }
    }
}

/// 从廉价事实 + engine 深度自述派生健康状态（纯函数；S3-B 健康加深，可单测）。
///
/// 当 host 已取得 engine 的 `ServiceSelfReport`（Tier 2 零 UAC 查询，SCM Running 场景）：
/// 报告 self-not-ready（`!control_plane_ready || !psk_present`）→ **InstalledUnavailable**
/// （「已安装但实际不可用」，复用 R3 既有 5 态词汇，**不新增第 6 态**）——SCM 报告 Running
/// 但 engine 自述控制面未就绪 / PSK 缺席（fail-closed 启动拒绝），比 SCM 单一事实更深。
/// 报告为 `None`（探针未触发/失败）→ 保守保留既有 [`derive_health`]（**不把探针失败当
/// 引擎失败**）。
#[must_use]
pub fn derive_health_with_self_report(
    h: &ServiceHealth,
    self_report: Option<&ServiceSelfReport>,
) -> HealthState {
    if let Some(report) = self_report {
        if !report.control_plane_ready || !report.psk_present {
            return HealthState::InstalledUnavailable;
        }
    }
    derive_health(h)
}

/// 从快照 + 廉价文件/密钥检查构建健康事实（GetSnapshot 热路径可用——全部廉价）。
///
/// `binary_path` 可能含启动参数（`C:\...\exv-engine.exe --service`）——先剥离参数提取
/// 可执行路径再做存在性/命名检查。SCM 未注册时无二进制路径可查（`engine_binary_exists`
/// /`binary_targets_engine` 留 false）；载荷信号由 `psk_readable` 承担（卸载清理对象）。
#[must_use]
pub fn service_health_from_snapshot(snap: &ServiceStatusSnapshot) -> ServiceHealth {
    let exe = if snap.state.is_installed() {
        snap.binary_path
            .as_deref()
            .and_then(exe_path_from_binary_path)
    } else {
        None
    };
    ServiceHealth {
        scm_registered: snap.state.is_installed(),
        binary_path_present: snap
            .binary_path
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty()),
        engine_binary_exists: exe.as_deref().is_some_and(|p| p.exists()),
        binary_targets_engine: exe.as_deref().is_some_and(is_engine_binary_name),
        psk_readable: exv_vpn_win32_ipc::service_key::read_service_psk().is_ok(),
        state: snap.state,
        control_pipe_reachable: None,
        keepalive_replies: None,
    }
}

/// 从 SCM `binary_path`（含启动参数）剥离可执行路径。
///
/// 处理带引号路径（`"C:\path with spaces\exv-engine.exe" --service`）与裸路径
/// （`C:\exv\exv-engine.exe --service` 取首个空白 token）。空/全空白 → `None`。
fn exe_path_from_binary_path(binary_path: &str) -> Option<PathBuf> {
    let trimmed = binary_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('"') {
        let end = trimmed[1..].find('"')?;
        return Some(PathBuf::from(&trimmed[1..1 + end]));
    }
    let first = trimmed.split_whitespace().next()?;
    Some(PathBuf::from(first))
}

/// 二进制文件是否匹配 engine 可执行名（新旧产品名都认：`exv-engine` / `exv-vpn-win32-engine`）。
fn is_engine_binary_name(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let name = name.to_string_lossy().to_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    stem == "exv-engine" || stem == "exv-vpn-win32-engine"
}

// ---------------------------------------------------------------------------
// 单元测试：原始状态映射 + 快照规范化（纯）+ 注入 fake 源的查询契约。
// SCM 真机集成（需 admin + 服务环境）在 tests/service_lifecycle.rs，env 门控。
// ---------------------------------------------------------------------------
