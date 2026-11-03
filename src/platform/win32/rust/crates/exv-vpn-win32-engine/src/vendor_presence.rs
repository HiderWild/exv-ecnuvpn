
//! 厂商在场注记（D8 阶段 2，计划 §3.3/§4.7；2026-10-09 用户裁定后的降级用途）。
//!
//! 原设计（主计划 §五）的「官方客户端在场检测 → 禁用代删按钮」已随方案 R 授权
//! 握手整体退役；本模块降级为**静默覆盖事件的日志注记**，分两层记录：
//! - **客户端管理层**：aTrust 三进程在场 + `aTrustService` SCM 状态——管理层
//!   在场暗示「规则是活的、重建概率高」（预期 rebuild_detected）。
//! - **生态组件层**：`eaio_service` SCM 状态 + `eaio_agent_sys` 进程在场 +
//!   `aTrustNetflt_1` 过滤驱动状态——生态层单独在场支持「瞬态、无人管理」的
//!   归因假设（2026-10-09 00:36 事故：aTrust 三进程全不在场、EAIO 自启活跃）。
//!
//! 两层都只进 `tunnel.nrpt.foreign.*` 事件与 journal 的 fields，**不进入任何
//! 判定分支**——静默覆盖对两层行为一致（覆盖 → 验证 → 重建才停）。
//!
//! 容错设计：检测失败一律返回 `"unknown"`（注记用途容错高，误报只污染日志）。
//! 进程名单与 `exv-vpn-win32-agent-sim/src/probe.rs` 的 `OFFICIAL_CLIENT_EXES`
//! 注释互指同源（不引 crate 依赖——名单漂移风险由注释锚定）。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
};

/// 官方客户端进程名（**与 `exv-vpn-win32-agent-sim/src/probe.rs:15` 的
/// `OFFICIAL_CLIENT_EXES` 同源**——两处名单必须同步维护，注释互指锚定）。
const OFFICIAL_CLIENT_EXES: [&str; 3] = ["aTrustTray.exe", "aTrustAgent.exe", "aTrustXtunnel.exe"];

/// 深信服 EAIO 生态组件进程名（事故归因素材：2026-10-09 00:32:30 eaio_agent_sys
/// 活跃做 bypass 驱动操作，与外来 NRPT 规则出现窗口相容）。
const EAIO_PROCESSES: [&str; 1] = ["eaio_agent_sys.exe"];

/// 观察的服务名（SCM 状态查询；`aTrustNetflt_1` 为内核过滤驱动服务——SCM 同一
/// 查询通道覆盖驱动服务）。
const WATCHED_SERVICES: [&str; 3] = ["aTrustService", "eaio_service", "aTrustNetflt_1"];

/// 厂商在场注记快照（全部字段为稳定短标签；`unknown` = 查询失败）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VendorPresence {
    /// 命中的官方客户端进程名（空 = none；管理层）。
    pub(crate) official_processes: Vec<String>,
    /// `aTrustService` SCM 状态（管理层）。
    pub(crate) official_service: String,
    /// `eaio_service` SCM 状态（生态层）。
    pub(crate) eaio_service: String,
    /// `aTrustNetflt_1` 驱动服务状态（生态层）。
    pub(crate) netflt_driver: String,
    /// 命中的 EAIO 生态进程名（生态层）。
    pub(crate) eaio_processes: Vec<String>,
}

impl VendorPresence {
    /// 日志/journal 用的紧凑注记字段（键与计划 §4.6 payload 的 `vendor_presence`
    /// 对齐；返回 `(键, 值)` 序列，值已是稳定短标签）。
    pub(crate) fn annotation_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "official_client",
                if self.official_processes.is_empty() {
                    "none".to_owned()
                } else {
                    self.official_processes.join("|")
                },
            ),
            ("official_service", self.official_service.clone()),
            ("eaio", self.eaio_service.clone()),
            ("netflt", self.netflt_driver.clone()),
            (
                "eaio_processes",
                if self.eaio_processes.is_empty() {
                    "none".to_owned()
                } else {
                    self.eaio_processes.join("|")
                },
            ),
        ]
    }
}

/// 采集一次厂商在场注记（Toolhelp32 进程快照 + SCM 状态查询；全程容错）。
pub(crate) fn snapshot() -> VendorPresence {
    let processes = enumerate_process_names();
    let mut official_processes = Vec::new();
    let mut eaio_processes = Vec::new();
    for name in &processes {
        if OFFICIAL_CLIENT_EXES
            .iter()
            .any(|known| name.eq_ignore_ascii_case(known))
        {
            official_processes.push(name.clone());
        }
        if EAIO_PROCESSES
            .iter()
            .any(|known| name.eq_ignore_ascii_case(known))
        {
            eaio_processes.push(name.clone());
        }
    }
    official_processes.sort();
    official_processes.dedup();
    eaio_processes.sort();
    eaio_processes.dedup();
    let [official_service, eaio_service, netflt_driver] = [
        service_state("aTrustService"),
        service_state("eaio_service"),
        service_state("aTrustNetflt_1"),
    ];
    VendorPresence {
        official_processes,
        official_service,
        eaio_service,
        netflt_driver,
        eaio_processes,
    }
}

/// Toolhelp32 快照枚举全部进程名（失败 → 空表，注记层如实缺失）。
fn enumerate_process_names() -> Vec<String> {
    // SAFETY: 快照句柄由系统返回；INVALID_HANDLE_VALUE 以外的值在使用后关闭。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let Ok(handle) = snapshot else {
        return Vec::new();
    };
    if handle == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut names = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: entry 按 dwSize 初始化后作为迭代游标传入；handle 来自上方快照。
    if unsafe { Process32FirstW(handle, &raw mut entry) }.is_ok() {
        loop {
            let len = entry
                .szExeFile
                .iter()
                .position(|ch| *ch == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            names.push(name);
            // SAFETY: 同上游迭代语义。
            if unsafe { Process32NextW(handle, &raw mut entry) }.is_err() {
                break;
            }
        }
    }
    // SAFETY: handle 是本函数获得的快照句柄，使用完毕关闭。
    unsafe {
        let _ = CloseHandle(handle);
    }
    names
}

/// 单个服务的 SCM 状态短标签（running/stopped/…；打开/查询失败 → unknown）。
fn service_state(service_name: &str) -> String {
    let name_w: Vec<u16> = service_name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NULL 机器名/数据库名 = 本机 SERVICES_ACTIVE_DATABASE；句柄用毕关闭。
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) };
    let Ok(scm) = scm else {
        return "unknown".to_owned();
    };
    let result = (|| {
        // SAFETY: name_w 是 NUL 结尾宽字符串；服务句柄用毕关闭。
        let service = unsafe {
            OpenServiceW(
                scm,
                PCWSTR::from_raw(name_w.as_ptr()),
                SERVICE_QUERY_STATUS,
            )
        };
        let Ok(service) = service else {
            return "absent".to_owned();
        };
        let mut status = SERVICE_STATUS::default();
        // SAFETY: status 为输出缓冲。
        let queried = unsafe { QueryServiceStatus(service, &raw mut status) };
        // SAFETY: service 是上方打开的句柄。
        unsafe {
            let _ = CloseServiceHandle(service);
        }
        if queried.is_err() {
            return "unknown".to_owned();
        }
        if status.dwCurrentState == SERVICE_RUNNING {
            "running".to_owned()
        } else if status.dwCurrentState == windows::Win32::System::Services::SERVICE_STOPPED {
            "stopped".to_owned()
        } else {
            format!("state_{:?}", status.dwCurrentState)
        }
    })();
    // SAFETY: scm 是本函数打开的管理器句柄。
    unsafe {
        let _ = CloseServiceHandle(scm);
    }
    result
}

// ---------------------------------------------------------------------------
// 单元测试：快照结构只测纯逻辑（注记字段渲染）；进程/SCM 查询是真系统调用，
// 行为归本机验收（注记用途容错高，不因环境差异失败）。
// ---------------------------------------------------------------------------
