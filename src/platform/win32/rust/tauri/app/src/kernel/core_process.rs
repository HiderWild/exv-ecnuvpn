//! core 进程归属（O3 强绑定，P4-b 决策）：**core 由 Tauri 宿主进程 spawn**。
//!
//! 决策依据（计划 Phase 4 §5 / O3）：UI 与 core 强绑定——UI 彻底退出 → core+engine
//! 一并终止。core 是普通用户 token 的纯协调层（永不特权），由 UI 宿主直接
//! `std::process::Command` 拉起（无需提权路径；engine 才经 core 的
//! `ShellExecuteExW(runas)` 提权拉起）。UI 退出时经 [`CoreChild`] 终止/等待退出——
//! core 不得遗留（O3）。
//!
//! 与 host `process_lifecycle`（core→engine）的差异：本模块是 **UI→core** 的原语，
//! 非特权 spawn（core 不 elevated）、无 wintun/journal/authority 参数。
//!
//! ## 启动参数契约（UI → core；core 侧解析由 P5 落 host main 时对齐）
//!
//! - `--control-pipe <name>`：core 建 `KernelControl` 服务管道（`serve_kernel_control_pipe`），
//!   UI 用同一名字拨号；名按 UI PID 唯一（`\\.\pipe\exv-core-<ui_pid>`）。
//! - `--ui-sid <sid>`：UI 进程用户 SID——core 用它建管道 DACL（SYSTEM + UI SID）并
//!   在 accept 后验证 UI peer（`verify_ui_peer` 要求 SID 精确匹配）。
//! - `--ui-pid <pid>`：UI 进程 PID（core 日志/身份记录用）。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use super::core_transport::current_user_sid;

/// core 控制面 Named Pipe 名（按 UI PID 唯一——core 每次由 UI 拉起，名不冲突）。
#[must_use]
pub fn core_control_pipe_name() -> String {
    let ui_pid = std::process::id();
    format!(r"\\.\pipe\exv-core-{ui_pid}")
}

/// core 可执行路径（测试流：`CARGO_BIN_EXE_exv-core`；bin 流：`current_exe`
/// 同目录 sibling `exv-core(.exe)`）。镜像 host `engine_bin_path` 模式。
#[must_use]
pub fn core_bin_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_exv-core") {
        let bp = PathBuf::from(p);
        if bp.exists() {
            return Some(bp);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if exe.file_name()?.to_string_lossy().ends_with(".exe") {
        "exv-core.exe"
    } else {
        "exv-core"
    };
    Some(dir.join(name))
}

/// core 的启动参数（UI 侧构造；管道名按 UI PID 唯一）。
///
/// SID 不可解析（极罕见）时省略 `--ui-sid`——core 回退本进程用户 SID，同用户拓扑下
/// 两者相等（与 host `engine_args_for_spawn` 同构的容错）。
#[must_use]
pub fn core_args_for_spawn() -> Vec<String> {
    let ui_pid = std::process::id();
    let mut args = vec![
        "--control-pipe".to_string(),
        core_control_pipe_name(),
        "--ui-pid".to_string(),
        ui_pid.to_string(),
    ];
    if let Some(sid) = current_user_sid() {
        args.push("--ui-sid".to_string());
        args.push(sid);
    }
    args
}

/// spawn core 进程失败的 typed 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreSpawnError {
    /// 进程拉起失败（操作系统错误码）。
    Spawn(String),
}

impl std::fmt::Display for CoreSpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "core spawn failed: {e}"),
        }
    }
}

impl std::error::Error for CoreSpawnError {}

/// spawn core 进程（非特权；核心参数契约见模块文档）。
///
/// # Errors
/// 拉起失败 → `CoreSpawnError::Spawn`。
pub fn spawn_core(exe: &Path) -> Result<CoreChild, CoreSpawnError> {
    let child = Command::new(exe)
        .args(core_args_for_spawn())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| CoreSpawnError::Spawn(e.to_string()))?;
    let pid = child.id();
    Ok(CoreChild {
        child: Some(child),
        pid,
    })
}

/// core 子进程句柄：持有 `(pid, Child)`，生命周期安全（O3）。
///
/// - [`CoreChild::terminate`]：强制终止（显式调用；core 挂死时的兜底）。
///
/// **有意不做 Drop 自动 terminate**（对比 host `EngineChild`）：core 侧 O3 语义是
/// 「UI 退出 → 管道关闭 → core `serve_task` resolve → core 有序停机（engine
/// StopTunnel → engine 终止）→ core 退出」——UI 侧 kill 会打断有序停机。core 由
/// pipe-close 感知自行退出，UI 无需（也不应）在 Drop 时强杀。
pub struct CoreChild {
    child: Option<Child>,
    pid: u32,
}

impl CoreChild {
    /// 进程 PID。
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 只在用户主动连接而控制管道已经不可达时调用的非阻塞子进程确认。
    ///
    /// `true` 表示受 UI 托管的 core 确已退出（或从未成功拉起），调用方才可重拉；
    /// 查询句柄失败时保守返回 `false`，避免把“无法确认”误判为已退出并重复启动 core。
    pub fn has_exited(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return true;
        };
        match child.try_wait() {
            Ok(Some(_)) => {
                self.child = None;
                true
            }
            Ok(None) | Err(_) => false,
        }
    }

    /// 强制终止进程（幂等；已退出/已 terminate 时 no-op）。core 挂死时的显式兜底。
    #[allow(dead_code)]
    pub fn terminate(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
        self.child = None;
    }
}

// ---------------------------------------------------------------------------
// 单元测试：管道名唯一 + 参数契约往返 + bin 路径解析。
// ---------------------------------------------------------------------------
