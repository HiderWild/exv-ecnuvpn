//! 独立服务代理的固定 CLI 入口。
//!
//! 可解析的形式仅有：裸 `install`/`uninstall`（终端 sudo 形态，从 `SUDO_UID`/`SUDO_GID`
//! 解析 enrolled owner）、`install --owner-uid N --owner-gid N [--engine-path P]` 与
//! `uninstall --owner-uid N --owner-gid N`（osascript 提权形态：root shell 没有
//! `SUDO_UID`/`SUDO_GID`，显式 flags 给出 owner 并完全忽略 `SUDO_*` 环境）、裸 `start`
//! （core 经 osascript 提权按需启动 daemon 的最小系统操作）、普通用户的 `status`/
//! `cleanup`，以及安装描述内部使用的严格 `serve` 形式。除已校验的 `--engine-path`
//! 固定 Engine 绝对路径外，没有 path、label、command 或额外 argv 的接口；解析层不
//! 保留任何其他调用者输入。

use std::{env, ffi::OsString, str::FromStr};

use crate::{
    ServiceAgentAction, ServiceAgentOutcome,
    macos::{MacosPlatform, engine_path_is_valid},
    platform::{EnrolledOwner, PlatformError},
    socket::{request_from_ordinary_user, serve_forever},
};

/// 固定 CLI 的解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    /// 显式 root install（终端 sudo 形态：读 `SUDO_UID`/`SUDO_GID`）。
    Install,
    /// 显式 root install（osascript 提权形态：显式 owner，完全忽略 `SUDO_*`）。
    ///
    /// `--engine-path` 是 V1 唯一被批准的自由文本输入：必须是绝对路径、长度受限、无
    /// `..` 组件、常规文件且非 symlink，校验通过后原文进入 root 0600 固定 state 叶。
    InstallWithOwner(EnrolledOwner, Option<String>),
    /// 显式 root uninstall（终端 sudo 形态：读 `SUDO_UID`/`SUDO_GID`）。
    Uninstall,
    /// 显式 root uninstall（osascript 提权形态：显式 owner，完全忽略 `SUDO_*`）。
    UninstallWithOwner(EnrolledOwner),
    /// 显式 root start：core 经 osascript 提权按需启动 daemon 的唯一入口（无 owner
    /// 语义，与 launchctl 同级的最小系统操作）。
    Start,
    /// W3 oneshot：osascript 提权形态的一次性 Engine 拉起（显式 owner、runtime
    /// 目录、core pid 与 engine 绝对路径；stdout 打印恰好一行十进制 pid 供无特权
    /// Core 经 Elevator 捕获解析）。
    StartEngineOnce {
        owner: EnrolledOwner,
        runtime_dir: String,
        core_pid: u32,
        engine_path: String,
    },
    /// 普通用户 socket status。
    Status,
    /// 普通用户 socket cleanup。
    Cleanup,
    /// 2026-09-12：一次性回收历史（已退役）组件遗留。无参数、无 owner 语义，
    /// 仅 root；固定 label/路径白名单见 `LEGACY_*` 常量。
    RetireLegacy,
    /// 2026-09-12：清扫 root 属主 runtime 残留（无参数、无 owner 语义，仅 root；
    /// 规则见 `RUNTIME_RESIDUE_*` 常量）。
    SweepRuntimeResidue,
    /// 固定安装描述启动的 root daemon。
    Serve(EnrolledOwner),
}

/// 解析进程 argv；非 UTF-8、额外参数或非固定文字均拒绝。
///
/// # Errors
///
/// argv 不匹配 V1 固定形式时返回 [`PlatformError::CliUsage`]。
pub fn parse_process_command() -> Result<CliCommand, PlatformError> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    parse_command(&args)
}

/// 解析已经分离出的参数，供单元测试使用。
///
/// # Errors
///
/// 任何自由 path、label、command、额外参数、非法 owner number 或不合法
/// `--engine-path`（相对路径、超长、含 `..`、非常规文件或 symlink）返回
/// [`PlatformError::CliUsage`]。
pub fn parse_command(args: &[OsString]) -> Result<CliCommand, PlatformError> {
    let literals = args
        .iter()
        .map(|argument| argument.to_str().ok_or(PlatformError::CliUsage))
        .collect::<Result<Vec<_>, _>>()?;
    match literals.as_slice() {
        ["install"] => Ok(CliCommand::Install),
        ["install", "--owner-uid", uid, "--owner-gid", gid] => Ok(CliCommand::InstallWithOwner(
            explicit_owner(uid, gid)?,
            None,
        )),
        [
            "install",
            "--owner-uid",
            uid,
            "--owner-gid",
            gid,
            "--engine-path",
            engine_path,
        ] => Ok(CliCommand::InstallWithOwner(
            explicit_owner(uid, gid)?,
            Some(validated_engine_path(engine_path)?),
        )),
        ["uninstall"] => Ok(CliCommand::Uninstall),
        ["uninstall", "--owner-uid", uid, "--owner-gid", gid] => {
            Ok(CliCommand::UninstallWithOwner(explicit_owner(uid, gid)?))
        }
        ["start"] => Ok(CliCommand::Start),
        [
            "start-engine-once",
            "--owner-uid",
            uid,
            "--owner-gid",
            gid,
            "--runtime-dir",
            runtime_dir,
            "--core-pid",
            core_pid,
            "--engine-path",
            engine_path,
        ] => {
            let core_pid = u32::from_str(core_pid).map_err(|_| PlatformError::CliUsage)?;
            // 零 pid 不是合法 core 身份（与 root 侧 spawn 前复核同规约）。
            if core_pid == 0 {
                return Err(PlatformError::CliUsage);
            }
            Ok(CliCommand::StartEngineOnce {
                owner: explicit_owner(uid, gid)?,
                runtime_dir: validated_runtime_dir(runtime_dir)?,
                core_pid,
                engine_path: validated_engine_path(engine_path)?,
            })
        }
        ["status"] => Ok(CliCommand::Status),
        ["cleanup"] => Ok(CliCommand::Cleanup),
        ["retire-legacy"] => Ok(CliCommand::RetireLegacy),
        ["sweep-runtime-residue"] => Ok(CliCommand::SweepRuntimeResidue),
        ["serve", "--owner-uid", uid, "--owner-gid", gid] => {
            let uid = u32::from_str(uid).map_err(|_| PlatformError::CliUsage)?;
            let gid = u32::from_str(gid).map_err(|_| PlatformError::CliUsage)?;
            EnrolledOwner::new(uid, gid)
                .map_err(|_| PlatformError::CliUsage)
                .map(CliCommand::Serve)
        }
        _ => Err(PlatformError::CliUsage),
    }
}

/// 解析显式 owner 数字对；root uid 或非法数字按白名单风格统一为
/// [`PlatformError::CliUsage`]（既有形态 `serve` 同样如此映射）。
fn explicit_owner(uid: &str, gid: &str) -> Result<EnrolledOwner, PlatformError> {
    let uid = u32::from_str(uid).map_err(|_| PlatformError::CliUsage)?;
    let gid = u32::from_str(gid).map_err(|_| PlatformError::CliUsage)?;
    EnrolledOwner::new(uid, gid).map_err(|_| PlatformError::CliUsage)
}

/// 校验并保留唯一被批准的自由文本输入：固定 Engine 绝对路径。
///
/// 不合法（相对路径、超长、含 `..`、非常规文件或 symlink）返回
/// [`PlatformError::CliUsage`]——该稳定错误码已完整表达「调用形态不被接受」，无需
/// 新增错误类别，也不会泄露被拒绝的输入内容。
fn validated_engine_path(path: &str) -> Result<String, PlatformError> {
    if engine_path_is_valid(path) {
        Ok(path.to_owned())
    } else {
        Err(PlatformError::CliUsage)
    }
}

/// oneshot runtime 目录的词法校验（绝对、限长、无 `..`）；属主/symlink/目录性由
/// root 执行层的目录级校验复核（`macos::start_engine_once_from_current_process`）。
fn validated_runtime_dir(dir: &str) -> Result<String, PlatformError> {
    const MAX_RUNTIME_DIR_LEN: usize = 512;
    let path = std::path::Path::new(dir);
    if path.is_absolute()
        && !dir.is_empty()
        && dir.len() <= MAX_RUNTIME_DIR_LEN
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        Ok(dir.to_owned())
    } else {
        Err(PlatformError::CliUsage)
    }
}

/// 执行当前进程的固定 CLI command。
///
/// `install`、`uninstall`、`start` 和 `serve` 只会在各自显式入口调用真实平台代码；
/// `status` 与 `cleanup` 只通过固定 Unix socket。
///
/// # Errors
///
/// 解析、进程身份、固定 socket 或固定平台操作失败时返回稳定 [`PlatformError`]。
pub fn run_process() -> Result<Option<ServiceAgentOutcome>, PlatformError> {
    match parse_process_command()? {
        CliCommand::Install => {
            MacosPlatform::install_from_current_process()?;
            Ok(None)
        }
        CliCommand::InstallWithOwner(owner, engine_path) => {
            MacosPlatform::install_from_current_process_with(owner, engine_path.as_deref())?;
            Ok(None)
        }
        CliCommand::Uninstall => {
            MacosPlatform::uninstall_from_current_process()?;
            Ok(None)
        }
        CliCommand::UninstallWithOwner(owner) => {
            MacosPlatform::uninstall_from_current_process_with(owner)?;
            Ok(None)
        }
        CliCommand::Start => {
            MacosPlatform::start_from_current_process()?;
            Ok(None)
        }
        CliCommand::StartEngineOnce {
            owner,
            runtime_dir,
            core_pid,
            engine_path,
        } => {
            let pid = MacosPlatform::start_engine_once_from_current_process(
                owner,
                &runtime_dir,
                core_pid,
                &engine_path,
            )?;
            // 规约：stdout 恰好一行十进制 pid（无特权 Core 经 osascript 捕获解析，
            // 规约见 core 侧 `parse_engine_pid_line`）。
            println!("{pid}");
            Ok(None)
        }
        CliCommand::Status => request_from_ordinary_user(ServiceAgentAction::Status).map(Some),
        CliCommand::Cleanup => request_from_ordinary_user(ServiceAgentAction::Cleanup).map(Some),
        CliCommand::RetireLegacy => {
            MacosPlatform::retire_legacy_from_current_process()?;
            Ok(None)
        }
        CliCommand::SweepRuntimeResidue => {
            let report = MacosPlatform::sweep_runtime_residue_from_current_process()?;
            // 规约：stdout 输出两行汇总（`removed=<n>` / `skipped=<n>`），供提权方捕获记录；
            // 具体名字只进 stderr 诊断，不进 stdout 的机器可读行。
            println!("removed={}", report.removed.len());
            println!("skipped={}", report.skipped.len());
            for (leaf, reason) in &report.skipped {
                eprintln!("skip {leaf}: {reason}");
            }
            Ok(None)
        }
        CliCommand::Serve(owner) => {
            serve_forever(owner)?;
            Ok(None)
        }
    }
}
