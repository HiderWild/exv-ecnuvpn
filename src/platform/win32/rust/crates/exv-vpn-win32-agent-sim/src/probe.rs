//! 生命周期断言：启动前确认官方客户端未运行（规格 §5 启动顺序 ①）。
//!
//! 两条独立证据，任一命中即拒绝启动并如实记录：
//! * 存在 `aTrustTray.exe` / `aTrustAgent.exe` / `aTrustXtunnel.exe` 进程；
//! * 目标端口（默认 54630/54631）已被监听。
//!
//! 探测经 [`HostProbe`] 抽象，断言逻辑可离线单测；Windows 实现用 Toolhelp 快照枚举进程。

use std::net::TcpListener;

/// 官方客户端进程名（大小写不敏感匹配）。
///
/// S4 补齐 `aTrustXtunnel.exe`（官方隧道数据面进程；2026-10-05 审计事实：官方进程集合
/// 含它——官方客户端运行时它与 Tray/Agent 并存）。语义不变：命中**任一**官方进程即拒绝启动。
pub const OFFICIAL_CLIENT_EXES: [&str; 3] = ["aTrustTray.exe", "aTrustAgent.exe", "aTrustXtunnel.exe"];

/// 宿主探测抽象。
pub trait HostProbe: Send + Sync {
    /// 当前正在运行的官方客户端进程名（仅命中名单内的）。
    fn running_official_client_processes(&self) -> Vec<String>;
    /// `127.0.0.1:<port>` 是否已被监听。
    fn port_in_use(&self, port: u16) -> bool;
}

/// 进程名是否为官方客户端（大小写不敏感）。
#[must_use]
pub fn is_official_client_process(name: &str) -> bool {
    OFFICIAL_CLIENT_EXES
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known))
}

/// 真实宿主探测。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProbe;

impl HostProbe for SystemProbe {
    fn running_official_client_processes(&self) -> Vec<String> {
        enumerate_process_names()
            .into_iter()
            .filter(|name| is_official_client_process(name))
            .collect()
    }

    fn port_in_use(&self, port: u16) -> bool {
        // 绑定成功即端口空闲；失败（含占用/TIME_WAIT）一律保守判为「不可用」。
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                false
            }
            Err(_) => true,
        }
    }
}

/// 官方客户端在运行（或端口被占），拒绝启动的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficialClientPresent {
    /// 命中官方客户端进程。
    Process(Vec<String>),
    /// 目标端口已被监听。
    Port(Vec<u16>),
}

impl std::fmt::Display for OfficialClientPresent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Process(names) => {
                write!(f, "官方客户端进程在运行: {}", names.join(", "))
            }
            Self::Port(ports) => {
                let joined = ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "目标端口已被监听: {joined}")
            }
        }
    }
}

impl std::error::Error for OfficialClientPresent {}

/// 断言官方客户端未运行且目标端口空闲。
///
/// # Errors
/// 命中进程或端口占用时返回 [`OfficialClientPresent`]。
pub fn assert_official_client_absent(
    probe: &dyn HostProbe,
    ports: &[u16],
) -> Result<(), OfficialClientPresent> {
    let processes = probe.running_official_client_processes();
    if !processes.is_empty() {
        return Err(OfficialClientPresent::Process(processes));
    }
    let busy: Vec<u16> = ports
        .iter()
        .copied()
        .filter(|port| probe.port_in_use(*port))
        .collect();
    if !busy.is_empty() {
        return Err(OfficialClientPresent::Port(busy));
    }
    Ok(())
}

#[cfg(windows)]
fn enumerate_process_names() -> Vec<String> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let mut names = Vec::new();
    // SAFETY: 只调用 Toolhelp 快照 API；句柄在函数末尾关闭，`entry` 为定长值类型。
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return names;
        };
        if snapshot == INVALID_HANDLE_VALUE {
            return names;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                names.push(String::from_utf16_lossy(&entry.szExeFile[..end]));
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    names
}

#[cfg(not(windows))]
fn enumerate_process_names() -> Vec<String> {
    Vec::new()
}
