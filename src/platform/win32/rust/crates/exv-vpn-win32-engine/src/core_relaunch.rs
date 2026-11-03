
//! T10：engine 拉起 core（连接态主路径，计划 §2.5 / D5 / §2.9 缺口①②）。
//!
//! service 形态（LocalSystem，session 0）下 core 死亡且策略 keep 时，engine 主动
//! 拉起新 core（无头续存的延续动作——拉起成功后新 core 走 T8 采纳握手接管，engine
//! 无需另行通知）。**core 永不特权铁律**：拉起必须以**用户令牌**执行
//! （`WTSQueryUserToken` → `CreateProcessAsUserW`），绝不继承 engine 的 SYSTEM
//! 令牌直接 spawn——oneshot 形态（提权 engine）因此**明确不做**拉起（直接 spawn
//! 会把管理员令牌遗传给 core；explorer 令牌复制 + `CreateProcessWithTokenW` 的
//! 底质后置分期，见计划 §2.5 拉起底质②）。
//!
//! # 三步底质（§2.5 拉起底质①）
//!
//! 1. **选会话**：优先受理期记录的 core 原会话 id（`ProcessIdToSessionId`，core
//!    活着时查得——多用户/断开场景下比拉起时查 `WTSGetActiveConsoleSessionId`
//!    稳：活动控制台会话可能已不是 core 原会话）；缺失时回退活动控制台会话。
//! 2. **取用户令牌**：`WTSQueryUserToken(session)`（需 `SE_TCB`——SYSTEM 服务
//!    持有）。
//! 3. **用户态创建**：`CreateEnvironmentBlock(用户 token)` 构造用户环境 +
//!    `CreateProcessAsUserW`（lpDesktop = `winsta0\default`、
//!    `CREATE_UNICODE_ENVIRONMENT`——环境块为 UTF-16）。
//!
//! # 拉起参数（§2.9 缺口① ui_pid 传递链，边界 21）
//!
//! 对齐 UI 拉起 core 的既有契约（`core_process.rs` → core `main.rs` 的
//! `--control-pipe`/`--ui-pid`/`--ui-sid`）：engine 拉起的 core 用 trace 的
//! `ui_pid` 构造**与 UI 侧相同的管道名** `\\.\pipe\exv-core-{ui_pid}`——活着的
//! 原 UI（pid 未变）照常拨到；UI 已亡（ui_pid=0 或已不复存在）时 core 照常起
//! （engine↔core 采纳握手不依赖该管道），新 UI 走 T9 自注册路径。
//!
//! # seam 纪律
//!
//! [`CoreSpawner`] 是构造参数注入接缝（T5/T7/T8 已接受形态）：生产 =
//! [`wts_spawner`]（真实 WTS + CreateProcessAsUser 包装），测试 = fake 记录
//! 请求与可控返回。真实 `CreateProcessAsUserW` 的行为验证归 T13 实机验收
//! （本机单测无法造真实会话令牌）。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Environment::{
    CreateEnvironmentBlock, DestroyEnvironmentBlock,
};
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTSGetActiveConsoleSessionId, WTSQueryUserToken,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION,
    PROCESS_CREATION_FLAGS, STARTUPINFOW,
};

/// 一次 core 拉起的完整参数（seam 载体——测试记录断言用；生产是真实 spawn 输入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreRelaunchRequest {
    /// core 可执行路径（engine 同目录发现，[`core_exe_path`]）。
    pub exe: PathBuf,
    /// 完整命令行参数（[`core_relaunch_args`] 构造：`--control-pipe`/`--ui-pid`/
    /// `--ui-sid`，与 UI 拉起 core 的契约逐字对齐）。
    pub args: Vec<String>,
    /// 发起用户 SID（engine 受理事实；即 core 参数 `--ui-sid` 的值）。
    pub user_sid: String,
    /// core 背后的 UI PID（trace 受理事实；0 = 旧 core 未上报——管道名仍按 0
    /// 构造，UI 侧由 T9 自注册兜底）。
    pub ui_pid: u32,
    /// 拉起目标会话 id：受理期记录的 core 原会话（优先）；`None` = 未记录 →
    /// 生产实现回退 `WTSGetActiveConsoleSessionId`。
    pub session_id: Option<u32>,
}

/// 一次成功拉起的记录（崩溃环账本材料 + `relaunched_by_engine` 匹配键）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedCore {
    /// 新 core 进程 PID。
    pub pid: u32,
    /// 拉起成功时刻（Unix 毫秒；时间窗匹配材料）。
    pub launched_at_ms: u64,
}

/// 拉起器接缝：生产 = [`wts_spawner`]；测试 = fake 记录请求与可控返回。
///
/// 同步签名（Win32 spawn 是阻塞调用——调用方负责 `spawn_blocking` 承载）。
pub type CoreSpawner = Arc<dyn Fn(CoreRelaunchRequest) -> Result<SpawnedCore, String> + Send + Sync>;

/// core 的 UI 侧控制面管道名（与 UI 侧 `core_process.rs` 的
/// `core_control_pipe_name` 同一形态——按 ui_pid 命名，UI 用同一名字拨号）。
#[must_use]
pub fn core_control_pipe_name(ui_pid: u32) -> String {
    format!(r"\\.\pipe\exv-core-{ui_pid}")
}

/// core 可执行路径发现：engine 同目录 `exv-core(.exe)`（sibling 布局——镜像 UI 侧
/// `core_bin_path` / host `engine_bin_path` 先例：安装布局中 core 与 engine 同目录，
/// **不查存在性**——路径是布局约定，存在性由 spawn 失败诚实上报）。
#[must_use]
pub fn core_exe_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if exe.file_name()?.to_string_lossy().ends_with(".exe") {
        "exv-core.exe"
    } else {
        "exv-core"
    };
    Some(dir.join(name))
}

/// engine 拉起 core 的启动参数（对齐 core `main.rs::CoreArgs::parse` 契约与 UI 侧
/// `core_args_for_spawn` 的形态——缺口①：`ui_pid` 使活着的 UI 能拨到新 core）。
#[must_use]
pub fn core_relaunch_args(user_sid: &str, ui_pid: u32) -> Vec<String> {
    vec![
        "--control-pipe".to_owned(),
        core_control_pipe_name(ui_pid),
        "--ui-pid".to_owned(),
        ui_pid.to_string(),
        "--ui-sid".to_owned(),
        user_sid.to_owned(),
    ]
}

/// 查询进程所在终端会话 id（受理期记录 `ProcessIdToSessionId` 的底质；失败 =
/// `None`——core 死后不可再查，正是受理期记录的原因）。
#[must_use]
pub fn process_session_id(pid: u32) -> Option<u32> {
    let mut session_id = 0u32;
    // SAFETY: 输出指针指向栈上有效的 u32；调用不持有任何句柄。
    (unsafe { ProcessIdToSessionId(pid, &mut session_id) })
        .ok()
        .map(|()| session_id)
}

/// 宽字符串（NUL 终结，Win32 PCWSTR/PWSTR 输入用）。
fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(std::iter::once(0)).collect()
}

/// Unix 毫秒（时钟回拨等异常退化为 0；与 `connection_trace` 同款约定）。
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// 生产拉起器：WTS 会话 → 用户令牌 → 用户环境 → `CreateProcessAsUserW`
/// （§2.5 拉起底质①的三步；全句柄恰一次释放）。
#[must_use]
pub fn wts_spawner() -> CoreSpawner {
    Arc::new(|request| spawn_core_as_user(&request))
}

/// 真实拉起主体（[`wts_spawner`] 的实现；单测不触——行为验证归 T13 实机）。
fn spawn_core_as_user(request: &CoreRelaunchRequest) -> Result<SpawnedCore, String> {
    // 1. 选会话：受理期记录优先（core 原会话——见模块文档"选会话"）；缺失回退
    //    活动控制台会话（0xFFFFFFFF = 无活动控制台会话，WTSQueryUserToken 将
    //    失败——诚实报错）。
    // SAFETY: 纯查询，无句柄无指针。
    let session_id = request
        .session_id
        .unwrap_or_else(|| unsafe { WTSGetActiveConsoleSessionId() });

    // 2. 用户 primary token（SE_TCB 由 SYSTEM 服务持有；oneshot 提权进程在此
    //    失败是预期防线的体现——core 永不特权）。
    let mut token = HANDLE::default();
    // SAFETY: token 输出指针有效；WTSQueryUserToken 在调用成功时把句柄所有权
    // 移交调用方（下方统一恰一次 CloseHandle）。
    unsafe { WTSQueryUserToken(session_id, &mut token) }
        .map_err(|e| format!("WTSQueryUserToken(session={session_id}): {e}"))?;

    // 3. 用户环境块（UTF-16；CREATE_UNICODE_ENVIRONMENT 与之配套）。
    let mut environment: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: environment 输出指针有效；块所有权归调用方（DestroyEnvironmentBlock
    // 释放，下方所有路径恰一次）。
    let env_result =
        unsafe { CreateEnvironmentBlock(&mut environment, Some(token), false) };
    if let Err(e) = env_result {
        // SAFETY: token 已取得且尚未关闭——失败路径先释放再返回。
        unsafe {
            let _ = CloseHandle(token);
        }
        return Err(format!("CreateEnvironmentBlock: {e}"));
    }

    // 命令行（引号路径 + 参数——lpApplicationName 与 lpCommandLine 同给，路径含
    // 空格时由引号保护）。
    let mut cmdline = format!("\"{}\"", request.exe.display());
    for arg in &request.args {
        cmdline.push(' ');
        cmdline.push_str(arg);
    }
    let app_wide = wide(&request.exe.to_string_lossy());
    let mut cmdline_wide = wide(&cmdline);
    let mut desktop_wide = wide("winsta0\\default");

    let startup = STARTUPINFOW {
        cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).unwrap_or(0),
        lpDesktop: PWSTR(desktop_wide.as_mut_ptr()),
        ..STARTUPINFOW::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: app/cmdline/desktop 宽字符串 NUL 终结且活过调用；startup/info 为
    // 输出容器；CREATE_UNICODE_ENVIRONMENT 声明 environment 指向 UTF-16 块；
    // 不继承句柄（binherithandles=false，core 不持有 engine 句柄——oneshot
    // 不变量同款纪律）。成功时 info 的进程/线程句柄所有权归调用方。
    let create_result = unsafe {
        CreateProcessAsUserW(
            Some(token),
            PCWSTR(app_wide.as_ptr()),
            Some(windows::core::PWSTR(cmdline_wide.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(CREATE_UNICODE_ENVIRONMENT.0),
            Some(environment),
            PCWSTR(std::ptr::null()), // lpCurrentDirectory：NULL = 继承目标令牌 cwd 语义
            &startup,
            &mut info,
        )
    };

    // 环境块与令牌的释放独立于 create 结局（成功失败都恰一次）。
    // SAFETY: environment 由 CreateEnvironmentBlock 产出的块指针；恰一次销毁。
    unsafe {
        let _ = DestroyEnvironmentBlock(environment);
    }
    // SAFETY: token 由 WTSQueryUserToken 产出；恰一次关闭。
    unsafe {
        let _ = CloseHandle(token);
    }

    create_result.map_err(|e| format!("CreateProcessAsUserW: {e}"))?;
    // SAFETY: info 的进程/线程句柄由 CreateProcessAsUserW 产出；engine 不等待/
    // 不操控新 core（生命周期由 core 单实例与 EOF 语义管理），取得 pid 后立即
    // 关闭两句柄（恰一次）。
    unsafe {
        let _ = CloseHandle(info.hProcess);
        let _ = CloseHandle(info.hThread);
    }
    Ok(SpawnedCore {
        pid: info.dwProcessId,
        launched_at_ms: unix_now_ms(),
    })
}

// ---------------------------------------------------------------------------
// 单元测试：参数契约（纯函数）；真实 WTS/CreateProcessAsUser 行为归 T13 实机验收。
// ---------------------------------------------------------------------------
