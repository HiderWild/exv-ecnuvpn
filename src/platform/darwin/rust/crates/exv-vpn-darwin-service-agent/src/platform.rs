//! Darwin 服务代理的固定平台常量、root 入口判定和可替换平台 seam。
//!
//! 本模块不直接执行系统调用。真实 macOS 实现在 [`crate::macos`]；这里的操作顺序和
//! fake 测试先固定 V1 业务契约，避免测试触及 `/Library`、`/var/run` 或系统服务域。
//!
//! install 是替换式（幂等）顺序，对齐 win32 修复路径：已安装（enrollment
//! descriptor 在场）时先验证 enrolled owner、best-effort `bootout`，再按严格守卫
//! guarded remove 旧 artifact 后重装；descriptor 缺失（干净首装或孤儿残留态）时
//! 同一位置的回收走孤儿规则——父目录缺失视为无需回收的成功，父目录受控时放宽
//! 回收（2026-09-20 问题四第一层，规则见 [`OrphanDimension`] 与 trait 方法文档）。
//! owner 解析固定在入口完成
//! （sudo 上下文形态读 `SUDO_UID`/`SUDO_GID`；osascript 提权形态使用显式 flags，忽略
//! `SUDO_*`），编排层只消费已解析的 [`EnrolledOwner`]。

use std::{env, fmt, path::Component, path::Path, str::FromStr};

use crate::{
    ArtifactPathId, SERVICE_AGENT_BINARY, SERVICE_AGENT_CLEANUP_ARTIFACTS,
    SERVICE_AGENT_DESCRIPTOR, SERVICE_AGENT_STATE_LEAF, SERVICE_AGENT_UNINSTALL_ARTIFACTS,
    SERVICE_ENGINE_BINARY, SERVICE_ENGINE_DESCRIPTOR, SERVICE_ENGINE_SOCKET_LEAF, RUNTIME_RESIDUE_PID_HEX_MAX,
};

/// 固定安装目录；没有任何 CLI 参数可以覆盖它。
pub const SERVICE_AGENT_INSTALL_DIR: &str = "/Library/Application Support/EXV/ServiceAgent";
/// 产品安装根目录（各组件安装目录的父目录；卸载尾段只回收**空**目录）。
pub const INSTALL_ROOT_DIR: &str = "/Library/Application Support/EXV";
/// 产品安装根目录的叶名（`/Library/Application Support` 下）。
pub const INSTALL_ROOT_DIR_LEAF: &str = "EXV";
/// 固定安装目录叶名（[`INSTALL_ROOT_DIR`] 下）。
pub const SERVICE_AGENT_INSTALL_DIR_LEAF: &str = "ServiceAgent";
/// 固定安装二进制路径。
pub const SERVICE_AGENT_BINARY_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/exv-vpn-darwin-service-agent";
/// 固定系统描述路径。
pub const SERVICE_AGENT_DESCRIPTOR_PATH: &str =
    "/Library/LaunchDaemons/com.exv.vpn.service-agent.plist";
/// 固定 代理自有 state leaf 路径。
pub const SERVICE_AGENT_STATE_PATH: &str = "/Library/Application Support/EXV/ServiceAgent/state.v1";
/// 固定 代理自有 log leaf 路径。
pub const SERVICE_AGENT_LOG_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/service.log";
/// root 控制的固定 socket parent；安装时明确设为 root-owned 0755。
pub const SERVICE_AGENT_SOCKET_PARENT: &str = SERVICE_AGENT_INSTALL_DIR;
/// 固定控制 socket 路径。
pub const SERVICE_AGENT_SOCKET_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/control.sock";
/// serve 端 Engine 路径的固定回退值：state 叶缺失或内容不合法时使用。
///
/// 回退目标是**已装 Engine 的固定路径**（[`ENGINE_BINARY_PATH`]）——服务代理安装时
/// 自己写下的那一份。刻意不保留任何开发机绝对路径回退：生产组件不得依赖开发检出。
pub const INSTALLED_ENGINE_PATH: &str = ENGINE_BINARY_PATH;
/// CLI `--engine-path` 与固定 state 叶内容允许的 Engine 绝对路径最大字节数。
pub const SERVICE_ENGINE_PATH_MAX_LEN: usize = 1024;
/// 系统服务控制程序的固定绝对路径。
pub const LAUNCHCTL_PATH: &str = "/bin/launchctl";
/// 系统服务域的固定文字。
pub const LAUNCHCTL_SYSTEM_DOMAIN: &str = "system";
/// 固定服务 target；只用于 `bootout`。
pub const SERVICE_AGENT_SERVICE_TARGET: &str = "system/com.exv.vpn.service-agent";

// ---- W2.5：service engine job（常驻 engine）固定常量 ----

/// service engine job 的唯一固定 label。
pub const ENGINE_LABEL: &str = "com.exv.vpn.engine";
/// service engine 安装二进制的固定路径（install 从 `--engine-path` 或
/// `current_exe` 同目录解析源并拷贝入此路径；非 bundle 内路径）。
pub const ENGINE_BINARY_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/exv-vpn-darwin-engine";
/// service engine 固定控制端点（engine `--control-socket` 的生产取值）。
pub const ENGINE_SOCKET_PATH: &str = "/Library/Application Support/EXV/ServiceAgent/engine.sock";
/// service engine job 的固定系统描述路径。
pub const ENGINE_PLIST_PATH: &str = "/Library/LaunchDaemons/com.exv.vpn.engine.plist";
/// service engine job 的固定服务 target；只用于 `bootout`。
pub const ENGINE_SERVICE_TARGET: &str = "system/com.exv.vpn.engine";
/// service engine 安装源解析时的固定二进制名（开发检出与 bundle 布局一致：
/// `target/debug/` 与 `Contents/MacOS/` 均为平铺四件套：tauri/core/engine/服务代理）。
pub const ENGINE_BINARY_NAME: &str = "exv-vpn-darwin-engine";

const ROOT_UID: u32 = 0;

/// 受信开发者身份；只能由 root 安装入口的 `SUDO_UID`/`SUDO_GID` 环境事实构造。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnrolledOwner {
    uid: u32,
    gid: u32,
}

impl EnrolledOwner {
    /// 构造一个非 root 开发者身份。
    ///
    /// # Errors
    ///
    /// root uid 不能成为服务代理控制 socket 的 enrolled owner。
    pub const fn new(uid: u32, gid: u32) -> Result<Self, PlatformError> {
        if uid == ROOT_UID {
            return Err(PlatformError::OwnerIdentityMalformed);
        }
        Ok(Self { uid, gid })
    }

    /// enrolled 开发者 uid。
    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// enrolled 开发者 gid。
    #[must_use]
    pub const fn gid(self) -> u32 {
        self.gid
    }
}

/// 从显式 root 入口的进程身份和 `SUDO_UID`/`SUDO_GID` 解析 enrolled 开发者。
///
/// 直接 root、非 root 或缺失环境变量均拒绝；这条契约避免 install/uninstall 从 CLI 参数、
/// socket frame 或可配置文件读取 owner identity。
///
/// # Errors
///
/// 进程不是 real/effective root、环境变量缺失或其值不是非 root uid 与有效 gid 时返回稳定
/// [`PlatformError`]。
pub fn enrolled_owner_from_sudo_context(
    real_uid: u32,
    effective_uid: u32,
    sudo_user: Option<&str>,
    sudo_group: Option<&str>,
) -> Result<EnrolledOwner, PlatformError> {
    if real_uid != ROOT_UID || effective_uid != ROOT_UID {
        return Err(PlatformError::RootRequired);
    }
    let uid = parse_sudo_value(sudo_user)?;
    let gid = parse_sudo_value(sudo_group)?;
    EnrolledOwner::new(uid, gid)
}

fn parse_sudo_value(value: Option<&str>) -> Result<u32, PlatformError> {
    value
        .ok_or(PlatformError::SudoIdentityRequired)
        .and_then(|raw| u32::from_str(raw).map_err(|_| PlatformError::OwnerIdentityMalformed))
}

/// 仅要求 real/effective uid 均为 root 的最小门卫；不读取任何 `SUDO_*` 事实。
///
/// 供 osascript 提权形态（显式 owner install/uninstall 与裸 `start`）复用：这些入口在
/// root shell 中没有 `SUDO_UID`/`SUDO_GID`，也不需要它们。
///
/// # Errors
///
/// real 或 effective uid 不是 root 时返回 [`PlatformError::RootRequired`]。
pub fn require_process_root(real_uid: u32, effective_uid: u32) -> Result<(), PlatformError> {
    if real_uid != ROOT_UID || effective_uid != ROOT_UID {
        return Err(PlatformError::RootRequired);
    }
    Ok(())
}

/// 接受 CLI 显式 flags 给出的 enrolled owner。
///
/// 与 [`enrolled_owner_from_sudo_context`] 的唯一区别是完全不读取 `SUDO_UID`/
/// `SUDO_GID`：同一 root 进程内两条解析路径互不影响，显式形态始终返回 flags 给出的
/// owner（即共存时显式 owner 优先，`SUDO_*` 被整体忽略）。
///
/// # Errors
///
/// 进程不是 real/effective root 时返回 [`PlatformError::RootRequired`]；owner 数字本身的
/// root/范围校验已在 CLI 解析层完成。
pub fn enrolled_owner_from_explicit_owner(
    real_uid: u32,
    effective_uid: u32,
    owner: EnrolledOwner,
) -> Result<EnrolledOwner, PlatformError> {
    require_process_root(real_uid, effective_uid)?;
    Ok(owner)
}

/// Engine 绝对路径的纯词法校验：以 `/` 开头、长度不超过 [`SERVICE_ENGINE_PATH_MAX_LEN`]、
/// 路径组件不含 `..`。文件系统事实（常规文件、非 symlink）由真实实现的
/// [`crate::macos::engine_path_is_valid`] 补充。
#[must_use]
pub fn engine_path_lexically_valid(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= SERVICE_ENGINE_PATH_MAX_LEN
        && !Path::new(path)
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// 固定 artifact 的预期 owner 类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactOwner {
    /// descriptor、binary、state 与 log 均由 root 持有。
    Root,
    /// 控制 socket 只允许 enrolled 开发者 uid/gid 持有。
    EnrolledOwner(EnrolledOwner),
    /// W2.5 service engine 固定控制端点只按 enrolled 开发者 **uid** 匹配
    ///（engine `--service` 形态无 gid 参数：socket 属主 uid=owner、gid 保持
    /// 创建进程 egid（生产 root=0），与 control.sock 的 uid+gid 双匹配不同）。
    EnrolledOwnerUid(u32),
}

/// 孤儿回收（descriptor 缺失分支）的处置/不符维度：无路径无秘密的固定词，
/// 经 [`orphan_guard_report_line`] 进 stderr 诊断报告（2026-09-20 问题四第一层）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrphanDimension {
    /// 固定 artifact 的父目录缺失（ENOENT）：无需回收（干净首装机常态）。
    ParentMissing,
    /// 残留身份与当前规格不符，但父目录通过受控校验，已按放宽规则回收。
    Recycled,
    /// 固定叶名处无残留：无可回收（不产生 stderr 报告）。
    Absent,
    /// 父目录不满足「属主受控且无 022 写位且非符号链接」三重校验。
    ParentGuard,
    /// 固定叶名处是符号链接或目录等意外类型：拒绝删除。
    LeafKind,
    /// unlink 前 device/inode 复核失败（TOCTOU 竞争）：拒绝删除。
    IdentityRace,
    /// 受控父 fd 上的 `unlinkat` 失败。
    UnlinkFailed,
}

impl OrphanDimension {
    /// 无秘密稳定维度词（stderr 报告用；固定常量，不得拼接任何路径或身份）。
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::ParentMissing => "parent-missing",
            Self::Recycled => "orphan-recycled",
            Self::Absent => "absent",
            Self::ParentGuard => "parent-guard",
            Self::LeafKind => "leaf-kind",
            Self::IdentityRace => "identity-race",
            Self::UnlinkFailed => "unlink-failed",
        }
    }
}

/// 孤儿回收守卫事件的 stderr 单行文本（固定常量拼接：稳定码/动词 + `contract_key` +
/// 维度；无路径、无秘密、无调用方输入）。
///
/// 拒绝行以 `DARWIN_SERVICE_AGENT_*` 稳定码开头，且先于 `main` 的裸码行输出——
/// core 侧 `elevation_failure_detail` 取「首个稳定码 + 同行有界剩余」时正好捕获
/// 完整的 `contract_key` 与维度。处置行（父目录缺失/已回收）刻意**不带**稳定码
/// 前缀，绝不抢占失败码提取。
#[must_use]
pub fn orphan_guard_report_line(
    rejection: Option<PlatformError>,
    artifact: ArtifactPathId,
    dimension: OrphanDimension,
) -> String {
    match rejection {
        Some(error) => format!(
            "{} artifact={} dimension={}",
            error.code(),
            artifact.contract_key(),
            dimension.word()
        ),
        None => format!(
            "exv-service-agent orphan-guard artifact={} dimension={}",
            artifact.contract_key(),
            dimension.word()
        ),
    }
}

/// 历史/退役组件回收守卫事件的 stderr 单行文本（与 [`orphan_guard_report_line`] 同形）。
///
/// `leaf` 只能取自 `LEGACY_*` 编译期白名单（不是调用方输入），因此可与稳定码同行
/// 报出以便定位是哪一件残留被拒/被回收；旧实现用 `?` 串联且不报告，任一件不符即
/// 静默中断整组回收。
#[must_use]
pub fn legacy_guard_report_line(
    rejection: Option<PlatformError>,
    leaf: &str,
    dimension: OrphanDimension,
) -> String {
    match rejection {
        Some(error) => format!(
            "{} artifact=legacy:{} dimension={}",
            error.code(),
            leaf,
            dimension.word()
        ),
        None => format!(
            "exv-service-agent legacy-guard artifact=legacy:{} dimension={}",
            leaf,
            dimension.word()
        ),
    }
}

/// 固定系统服务控制动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceControlAction {
    /// 唯一合法序列：`/bin/launchctl bootstrap system <固定 descriptor>`。
    Bootstrap,
    /// 唯一合法序列：`/bin/launchctl bootout system/com.exv.vpn.service-agent`。
    Bootout,
}

/// W2.5 service engine job 的固定控制动作（与 daemon 的
/// [`ServiceControlAction`] 并列；target 固定为 engine job）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineJobAction {
    /// 幂等 bootstrap：`launchctl bootstrap system <engine plist>`；job 已加载
    /// （bootstrap 失败但 `launchctl print system/<label>` 成功）视成功。
    EnsureBootstrap,
    /// `launchctl bootout system/com.exv.vpn.engine`（调用方决定失败语义）。
    Bootout,
}
/// 无泄密的稳定平台错误类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformError {
    /// real/effective uid 不是 root。
    RootRequired,
    /// 直接 root 或缺失 `SUDO_UID`/`SUDO_GID`。
    SudoIdentityRequired,
    /// `SUDO_UID`/`SUDO_GID` 不能解析或 uid 为 root。
    OwnerIdentityMalformed,
    /// 固定 install directory、binary、descriptor 或 state leaf 无法安全准备。
    InstallPreparationFailed,
    /// 固定 descriptor 与 enrolled owner 不匹配。
    EnrolledOwnerMismatch,
    /// 固定服务控制操作失败。
    ServiceControlFailed,
    /// 固定 artifact 的 nofollow/type/owner/identity guard 拒绝删除。
    ArtifactGuardRejected,
    /// 固定 artifact 的删除失败。
    ArtifactRemovalFailed,
    /// root-controlled socket parent 不符合 V1。
    SocketParentRejected,
    /// 固定 socket 无法建立或重验。
    SocketSetupFailed,
    /// UDS peer credential 不符合 enrolled owner 或 root server。
    SocketPeerRejected,
    /// 固定帧 I/O、EOF 或超时不符合 V1。
    SocketProtocolFailed,
    /// daemon socket 不允许 install/uninstall。
    DaemonActionRejected,
    /// 固定 cleanup action 失败；不暴露底层路径或系统错误。
    CleanupFailed,
    /// 固定 Engine 进程无法启动。
    EngineLaunchFailed,
    /// CLI 不是 V1 固定字面量与参数数量。
    CliUsage,
    /// 仅普通用户可通过 socket 发起 status/cleanup。
    OrdinaryUserRequired,
}

impl PlatformError {
    /// 无秘密、无路径的稳定错误码。
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::RootRequired => "DARWIN_SERVICE_AGENT_ROOT_REQUIRED",
            Self::SudoIdentityRequired => "DARWIN_SERVICE_AGENT_SUDO_IDENTITY_REQUIRED",
            Self::OwnerIdentityMalformed => "DARWIN_SERVICE_AGENT_SUDO_IDENTITY_MALFORMED",
            Self::InstallPreparationFailed => "DARWIN_SERVICE_AGENT_INSTALL_PREPARATION_FAILED",
            Self::EnrolledOwnerMismatch => "DARWIN_SERVICE_AGENT_ENROLLED_OWNER_MISMATCH",
            Self::ServiceControlFailed => "DARWIN_SERVICE_AGENT_SERVICE_CONTROL_FAILED",
            Self::ArtifactGuardRejected => "DARWIN_SERVICE_AGENT_ARTIFACT_GUARD_REJECTED",
            Self::ArtifactRemovalFailed => "DARWIN_SERVICE_AGENT_ARTIFACT_REMOVAL_FAILED",
            Self::SocketParentRejected => "DARWIN_SERVICE_AGENT_SOCKET_PARENT_REJECTED",
            Self::SocketSetupFailed => "DARWIN_SERVICE_AGENT_SOCKET_SETUP_FAILED",
            Self::SocketPeerRejected => "DARWIN_SERVICE_AGENT_SOCKET_PEER_REJECTED",
            Self::SocketProtocolFailed => "DARWIN_SERVICE_AGENT_SOCKET_PROTOCOL_FAILED",
            Self::DaemonActionRejected => "DARWIN_SERVICE_AGENT_DAEMON_ACTION_REJECTED",
            Self::CleanupFailed => "DARWIN_SERVICE_AGENT_CLEANUP_FAILED",
            Self::EngineLaunchFailed => "DARWIN_SERVICE_AGENT_ENGINE_LAUNCH_FAILED",
            Self::CliUsage => "DARWIN_SERVICE_AGENT_CLI_USAGE",
            Self::OrdinaryUserRequired => "DARWIN_SERVICE_AGENT_ORDINARY_USER_REQUIRED",
        }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for PlatformError {}

/// 可替换的 fixed-path 平台 seam。
///
/// 除 `install_state` 的已校验 Engine 绝对路径（V1 唯一被批准的自由文本输入）外，所有
/// 方法都没有 path、label、command 或 argv 参数。真实实现只能映射 V1 常量；测试 fake
/// 仅记录调用顺序，绝不触及系统位置。
pub trait PrivilegedPlatform {
    /// 探测固定 descriptor 是否已存在；替换式 install 据此决定验证/bootout 路径。
    ///
    /// # Errors
    ///
    /// 固定 artifact parent guard 或 stat 失败时返回稳定 [`PlatformError`]。
    fn install_descriptor_present(&mut self) -> Result<bool, PlatformError>;

    /// 准备固定安装目录并复制当前进程二进制（sudo 或 osascript 提权调用）。
    ///
    /// # Errors
    ///
    /// 固定安装目录、源二进制或目标 artifact 无法安全准备时返回稳定 [`PlatformError`]。
    fn install_binary(&mut self) -> Result<(), PlatformError>;

    /// 写入唯一固定 descriptor，内含受信 enrolled owner uid/gid。
    ///
    /// # Errors
    ///
    /// 固定 descriptor 无法安全写入或重验失败时返回稳定 [`PlatformError`]。
    fn install_descriptor(&mut self, owner: EnrolledOwner) -> Result<(), PlatformError>;

    /// 将已校验 Engine 绝对路径原文写入唯一固定 state 叶（root 0600，无换行）。
    ///
    /// # Errors
    ///
    /// 路径复验失败或固定 state 叶无法安全写入时返回稳定 [`PlatformError`]。
    fn install_state(&mut self, engine_path: &str) -> Result<(), PlatformError>;

    /// 验证现有固定 descriptor 仍绑定同一 enrolled owner。
    ///
    /// # Errors
    ///
    /// descriptor 缺失、内容或 owner 不匹配时返回 [`PlatformError::EnrolledOwnerMismatch`]。
    fn verify_enrolled_owner(&mut self, owner: EnrolledOwner) -> Result<(), PlatformError>;

    /// 仅执行两个固定服务控制序列之一。
    ///
    /// # Errors
    ///
    /// 固定 `launchctl` 序列执行失败时返回 [`PlatformError::ServiceControlFailed`]。
    fn control_service(&mut self, action: ServiceControlAction) -> Result<(), PlatformError>;

    /// W2.5：把已解析源的 service engine 二进制拷贝入固定安装路径
    /// （root:root 0755，复用 [`Self::install_binary`] 的拷贝纪律）。
    ///
    /// # Errors
    ///
    /// 固定安装目录、源二进制或目标 artifact 无法安全准备时返回稳定
    /// [`PlatformError`]。
    fn install_engine_binary(&mut self, source: &str) -> Result<(), PlatformError>;

    /// W2.5：写入 service engine job 的唯一固定 descriptor（root 0644）。
    ///
    /// # Errors
    ///
    /// 固定 descriptor 无法安全写入或重验失败时返回稳定 [`PlatformError`]。
    fn install_engine_descriptor(&mut self, owner: EnrolledOwner) -> Result<(), PlatformError>;

    /// W2.5：对 service engine job 执行固定控制动作。
    ///
    /// # Errors
    ///
    /// 固定 `launchctl` 序列执行失败时返回 [`PlatformError::ServiceControlFailed`]；
    /// `EnsureBootstrap` 对已加载 job 幂等成功。
    fn control_engine_job(&mut self, action: EngineJobAction) -> Result<(), PlatformError>;

    /// W2.5：best-effort 停机信号——以 owner uid 连接 engine 固定端点后立即 EOF
    ///（无 tonic 栈，无法发送字面 `StopTunnel` RPC；uid 门连接+EOF 即既有会话循环
    /// 的拆隧道触发器。失败一律忽略——真正的停机权威是随后的 bootout SIGTERM）。
    ///
    /// # Errors
    ///
    /// 本 best-effort 动作恒返回 `Ok`；底层失败由实现内部吞掉。
    fn stop_engine_tunnel_best_effort(
        &mut self,
        owner: EnrolledOwner,
    ) -> Result<(), PlatformError>;

    /// 对唯一固定 artifact 执行 nofollow/type/owner/identity guarded removal。
    ///
    /// # Errors
    ///
    /// 固定 artifact 的 guard 拒绝或删除失败时返回稳定 [`PlatformError`]。
    fn remove_artifact(
        &mut self,
        artifact: ArtifactPathId,
        owner: ArtifactOwner,
    ) -> Result<(), PlatformError>;

    /// descriptor 缺失分支的孤儿/首装回收（2026-09-20 问题四第一层）。
    ///
    /// 与 [`Self::remove_artifact`] 的严格精确匹配守卫并列，供替换式 install 在
    /// **enrollment descriptor 不在场**（信任锚缺失）时使用：
    ///
    /// - 固定 artifact 的父目录缺失（ENOENT）→ 无需回收的成功（干净首装机
    ///   确定性失败的修复点——安装目录由随后的 install 步骤创建）；
    /// - 父目录在场 → 「属主受控（root）且无 022 写位 + 非 symlink + 固定叶名」
    ///   三重校验后删除，**不再要求残留 mode/owner 与当前规格精确相等**（接受
    ///   旧代/孤儿残留；非 root 无法在 `/Library` 层级预置 artifact 或预建目录，
    ///   制造该状态需先删 descriptor——本身需要 root）；
    /// - 沿用既有纪律：`fstatat` NOFOLLOW、device/inode 复核、父 fd `unlinkat`；
    /// - 被拒/被回收的 artifact 以 `contract_key` 与维度先行报告 stderr（固定常量）。
    ///
    /// descriptor 在场的替换式重装仍走 [`Self::remove_artifact`] 严格语义
    /// （防劫持不回退）。
    ///
    /// # Errors
    ///
    /// 父目录守卫拒绝、固定叶名处为符号链接/目录、device/inode 复核失败或
    /// `unlinkat` 失败时返回稳定 [`PlatformError`]。
    fn remove_orphan_artifact(&mut self, artifact: ArtifactPathId) -> Result<(), PlatformError>;

    // ---- 2026-09-12 一次性历史清理（`retire-legacy` 动词专用）----
    //
    // 三个方法的入参都必须在 [`crate::LEGACY_*`] 固定集合内，由 [`retire_legacy_with`]
    // 先行校验、实现内再校验一次（纵深防御）。**不允许任意 label 或任意 leaf**。

    /// `bootout` 固定历史 label（`system/<label>`）。
    ///
    /// # Errors
    ///
    /// label 不在 [`crate::LEGACY_LAUNCHD_LABELS`] 内，或 `bootout` 失败时返回稳定错误。
    fn bootout_legacy_label(&mut self, label: &str) -> Result<(), PlatformError>;

    /// 删除固定历史 descriptor（`/Library/LaunchDaemons` 下）。
    ///
    /// # Errors
    ///
    /// leaf 不在 [`crate::LEGACY_DESCRIPTOR_LEAVES`] 内，或 guarded removal 失败时返回稳定错误。
    fn remove_legacy_descriptor(&mut self, leaf: &str) -> Result<(), PlatformError>;

    /// 递归回收固定历史安装目录（`/Library/Application Support/EXV` 下）。
    ///
    /// # Errors
    ///
    /// leaf 不在 [`crate::LEGACY_INSTALL_DIR_LEAVES`] 内，或回收失败时返回稳定错误。
    fn remove_legacy_install_dir(&mut self, leaf: &str) -> Result<(), PlatformError>;

    /// 清扫 `/private/tmp` 下**名字匹配固定形状**的 root 属主 runtime 残留。
    ///
    /// 实现必须自行执行 `crate::RUNTIME_RESIDUE_*` 的全部过滤规则（形状 / 属主 / 活会话 /
    /// 非空保留）；单个目标的失败记入报告而非返回错误。
    ///
    /// # Errors
    ///
    /// 无法枚举或无法安全打开父目录时返回稳定错误。
    fn sweep_runtime_residue(&mut self) -> Result<ResidueSweepReport, PlatformError>;

    /// 卸载尾段：回收**空的**固定安装目录（安装目录本身与其父目录
    /// `/Library/Application Support/EXV`）。
    ///
    /// 语义（2026-09-20）：`uninstall` 的 artifact 集合只含文件与 socket，目录本身
    /// 从不被回收——残留的空目录会让 core 的「安装根目录非空」触发判据长期为真，
    /// 每次「卸载服务」都白弹一次管理员密码。本方法只 `rmdir` **空**目录：目录非空
    /// （未知条目/仍有残留）即**保留现场**并返回成功，绝不递归删除、不触碰其它名字。
    ///
    /// # Errors
    ///
    /// 父目录不受控（非 root 属主 / 有 group・other 写位 / symlink）或目标不是
    /// 常规受控目录时返回稳定 [`PlatformError`]。
    fn remove_empty_install_dirs(&mut self) -> Result<(), PlatformError>;
}

/// 解析 service engine 安装源（W2.5）：`--engine-path` 给出则用之（CLI 已校验），
/// 否则回退 `current_exe` 同目录的固定 Engine 二进制名（开发检出与 bundle 布局一致）。
///
/// # Errors
///
/// 两个来源都不可用时返回 [`PlatformError::InstallPreparationFailed`]——service
/// engine 是 install 契约的一部分（安装即拉起），源缺失即安装失败。
pub fn resolve_engine_install_source(engine_path: Option<&str>) -> Result<String, PlatformError> {
    resolve_engine_source_at(env::current_exe().ok().as_deref(), engine_path)
}

/// [`resolve_engine_install_source`] 的纯函数变体（exe 路径可注入，单测钉死解析
/// 规则；生产经 `current_exe`）。
fn resolve_engine_source_at(
    exe: Option<&Path>,
    engine_path: Option<&str>,
) -> Result<String, PlatformError> {
    if let Some(path) = engine_path {
        return Ok(path.to_owned());
    }
    let sibling = exe
        .and_then(|exe| exe.parent().map(|dir| dir.join(ENGINE_BINARY_NAME)))
        .filter(|path| path.is_file());
    sibling
        .map(|path| path.to_string_lossy().into_owned())
        .ok_or(PlatformError::InstallPreparationFailed)
}

/// 替换式（幂等）安装固定服务代理（W2.5 起含 service engine job）。
///
/// owner 由入口解析后传入（sudo 上下文形态读 `SUDO_UID`/`SUDO_GID`；osascript 提权形态
/// 使用 CLI 显式 flags）。顺序契约：
///
/// 1. 探测固定 daemon descriptor 是否已存在；存在则先 `verify_enrolled_owner`（不同用户
///    安装返回 [`PlatformError::EnrolledOwnerMismatch`]，防止劫持他人 daemon），随后
///    best-effort `bootout`（失败忽略——可能本就未 bootstrap）；全新安装跳过这两步；
/// 2. best-effort `bootout` engine job（替换运行中的 service engine；失败忽略——
///    可能本就未装）；
/// 3. 回收六件均可缺席的 Root-owned artifact（daemon descriptor、state、daemon
///    binary 与 engine descriptor、engine binary、engine socket）——按第 1 步的
///    descriptor 探测结果分流：**在场**走 `remove_artifact` 严格精确匹配守卫
///    （替换式重装语义，防劫持不回退）；**缺失**（信任锚不在场：干净首装或孤儿
///    残留态）走 `remove_orphan_artifact` 孤儿回收——父目录缺失视为无需回收的
///    成功，父目录受控时放宽回收残留（规则见 trait 方法文档；stderr 报告
///    `contract_key` 与维度）。两分支顺序一致；
/// 4. `install_binary`（daemon）→ `install_descriptor`（daemon plist）；
/// 5. 给出 `engine_path` 时写入固定 state 叶（root 0600，会话形态 Engine 路径——
///    过渡兼容保留，E8 删除随 W3 收口）；
/// 6. `install_engine_binary`（源=`--engine-path` 或 `current_exe` 同目录解析）→
///    `install_engine_descriptor`（engine plist：`RunAtLoad` + `KeepAlive`
///    `{SuccessfulExit:false}`）；
/// 7. `bootstrap` daemon → 幂等 `EnsureBootstrap` engine job（安装即拉起，Q2）。
///
/// # Errors
///
/// 替换路径 owner 不匹配返回 [`PlatformError::EnrolledOwnerMismatch`]；其余固定 install
/// artifact、孤儿回收守卫或固定 `bootstrap` 失败时返回稳定 [`PlatformError`]。失败时不
/// 猜测或清理未知系统状态。
pub fn install_with<P>(
    platform: &mut P,
    owner: EnrolledOwner,
    engine_path: Option<&str>,
) -> Result<(), PlatformError>
where
    P: PrivilegedPlatform,
{
    // 0. 先 best-effort 停掉 engine job：退役回收会递归删除 `DevCompanion/`，而旧
    //    engine 进程可能正从该目录运行（macOS 允许删除运行中的二进制，但先停更干净，
    //    也避免 launchd 在窗口内反复重启一个已被删除的程序）。engine 作业 label 新旧
    //    一致，bootout 按 target 串而不是 plist，故即使 plist 已被回收也能停。
    let _ = platform.control_engine_job(EngineJobAction::Bootout);
    let descriptor_present = platform.install_descriptor_present()?;
    if descriptor_present {
        platform.verify_enrolled_owner(owner)?;
        // best-effort：可能本就未 bootstrap；失败不阻断替换式重装。
        let _ = platform.control_service(ServiceControlAction::Bootout);
    }
    // 1. 一次性退役回收（2026-09-20，幂等）：停止并回收已退役组件——开发伴侣 daemon
    //    （`com.exv.vpn.dev-companion`）、它的 plist 与 `DevCompanion/` 安装目录，以及
    //    更早的 C++ 时代残留。目标缺席即成功；失败不静默（原样上抛）。安装新命名空间
    //    之前完成，确保退役组件不会以 root 常驻继续存活。
    retire_legacy_with(platform)?;
    if descriptor_present {
        // 替换式重装：严格精确匹配守卫（语义与 2026-09-20 前完全一致，不回退）。
        platform.remove_artifact(SERVICE_AGENT_DESCRIPTOR, ArtifactOwner::Root)?;
        platform.remove_artifact(SERVICE_AGENT_STATE_LEAF, ArtifactOwner::Root)?;
        platform.remove_artifact(SERVICE_AGENT_BINARY, ArtifactOwner::Root)?;
        platform.remove_artifact(SERVICE_ENGINE_DESCRIPTOR, ArtifactOwner::Root)?;
        platform.remove_artifact(SERVICE_ENGINE_BINARY, ArtifactOwner::Root)?;
        platform.remove_artifact(
            SERVICE_ENGINE_SOCKET_LEAF,
            ArtifactOwner::EnrolledOwnerUid(owner.uid()),
        )?;
    } else {
        // 孤儿/首装回收（descriptor 缺失）：父目录缺失=成功；父目录受控时放宽回收
        // 残留。顺序与严格分支一致；处置/拒绝经 stderr 报告维度。
        platform.remove_orphan_artifact(SERVICE_AGENT_DESCRIPTOR)?;
        platform.remove_orphan_artifact(SERVICE_AGENT_STATE_LEAF)?;
        platform.remove_orphan_artifact(SERVICE_AGENT_BINARY)?;
        platform.remove_orphan_artifact(SERVICE_ENGINE_DESCRIPTOR)?;
        platform.remove_orphan_artifact(SERVICE_ENGINE_BINARY)?;
        platform.remove_orphan_artifact(SERVICE_ENGINE_SOCKET_LEAF)?;
    }
    platform.install_binary()?;
    platform.install_descriptor(owner)?;
    if let Some(engine_path) = engine_path {
        platform.install_state(engine_path)?;
    }
    let engine_source = resolve_engine_install_source(engine_path)?;
    platform.install_engine_binary(&engine_source)?;
    platform.install_engine_descriptor(owner)?;
    platform.control_service(ServiceControlAction::Bootstrap)?;
    // 安装即拉起（Q2）：幂等确保 engine job 在跑。
    platform.control_engine_job(EngineJobAction::EnsureBootstrap)
}

/// 卸载固定服务代理（W2.5 起含 service engine 三件）。
///
/// 顺序契约：验证 owner（**descriptor 不在场时改走孤儿规则**，不再以守卫拒绝中止
/// ——旧实现使「descriptor 缺失 + 残留在场」的机器确定性无法自清；owner 校验在任何
/// 删除之前）→ best-effort 停机信号（engine 端点 uid 门连接+EOF）→ `bootout` engine
/// job（best-effort：旧安装可能无 engine job；先停旧引擎再回收退役命名空间，回收会
/// 删除旧引擎的安装目录）→ 一次性退役回收（幂等）→ `bootout` daemon（失败直接返回，
/// 绝不删除安装文件——防半卸载）→ guarded remove 全部固定 artifact（两份描述、
/// state/log、两份二进制、两个 socket）→ 回收空的固定安装目录（**只 rmdir 空目录**；
/// 非空即保留现场）。
///
/// # Errors
///
/// descriptor owner 不匹配、`bootout`（daemon）或任意 guarded removal 失败时返回稳定
/// [`PlatformError`]。
pub fn uninstall_with<P>(platform: &mut P, owner: EnrolledOwner) -> Result<(), PlatformError>
where
    P: PrivilegedPlatform,
{
    let descriptor_present = platform.install_descriptor_present()?;
    if descriptor_present {
        platform.verify_enrolled_owner(owner)?;
    }
    // best-effort：连不上端点/被拒一律忽略（停机权威是下面的 bootout SIGTERM）。
    let _ = platform.stop_engine_tunnel_best_effort(owner);
    // best-effort：旧安装（W2.5 前）没有 engine job；bootout 失败不阻断。先停 engine
    // 再回收退役命名空间（回收会删除旧 engine 正在使用的安装目录）。
    let _ = platform.control_engine_job(EngineJobAction::Bootout);
    // 退役组件回收：与 install 同源，使「卸载服务」在老机器上也能清掉旧命名空间。
    retire_legacy_with(platform)?;
    platform.control_service(ServiceControlAction::Bootout)?;

    for artifact in SERVICE_AGENT_UNINSTALL_ARTIFACTS {
        if descriptor_present {
            platform.remove_artifact(artifact, expected_owner(artifact, owner))?;
        } else {
            // descriptor 缺失（信任锚不在场）：孤儿规则——父目录受控 + 固定叶名 +
            // 非 symlink + device/inode 复核后回收；父目录缺失=无需回收的成功。
            platform.remove_orphan_artifact(artifact)?;
        }
    }
    platform.remove_empty_install_dirs()
}

/// 一次性回收历史/已退役组件遗留（2026-09-12 起用于预卸载；2026-09-20 起同时被
/// install/uninstall 首段调用以完成开发伴侣退役）。
///
/// 这些产物来自 C++ 时代与早期 Rust 形态，**当前产品代码无任何引用**；它们不在
/// [`ArtifactPathId`] 契约内（那组是"当前服务真实会产生"的集合），因此单列一组
/// 带退役日期的固定常量。本函数只允许操作 [`crate::LEGACY_LAUNCHD_LABELS`]、
/// [`crate::LEGACY_DESCRIPTOR_LEAVES`]、[`crate::LEGACY_INSTALL_DIR_LEAVES`] 三个白名单，
/// **不接受任何调用方提供的 label 或路径**。
///
/// 顺序契约：先 `bootout` 全部固定 label（否则 launchd 会因 `RunAtLoad`/`KeepAlive`
/// 持续尝试拉起已删二进制）→ 再删 descriptor（含 `.previous` 残片）→ 最后递归回收安装
/// 目录（父目录缺失视为无需回收的成功；逐件向 stderr 报告 `contract_key` 与维度）。
/// 任一步的"目标本就不存在"视为成功（幂等）；guard 拒绝或删除失败原样上抛。
///
/// # Errors
///
/// 任一固定 label 的 `bootout`、任一 descriptor 的 guarded removal 或任一安装目录的
/// 回收失败时返回稳定 [`PlatformError`]。
pub fn retire_legacy_with<P>(platform: &mut P) -> Result<(), PlatformError>
where
    P: PrivilegedPlatform,
{
    for label in crate::LEGACY_LAUNCHD_LABELS {
        platform.bootout_legacy_label(label)?;
    }
    for leaf in crate::LEGACY_DESCRIPTOR_LEAVES {
        platform.remove_legacy_descriptor(leaf)?;
    }
    for leaf in crate::LEGACY_INSTALL_DIR_LEAVES {
        platform.remove_legacy_install_dir(leaf)?;
    }
    Ok(())
}

/// 判定 `exv-vpn-<pid:x>-<16 hex>` 形状（与壳层 `kernel/uninstall.rs::is_runtime_dir_name` 同源）。
///
/// **pid 下界是 1 位而非 4 位**：`create_runtime_dir_name` 用 `{:x}` 格式化 pid，pid 小时
/// 只有 3 位十六进制；实测残留中 `exv-vpn-5e6-…`（pid 1510）与 `exv-vpn-641-…`（pid 1601）
/// 用 4 位下界会**静默漏删**。
#[must_use]
pub fn runtime_residue_name_matches(leaf: &str) -> bool {
    let Some(rest) = leaf.strip_prefix("exv-vpn-") else {
        return false;
    };
    let Some((pid_hex, suffix)) = rest.split_once('-') else {
        return false;
    };
    let is_lower_hex = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    (1..=RUNTIME_RESIDUE_PID_HEX_MAX).contains(&pid_hex.len())
        && suffix.len() == 16
        && is_lower_hex(pid_hex)
        && is_lower_hex(suffix)
}

/// 从 runtime 目录名反解创建者 pid（形状不符返回 `None`）。
#[must_use]
pub fn runtime_residue_pid(leaf: &str) -> Option<u32> {
    if !runtime_residue_name_matches(leaf) {
        return None;
    }
    let rest = leaf.strip_prefix("exv-vpn-")?;
    let (pid_hex, _) = rest.split_once('-')?;
    u32::from_str_radix(pid_hex, 16).ok().filter(|pid| *pid > 0)
}

/// 一次 runtime 残留清扫的结果（逐项，便于上层如实报告）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResidueSweepReport {
    /// 已回收的目录名（按发现顺序）。
    pub removed: Vec<String>,
    /// 保留的目录名 + 可读原因（活会话 / 非空 / 属主不符 / 形状不符）。
    pub skipped: Vec<(String, &'static str)>,
}

/// 清扫 root 属主 runtime 残留（`sweep-runtime-residue` 动词）。
///
/// 安全边界（与壳层同源的规则，见 `crate::RUNTIME_RESIDUE_*` 常量）：
/// 只处理 `/private/tmp` 下**名字匹配固定形状**的目录；跳过**活会话**（名字里的 pid 仍存活——
/// 实测 `exv-vpn-133b9-…` 正被运行的 root engine 持有）、**非 root 属主**与非**目录**目标；
/// 目录内只 `unlink` [`crate::RUNTIME_RESIDUE_KNOWN_LEAVES`] 列出的已知 leaf，
/// 之后 `rmdir`；**非空即保留并报告**（不动未知条目）。绝不递归删除、绝不触碰白名单外的名字。
///
/// # Errors
///
/// 平台实现无法枚举或无法安全打开父目录时返回稳定 [`PlatformError`]。单个目标的失败不
/// 使整体失败——记入 [`ResidueSweepReport::skipped`]。
pub fn sweep_runtime_residue_with<P>(platform: &mut P) -> Result<ResidueSweepReport, PlatformError>
where
    P: PrivilegedPlatform,
{
    platform.sweep_runtime_residue()
}

/// 以最小系统操作 ensure-running：先 best-effort `bootout`（可能本就未 bootstrap），再
/// `bootstrap`。
///
/// 这是 core 经 osascript 提权调用的唯一「按需启动」入口：core 探测到「已安装但 daemon
/// 不可用」时使用；对健康 daemon 等价于重启（KeepAlive 场景下极少触发）。无 owner 语义，
/// 与 launchctl 同级；入口仅要求 real/effective root。W2.5 起扩展为同时幂等确保
/// service engine job 已 bootstrap（`start` 动词契约）。
///
/// # Errors
///
/// 固定 `bootstrap` 失败时返回 [`PlatformError::ServiceControlFailed`]；`bootout` 失败被
/// 忽略。
pub fn start_with<P>(platform: &mut P) -> Result<(), PlatformError>
where
    P: PrivilegedPlatform,
{
    let _ = platform.control_service(ServiceControlAction::Bootout);
    platform.control_service(ServiceControlAction::Bootstrap)?;
    platform.control_engine_job(EngineJobAction::EnsureBootstrap)
}

/// 仅清理正常维护允许的 state/log leaf；控制 socket 不在此路径中。
///
/// # Errors
///
/// 任一 guarded removal 失败时返回稳定 [`PlatformError`]，不会额外移除 socket、binary 或
/// descriptor。
pub fn cleanup_with<P>(platform: &mut P) -> Result<(), PlatformError>
where
    P: PrivilegedPlatform,
{
    for artifact in SERVICE_AGENT_CLEANUP_ARTIFACTS {
        platform
            .remove_artifact(artifact, ArtifactOwner::Root)
            .map_err(|_| PlatformError::CleanupFailed)?;
    }
    Ok(())
}

const fn expected_owner(artifact: ArtifactPathId, owner: EnrolledOwner) -> ArtifactOwner {
    match artifact {
        ArtifactPathId::SocketLeaf => ArtifactOwner::EnrolledOwner(owner),
        ArtifactPathId::EngineSocketLeaf => ArtifactOwner::EnrolledOwnerUid(owner.uid()),
        _ => ArtifactOwner::Root,
    }
}
