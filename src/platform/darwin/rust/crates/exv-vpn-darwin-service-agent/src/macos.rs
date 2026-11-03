//! macOS 固定路径平台实现。
//!
//! 这里没有可配置的文件、label、服务目标或启动参数（唯一例外：CLI 已校验的
//! `--engine-path` 固定 Engine 绝对路径，原文写入 root 0600 state 叶）。所有实际系统
//! 改动只能从显式 root CLI 进入 [`MacosPlatform`] 的 [`PrivilegedPlatform`] 实现；
//! 单元测试只使用 `platform` 模块的 fake seam 与临时目录。

use std::{
    borrow::Cow,
    env,
    ffi::{CStr, CString},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    mem::MaybeUninit,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, PermissionsExt},
            io::RawFd,
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::Path,
    process::{Command, ExitStatus, Stdio},
    ptr::addr_of_mut,
};

use crate::{
    ArtifactPathId, SERVICE_AGENT_LABEL,
    platform::{
        ArtifactOwner, ENGINE_BINARY_PATH, ENGINE_LABEL, ENGINE_PLIST_PATH, ENGINE_SERVICE_TARGET,
        ENGINE_SOCKET_PATH, EngineJobAction, EnrolledOwner, INSTALL_ROOT_DIR,
        INSTALL_ROOT_DIR_LEAF, INSTALLED_ENGINE_PATH, LAUNCHCTL_PATH, LAUNCHCTL_SYSTEM_DOMAIN,
        OrphanDimension, PlatformError, PrivilegedPlatform, SERVICE_AGENT_BINARY_PATH,
        SERVICE_AGENT_DESCRIPTOR_PATH, SERVICE_AGENT_INSTALL_DIR, SERVICE_AGENT_INSTALL_DIR_LEAF,
        SERVICE_AGENT_SERVICE_TARGET, SERVICE_AGENT_SOCKET_PARENT, SERVICE_AGENT_STATE_PATH,
        ServiceControlAction, engine_path_lexically_valid, install_with, legacy_guard_report_line,
        orphan_guard_report_line, require_process_root, retire_legacy_with, start_with,
        uninstall_with,
    },
};

const ROOT_UID: u32 = 0;
const ROOT_GID: u32 = 0;
const ROOT_FILE_MODE: u32 = 0o600;
const BINARY_MODE: u32 = 0o755;
const DESCRIPTOR_MODE: u32 = 0o644;
const SOCKET_MODE: u32 = 0o600;

/// 服务代理文件日志快捷入口（component 固定 `agent`；S7；`pub(crate)` 供
/// [`crate::cli`] 的 serve 挂点复用）。
///
/// 挂点纪律：事件字段只携带 pid/owner uid/决策布尔/错误类别（wire 契约「错误只
/// 编码稳定类别」同纪律）；凭据/cookie/token/session id 一律不进日志（`file_log`
/// 的 key 脱敏是兜底防线，不是许可）。写失败静默降级，不阻断业务。
pub(crate) fn agent_log(level: &str, code: &str, message: &str, fields: &[(&str, String)]) {
    crate::file_log::default_log().log(level, "agent", code, message, fields);
}

/// [`crate::macos::SocketStartupError`] 的稳定短类别（serve 启动失败日志字段用）。
pub(crate) fn socket_startup_category(error: &SocketStartupError) -> String {
    match error {
        SocketStartupError::Existing(platform) => platform.code().to_owned(),
        SocketStartupError::Io { step, .. } => format!("io:{step}"),
    }
}

/// install 结果落日志（OK/FAILED；owner uid 供取证对齐）。
fn install_logged(outcome: Result<(), PlatformError>, owner_uid: u32) -> Result<(), PlatformError> {
    match outcome {
        Ok(()) => {
            agent_log(
                "info",
                "INSTALL_OK",
                "service agent install finished",
                &[("owner_uid", owner_uid.to_string())],
            );
            Ok(())
        }
        Err(error) => {
            agent_log(
                "error",
                "INSTALL_FAILED",
                "service agent install failed",
                &[("error", error.code().to_owned())],
            );
            Err(error)
        }
    }
}

/// uninstall 结果落日志（OK/FAILED；owner uid 供取证对齐）。
fn uninstall_logged(
    outcome: Result<(), PlatformError>,
    owner_uid: u32,
) -> Result<(), PlatformError> {
    match outcome {
        Ok(()) => {
            eprintln!("UNINSTALL_OK owner_uid={owner_uid}");
            agent_log(
                "info",
                "UNINSTALL_OK",
                "service agent uninstall finished",
                &[("owner_uid", owner_uid.to_string())],
            );
            Ok(())
        }
        Err(error) => {
            eprintln!("UNINSTALL_FAILED owner_uid={owner_uid} error={error}");
            agent_log(
                "error",
                "UNINSTALL_FAILED",
                "service agent uninstall failed",
                &[("error", error.code().to_owned())],
            );
            Err(error)
        }
    }
}

/// macOS 固定路径平台实现；构造本身没有副作用。
#[derive(Debug)]
pub struct MacosPlatform {
    /// serve 端固定 Engine 路径：state 叶校验通过的路径，或 canonical 回退。
    engine_path: Cow<'static, str>,
}

impl MacosPlatform {
    /// 创建固定路径平台实现（serve 端 Engine 路径回退 canonical）。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            engine_path: Cow::Borrowed(INSTALLED_ENGINE_PATH),
        }
    }

    /// serve 入口构造：优先固定 state 叶中校验通过的 Engine 路径，缺失或不合法时回退
    /// [`INSTALLED_ENGINE_PATH`]（服务代理安装时写下的已装 Engine 路径）。
    #[must_use]
    pub fn for_serve() -> Self {
        Self {
            engine_path: resolve_serve_engine_path(),
        }
    }

    /// serve 端当前固定 Engine 路径。
    #[must_use]
    pub fn engine_path(&self) -> &str {
        self.engine_path.as_ref()
    }

    /// 读取当前 real/effective uid，并从 `SUDO_UID`/`SUDO_GID` 构造 enrolled owner。
    ///
    /// # Errors
    ///
    /// 直接 root、非 root、缺失或非法 `SUDO_UID`/`SUDO_GID` 均返回稳定
    /// [`PlatformError`]。
    pub fn enrolled_developer_from_process() -> Result<EnrolledOwner, PlatformError> {
        let (real_uid, effective_uid) = current_process_uids();
        let sudo_user = env::var("SUDO_UID").ok();
        let sudo_group = env::var("SUDO_GID").ok();
        crate::platform::enrolled_owner_from_sudo_context(
            real_uid,
            effective_uid,
            sudo_user.as_deref(),
            sudo_group.as_deref(),
        )
    }

    /// 验证当前进程 real/effective uid 均为 root；不读取 `SUDO_*`。
    ///
    /// 供 root daemon serve 与 osascript 提权形态（显式 owner install/uninstall、裸
    /// `start`）共用。
    ///
    /// # Errors
    ///
    /// real 或 effective uid 不是 root 时返回 [`PlatformError::RootRequired`]。
    pub fn require_root_process() -> Result<(), PlatformError> {
        let (real_uid, effective_uid) = current_process_uids();
        require_process_root(real_uid, effective_uid)
    }

    /// 验证当前进程是普通用户 socket client。
    ///
    /// # Errors
    ///
    /// real 或 effective uid 为 root 时返回 [`PlatformError::OrdinaryUserRequired`]。
    pub fn ordinary_user_uid() -> Result<u32, PlatformError> {
        let (real_uid, effective_uid) = current_process_uids();
        if real_uid == ROOT_UID || effective_uid == ROOT_UID {
            return Err(PlatformError::OrdinaryUserRequired);
        }
        Ok(real_uid)
    }

    /// 从当前显式 root CLI（sudo 上下文形态）运行替换式固定 install 顺序。
    ///
    /// # Errors
    ///
    /// 进程身份、`SUDO_UID`/`SUDO_GID`、固定 artifact 或 `bootstrap` 失败时返回稳定
    /// [`PlatformError`]。
    pub fn install_from_current_process() -> Result<(), PlatformError> {
        let owner = match Self::enrolled_developer_from_process() {
            Ok(owner) => owner,
            Err(error) => {
                agent_log(
                    "error",
                    "INSTALL_FAILED",
                    "install rejected before artifacts",
                    &[("error", error.code().to_owned())],
                );
                return Err(error);
            }
        };
        agent_log(
            "info",
            "INSTALL_STARTED",
            "service agent install started",
            &[
                ("owner_uid", owner.uid().to_string()),
                ("pid", std::process::id().to_string()),
            ],
        );
        install_logged(install_with(&mut Self::new(), owner, None), owner.uid())
    }

    /// 从 osascript 提权 root shell 运行替换式固定 install 顺序（显式 owner 形态）。
    ///
    /// root shell 没有 `SUDO_UID`/`SUDO_GID`，本入口完全忽略 `SUDO_*` 环境，owner 完全
    /// 由 CLI 显式 flags 给出；信任边界是 macOS 系统密码弹窗授权整条 payload 文本。
    ///
    /// # Errors
    ///
    /// 非 real/effective root、固定 artifact 或 `bootstrap` 失败时返回稳定
    /// [`PlatformError`]。
    pub fn install_from_current_process_with(
        owner: EnrolledOwner,
        engine_path: Option<&str>,
    ) -> Result<(), PlatformError> {
        if let Err(error) = Self::require_root_process() {
            agent_log(
                "error",
                "INSTALL_FAILED",
                "install rejected: caller is not root",
                &[("error", error.code().to_owned())],
            );
            return Err(error);
        }
        agent_log(
            "info",
            "INSTALL_STARTED",
            "service agent install started",
            &[
                ("owner_uid", owner.uid().to_string()),
                ("engine_path_given", engine_path.is_some().to_string()),
                ("pid", std::process::id().to_string()),
            ],
        );
        let mut platform = Self::new();
        install_logged(install_with(&mut platform, owner, engine_path), owner.uid())
    }

    /// 从当前显式 root CLI（sudo 上下文形态）运行固定 uninstall 顺序。
    ///
    /// # Errors
    ///
    /// 进程身份、`SUDO_UID`/`SUDO_GID`、固定 descriptor owner、`bootout` 或任一固定
    /// artifact 删除失败时返回稳定 [`PlatformError`]。
    pub fn uninstall_from_current_process() -> Result<(), PlatformError> {
        let owner = match Self::enrolled_developer_from_process() {
            Ok(owner) => owner,
            Err(error) => {
                agent_log(
                    "error",
                    "UNINSTALL_FAILED",
                    "uninstall rejected before artifacts",
                    &[("error", error.code().to_owned())],
                );
                return Err(error);
            }
        };
        agent_log(
            "info",
            "UNINSTALL_STARTED",
            "service agent uninstall started",
            &[
                ("owner_uid", owner.uid().to_string()),
                ("pid", std::process::id().to_string()),
            ],
        );
        uninstall_logged(uninstall_with(&mut Self::new(), owner), owner.uid())
    }

    /// 从 osascript 提权 root shell 运行固定 uninstall 顺序（显式 owner 形态）。
    ///
    /// 与 [`Self::install_from_current_process_with`] 相同：忽略 `SUDO_*`，owner 由显式
    /// flags 给出。
    ///
    /// # Errors
    ///
    /// 非 real/effective root、固定 descriptor owner、`bootout` 或任一固定 artifact 删除
    /// 失败时返回稳定 [`PlatformError`]。
    pub fn uninstall_from_current_process_with(owner: EnrolledOwner) -> Result<(), PlatformError> {
        if let Err(error) = Self::require_root_process() {
            agent_log(
                "error",
                "UNINSTALL_FAILED",
                "uninstall rejected: caller is not root",
                &[("error", error.code().to_owned())],
            );
            return Err(error);
        }
        agent_log(
            "info",
            "UNINSTALL_STARTED",
            "service agent uninstall started",
            &[
                ("owner_uid", owner.uid().to_string()),
                ("pid", std::process::id().to_string()),
            ],
        );
        let mut platform = Self::new();
        uninstall_logged(uninstall_with(&mut platform, owner), owner.uid())
    }

    /// 从 osascript 提权 root shell 清扫 root 属主 runtime 残留。
    ///
    /// 无 owner 语义（与 `start`/`retire-legacy` 同级的最小系统操作）；规则见
    /// [`crate::RUNTIME_RESIDUE_*`]，**不接受任何调用方输入**。单项失败记入报告而非
    /// 返回错误——卸载不应因一个残留清不掉而中止。
    ///
    /// # Errors
    ///
    /// 非 real/effective root，或无法枚举/安全打开 `/private/tmp` 时返回稳定
    /// [`PlatformError`]。
    pub fn sweep_runtime_residue_from_current_process()
    -> Result<crate::platform::ResidueSweepReport, PlatformError> {
        if let Err(error) = Self::require_root_process() {
            agent_log(
                "error",
                "RESIDUE_SWEEP_FAILED",
                "runtime residue sweep rejected: caller is not root",
                &[("error", error.code().to_owned())],
            );
            return Err(error);
        }
        agent_log(
            "info",
            "RESIDUE_SWEEP_STARTED",
            "runtime residue sweep started",
            &[("pid", std::process::id().to_string())],
        );
        match Self::sweep_runtime_residue_impl() {
            Ok(report) => {
                agent_log(
                    "info",
                    "RESIDUE_SWEEP_OK",
                    "runtime residue sweep finished",
                    &[
                        ("removed", report.removed.len().to_string()),
                        ("skipped", report.skipped.len().to_string()),
                    ],
                );
                Ok(report)
            }
            Err(error) => {
                agent_log(
                    "error",
                    "RESIDUE_SWEEP_FAILED",
                    "runtime residue sweep failed",
                    &[("error", error.code().to_owned())],
                );
                Err(error)
            }
        }
    }

    /// 从 osascript 提权 root shell 一次性回收历史（已退役）组件遗留。
    ///
    /// 无 owner 语义（与 `start` 同级的最小系统操作）：只 bootout 三个固定历史 label，
    /// 删除四个固定历史 descriptor 与三个固定历史安装目录。**不接受任何调用方输入的
    /// label 或路径**；目标本就不存在视为成功（幂等）。
    ///
    /// # Errors
    ///
    /// 非 real/effective root，或任一固定 label 的 `bootout`、descriptor 删除、安装目录
    /// 回收失败时返回稳定 [`PlatformError`]。
    pub fn retire_legacy_from_current_process() -> Result<(), PlatformError> {
        Self::require_root_process()?;
        let mut platform = Self::new();
        retire_legacy_with(&mut platform)
    }

    /// 从 osascript 提权 root shell 运行「按需启动」（显式 root 形态，无 owner 语义）。
    ///
    /// core 探测到「已安装但 daemon 不可用」时的唯一启动入口：best-effort `bootout` 后
    /// `bootstrap`；对健康 daemon 等价于重启（KeepAlive 场景下极少触发）。
    ///
    /// # Errors
    ///
    /// 非 real/effective root 或固定 `bootstrap` 失败时返回稳定 [`PlatformError`]；
    /// `bootout` 失败被忽略。
    pub fn start_from_current_process() -> Result<(), PlatformError> {
        if let Err(error) = Self::require_root_process() {
            agent_log(
                "error",
                "START_FAILED",
                "daemon start rejected: caller is not root",
                &[("error", error.code().to_owned())],
            );
            return Err(error);
        }
        agent_log(
            "info",
            "START_STARTED",
            "daemon on-demand start began",
            &[("pid", std::process::id().to_string())],
        );
        let mut platform = Self::new();
        match start_with(&mut platform) {
            Ok(()) => {
                agent_log(
                    "info",
                    "START_OK",
                    "daemon on-demand start finished",
                    &[],
                );
                Ok(())
            }
            Err(error) => {
                agent_log(
                    "error",
                    "START_FAILED",
                    "daemon on-demand start failed",
                    &[("error", error.code().to_owned())],
                );
                Err(error)
            }
        }
    }

    /// W3 oneshot：osascript 提权形态的 `start-engine-once` 入口——显式 owner、
    /// runtime 目录、core pid 与 engine 绝对路径，守护式 spawn 后返回 engine pid
    /// （CLI 层打印到 stdout 恰好一行，供无特权 Core 经 Elevator 捕获解析）。
    ///
    /// 与 daemon 帧路径（E8 已退役）的唯一差异：engine 二进制不经安装态 state 叶，
    /// 由 Core 显式传入 bundle 内路径并全量复核。
    ///
    /// # Errors
    ///
    /// 非 real/effective root、engine 路径不合法、runtime 目录不合规（词法/非目录/
    /// symlink/属主不符）、零 core pid 或 spawn 失败时返回稳定 [`PlatformError`]。
    pub fn start_engine_once_from_current_process(
        owner: EnrolledOwner,
        runtime_dir: &str,
        core_pid: u32,
        engine_path: &str,
    ) -> Result<u32, PlatformError> {
        let oneshot_rejected = |error: PlatformError| {
            agent_log(
                "error",
                "ENGINE_ONESHOT_FAILED",
                "one-shot engine launch rejected before spawn",
                &[
                    ("error", error.code().to_owned()),
                    ("owner_uid", owner.uid().to_string()),
                    ("core_pid", core_pid.to_string()),
                ],
            );
            error
        };
        if let Err(error) = Self::require_root_process() {
            return Err(oneshot_rejected(error));
        }
        if !engine_path_is_valid(engine_path) {
            return Err(oneshot_rejected(PlatformError::CliUsage));
        }
        if let Err(error) = validate_oneshot_runtime_dir(runtime_dir, owner.uid()) {
            return Err(oneshot_rejected(error));
        }
        if core_pid == 0 {
            return Err(oneshot_rejected(PlatformError::CliUsage));
        }
        agent_log(
            "info",
            "ENGINE_ONESHOT_STARTED",
            "one-shot engine launch began",
            &[
                ("owner_uid", owner.uid().to_string()),
                ("core_pid", core_pid.to_string()),
                ("pid", std::process::id().to_string()),
            ],
        );
        match spawn_engine_guarded(engine_path, runtime_dir, owner.uid(), core_pid) {
            Ok(engine_pid) => {
                agent_log(
                    "info",
                    "ENGINE_ONESHOT_OK",
                    "one-shot engine launched",
                    &[
                        ("engine_pid", engine_pid.to_string()),
                        ("core_pid", core_pid.to_string()),
                    ],
                );
                Ok(engine_pid)
            }
            Err(error) => {
                agent_log(
                    "error",
                    "ENGINE_ONESHOT_FAILED",
                    "one-shot engine spawn failed",
                    &[("error", error.code().to_owned())],
                );
                Err(error)
            }
        }
    }
}

impl Default for MacosPlatform {
    fn default() -> Self {
        Self::new()
    }
}

const PROXY_POLICY_RECORD: &str =
    "/Library/Application Support/EXV/ServiceAgent/proxy-policy-owner.json";
const PROXY_POLICY_CLEANUP_ARG: &str = "--internal-cleanup-proxy-policy";

fn proxy_cleanup_io(step: &'static str, error: &io::Error) -> PlatformError {
    eprintln!("系统代理清理失败 step={step} error={error}");
    PlatformError::ProxyPolicyCleanupFailed {
        step, errno: error.raw_os_error(), exit_code: None, signal: None,
    }
}

fn run_proxy_cleanup_engine(engine: &Path, argument: &str) -> io::Result<ExitStatus> {
    Command::new(engine).arg(argument).stdin(Stdio::null()).status()
}

// 记录解析、文件锁及恢复语义由同一个 Engine 内部动作处理；测试接缝仅替换路径与执行器。
pub(crate) fn cleanup_proxy_policy_at(
    record: &Path,
    installed_engine: &Path,
    uninstall: bool,
    fallback: impl FnOnce() -> Result<std::path::PathBuf, PlatformError>,
    mut run: impl FnMut(&Path, &str) -> io::Result<ExitStatus>,
) -> Result<(), PlatformError> {
    if !record.try_exists().map_err(|error| proxy_cleanup_io("record.inspect", &error))? {
        return Ok(());
    }
    let status = match run(installed_engine, PROXY_POLICY_CLEANUP_ARG) {
        Ok(status) => status,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // 旧服务已卸载时，复用本次 Agent 同目录的 Engine；实际执行错误沿原结果返回。
            let engine = fallback().map_err(|cause| {
                eprintln!("系统代理清理执行器解析失败 previous_error={error} cause={cause}");
                PlatformError::ProxyPolicyCleanupFailed {
                    step: "engine.resolve_fallback", errno: None, exit_code: None, signal: None,
                }
            })?;
            run(&engine, PROXY_POLICY_CLEANUP_ARG)
                .map_err(|error| proxy_cleanup_io("engine.fallback", &error))?
        }
        Err(error) => return Err(proxy_cleanup_io("engine.installed", &error)),
    };
    if !status.success() {
        let error = PlatformError::ProxyPolicyCleanupFailed {
            step: "engine.cleanup", errno: None, exit_code: status.code(), signal: status.signal(),
        };
        eprintln!("系统代理清理执行失败 {error}");
        return Err(error);
    }
    if uninstall {
        fs::remove_file(record).map_err(|error| proxy_cleanup_io("record.remove", &error))?;
    }
    Ok(())
}

impl PrivilegedPlatform for MacosPlatform {
    fn cleanup_proxy_policy(&mut self, uninstall: bool) -> Result<(), PlatformError> {
        cleanup_proxy_policy_at(
            Path::new(PROXY_POLICY_RECORD),
            Path::new(INSTALLED_ENGINE_PATH),
            uninstall,
            || {
                let source = crate::platform::resolve_engine_install_source(None)?;
                if !engine_path_is_valid(&source) {
                    return Err(PlatformError::InstallPreparationFailed);
                }
                Ok(std::path::PathBuf::from(source))
            },
            run_proxy_cleanup_engine,
        )
    }

    fn install_descriptor_present(&mut self) -> Result<bool, PlatformError> {
        stat_artifact(ArtifactPathId::InstallDescriptor).map(|identity| identity.is_some())
    }

    fn install_binary(&mut self) -> Result<(), PlatformError> {
        ensure_install_directory()?;
        ensure_absent(ArtifactPathId::InstallBinary)?;

        let source_path = env::current_exe()
            .map_err(|error| install_io_failure("install_binary.current_exe", error))?;
        let source_metadata = fs::metadata(&source_path)
            .map_err(|error| install_io_failure("install_binary.source_metadata", error))?;
        if !source_metadata.is_file() {
            return Err(PlatformError::InstallPreparationFailed);
        }

        let mut source = File::open(source_path)
            .map_err(|error| install_io_failure("install_binary.open_source", error))?;
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(SERVICE_AGENT_BINARY_PATH)
            .map_err(|error| install_io_failure("install_binary.create_destination", error))?;
        io::copy(&mut source, &mut destination)
            .map_err(|error| install_io_failure("install_binary.copy", error))?;
        destination
            .sync_all()
            .map_err(|error| install_io_failure("install_binary.sync_all", error))?;
        drop(destination);

        fs::set_permissions(
            SERVICE_AGENT_BINARY_PATH,
            fs::Permissions::from_mode(BINARY_MODE),
        )
        .map_err(|error| install_io_failure("install_binary.set_permissions", error))?;
        lchown_fixed_path(
            SERVICE_AGENT_BINARY_PATH,
            ROOT_UID,
            ROOT_GID,
            "install_binary.lchown",
        )
        .map_err(|_| PlatformError::InstallPreparationFailed)?;
        Ok(())
    }

    fn install_descriptor(&mut self, owner: EnrolledOwner) -> Result<(), PlatformError> {
        ensure_root_controlled_directory(Path::new("/Library/LaunchDaemons"))?;
        ensure_absent(ArtifactPathId::InstallDescriptor)?;
        write_new_fixed_file(
            SERVICE_AGENT_DESCRIPTOR_PATH,
            descriptor_contents(owner).as_bytes(),
            DESCRIPTOR_MODE,
            "install_descriptor",
        )?;
        lchown_fixed_path(
            SERVICE_AGENT_DESCRIPTOR_PATH,
            ROOT_UID,
            ROOT_GID,
            "install_descriptor.lchown",
        )
        .map_err(|_| PlatformError::InstallPreparationFailed)?;
        Ok(())
    }

    fn install_state(&mut self, engine_path: &str) -> Result<(), PlatformError> {
        // 入口已校验；此处重复断言同一不变量，防止任何旁路写入非法路径。
        if !engine_path_is_valid(engine_path) {
            return Err(PlatformError::InstallPreparationFailed);
        }
        ensure_install_directory()?;
        write_new_fixed_file(
            SERVICE_AGENT_STATE_PATH,
            engine_path.as_bytes(),
            ROOT_FILE_MODE,
            "install_state",
        )?;
        lchown_fixed_path(
            SERVICE_AGENT_STATE_PATH,
            ROOT_UID,
            ROOT_GID,
            "install_state.lchown",
        )
        .map_err(|_| PlatformError::InstallPreparationFailed)?;
        Ok(())
    }

    fn verify_enrolled_owner(&mut self, owner: EnrolledOwner) -> Result<(), PlatformError> {
        verify_artifact(
            ArtifactPathId::InstallDescriptor,
            ArtifactOwner::Root,
            DESCRIPTOR_MODE,
        )?;
        let expected = descriptor_contents(owner);
        let actual = fs::read(SERVICE_AGENT_DESCRIPTOR_PATH)
            .map_err(|_| PlatformError::EnrolledOwnerMismatch)?;
        if actual != expected.as_bytes() {
            return Err(PlatformError::EnrolledOwnerMismatch);
        }
        Ok(())
    }

    fn control_service(&mut self, action: ServiceControlAction) -> Result<(), PlatformError> {
        run_fixed_launchctl(action)
    }

    fn remove_artifact(
        &mut self,
        artifact: ArtifactPathId,
        owner: ArtifactOwner,
    ) -> Result<(), PlatformError> {
        guarded_remove_artifact(artifact, owner)
    }

    fn remove_orphan_artifact(&mut self, artifact: ArtifactPathId) -> Result<(), PlatformError> {
        orphan_remove_artifact(artifact)
    }

    fn finish_uninstall_logging(&mut self) {
        agent_log("info", "UNINSTALL_LOG_CLEANUP_STARTED", "service artifacts removed; cleaning local logs", &[]);
        crate::file_log::default_log().shutdown();
    }

    fn remove_empty_install_dirs(&mut self) -> Result<(), PlatformError> {
        remove_empty_dir_guarded(SERVICE_AGENT_INSTALL_DIR, "logs")?;
        // 先子后父：安装目录为空才轮到父目录（父目录还含其它条目时 rmdir 失败即保留）。
        remove_empty_dir_guarded(INSTALL_ROOT_DIR, SERVICE_AGENT_INSTALL_DIR_LEAF)?;
        remove_empty_dir_guarded("/Library/Application Support", INSTALL_ROOT_DIR_LEAF)
    }

    fn bootout_legacy_label(&mut self, label: &str) -> Result<(), PlatformError> {
        run_fixed_launchctl_bootout_label(label)
    }

    fn remove_legacy_descriptor(&mut self, leaf: &str) -> Result<(), PlatformError> {
        guarded_remove_legacy_descriptor(leaf)
    }

    fn remove_legacy_install_dir(&mut self, leaf: &str) -> Result<(), PlatformError> {
        remove_legacy_install_dir_guarded(leaf)
    }

    fn sweep_runtime_residue(
        &mut self,
    ) -> Result<crate::platform::ResidueSweepReport, PlatformError> {
        Self::sweep_runtime_residue_impl()
    }

    fn install_engine_binary(&mut self, source: &str) -> Result<(), PlatformError> {
        ensure_install_directory()?;
        ensure_absent(ArtifactPathId::EngineBinary)?;

        let source_metadata = fs::metadata(source)
            .map_err(|error| install_io_failure("install_engine_binary.source_metadata", error))?;
        if !source_metadata.is_file() {
            return Err(PlatformError::InstallPreparationFailed);
        }
        let mut source = File::open(source)
            .map_err(|error| install_io_failure("install_engine_binary.open_source", error))?;
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(ENGINE_BINARY_PATH)
            .map_err(|error| install_io_failure("install_engine_binary.create_destination", error))?;
        io::copy(&mut source, &mut destination)
            .map_err(|error| install_io_failure("install_engine_binary.copy", error))?;
        destination
            .sync_all()
            .map_err(|error| install_io_failure("install_engine_binary.sync_all", error))?;
        drop(destination);

        fs::set_permissions(ENGINE_BINARY_PATH, fs::Permissions::from_mode(BINARY_MODE))
            .map_err(|error| install_io_failure("install_engine_binary.set_permissions", error))?;
        lchown_fixed_path(
            ENGINE_BINARY_PATH,
            ROOT_UID,
            ROOT_GID,
            "install_engine_binary.lchown",
        )
        .map_err(|_| PlatformError::InstallPreparationFailed)?;
        Ok(())
    }

    fn install_engine_descriptor(&mut self, owner: EnrolledOwner) -> Result<(), PlatformError> {
        ensure_root_controlled_directory(Path::new("/Library/LaunchDaemons"))?;
        ensure_absent(ArtifactPathId::EngineDescriptor)?;
        write_new_fixed_file(
            ENGINE_PLIST_PATH,
            engine_descriptor_contents(owner).as_bytes(),
            DESCRIPTOR_MODE,
            "install_engine_descriptor",
        )?;
        lchown_fixed_path(
            ENGINE_PLIST_PATH,
            ROOT_UID,
            ROOT_GID,
            "install_engine_descriptor.lchown",
        )
        .map_err(|_| PlatformError::InstallPreparationFailed)?;
        Ok(())
    }

    fn control_engine_job(&mut self, action: EngineJobAction) -> Result<(), PlatformError> {
        run_fixed_launchctl_for_engine(action)
    }
    fn control_engine_job_detailed(&mut self, action: EngineJobAction) -> Result<(), crate::platform::ServiceOperationFailure> {
        match action {
            EngineJobAction::EnsureBootstrap => ensure_engine_started_detailed_with(|args| {
                Command::new(LAUNCHCTL_PATH).env_clear().args(args).output()
            }),
            EngineJobAction::Bootout => run_fixed_launchctl_for_engine(action).map_err(Into::into),
        }
    }


    fn stop_engine_tunnel_best_effort(
        &mut self,
        owner: EnrolledOwner,
    ) -> Result<(), PlatformError> {
        nudge_engine_endpoint_eof(owner)
    }
}

/// serve 端固定 Engine 路径解析：state 叶存在且内容校验通过则采用，否则回退
/// [`INSTALLED_ENGINE_PATH`]。
fn resolve_serve_engine_path() -> Cow<'static, str> {
    state_engine_path().map_or_else(|| Cow::Borrowed(INSTALLED_ENGINE_PATH), Cow::Owned)
}

/// 守卫读取固定 state 叶：仅接受 root-owned 0600 常规文件中的已校验 Engine 路径。
fn state_engine_path() -> Option<String> {
    if verify_artifact(
        ArtifactPathId::StateLeaf,
        ArtifactOwner::Root,
        ROOT_FILE_MODE,
    )
    .is_err()
    {
        return None;
    }
    let contents = fs::read_to_string(SERVICE_AGENT_STATE_PATH).ok()?;
    accepted_state_engine_path(&contents)
}

/// 校验 state 叶候选内容并原样返回；任何不合法（含尾部换行）都让调用方回退。
fn accepted_state_engine_path(contents: &str) -> Option<String> {
    engine_path_is_valid(contents).then(|| contents.to_owned())
}

/// Engine 路径完整校验：词法规则（绝对、长度上限、无 `..` 组件）加文件系统事实
/// （存在、非 symlink、常规文件）。
pub(crate) fn engine_path_is_valid(path: &str) -> bool {
    engine_path_lexically_valid(path)
        && fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_symlink())
        && fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// 守护式 Engine spawn：固定 argv 形态（`--runtime-dir`/`--owner-uid`/`--core-pid`），
/// `setreuid(owner, 0)` 降实 uid 保有效 uid 0，stdio 全 null。
///
/// W3 起唯一调用方是 `start-engine-once`（daemon 的 StartEngine 帧已随 E8 退役）；
/// 调用方保证已过 root 门禁与全部参数校验。
fn spawn_engine_guarded(
    engine_path: &str,
    runtime_dir: &str,
    owner_uid: u32,
    core_pid: u32,
) -> Result<u32, PlatformError> {
    let mut command = Command::new(engine_path);
    command
        .arg("--runtime-dir")
        .arg(runtime_dir)
        .arg("--owner-uid")
        .arg(owner_uid.to_string())
        .arg("--core-pid")
        .arg(core_pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the caller is root (osascript-elevated `start-engine-once`); the fixed
    // Engine contract requires a normal real uid while retaining effective uid 0. No
    // user-supplied program or extra argv is involved.
    unsafe {
        command.pre_exec(move || {
            if libc::setreuid(owner_uid, 0) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    let child = command
        .spawn()
        .map_err(|_| PlatformError::EngineLaunchFailed)?;
    Ok(child.id())
}

/// oneshot runtime 目录的 root 侧目录级校验：词法规则（绝对、限长、无 `..`）+
/// 文件系统事实（非 symlink、真实目录、属主恰为 `--owner-uid`——目录由无特权
/// Core 以自身身份创建，属主校验即「只为发起连接的用户拉起」）。
fn validate_oneshot_runtime_dir(runtime_dir: &str, owner_uid: u32) -> Result<(), PlatformError> {
    const MAX_RUNTIME_DIR_LEN: usize = 512;
    let path = Path::new(runtime_dir);
    let lexical = path.is_absolute()
        && !runtime_dir.is_empty()
        && runtime_dir.len() <= MAX_RUNTIME_DIR_LEN
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir));
    if !lexical {
        return Err(PlatformError::CliUsage);
    }
    let symlink = fs::symlink_metadata(path).map_err(|_| PlatformError::CliUsage)?;
    if symlink.file_type().is_symlink() {
        return Err(PlatformError::CliUsage);
    }
    let metadata = fs::metadata(path).map_err(|_| PlatformError::CliUsage)?;
    if metadata.is_dir() && metadata.uid() == owner_uid {
        Ok(())
    } else {
        Err(PlatformError::CliUsage)
    }
}

/// 对固定 socket parent 执行 root ownership 与不可组/其他用户写入检查。
///
/// # Errors
///
/// parent 不是 root-controlled directory 时返回 [`PlatformError::SocketParentRejected`]。
pub(crate) fn ensure_socket_parent() -> Result<(), PlatformError> {
    ensure_root_controlled_directory(Path::new(SERVICE_AGENT_SOCKET_PARENT))
        .map_err(|_| PlatformError::SocketParentRejected)
}

/// 以 `getpeereid` 读取 Unix stream 对端的 effective uid/gid。
///
/// # Errors
///
/// macOS 无法取得 peer credentials 时返回 [`PlatformError::SocketPeerRejected`]。
pub(crate) fn peer_identity(stream_fd: RawFd) -> Result<EnrolledOwner, PlatformError> {
    let mut peer_user: libc::uid_t = 0;
    let mut peer_group: libc::gid_t = 0;
    // SAFETY: `stream_fd` comes from a live Unix stream, and both raw pointers refer to
    // initialized local storage for the duration of this synchronous libc call.
    let result =
        unsafe { libc::getpeereid(stream_fd, addr_of_mut!(peer_user), addr_of_mut!(peer_group)) };
    if result != 0 {
        return Err(PlatformError::SocketPeerRejected);
    }
    EnrolledOwner::new(uid_from_libc(peer_user), gid_from_libc(peer_group))
        .map_err(|_| PlatformError::SocketPeerRejected)
}

/// 验证 socket client 所见 server peer 是 root。
///
/// # Errors
///
/// `getpeereid` 失败或 server uid 不是 root 时返回 [`PlatformError::SocketPeerRejected`]。
pub(crate) fn verify_root_socket_server(stream_fd: RawFd) -> Result<(), PlatformError> {
    let mut peer_user: libc::uid_t = 0;
    let mut peer_group: libc::gid_t = 0;
    // SAFETY: `stream_fd` comes from a live Unix stream, and both raw pointers refer to
    // initialized local storage for the duration of this synchronous libc call.
    let result =
        unsafe { libc::getpeereid(stream_fd, addr_of_mut!(peer_user), addr_of_mut!(peer_group)) };
    if result != 0 || uid_from_libc(peer_user) != ROOT_UID {
        return Err(PlatformError::SocketPeerRejected);
    }
    let _ = peer_group;
    Ok(())
}

/// 仅删除同一 fixed socket 及其已验证 identity；供 daemon shutdown 或显式 uninstall 使用。
pub(crate) fn remove_socket_if_identity(
    identity: ArtifactIdentity,
    owner: EnrolledOwner,
) -> Result<(), PlatformError> {
    guarded_remove_identity(
        ArtifactPathId::SocketLeaf,
        ArtifactOwner::EnrolledOwner(owner),
        SOCKET_MODE,
        identity,
    )
}

/// socket bind 后设置 enrolled owner uid/gid、0600 并取得已复验的 identity。
///
/// # Errors
///
/// 任何 nofollow ownership、mode 或 identity 重验失败时返回
/// [`PlatformError::SocketSetupFailed`]。
pub(crate) fn seal_bound_socket(owner: EnrolledOwner) -> Result<ArtifactIdentity, PlatformError> {
    let initial =
        stat_artifact(ArtifactPathId::SocketLeaf)?.ok_or(PlatformError::SocketSetupFailed)?;
    validate_identity(
        initial,
        ArtifactOwner::Root,
        ArtifactKind::Socket,
        SOCKET_MODE,
    )
    .map_err(|_| PlatformError::SocketSetupFailed)?;

    let artifact = fixed_artifact(ArtifactPathId::SocketLeaf);
    let parent = open_root_controlled_parent(artifact.parent)
        .map_err(|_| PlatformError::SocketSetupFailed)?;
    let leaf = c_string(artifact.leaf).map_err(|_| PlatformError::SocketSetupFailed)?;
    let uid = libc_uid(owner.uid()).map_err(|_| PlatformError::SocketSetupFailed)?;
    let gid = libc_gid(owner.gid()).map_err(|_| PlatformError::SocketSetupFailed)?;
    // SAFETY: `parent` is an open fixed root-controlled directory; `leaf` is a fixed NUL-free
    // artifact name. `AT_SYMLINK_NOFOLLOW` prevents ownership changes through a symlink.
    let chown_result = unsafe {
        libc::fchownat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            uid,
            gid,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if chown_result != 0 {
        return Err(PlatformError::SocketSetupFailed);
    }
    let current =
        stat_artifact(ArtifactPathId::SocketLeaf)?.ok_or(PlatformError::SocketSetupFailed)?;
    if current.device != initial.device || current.inode != initial.inode {
        return Err(PlatformError::SocketSetupFailed);
    }
    validate_identity(
        current,
        ArtifactOwner::EnrolledOwner(owner),
        ArtifactKind::Socket,
        SOCKET_MODE,
    )
    .map_err(|_| PlatformError::SocketSetupFailed)?;
    Ok(current)
}

/// 仅 socket 启动链持有的原错误，不扩大全局 Copy 平台错误。
#[derive(Debug)]
pub enum SocketStartupError {
    /// 既有平台拒绝，保留原稳定类别。
    Existing(PlatformError),
    /// 本次 socket connect 或 bind 的原 IO。
    Io {
        step: &'static str,
        source: io::Error,
    },
}

impl From<PlatformError> for SocketStartupError {
    fn from(error: PlatformError) -> Self {
        Self::Existing(error)
    }
}

impl std::fmt::Display for SocketStartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Existing(error) => std::fmt::Display::fmt(error, formatter),
            Self::Io { step, source } => write!(
                formatter,
                "{} step={step} kind={:?} errno={:?}: {source}",
                PlatformError::SocketSetupFailed, source.kind(), source.raw_os_error(),
            ),
        }
    }
}

impl std::error::Error for SocketStartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Existing(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// stale control.sock 的回收计划（纯决策，单测钉死分流判据）。
enum StaleSocketPlan {
    /// 身份精确匹配（enrolled uid+gid、socket、0600）：既有安全删除路径
    ///（device/inode 复核 + 父 fd unlinkat）。
    StrictRemoval,
    /// 身份不符（旧代安装/其它 enrollment 的孤儿残留）：孤儿语义回收。
    OrphanRecycle,
}

fn stale_socket_plan(identity: ArtifactIdentity, owner: EnrolledOwner) -> StaleSocketPlan {
    if validate_identity(
        identity,
        ArtifactOwner::EnrolledOwner(owner),
        ArtifactKind::Socket,
        SOCKET_MODE,
    )
    .is_ok()
    {
        StaleSocketPlan::StrictRemoval
    } else {
        StaleSocketPlan::OrphanRecycle
    }
}

/// 若已有安全可回收的 stale socket，才允许在 bind 前回收它；身份不符的孤儿残留
/// 按同一孤儿语义回收（2026-09-20 问题四第一层第 3 点）。
///
/// # Errors
///
/// 精确匹配路径沿用既有拒绝（非 socket、symlink、owner/mode 不符或删除失败）；
/// 孤儿路径拒绝于父目录守卫/叶类型/unlink（见 [`orphan_remove_leaf`]）。绝不删除
/// 受控父目录中固定叶名之外的任何文件。
pub(crate) fn remove_safe_stale_socket(owner: EnrolledOwner) -> Result<(), SocketStartupError> {
    let Some(identity) = stat_artifact(ArtifactPathId::SocketLeaf)? else {
        return Ok(());
    };
    match stale_socket_plan(identity, owner) {
        StaleSocketPlan::StrictRemoval => remove_strict_stale_socket_with_connect(
            || std::os::unix::net::UnixStream::connect(crate::platform::SERVICE_AGENT_SOCKET_PATH),
            || remove_socket_if_identity(identity, owner),
        ),
        // 身份不符的旧语义是直接 Err，失败推迟为 core 侧
        // `DARWIN_CORE_SERVICE_NOT_READY` 二次卡点；改为孤儿语义回收——父目录
        // root 属主且无 022 写位 + 非 symlink + 固定叶名即删除，不再要求残留
        // uid/gid/mode 与当前规格精确相等（预置该残留本身需要 root）。父目录
        // 缺失对运行中的 daemon 不可达（其二进制就在该目录内），无需专门处理。
        StaleSocketPlan::OrphanRecycle => {
            orphan_remove_artifact(ArtifactPathId::SocketLeaf).map_err(Into::into)
        }
    }
}

// 只消费生产原本的一次 connect；删除仍由捕获原 identity 的既有动作执行。
fn remove_strict_stale_socket_with_connect(
    connect: impl FnOnce() -> io::Result<std::os::unix::net::UnixStream>,
    remove: impl FnOnce() -> Result<(), PlatformError>,
) -> Result<(), SocketStartupError> {
    match connect() {
        Ok(_) => Err(PlatformError::ArtifactGuardRejected.into()),
        Err(error) if error.raw_os_error() == Some(libc::ECONNREFUSED) => {
            remove().map_err(Into::into)
        }
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => Ok(()),
        Err(source) => Err(SocketStartupError::Io { step: "stale.connect", source }),
    }
}

/// daemon startup 临界区设置 0177 的 guard；drop 立即恢复先前 umask。
pub(crate) struct UmaskGuard {
    previous: libc::mode_t,
}

impl UmaskGuard {
    /// 在 daemon 单线程 socket bind 临界区启用 0177。
    #[must_use]
    pub(crate) fn restrictive() -> Self {
        // SAFETY: daemon only creates this guard before accepting connections; the previous
        // process umask is retained and restored by `Drop`.
        let previous = unsafe { libc::umask(0o177) };
        Self { previous }
    }
}

impl Drop for UmaskGuard {
    fn drop(&mut self) {
        // SAFETY: this restores exactly the value returned by the paired `umask` call.
        unsafe { libc::umask(self.previous) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    kind: ArtifactKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactKind {
    Regular,
    Socket,
    /// 2026-09-12：历史安装目录（`retire-legacy` 专用）。固定 artifact 永不使用该
    /// 变体——`fixed_artifact` 只返回 `Regular`/`Socket`。
    Directory,
}

#[derive(Clone, Copy)]
struct FixedArtifact {
    parent: &'static str,
    leaf: &'static str,
    kind: ArtifactKind,
    mode: u32,
}

const fn fixed_artifact(artifact: ArtifactPathId) -> FixedArtifact {
    match artifact {
        ArtifactPathId::InstallBinary => FixedArtifact {
            parent: SERVICE_AGENT_INSTALL_DIR,
            leaf: "exv-vpn-darwin-service-agent",
            kind: ArtifactKind::Regular,
            mode: BINARY_MODE,
        },
        ArtifactPathId::InstallDescriptor => FixedArtifact {
            parent: "/Library/LaunchDaemons",
            leaf: "com.exv.vpn.service-agent.plist",
            kind: ArtifactKind::Regular,
            mode: DESCRIPTOR_MODE,
        },
        ArtifactPathId::StateLeaf => FixedArtifact {
            parent: SERVICE_AGENT_INSTALL_DIR,
            leaf: "state.v1",
            kind: ArtifactKind::Regular,
            mode: ROOT_FILE_MODE,
        },
        ArtifactPathId::LogLeaf => FixedArtifact {
            parent: SERVICE_AGENT_INSTALL_DIR,
            leaf: "service.log",
            kind: ArtifactKind::Regular,
            mode: ROOT_FILE_MODE,
        },
        ArtifactPathId::AgentLog => FixedArtifact {
            parent: "/Library/Application Support/EXV/ServiceAgent/logs",
            leaf: "service-agent.log",
            kind: ArtifactKind::Regular,
            mode: 0o644,
        },
        ArtifactPathId::AgentLogFirst => FixedArtifact {
            parent: "/Library/Application Support/EXV/ServiceAgent/logs",
            leaf: "service-agent.log.1",
            kind: ArtifactKind::Regular,
            mode: 0o644,
        },
        ArtifactPathId::AgentLogSecond => FixedArtifact {
            parent: "/Library/Application Support/EXV/ServiceAgent/logs",
            leaf: "service-agent.log.2",
            kind: ArtifactKind::Regular,
            mode: 0o644,
        },
        ArtifactPathId::EngineLog => FixedArtifact {
            parent: "/Library/Application Support/EXV/ServiceAgent/logs",
            leaf: "engine.log",
            kind: ArtifactKind::Regular,
            mode: 0o644,
        },
        ArtifactPathId::EngineLogFirst => FixedArtifact {
            parent: "/Library/Application Support/EXV/ServiceAgent/logs",
            leaf: "engine.log.1",
            kind: ArtifactKind::Regular,
            mode: 0o644,
        },
        ArtifactPathId::EngineLogSecond => FixedArtifact {
            parent: "/Library/Application Support/EXV/ServiceAgent/logs",
            leaf: "engine.log.2",
            kind: ArtifactKind::Regular,
            mode: 0o644,
        },
        ArtifactPathId::SocketLeaf => FixedArtifact {
            parent: SERVICE_AGENT_SOCKET_PARENT,
            leaf: "control.sock",
            kind: ArtifactKind::Socket,
            mode: SOCKET_MODE,
        },
        ArtifactPathId::EngineBinary => FixedArtifact {
            parent: SERVICE_AGENT_INSTALL_DIR,
            leaf: "exv-vpn-darwin-engine",
            kind: ArtifactKind::Regular,
            mode: BINARY_MODE,
        },
        ArtifactPathId::EngineDescriptor => FixedArtifact {
            parent: "/Library/LaunchDaemons",
            leaf: "com.exv.vpn.engine.plist",
            kind: ArtifactKind::Regular,
            mode: DESCRIPTOR_MODE,
        },
        ArtifactPathId::EngineSocketLeaf => FixedArtifact {
            parent: SERVICE_AGENT_SOCKET_PARENT,
            leaf: "engine.sock",
            kind: ArtifactKind::Socket,
            mode: SOCKET_MODE,
        },
    }
}

fn ensure_install_directory() -> Result<(), PlatformError> {
    let install_dir = Path::new(SERVICE_AGENT_INSTALL_DIR);
    if fs::symlink_metadata(install_dir).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(PlatformError::InstallPreparationFailed);
    }
    fs::create_dir_all(install_dir)
        .map_err(|error| install_io_failure("install_directory.create_dir_all", error))?;
    fs::set_permissions(install_dir, fs::Permissions::from_mode(0o755))
        .map_err(|error| install_io_failure("install_directory.set_permissions", error))?;
    lchown_fixed_path(
        SERVICE_AGENT_INSTALL_DIR,
        ROOT_UID,
        ROOT_GID,
        "install_directory.lchown",
    )
    .map_err(|_| PlatformError::InstallPreparationFailed)?;
    Ok(())
}

fn ensure_absent(artifact: ArtifactPathId) -> Result<(), PlatformError> {
    if stat_artifact(artifact)?.is_some() {
        return Err(PlatformError::InstallPreparationFailed);
    }
    Ok(())
}

fn descriptor_contents(owner: EnrolledOwner) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{SERVICE_AGENT_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{SERVICE_AGENT_BINARY_PATH}</string>\n    <string>serve</string>\n    <string>--owner-uid</string>\n    <string>{}</string>\n    <string>--owner-gid</string>\n    <string>{}</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n</dict>\n</plist>\n",
        owner.uid(),
        owner.gid()
    )
}

// 安装命令已有stderr消费者；在原IO仍在手时交原因，main另交稳定失败码。
fn install_io_failure(step: &str, error: io::Error) -> PlatformError {
    eprintln!(
        "install failed step={step} errno={:?} error={error}",
        error.raw_os_error()
    );
    PlatformError::InstallPreparationFailed
}

fn write_new_fixed_file(
    path: &str,
    contents: &[u8],
    mode: u32,
    step_prefix: &'static str,
) -> Result<(), PlatformError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| install_io_failure(&format!("{step_prefix}.create_new"), error))?;
    file.write_all(contents)
        .map_err(|error| install_io_failure(&format!("{step_prefix}.write_all"), error))?;
    file.sync_all()
        .map_err(|error| install_io_failure(&format!("{step_prefix}.sync_all"), error))?;
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| install_io_failure(&format!("{step_prefix}.set_permissions"), error))
}

// ---- 历史/已退役（一次性）组件清理 ----
//
// 三个方法都只接受 `lib.rs` 中 `LEGACY_*` 固定集合内的 leaf/label（当前 4 label /
// 5 descriptor / 4 目录，含 2026-09-20 退役的开发伴侣命名空间），并在实现内**再次**
// 校验（纵深防御）。父目录统一走受控校验（root 所有、mode 无 group/other 写位、
// 非 symlink），删除统一走 `unlinkat`；不引入任何调用方可控的路径或 label。
//
// 2026-09-20：descriptor 回收改走孤儿规则（同 `orphan_remove_leaf`），目录回收把
// 父目录缺失（ENOENT）视为幂等成功，逐件报出 `contract_key` 与维度——旧实现用 `?`
// 串联且不报告，任一件 mode 不符即静默中断整组回收，且干净机（父目录不存在）
// 必然非零退出。

/// 回收历史安装目录与空安装目录共用的父目录（`/Library/Application Support/EXV`，
/// 见 [`INSTALL_ROOT_DIR`]）。
const LEGACY_INSTALL_DIR_PARENT: &str = INSTALL_ROOT_DIR;

/// 只回收**空**目录（`rmdir` 语义）的生产入口：受控属主固定 root。
fn remove_empty_dir_guarded(parent: &str, leaf: &str) -> Result<(), PlatformError> {
    remove_empty_dir_guarded_by(parent, leaf, ROOT_UID)
}

/// 只回收**空**目录（`rmdir` 语义）：父目录受控 + 目标为受控属主常规目录 +
/// device/inode 二次复核；非空（`ENOTEMPTY`/`EEXIST`）即保留现场并返回成功。
///
/// 用于卸载尾段的安装目录回收：`uninstall` 的 artifact 集合只含文件与 socket，
/// 目录若不被回收会长期留下空壳并让 core 的「安装根目录非空」判据误触发。
///
/// 受控属主可注入：生产固定 root；单测注入当前进程 uid（测试不持 root，用临时
/// 目录验证同一规则）。
fn remove_empty_dir_guarded_by(
    parent: &str,
    leaf: &str,
    controlled_uid: u32,
) -> Result<(), PlatformError> {
    remove_empty_dir_with_report(parent, leaf, controlled_uid, |detail| eprintln!("{detail}"))
}

fn remove_empty_dir_with_report(
    parent: &str,
    leaf: &str,
    controlled_uid: u32,
    mut report: impl FnMut(String),
) -> Result<(), PlatformError> {
    match fs::symlink_metadata(parent) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PlatformError::ArtifactGuardRejected),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(PlatformError::ArtifactGuardRejected);
            }
        }
    }
    ensure_directory_controlled_by(Path::new(parent), controlled_uid)?;
    let parent = File::open(parent).map_err(|_| PlatformError::ArtifactGuardRejected)?;
    let name = c_string(leaf)?;
    let Some(identity) = stat_at(parent.as_raw_fd(), &name)? else {
        return Ok(());
    };
    if identity.kind != ArtifactKind::Directory || identity.uid != controlled_uid {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    let current =
        stat_at(parent.as_raw_fd(), &name)?.ok_or(PlatformError::ArtifactGuardRejected)?;
    if current.device != identity.device || current.inode != identity.inode {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    // SAFETY: `parent` is an open controlled directory fd and `name` is a fixed
    // compile-time leaf (`ServiceAgent` / `EXV`); the directory identity was revalidated
    // from that same fd immediately before this rmdir.
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
    if result != 0 {
        let error = io::Error::last_os_error();
        // 非空即保留现场：未知条目/仍有残留时绝不递归删除。
        if matches!(
            error.raw_os_error(),
            Some(libc::ENOTEMPTY) | Some(libc::EEXIST)
        ) {
            report(format!("service-uninstall retained directory={leaf} reason=nonempty"));
            return Ok(());
        }
        return Err(PlatformError::ArtifactRemovalIoFailed { step: "rmdir", errno: error.raw_os_error() });
    }
    Ok(())
}

/// `bootout` 固定历史 label；label 必须先通过白名单校验。
fn run_fixed_launchctl_bootout_label(label: &str) -> Result<(), PlatformError> {
    let target = match label {
        "com.exv.helper" => "system/com.exv.helper",
        "com.exv.dev-service-installer" => "system/com.exv.dev-service-installer",
        "com.exv.helper.dev" => "system/com.exv.helper.dev",
        "com.exv.vpn.dev-companion" => "system/com.exv.vpn.dev-companion",
        _ => return Err(PlatformError::CliUsage),
    };
    // 保原维护命令的stdout/stderr继承，只有当次bootout结果决定返回。
    legacy_bootout_result(
        target,
        Command::new(LAUNCHCTL_PATH)
            .env_clear()
            .args(["bootout", target])
            .status(),
    )
}

fn legacy_bootout_result(
    target: &'static str,
    status: io::Result<std::process::ExitStatus>,
) -> Result<(), PlatformError> {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(PlatformError::ServiceControlCommandFailed {
            action: "bootout",
            target,
            exit_code: status.code(),
            signal: status.signal(),
            errno: None,
        }),
        Err(error) => {
            eprintln!(
                "launchctl bootout target={target} errno={:?} error={error}",
                error.raw_os_error()
            );
            Err(PlatformError::ServiceControlCommandFailed {
                action: "bootout",
                target,
                exit_code: None,
                signal: None,
                errno: error.raw_os_error(),
            })
        }
    }
}

/// 删除固定历史 descriptor（含 `.previous` 残片）。
///
/// 2026-09-20 起与孤儿回收同源：`/Library/LaunchDaemons` 是 root 受控目录（非
/// group/world 可写），非 root 无法在其中预置固定叶名，因此只要求「受控父目录 +
/// 固定叶名 + 非 symlink + device/inode 复核」，不再要求残留 mode/owner 与当前规格
/// 精确相等（旧代残留的 mode 可能与今日规格不同，曾使整组回收静默中断）。
/// 父目录/叶名缺席均为幂等成功，处置与拒绝逐件报告。
fn guarded_remove_legacy_descriptor(leaf: &str) -> Result<(), PlatformError> {
    if !crate::LEGACY_DESCRIPTOR_LEAVES.contains(&leaf) {
        return Err(PlatformError::CliUsage);
    }
    match orphan_remove_leaf("/Library/LaunchDaemons", leaf, ROOT_UID) {
        Ok(OrphanDimension::Absent | OrphanDimension::ParentMissing) => Ok(()),
        Ok(_) => Ok(()),
        Err((error, dimension)) => {
            eprintln!("{}", legacy_guard_report_line(Some(error), leaf, dimension));
            Err(error)
        }
    }
}

/// 递归回收固定历史安装目录；目录必须 root 所有且非 symlink。
///
/// 历史目录内含混合权限的子项（如 `DevHelper/.install-stage` 为 0700），无法用
/// `ArtifactKind::Regular/Directory` 的单一 mode 断言覆盖，因此此处只做
/// **父目录受控 + 目标非 symlink + root 所有** 三重校验，再 `remove_dir_all`。
///
/// 2026-09-20：父目录缺失（干净机从未安装过任何历史组件）视为**无需回收的成功**
/// ——旧实现对 ENOENT 同样以守卫拒绝收场，使整组回收在干净机上确定性中止。
fn remove_legacy_install_dir_guarded(leaf: &str) -> Result<(), PlatformError> {
    if !crate::LEGACY_INSTALL_DIR_LEAVES.contains(&leaf) {
        return Err(PlatformError::CliUsage);
    }
    match fs::symlink_metadata(LEGACY_INSTALL_DIR_PARENT) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(_) => return Err(PlatformError::ArtifactGuardRejected),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(PlatformError::ArtifactGuardRejected);
            }
        }
    }
    ensure_root_controlled_directory(Path::new(LEGACY_INSTALL_DIR_PARENT))?;
    let parent =
        File::open(LEGACY_INSTALL_DIR_PARENT).map_err(|_| PlatformError::ArtifactGuardRejected)?;
    let name = c_string(leaf)?;
    let Some(identity) = stat_at(parent.as_raw_fd(), &name)? else {
        return Ok(());
    };
    if identity.kind != ArtifactKind::Directory || identity.uid != ROOT_UID {
        eprintln!(
            "{}",
            legacy_guard_report_line(
                Some(PlatformError::ArtifactGuardRejected),
                leaf,
                OrphanDimension::LeafKind
            )
        );
        return Err(PlatformError::ArtifactGuardRejected);
    }
    // 递归删除前以 `fstatat` 二次确认身份未变（TOCTOU 收窄到同一父 fd 内）。
    let current =
        stat_at(parent.as_raw_fd(), &name)?.ok_or(PlatformError::ArtifactGuardRejected)?;
    if current.device != identity.device || current.inode != identity.inode {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    // 保留 `parent` fd 直到删除返回（父目录身份不被释放；`remove_dir_all` 仍按路径
    // 遍历，属既有纪律，不因本轮新增分支而放宽）。
    let path = format!("{LEGACY_INSTALL_DIR_PARENT}/{leaf}");
    fs::remove_dir_all(&path).map_err(|_| PlatformError::ArtifactRemovalFailed)?;
    drop(parent);
    Ok(())
}

/// W2.5：service engine job 的固定 plist。
///
/// `RunAtLoad=true`（挂系统启动，不随 core 启动——规范 Q2）+
/// `KeepAlive={SuccessfulExit:false}`（干净退出 exit(0) 不复活，崩溃自愈）；
/// argv 即 engine `--service` 形态的固定契约（owner uid 与固定控制端点）。
fn engine_descriptor_contents(owner: EnrolledOwner) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{ENGINE_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{ENGINE_BINARY_PATH}</string>\n    <string>--service</string>\n    <string>--owner-uid</string>\n    <string>{}</string>\n    <string>--control-socket</string>\n    <string>{ENGINE_SOCKET_PATH}</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n</dict>\n</plist>\n",
        owner.uid()
    )
}

/// 固定 launchctl 命令只按本次子进程结果返回；不通过 print 复判。
fn run_fixed_launchctl_for_engine(action: EngineJobAction) -> Result<(), PlatformError> {
    match action {
        EngineJobAction::EnsureBootstrap => ensure_engine_started_with(|args| {
            Command::new(LAUNCHCTL_PATH).env_clear().args(args).output()
        }),
        EngineJobAction::Bootout => {
            let result = run_launchctl(
                "bootout",
                ENGINE_SERVICE_TARGET,
                &["bootout", ENGINE_SERVICE_TARGET],
            );
            match &result {
                Ok(()) => agent_log(
                    "info",
                    "ENGINE_JOB_BOOTOUT",
                    "engine job bootout finished",
                    &[],
                ),
                Err(error) => agent_log(
                    "error",
                    "ENGINE_JOB_BOOTOUT",
                    "engine job bootout failed",
                    &[("error", error.code().to_owned())],
                ),
            }
            result
        }
    }
}

/// kickstart 不带 -k：启动停止的已注册 job，也不打断已运行的实例。
/// 只有原命令明确报告固定 system job 缺席时才首次注册；不追加 print 复验。
fn ensure_engine_started_with(
    mut run: impl FnMut(&[&str]) -> io::Result<std::process::Output>,
) -> Result<(), PlatformError> {
    ensure_engine_started_detailed_with(&mut run).map_err(|failure| failure.error)
}
fn ensure_engine_started_detailed_with(
    mut run: impl FnMut(&[&str]) -> io::Result<std::process::Output>,
) -> Result<(), crate::platform::ServiceOperationFailure> {
    let output = run(&["kickstart", ENGINE_SERVICE_TARGET]);
    let missing = output.as_ref().is_ok_and(|output| {
        output.status.code() == Some(113)
            && String::from_utf8_lossy(&output.stderr).trim()
                == format!("Could not find service \"{ENGINE_LABEL}\" in domain for system")
    });
    let result = if missing {
        launchctl_result_detailed("bootstrap", ENGINE_PLIST_PATH,
            run(&["bootstrap", LAUNCHCTL_SYSTEM_DOMAIN, ENGINE_PLIST_PATH]))
    } else {
        launchctl_result_detailed("kickstart", ENGINE_SERVICE_TARGET, output)
    };
    // S7：拉起 engine 的关键事件落文件日志（字段=决策布尔+错误类别+原命令
    // exit/signal/errno 数字；launchctl stderr 原文不落日志）。
    match &result {
        Ok(()) => agent_log(
            "info",
            "ENGINE_JOB_ENSURE",
            "engine job ensured",
            &[("bootstrapped", missing.to_string())],
        ),
        Err(failure) => {
            let mut fields = vec![
                ("bootstrapped", missing.to_string()),
                ("error", failure.error.code().to_owned()),
            ];
            if let crate::platform::PlatformError::ServiceControlCommandFailed {
                exit_code,
                signal,
                errno,
                ..
            } = &failure.error
            {
                fields.push(("exit_code", exit_code.map_or_else(|| "none".to_owned(), |code| code.to_string())));
                fields.push(("signal", signal.map_or_else(|| "none".to_owned(), |sig| sig.to_string())));
                fields.push(("errno", errno.map_or_else(|| "none".to_owned(), |code| code.to_string())));
            }
            agent_log(
                "error",
                "ENGINE_JOB_ENSURE",
                "engine job ensure failed",
                &fields,
            );
        }
    }
    result
}

fn launchctl_result(
    action: &'static str,
    target: &'static str,
    output: io::Result<std::process::Output>,
) -> Result<(), PlatformError> {
    launchctl_result_detailed(action, target, output).map_err(|failure| failure.error)
}
fn launchctl_result_detailed(
    action: &'static str, target: &'static str, output: io::Result<std::process::Output>,
) -> Result<(), crate::platform::ServiceOperationFailure> {
    use crate::platform::ServiceOperationFailure;
    use std::os::unix::process::ExitStatusExt;
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output)
            if action == "bootout"
                && matches!(target, SERVICE_AGENT_SERVICE_TARGET | ENGINE_SERVICE_TARGET)
                && output.status.code() == Some(libc::ESRCH)
                && output.status.signal().is_none()
                && String::from_utf8_lossy(&output.stderr).trim()
                    == "Boot-out failed: 3: No such process" =>
        {
            eprintln!(
                "服务启动项已卸载，跳过停止 action={action} target={target} exit=3 signal=None stderr={}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            Ok(())
        }
        Ok(output) => {
            // 详细系统原因沿现有 stderr 交调用方，终结稳定行由 main 输出。
            eprintln!(
                "launchctl {action} target={target} stderr={}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            Err(ServiceOperationFailure::with_reason(PlatformError::ServiceControlCommandFailed {
                action,
                target,
                exit_code: output.status.code(),
                signal: output.status.signal(),
                errno: None,
            }, String::from_utf8_lossy(&output.stderr).trim()))
        }
        Err(error) => Err(ServiceOperationFailure::with_reason(PlatformError::ServiceControlCommandFailed {
            action,
            target,
            exit_code: None,
            signal: None,
            errno: error.raw_os_error(),
        }, &error.to_string())),
    }
}

fn run_launchctl(
    action: &'static str,
    target: &'static str,
    args: &[&str],
) -> Result<(), PlatformError> {
    launchctl_result(
        action,
        target,
        Command::new(LAUNCHCTL_PATH).env_clear().args(args).output(),
    )
}

/// W2.5：best-effort 停机信号——以 owner uid 连接 engine 固定端点后立即关闭。
///
/// 无 tonic 栈、无法发送字面 `StopTunnel` RPC；uid 门连接 + 立即 EOF 即 engine 会话
/// 循环的既有拆隧道触发器（idle：EOF→回 accept；busy：typed 拒绝不 evict——真正
/// 的停机权威是随后的 bootout SIGTERM）。临时 `seteuid(owner)` 通过 engine 的
/// uid 门（root euid 会被拒），随即恢复 root；原操作错误返回给best-effort调用方，不追加探测。
fn nudge_engine_endpoint_eof(owner: EnrolledOwner) -> Result<(), PlatformError> {
    // SAFETY: 仅改变当前CLI effective uid；不新增身份查询。
    if unsafe { libc::seteuid(owner.uid()) } != 0 {
        return Err(PlatformError::ServiceControlIoFailed {
            step: "nudge.seteuid",
            errno: io::Error::last_os_error().raw_os_error(),
            restore_errno: None,
        });
    }
    let connect = std::os::unix::net::UnixStream::connect(ENGINE_SOCKET_PATH);
    // SAFETY: 原动作结束总是尝试恢复入口root身份，立即保存其原errno。
    let restore = if unsafe { libc::seteuid(ROOT_UID) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    };
    let result = nudge_result(connect.map(drop), restore);
    match &result {
        Ok(()) => agent_log(
            "info",
            "ENGINE_NUDGE_EOF",
            "engine endpoint eof nudge delivered",
            &[],
        ),
        Err(error) => agent_log(
            "warn",
            "ENGINE_NUDGE_EOF",
            "engine endpoint eof nudge failed",
            &[("error", error.code().to_owned())],
        ),
    }
    result
}

fn nudge_result(connect: io::Result<()>, restore: io::Result<()>) -> Result<(), PlatformError> {
    let restore_errno = restore.as_ref().err().and_then(io::Error::raw_os_error);
    match connect {
        Err(error) => Err(PlatformError::ServiceControlIoFailed {
            step: "nudge.connect",
            errno: error.raw_os_error(),
            restore_errno,
        }),
        Ok(()) => restore.map_err(|error| PlatformError::ServiceControlIoFailed {
            step: "nudge.restore_uid",
            errno: error.raw_os_error(),
            restore_errno: None,
        }),
    }
}

fn run_fixed_launchctl(action: ServiceControlAction) -> Result<(), PlatformError> {
    match action {
        ServiceControlAction::Bootstrap => run_launchctl(
            "bootstrap",
            SERVICE_AGENT_DESCRIPTOR_PATH,
            &[
                "bootstrap",
                LAUNCHCTL_SYSTEM_DOMAIN,
                SERVICE_AGENT_DESCRIPTOR_PATH,
            ],
        ),
        ServiceControlAction::Bootout => run_launchctl(
            "bootout",
            SERVICE_AGENT_SERVICE_TARGET,
            &["bootout", SERVICE_AGENT_SERVICE_TARGET],
        ),
    }
}

fn guarded_remove_artifact(
    artifact: ArtifactPathId,
    owner: ArtifactOwner,
) -> Result<(), PlatformError> {
    // 文件日志沿创建进程的umask落盘（现有产物0644）；固定目录与叶名复用既有回收规则。
    if artifact.is_runtime_log() {
        return orphan_remove_artifact(artifact);
    }
    let expected_mode = fixed_artifact(artifact).mode;
    let Some(identity) = stat_artifact(artifact)? else {
        return Ok(());
    };
    guarded_remove_identity(artifact, owner, expected_mode, identity)
}

// ---- 2026-09-20：descriptor 缺失分支的孤儿/首装回收（问题四第一层第 2 点） ----
//
// 信任锚是 enrollment descriptor。descriptor 在场时 [`guarded_remove_artifact`]
// 的严格精确匹配守卫（kind/mode/owner 与当前规格完全相等）一字不动；descriptor
// 缺失（干净首装或孤儿残留态）时本组函数接管同一位置的回收：
//   * 父目录缺失（ENOENT）→ 无需回收的成功——干净首装机在此确定性失败是用户
//     实测 `ARTIFACT_GUARD_REJECTED` 的主假设根因（`ensure_root_controlled_directory`
//     对 ENOENT 同样拒绝，六个 remove 的第二件 state 叶的父目录即安装目录，从未
//     安装过的机器必不在场，而它在 `install_binary` 创建目录**之前**执行）；
//   * 父目录在场 → 「root 属主且无 022 写位 + 非 symlink + 固定叶名」三重校验后
//     删除，放弃 mode/owner 精确相等（`/Library/Application Support` 与
//     `/Library/LaunchDaemons` 均非 group/world 可写，非 root 无法预置 artifact 或
//     预建目录；制造孤儿态需先删 descriptor，本身需要 root）；
//   * 沿用严格路径的全部纪律：fstatat NOFOLLOW、device/inode 二次复核、父 fd
//     unlinkat（TOCTOU 收窄到同一父 fd 内）。

/// 单件孤儿回收（生产入口：固定 artifact → 固定父目录/叶名，受控属主恒 root）。
/// 处置与拒绝均向 stderr 报告 `contract_key` 与维度（固定常量，先于 `main` 的裸码
/// 行输出，供 core 的稳定码提取捕获；见 [`orphan_guard_report_line`]）。
fn orphan_remove_artifact(artifact: ArtifactPathId) -> Result<(), PlatformError> {
    let spec = fixed_artifact(artifact);
    match orphan_remove_leaf(spec.parent, spec.leaf, ROOT_UID) {
        // 叶名缺席：无可回收，不产生报告（干净且父目录在场是正常子集）。
        Ok(OrphanDimension::Absent) => Ok(()),
        Ok(_) => Ok(()),
        Err((error, dimension)) => {
            eprintln!(
                "{}",
                orphan_guard_report_line(Some(error), artifact, dimension)
            );
            Err(error)
        }
    }
}

/// 孤儿回收核心（父目录/叶名/受控属主 uid 可注入：生产钉死固定常量与 root；
/// 单测注入临时目录与当前进程 uid——服务代理单测不持 root，绝不触碰 `/Library`）。
///
/// 返回处置维度（[`Ok`]：父目录缺失/叶名缺席/已回收）或错误+不符维度（[`Err`]）。
fn orphan_remove_leaf(
    parent: &str,
    leaf: &str,
    controlled_uid: u32,
) -> Result<OrphanDimension, (PlatformError, OrphanDimension)> {
    // 父目录缺失：无需回收（ENOENT；干净首装的成功分支）。
    match fs::symlink_metadata(parent) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(OrphanDimension::ParentMissing);
        }
        Err(_) => {
            return Err((
                PlatformError::ArtifactGuardRejected,
                OrphanDimension::ParentGuard,
            ));
        }
        Ok(metadata) => {
            // 符号链接父目录：不受控（真实目标未知），拒绝。
            if metadata.file_type().is_symlink() {
                return Err((
                    PlatformError::ArtifactGuardRejected,
                    OrphanDimension::ParentGuard,
                ));
            }
        }
    }
    // 父目录受控校验（目录 + 属主 + 无 022 写位）与打开。
    ensure_directory_controlled_by(Path::new(parent), controlled_uid)
        .map_err(|error| (error, OrphanDimension::ParentGuard))?;
    let parent_file = File::open(parent).map_err(|_| {
        (
            PlatformError::ArtifactGuardRejected,
            OrphanDimension::ParentGuard,
        )
    })?;
    let leaf = c_string(leaf).map_err(|error| (error, OrphanDimension::ParentGuard))?;
    // 固定叶名 fstatat NOFOLLOW：缺席=无可回收；Err 覆盖符号链接与不可 stat。
    let Some(identity) = stat_at(parent_file.as_raw_fd(), &leaf)
        .map_err(|error| (error, OrphanDimension::LeafKind))?
    else {
        return Ok(OrphanDimension::Absent);
    };
    if identity.kind == ArtifactKind::Directory {
        // 目录占位固定叶名：绝不递归、不换 AT_REMOVEDIR——按不符拒绝。
        return Err((
            PlatformError::ArtifactGuardRejected,
            OrphanDimension::LeafKind,
        ));
    }
    // device/inode 复核（TOCTOU 收窄到同一父 fd 内；变更即拒绝）。
    let current = stat_at(parent_file.as_raw_fd(), &leaf)
        .map_err(|error| (error, OrphanDimension::IdentityRace))?
        .ok_or((
            PlatformError::ArtifactGuardRejected,
            OrphanDimension::IdentityRace,
        ))?;
    if current.device != identity.device || current.inode != identity.inode {
        return Err((
            PlatformError::ArtifactGuardRejected,
            OrphanDimension::IdentityRace,
        ));
    }
    // SAFETY: `parent_file` is an open controlled directory and `leaf` is a fixed name. The
    // non-symlink identity was revalidated from that same parent fd immediately before this
    // unlink; residual mode/owner intentionally NOT re-checked (orphan relaxation).
    let result = unsafe { libc::unlinkat(parent_file.as_raw_fd(), leaf.as_ptr(), 0) };
    if result != 0 {
        return Err((
            PlatformError::ArtifactRemovalIoFailed { step: "unlinkat", errno: io::Error::last_os_error().raw_os_error() },
            OrphanDimension::UnlinkFailed,
        ));
    }
    Ok(OrphanDimension::Recycled)
}

fn guarded_remove_identity(
    artifact: ArtifactPathId,
    owner: ArtifactOwner,
    expected_mode: u32,
    identity: ArtifactIdentity,
) -> Result<(), PlatformError> {
    let spec = fixed_artifact(artifact);
    validate_identity(identity, owner, spec.kind, expected_mode)?;
    let parent = open_root_controlled_parent(spec.parent)?;
    let leaf = c_string(spec.leaf)?;
    let current =
        stat_at(parent.as_raw_fd(), &leaf)?.ok_or(PlatformError::ArtifactGuardRejected)?;
    if current.device != identity.device || current.inode != identity.inode {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    validate_identity(current, owner, spec.kind, expected_mode)?;
    // SAFETY: `parent` is an open root-controlled fixed parent and `leaf` is a fixed name. The
    // identity was revalidated from that same parent fd immediately before this unlink.
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), leaf.as_ptr(), 0) };
    if result != 0 {
        return Err(PlatformError::ArtifactRemovalFailed);
    }
    Ok(())
}

fn verify_artifact(
    artifact: ArtifactPathId,
    owner: ArtifactOwner,
    expected_mode: u32,
) -> Result<(), PlatformError> {
    let spec = fixed_artifact(artifact);
    let identity = stat_artifact(artifact)?.ok_or(PlatformError::ArtifactGuardRejected)?;
    validate_identity(identity, owner, spec.kind, expected_mode)
}

fn stat_artifact(artifact: ArtifactPathId) -> Result<Option<ArtifactIdentity>, PlatformError> {
    let spec = fixed_artifact(artifact);
    let parent = open_root_controlled_parent(spec.parent)?;
    let leaf = c_string(spec.leaf)?;
    stat_at(parent.as_raw_fd(), &leaf)
}

fn open_root_controlled_parent(parent: &str) -> Result<File, PlatformError> {
    ensure_root_controlled_directory(Path::new(parent))?;
    File::open(parent).map_err(|_| PlatformError::ArtifactGuardRejected)
}

fn ensure_root_controlled_directory(path: &Path) -> Result<(), PlatformError> {
    ensure_directory_controlled_by(path, ROOT_UID)
}

/// 目录受控校验：目录 + 属主 uid 精确 + 无 group/other 写位。
///
/// 属主 uid 可注入：生产固定 root（[`ensure_root_controlled_directory`]）；孤儿
/// 回收单测注入当前进程 uid（服务代理单测不持 root，用临时目录验证同一规则）。
fn ensure_directory_controlled_by(path: &Path, controlled_uid: u32) -> Result<(), PlatformError> {
    let metadata = fs::metadata(path).map_err(|_| PlatformError::ArtifactGuardRejected)?;
    let mode = metadata.mode() & 0o777;
    if !metadata.is_dir() || metadata.uid() != controlled_uid || mode & 0o022 != 0 {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    Ok(())
}

fn stat_at(parent_fd: RawFd, leaf: &CString) -> Result<Option<ArtifactIdentity>, PlatformError> {
    let mut stat = MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `parent_fd` is an open directory fd, `leaf` has a trailing NUL and no interior NUL,
    // and `stat` is valid uninitialized storage for libc to fill.
    let result = unsafe {
        libc::fstatat(
            parent_fd,
            leaf.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        if io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(PlatformError::ArtifactGuardRejected);
    }
    // SAFETY: a zero return from `fstatat` guarantees libc initialized the output `stat`.
    let stat = unsafe { stat.assume_init() };
    let kind = match stat.st_mode & libc::S_IFMT {
        libc::S_IFREG => ArtifactKind::Regular,
        libc::S_IFSOCK => ArtifactKind::Socket,
        libc::S_IFDIR => ArtifactKind::Directory,
        _ => return Err(PlatformError::ArtifactGuardRejected),
    };
    Ok(Some(ArtifactIdentity {
        device: u64::try_from(stat.st_dev).map_err(|_| PlatformError::ArtifactGuardRejected)?,
        inode: stat.st_ino,
        uid: uid_from_libc(stat.st_uid),
        gid: gid_from_libc(stat.st_gid),
        mode: u32::from(stat.st_mode) & 0o777,
        kind,
    }))
}

fn validate_identity(
    identity: ArtifactIdentity,
    owner: ArtifactOwner,
    kind: ArtifactKind,
    mode: u32,
) -> Result<(), PlatformError> {
    if identity.kind != kind || identity.mode != mode {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    match owner {
        ArtifactOwner::Root => {
            if identity.uid != ROOT_UID || identity.gid != ROOT_GID {
                return Err(PlatformError::ArtifactGuardRejected);
            }
        }
        ArtifactOwner::EnrolledOwner(owner) => {
            if identity.uid != owner.uid() || identity.gid != owner.gid() {
                return Err(PlatformError::ArtifactGuardRejected);
            }
        }
        ArtifactOwner::EnrolledOwnerUid(uid) => {
            if identity.uid != uid {
                return Err(PlatformError::ArtifactGuardRejected);
            }
        }
    }
    Ok(())
}

// ---- 2026-09-12：root 属主 runtime 残留清扫（`sweep-runtime-residue` 动词） ----

/// 读取目录项名字（跳过 `.`/`..`）。
fn read_dir_names(fd: RawFd) -> Result<Vec<CString>, PlatformError> {
    // `fdopendir` 接管 fd，故先 `dup` 一份交给它，原 fd 仍归调用方。
    // SAFETY: fd 是打开目录的有效 fd；dup 失败即返回，不产生悬垂。
    let dup = unsafe { libc::dup(fd) };
    if dup < 0 {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    // SAFETY: dup 是新建的目录 fd；fdopendir 成功即由其接管（之后只能 closedir）。
    let dir = unsafe { libc::fdopendir(dup) };
    if dir.is_null() {
        // SAFETY: fdopendir 失败时 dup 仍归调用方。
        unsafe { libc::close(dup) };
        return Err(PlatformError::ArtifactGuardRejected);
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: dir 是仍在遍历中的目录流；readdir 返回的指针在下次调用前有效。
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            break;
        }
        // SAFETY: d_name 是 NUL 结尾的名字数组，长度由内核保证。
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        if let Ok(owned) = CString::new(bytes) {
            names.push(owned);
        }
    }
    // SAFETY: dir 由 fdopendir 创建，close 一次。
    unsafe { libc::closedir(dir) };
    Ok(names)
}

/// 判定 pid 是否仍存活（`kill(pid, 0)`；`EPERM` 也算存活）。
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill 只探测进程存在性，pid 已校验为正。
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// 打开 `/private/tmp` 并要求它是 root 属主目录（**不**使用 `ensure_root_controlled_directory`：
/// 该函数要求无 group/other 写位，而 `/private/tmp` 是 `drwxrwxrwt` sticky 目录）。
fn open_runtime_residue_parent() -> Result<File, PlatformError> {
    let parent = crate::RUNTIME_RESIDUE_PARENT;
    let mut stat = MaybeUninit::<libc::stat>::zeroed();
    let path = c_string(parent)?;
    // SAFETY: path 是 NUL 结尾常量；AT_SYMLINK_NOFOLLOW 不跟随末组件符号链接。
    if unsafe { libc::lstat(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    // SAFETY: lstat 返回 0 保证 stat 已初始化。
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR || stat.st_uid != ROOT_UID {
        return Err(PlatformError::ArtifactGuardRejected);
    }
    File::open(parent).map_err(|_| PlatformError::ArtifactGuardRejected)
}

impl MacosPlatform {
    /// 清扫 root 属主 runtime 残留。规则见 [`crate::RUNTIME_RESIDUE_*`]。
    ///
    /// 单个目标的问题记入报告，不使整体失败——卸载流程不应因一个残留清不掉而中止。
    fn sweep_runtime_residue_impl() -> Result<crate::platform::ResidueSweepReport, PlatformError> {
        let parent = open_runtime_residue_parent()?;
        let parent_fd = parent.as_raw_fd();
        let mut report = crate::platform::ResidueSweepReport::default();

        for name in read_dir_names(parent_fd)? {
            let Ok(leaf) = name.to_str() else {
                continue;
            };
            if !crate::platform::runtime_residue_name_matches(leaf) {
                continue; // 形状不符：不是我们的产物，绝不动
            }
            let leaf_owned = leaf.to_owned();
            let mut stat = MaybeUninit::<libc::stat>::zeroed();
            // SAFETY: parent_fd 是打开的目录 fd，name NUL 结尾且无内部 NUL。
            if unsafe {
                libc::fstatat(
                    parent_fd,
                    name.as_ptr(),
                    stat.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                report.skipped.push((leaf_owned, "无法读取属性"));
                continue;
            }
            // SAFETY: fstatat 返回 0 保证 stat 已初始化。
            let stat = unsafe { stat.assume_init() };
            if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
                report.skipped.push((leaf_owned, "非目录"));
                continue;
            }
            if stat.st_uid != ROOT_UID {
                report.skipped.push((leaf_owned, "非 root 属主"));
                continue;
            }
            if let Some(pid) = crate::platform::runtime_residue_pid(leaf)
                && process_alive(pid)
            {
                report
                    .skipped
                    .push((leaf_owned, "活会话（创建者进程仍存活）"));
                continue;
            }

            // 打开目标目录 fd（不跟随符号链接）。
            // SAFETY: parent_fd 有效，name 是白名单形状叶子。
            let dir_fd = unsafe {
                libc::openat(
                    parent_fd,
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if dir_fd < 0 {
                report.skipped.push((leaf_owned, "无法打开目录"));
                continue;
            }
            // 只 unlink 已知 leaf；出现未知条目即保留现场。
            let mut unknown = false;
            match read_dir_names(dir_fd) {
                Ok(inner) => {
                    for entry in inner {
                        let Ok(inner_leaf) = entry.to_str() else {
                            unknown = true;
                            continue;
                        };
                        if !crate::RUNTIME_RESIDUE_KNOWN_LEAVES.contains(&inner_leaf) {
                            unknown = true;
                            continue;
                        }
                        // SAFETY: dir_fd 是打开的目录 fd，entry 是其中的名字。
                        let result = unsafe { libc::unlinkat(dir_fd, entry.as_ptr(), 0) };
                        if result != 0
                            && io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT)
                        {
                            unknown = true;
                        }
                    }
                }
                Err(_) => unknown = true,
            }
            // SAFETY: dir_fd 由 openat 打开，closedir 不接管它，故此处 close 一次。
            unsafe { libc::close(dir_fd) };
            if unknown {
                report.skipped.push((leaf_owned, "含未知条目（保留现场）"));
                continue;
            }

            // 不预检空：直接 rmdir，非空即保留（消除 TOCTOU）。
            // SAFETY: parent_fd 有效，name 是白名单形状叶子；AT_REMOVEDIR 仅删空目录。
            let removed = unsafe { libc::unlinkat(parent_fd, name.as_ptr(), libc::AT_REMOVEDIR) };
            if removed == 0 {
                report.removed.push(leaf_owned);
            } else if io::Error::last_os_error().raw_os_error() == Some(libc::ENOTEMPTY) {
                report.skipped.push((leaf_owned, "目录非空（保留现场）"));
            } else {
                report.skipped.push((leaf_owned, "删除失败"));
            }
        }

        Ok(report)
    }
}

fn lchown_fixed_path(
    path: &str,
    uid: u32,
    gid: u32,
    step: &'static str,
) -> Result<(), PlatformError> {
    let path = c_string(path)?;
    let uid = libc_uid(uid)?;
    let gid = libc_gid(gid)?;
    // SAFETY: `path` is a fixed NUL-terminated path; uid/gid use checked platform conversions.
    let result = unsafe { libc::lchown(path.as_ptr(), uid, gid) };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        Err(install_io_failure(step, error))
    }
}

fn c_string(value: &str) -> Result<CString, PlatformError> {
    CString::new(value).map_err(|_| PlatformError::ArtifactGuardRejected)
}

fn uid_from_libc(uid: libc::uid_t) -> u32 {
    // `libc::uid_t` 在 macOS 上就是 `u32`；保留此函数以集中类型收敛点。
    uid
}

fn current_process_uids() -> (u32, u32) {
    let real_uid = uid_from_libc(unsafe { libc::getuid() });
    let effective_uid = uid_from_libc(unsafe { libc::geteuid() });
    (real_uid, effective_uid)
}

fn gid_from_libc(gid: libc::gid_t) -> u32 {
    // `libc::gid_t` 在 macOS 上就是 `u32`；保留此函数以集中类型收敛点。
    gid
}

fn libc_uid(uid: u32) -> Result<libc::uid_t, PlatformError> {
    libc::uid_t::try_from(uid).map_err(|_| PlatformError::ArtifactGuardRejected)
}

fn libc_gid(gid: u32) -> Result<libc::gid_t, PlatformError> {
    libc::gid_t::try_from(gid).map_err(|_| PlatformError::ArtifactGuardRejected)
}

