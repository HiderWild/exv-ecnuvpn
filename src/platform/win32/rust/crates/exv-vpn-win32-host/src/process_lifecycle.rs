
//! core 侧 engine 进程原语（P3-c2：engine 由 core 拉起）。
//!
//! 两进程架构：**core**（普通 token，永不 elevated）经 `ShellExecuteExW(runas)` 提权拉起唯一
//! 特权进程 **engine**（建卡/路由/网络设置/认证/CSTP/数据面），随后经控制面 Named Pipe
//! （gRPC，P1-c `grpc_transport`）指挥它。本模块提供进程级原语：
//!
//! - [`engine_bin_path`]：定位生产 engine 可执行路径 `exv-engine`
//!   （`CARGO_BIN_EXE_exv-engine` 或 `current_exe` sibling）——生产 engine 是
//!   `exv-engine` crate 的 bin（HelperControl gRPC server，P1-b）；acceptance
//!   的 `exv-win32-engine`（legacy JSON frame）只供 acceptance 场景，非生产 core 的
//!   engine；
//! - [`engine_control_pipe_name`] + [`engine_args_for_spawn`]：构造 engine 启动参数（控制面
//!   管道名按 core PID 唯一、wintun.dll 路径、journal 目录、authority 名、host pid、adapter
//!   名、user SID），对齐生产 engine bin（`exv-engine` main）的解析契约；
//! - [`spawn_engine_elevated`]：ShellExecuteExW(runas) 提权 spawn，返回 `(pid, handle)`；
//! - [`verify_process_elevated`]：观测进程 token 是否 elevated（fail closed）；
//! - [`wait_process_exit`] / [`terminate_process`]：有界等待退出 / 强制终止（不关闭句柄）；
//! - [`EngineChild`]：持有 `(pid, handle)` 的子进程句柄，Drop 时未退出则 terminate——
//!   **engine 不得遗留**（core 退出时 engine 一并终止，O3 强绑定）。
//!
//! 与 acceptance `engine_spawn.rs` 的差异（产品化）：本模块为产品 host 的进程原语，
//! 不含 acceptance 的验收专用观测；`--log-pipe` 不传（产品日志通道是 gRPC `StreamLogs`，
//! legacy JSON log pipe 仅 acceptance 使用）。

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, TerminateProcess, WaitForSingleObject,
};

/// `STILL_ACTIVE`（259）：进程仍在运行（`GetExitCodeProcess` 对存活进程返回该值）。
const STILL_ACTIVE: u32 = 259;
use windows::Win32::System::Registry::HKEY;
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHELLEXECUTEINFOW_0,
    ShellExecuteExW,
};
use windows::core::{HSTRING, PCWSTR};

/// engine 创建的 Wintun adapter 名称（与 acceptance `engine::ENGINE_ADAPTER_NAME` 一致；
/// 可被 `--adapter-name` 覆盖）。
pub const ENGINE_ADAPTER_NAME: &str = "ExvEngine";

/// wintun.dll 冻结默认路径（`%USERPROFILE%\.exv\...`；与 acceptance `wintun_facts` 一致）。
fn default_wintun_dll_path() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").unwrap_or_else(|| ".".into());
    PathBuf::from(home)
        .join(".exv")
        .join("wintun")
        .join("wintun")
        .join("bin")
        .join("amd64")
        .join("wintun.dll")
}

/// 解析 wintun.dll 路径（`EXV_RUST_VPN_WINTUN_DLL` env 或 `~/.exv` 冻结默认）。
#[must_use]
pub fn resolve_wintun_dll_path() -> PathBuf {
    std::env::var_os("EXV_RUST_VPN_WINTUN_DLL")
        .map(PathBuf::from)
        .unwrap_or_else(default_wintun_dll_path)
}

/// 按 `CommandLineToArgvW` 兼容规则引用一个 Windows 命令行参数。
///
/// `ShellExecuteExW` 的 `lpParameters` 只接收一条命令行字符串，不能像
/// `CreateProcess` 的 argv 数组那样保留参数边界。尤其是用户配置目录可能位于带空格的
/// Windows profile 下，因此必须在空格、双引号和末尾反斜杠处做精确转义。
fn quote_windows_argument(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value
            .chars()
            .any(|ch| matches!(ch, ' ' | '\t' | '\n' | '\u{000b}' | '"'));
    if !needs_quotes {
        return value.to_string();
    }

    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            for _ in 0..(backslashes * 2 + 1) {
                quoted.push('\\');
            }
            quoted.push('"');
        } else {
            for _ in 0..backslashes {
                quoted.push('\\');
            }
            quoted.push(ch);
        }
        backslashes = 0;
    }
    // Backslashes immediately before the closing quote must be doubled, otherwise the closing
    // quote is consumed as a literal character by the Windows argv parser.
    for _ in 0..(backslashes * 2) {
        quoted.push('\\');
    }
    quoted.push('"');
    quoted
}

/// 把一组参数编码成 `ShellExecuteExW::lpParameters` 所需的命令行。
fn windows_command_line(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_windows_argument(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// engine bin 的可执行路径（测试流：`CARGO_BIN_EXE_exv-engine`；bin 流：
/// `current_exe` 同目录 sibling）。**生产 engine = `exv-engine`**（HelperControl
/// gRPC server，P1-b）；acceptance 的 `exv-win32-engine`（legacy JSON frame）只供
/// acceptance 场景，不是 core 的 gRPC client 契约对象。
#[must_use]
pub fn engine_bin_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_exv-engine") {
        let bp = PathBuf::from(p);
        if bp.exists() {
            return Some(bp);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if exe.file_name()?.to_string_lossy().ends_with(".exe") {
        "exv-engine.exe"
    } else {
        "exv-engine"
    };
    Some(dir.join(name))
}

/// engine 控制面 Named Pipe 名（按 core PID 唯一——同一 core 反复拉起 engine 不冲突）。
///
/// 与 [`engine_args_for_spawn`] 中的 `--control-pipe` 值一致；core 提权拉起 engine 后用
/// 它连接（`EngineControlGrpcClient::connect`，P1-c）。命名沿用 helper 约定的 `-c` 后缀。
#[must_use]
pub fn engine_control_pipe_name() -> String {
    let host_pid = std::process::id();
    format!(r"\\.\pipe\exv-engine-{host_pid}-c")
}

/// engine 的启动参数（core 侧构造；管道名按 core PID 唯一）。
///
/// 参数契约与生产 engine bin（`exv-engine` main）对齐：必需 `--control-pipe`、
/// `--dll`、`--host-pid`；可选 `--journal-dir`/`--authority-name`/`--adapter-name`/
/// `--user-sid`。wintun.dll 路径经 [`resolve_wintun_dll_path`] 解析（env 或默认路径）。
///
/// `--journal-dir` 是**机器级稳定目录**（[`JournalPath::machine_default`] =
/// `%ProgramData%\ExvVpn\journal`，不可写回退 `%LOCALAPPDATA%`）：engine 崩溃
/// （强杀/断电）后整机重启，新 engine 必须能在同一路径发现旧账本完成系统代理
/// 回放——按 host-pid 的临时目录会让崩溃残留永远不可发现。
///
/// `--user-sid` 携带 **core** 进程的用户 SID：engine 用它建控制面管道 DACL，授权普通用户
/// core 连接（engine 是特权进程，默认 ACL 只含 SYSTEM/Administrators，普通 core 会被拒）。
///
/// 与 acceptance 的差异：**不传 `--log-pipe`**——产品日志通道是 gRPC `StreamLogs`（P2 聚合），
/// legacy JSON log pipe 仅 acceptance 使用；传入会让 engine 空转重试一个不存在的 server。
#[must_use]
pub fn engine_args_for_spawn() -> Vec<String> {
    let host_pid = std::process::id();
    let journal_dir = exv_vpn_win32_resource::journal_path::JournalPath::machine_default()
        .as_path()
        .display()
        .to_string();
    let dll = resolve_wintun_dll_path().display().to_string();
    let mut args = vec![
        "--control-pipe".to_string(),
        engine_control_pipe_name(),
        "--dll".to_string(),
        dll,
        "--journal-dir".to_string(),
        journal_dir,
        "--authority-name".to_string(),
        format!("Local\\exv-engine-{host_pid}-authority"),
        "--host-pid".to_string(),
        host_pid.to_string(),
        "--adapter-name".to_string(),
        ENGINE_ADAPTER_NAME.to_string(),
    ];
    // SID 不可解析（极罕见）时省略该参数，engine 回退本进程用户 SID——同用户拓扑下两者相等。
    if let Some(sid) = current_user_sid() {
        args.push("--user-sid".to_string());
        args.push(sid);
    }
    args
}

/// 当前进程的用户 SID（engine 用它建控制面 DACL；与 acceptance `peer_auth` 同源）。
#[must_use]
fn current_user_sid() -> Option<String> {
    exv_vpn_win32_ipc::peer_auth::current_user_sid()
}

/// 当前进程是否 elevated（TokenElevation 观测；fail closed：无法查询即 false）。
#[must_use]
pub fn is_elevated() -> bool {
    // SAFETY: GetCurrentProcess 返回当前进程伪句柄，无需关闭。
    let process = unsafe { GetCurrentProcess() };
    let mut token = HANDLE::default();
    // SAFETY: OpenProcessToken 写入 token 句柄；成功后需关闭。
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) }.is_err() {
        return false;
    }
    let elevated = read_token_elevated(token);
    // SAFETY: token 是本进程新打开的句柄，使用后关闭。
    unsafe {
        let _ = CloseHandle(token);
    }
    elevated
}

/// 观测进程 token 是否 elevated（engine 进程的观测；fail closed：无法打开/查询即 false）。
#[must_use]
pub fn verify_process_elevated(pid: u32) -> bool {
    // SAFETY: OpenProcess 打开受限查询句柄；失败即返回 false（fail closed）。
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let mut token = HANDLE::default();
    // SAFETY: OpenProcessToken 写入 token 句柄；成功后需关闭。
    let Ok(_) = (unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) }) else {
        // SAFETY: process 句柄使用后关闭。
        unsafe {
            let _ = CloseHandle(process);
        }
        return false;
    };
    let elevated = read_token_elevated(token);
    // SAFETY: token/process 句柄使用后关闭。
    unsafe {
        let _ = CloseHandle(token);
        let _ = CloseHandle(process);
    }
    elevated
}

/// 读取 token 的 TokenElevation 事实。
fn read_token_elevated(token: HANDLE) -> bool {
    let mut elevated = false;
    let mut size = 0u32;
    // SAFETY: 尺寸查询不写任何位置。
    unsafe {
        let _ = GetTokenInformation(token, TokenElevation, None, 0, &raw mut size);
    }
    if size != 0 {
        let mut buffer = vec![0u8; size as usize];
        let len = u32::try_from(buffer.len()).unwrap_or_default();
        // SAFETY: `buffer` 是有效缓冲；TokenElevation 写入 TOKEN_ELEVATION。
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                len,
                &raw mut size,
            )
        };
        if ok.is_ok() && buffer.len() >= 4 {
            elevated = u32::from_ne_bytes(buffer[0..4].try_into().unwrap_or([0u8; 4])) != 0;
        }
    }
    elevated
}

/// 采集 UAC 提权事件（判据 6 的可复用断言/采集基建，P1 建立；P4 业务验收用）。
///
/// 查询 `Microsoft-Windows-UAC-Eventlog/Operational` 事件日志，统计给定进程可执行名
/// （如 `exv-engine.exe`）自 `start_time_iso`（ISO 8601，如
/// `2026-08-19T00:00:00`）以来的提权事件数。
///
/// 提权不变量（判据 6）：core 自身启动全程**不拉 UAC**；唯一 UAC 事件 = core spawn
/// engine 时的 runas 调用——完整"每 core 生命周期恰 1 次且归属 engine spawn"计数在
/// P4 业务验收断言（本函数提供采集/过滤器基建）。UAC 事件日志通道未启用 / 无匹配
/// 事件 → 返回 0（采集不可用不冒充失败，P4 在通道可用时断言恰 1 次）。
#[must_use]
pub fn collect_uac_events_for(exe_name: &str, start_time_iso: &str) -> u32 {
    let script = format!(
        "try {{ $evs = Get-WinEvent -FilterHashtable @{{LogName='Microsoft-Windows-UAC-Eventlog/Operational'; StartTime='{start_time_iso}'}} -ErrorAction Stop | Where-Object {{ $_.Message -like '*{exe_name}*' }}; [uint32](@($evs) | Measure-Object).Count }} catch {{ 0 }}"
    );
    match std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// 提权 spawn engine 进程（ShellExecuteExW runas；返回 pid + 进程句柄）。
///
/// 复用 acceptance `controlled::spawn_helper_elevated` 的同一 ShellExecuteExW(runas) 机制
/// （engine 是 helper 的同构特权进程——普通 core 拉起、UAC 静默通过或拒绝）。返回的
/// `HANDLE` 由调用者持有（推荐包装进 [`EngineChild`]）。
///
/// `SEE_MASK_FLAG_NO_UI`：engine bin 缺失时 ShellExecuteExW 直接返回错误（不弹 Windows
/// "找不到文件" 系统对话框），失败统一落到下方可读的 `Err(...)`。
///
/// # Errors
/// ShellExecuteExW(runas) 失败 / 未返回进程句柄 / PID 为 0。
pub fn spawn_engine_elevated(exe: &Path, args: &[String]) -> Result<(u32, HANDLE), String> {
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(windows_command_line(args));
    let dir = HSTRING::from(
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .as_os_str(),
    );
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or_default(),
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        hwnd: windows::Win32::Foundation::HWND::default(),
        lpVerb: PCWSTR::from_raw(verb.as_ptr()),
        lpFile: PCWSTR::from_raw(file.as_ptr()),
        lpParameters: PCWSTR::from_raw(params.as_ptr()),
        lpDirectory: PCWSTR::from_raw(dir.as_ptr()),
        nShow: 0, // SW_HIDE
        hInstApp: windows::Win32::Foundation::HINSTANCE::default(),
        lpIDList: std::ptr::null_mut(),
        lpClass: PCWSTR::null(),
        hkeyClass: HKEY::default(),
        dwHotKey: 0,
        Anonymous: SHELLEXECUTEINFOW_0 {
            hIcon: HANDLE::default(),
        },
        hProcess: HANDLE::default(),
    };
    // SAFETY: sei 的所有指针在调用期间存活（verb/file/params/dir 均为活宽字符串）；
    // hProcess 由 SEE_MASK_NOCLOSEPROCESS 输出，调用者持有。
    if unsafe { ShellExecuteExW(&raw mut sei) }.is_err() {
        // SAFETY: 无指针参数，读线程错误码。
        let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
        return Err(format!("ShellExecuteExW(runas) failed, error {code}"));
    }
    if sei.hProcess.is_invalid() {
        return Err("ShellExecuteExW returned no process handle".to_string());
    }
    // SAFETY: hProcess 有效；GetProcessId 返回其 PID。
    let pid = unsafe { GetProcessId(sei.hProcess) };
    if pid == 0 {
        return Err("engine process id is 0".to_string());
    }
    Ok((pid, sei.hProcess))
}

/// 等待进程退出（有界；**不关闭句柄**——句柄所有权由调用方持有）。
///
/// 返回 `true` 当进程已 signaled（WAIT_OBJECT_0）；`false` 当超时（进程仍存活）或等待
/// 失败（WAIT_FAILED——fail closed，不把失败当作已退出）。
#[must_use]
pub fn wait_process_exit(handle: HANDLE, timeout_ms: u32) -> bool {
    // SAFETY: handle 是有效进程句柄；超时返回 WAIT_TIMEOUT，失败返回 WAIT_FAILED。
    let rc = unsafe { WaitForSingleObject(handle, timeout_ms) };
    rc == WAIT_OBJECT_0
}

/// 强制终止进程（**不关闭句柄**——句柄所有权由调用方持有；调用方应在随后关闭）。
pub fn terminate_process(handle: HANDLE) {
    // SAFETY: handle 是有效进程句柄；TerminateProcess 主动结束进程。
    unsafe {
        let _ = TerminateProcess(handle, 1);
    }
}

/// engine 子进程句柄：持有 `(pid, handle)`，生命周期安全。
///
/// - [`EngineChild::wait_exit`]：有界等待退出；成功后关闭句柄（防 double-close）。
/// - [`EngineChild::terminate`]：强制终止 + 关闭句柄。
/// - `Drop`：句柄未关闭时（进程仍未退出）→ terminate + 关闭——**engine 不得遗留**。
pub struct EngineChild {
    pid: u32,
    handle: HANDLE,
    handle_closed: bool,
}

impl EngineChild {
    /// 用 spawn 返回的 `(pid, handle)` 构造（句柄所有权转入本结构）。
    #[must_use]
    pub fn new(pid: u32, handle: HANDLE) -> Self {
        Self {
            pid,
            handle,
            handle_closed: false,
        }
    }

    /// 进程 PID。
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 进程句柄是否已关闭（已退出或已 terminate）。
    #[must_use]
    pub fn handle_closed(&self) -> bool {
        self.handle_closed
    }

    /// 观测进程 token 是否 elevated（fail closed）。
    #[must_use]
    pub fn is_elevated(&self) -> bool {
        verify_process_elevated(self.pid)
    }

    /// 有界等待进程退出。成功退出后**关闭句柄**（所有权终结）并返回 `true`；超时返回
    /// `false`（进程仍存活，句柄未关闭，调用方可 [`EngineChild::terminate`]）。
    pub fn wait_exit(&mut self, timeout_ms: u32) -> bool {
        self.wait_exit_code(timeout_ms).is_some()
    }

    /// 有界等待进程退出并读取退出码。成功退出 → `Some(code)`（关句柄）；超时 → `None`
    /// （进程仍存活，句柄未关闭，调用方可 [`EngineChild::terminate`]）。
    ///
    /// 退出码是子命令成败的真相：`--service-install`/`--service-start` 等引擎子命令以
    /// 非零码退出（如 PSK 缺失）时，只观察"是否退出"会误报成功（掩盖真实失败）。
    pub fn wait_exit_code(&mut self, timeout_ms: u32) -> Option<i32> {
        if self.handle_closed {
            return Some(0);
        }
        if !wait_process_exit(self.handle, timeout_ms) {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: handle 是有效进程句柄；GetExitCodeProcess 读取退出码（进程已退出）。
        let ok = unsafe { GetExitCodeProcess(self.handle, &mut code) }.is_ok();
        // SAFETY: handle 使用后关闭；此后本结构不再持有。
        unsafe {
            let _ = CloseHandle(self.handle);
        }
        self.handle_closed = true;
        if ok && code != STILL_ACTIVE {
            Some(code as i32)
        } else {
            Some(0)
        }
    }

    /// 强制终止进程并关闭句柄（幂等；句柄已关闭时 no-op）。
    pub fn terminate(&mut self) {
        if self.handle_closed {
            return;
        }
        terminate_process(self.handle);
        // 等待进程 signaled（TerminateProcess 后立即 signaled），然后关闭句柄。
        // SAFETY: handle 是有效进程句柄，使用后关闭。
        unsafe {
            let _ = WaitForSingleObject(self.handle, 5_000);
            let _ = CloseHandle(self.handle);
        }
        self.handle_closed = true;
    }
}

impl Drop for EngineChild {
    fn drop(&mut self) {
        // 未明确 wait_exit/terminate 的句柄 → 强制终止（engine 不得遗留，O3）。
        self.terminate();
    }
}

// SAFETY: Windows HANDLE 是不透明内核对象值，跨线程移动/共享句柄值本身安全；对句柄
// 的 wait/terminate/close 等可变操作由持有方串行化（EngineSupervisor 经 tokio::sync::
// Mutex 互斥——provision/respawn/停机三方串行；与 `kernel_control_service::SendChild`
// 先例同理由）。按需拉起模型（2026-09-08 计划）下 supervisor 进入
// `EngineProvisioner`（KernelControlService 字段）跨 await 持有，必须 Send+Sync。
unsafe impl Send for EngineChild {}
unsafe impl Sync for EngineChild {}

// ---------------------------------------------------------------------------
// 单元测试：参数契约往返 + 管道名唯一 + bin 路径 + elevation 探针（探针 elevation-gated：
// 非 elevated 环境 runas 弹 UAC，自动化无法可靠推进，诚实短路）。
// ---------------------------------------------------------------------------
