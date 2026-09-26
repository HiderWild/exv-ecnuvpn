//! Darwin 服务代理 V1 的无副作用 policy 与 wire 契约。
//!
//! 这个独立产品组件只定义固定动作、固定 代理自有 artifact 身份、固定长度帧和
//! 调用者前置条件。它不创建进程、不安装系统对象、不删除文件，也不接受 path、label 或
//! program 输入（唯一例外见下文 engine-path 固定例外）。后续受控实现只能消费这里已验证
//! 的 [`ServiceAgentRequestV1`] 与 [`ExecutionIdentity`]，不能在边界重新引入自由输入。
//!
//! ## osascript 提权与显式 owner 形态
//!
//! 无特权 core 进程经 `osascript ... with administrator privileges` 提权调用本服务代理 CLI
//! 时，root shell 没有 `SUDO_UID`/`SUDO_GID`。因此 CLI 另有 `install/uninstall
//! --owner-uid N --owner-gid N` 显式 owner 形态与裸 `start` 动词：它们完全忽略
//! `SUDO_*` 环境，只要求 real/effective root。其信任边界是 macOS 系统密码弹窗——用户
//! 授权的是整条 payload 文本，owner 数字与（如给出的）Engine 路径都在该文本内显式可见，
//! 不存在 sudo 那样的环境注入通道。裸 `install`/`uninstall` 保留为终端 sudo 上下文
//! 形态，语义不变。
//!
//! ## engine-path 固定例外
//!
//! `--engine-path` 是 V1 唯一被批准的自由文本输入：必须以 `/` 开头、长度不超过 1024
//! 字节、路径组件不含 `..`、经文件系统验证为常规文件且非 symlink；不合法一律按
//! CLI 白名单拒绝。校验通过后路径原文写入 root 0600 的固定 state 叶（无换行）；daemon
//! serve 端读取该叶并执行同样校验，缺失或不合法即回退 `platform::INSTALLED_ENGINE_PATH`
//! （本代理安装时写下的已装 Engine 路径——生产组件不含任何开发检出路径回退）。除
//! 此之外，任何路径、label 或 program 输入仍然被拒绝。

use std::{convert::TryFrom, fmt};

pub mod cli;
pub mod macos;
pub mod platform;
pub mod socket;

/// 服务代理的唯一固定 label。
pub const SERVICE_AGENT_LABEL: &str = "com.exv.vpn.service-agent";

const REQUEST_MAGIC: [u8; 4] = *b"EXVA";
const RESPONSE_MAGIC: [u8; 4] = *b"EXVR";
const FRAME_VERSION: u8 = 1;
const ROOT_UID: u32 = 0;

/// V1 request frame 的固定字节数。
///
/// 布局为 `magic(4) || version(1) || action(1) || reserved(2) || owner_uid(4) || reserved(4)`，
/// 整数均为大端。V1 没有 path、label、program、参数或秘密字段。
pub const SERVICE_AGENT_REQUEST_V1_LEN: usize = 16;
/// V1 response frame 的固定字节数。
///
/// 布局为 `magic(4) || version(1) || action(1) || outcome(1) || reserved(1) || code(4) ||
/// reserved(4)`，整数均为大端。错误只编码稳定类别，绝不携带输入、路径或身份材料。
pub const SERVICE_AGENT_RESPONSE_V1_LEN: usize = 16;
/// 启动固定 Engine 的第二段请求长度。首段仍是 16-byte V1 action frame。
pub const SERVICE_AGENT_ENGINE_LAUNCH_V1_LEN: usize = 512;

/// V1 中唯一允许的逻辑 artifact identity。
///
/// 这里刻意不包含真实系统路径。后续实现必须把每个 identity 映射到经批准的固定路径，
/// 并只能处理该 identity 指向的单个 代理自有 leaf；不能接受调用者给出的路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactPathId {
    /// 固定安装二进制 identity。
    InstallBinary,
    /// 固定安装描述 identity。
    InstallDescriptor,
    /// 代理自有 state leaf。
    StateLeaf,
    /// 代理自有 log leaf。
    LogLeaf,
    /// 代理自有 socket leaf。
    SocketLeaf,
    /// W2.5 service engine 安装二进制 identity。
    EngineBinary,
    /// W2.5 service engine job 描述 identity。
    EngineDescriptor,
    /// W2.5 service engine 固定控制端点 identity。
    EngineSocketLeaf,
}

impl ArtifactPathId {
    /// 返回稳定、不含真实文件系统路径的 contract key。
    #[must_use]
    pub const fn contract_key(self) -> &'static str {
        match self {
            Self::InstallBinary => "install-binary",
            Self::InstallDescriptor => "install-descriptor",
            Self::StateLeaf => "state-leaf",
            Self::LogLeaf => "log-leaf",
            Self::SocketLeaf => "socket-leaf",
            Self::EngineBinary => "engine-binary",
            Self::EngineDescriptor => "engine-descriptor",
            Self::EngineSocketLeaf => "engine-socket-leaf",
        }
    }
}

/// 固定安装二进制 identity。
pub const SERVICE_AGENT_BINARY: ArtifactPathId = ArtifactPathId::InstallBinary;
/// 固定安装描述 identity。
pub const SERVICE_AGENT_DESCRIPTOR: ArtifactPathId = ArtifactPathId::InstallDescriptor;
/// 固定 state leaf identity。
pub const SERVICE_AGENT_STATE_LEAF: ArtifactPathId = ArtifactPathId::StateLeaf;
/// 固定 log leaf identity。
pub const SERVICE_AGENT_LOG_LEAF: ArtifactPathId = ArtifactPathId::LogLeaf;
/// 固定 socket leaf identity。
pub const SERVICE_AGENT_SOCKET_LEAF: ArtifactPathId = ArtifactPathId::SocketLeaf;
/// W2.5 service engine 安装二进制 identity。
pub const SERVICE_ENGINE_BINARY: ArtifactPathId = ArtifactPathId::EngineBinary;
/// W2.5 service engine job 描述 identity。
pub const SERVICE_ENGINE_DESCRIPTOR: ArtifactPathId = ArtifactPathId::EngineDescriptor;
/// W2.5 service engine 固定控制端点 identity。
pub const SERVICE_ENGINE_SOCKET_LEAF: ArtifactPathId = ArtifactPathId::EngineSocketLeaf;

/// 正常 `cleanup` 所允许的全部 代理自有 leaf。
///
/// 该动作故意不包含 [`SERVICE_AGENT_SOCKET_LEAF`]：正常维护必须保留 root 服务代理 的
/// 控制 socket。顺序也是稳定契约的一部分。
pub const SERVICE_AGENT_CLEANUP_ARTIFACTS: [ArtifactPathId; 2] =
    [SERVICE_AGENT_STATE_LEAF, SERVICE_AGENT_LOG_LEAF];

/// `uninstall` 所允许的全部 代理自有 artifact。
///
/// 顺序固定为先撤销两份安装描述（daemon 与 W2.5 service engine job——不留可重新
/// 激活的描述），再清理 state/log，再回收两份安装二进制，最后回收两个控制 socket
///（daemon control.sock 与 engine engine.sock）。root executor 因此能在终止协调
/// 完成前保留控制面。
pub const SERVICE_AGENT_UNINSTALL_ARTIFACTS: [ArtifactPathId; 8] = [
    SERVICE_AGENT_DESCRIPTOR,
    SERVICE_ENGINE_DESCRIPTOR,
    SERVICE_AGENT_STATE_LEAF,
    SERVICE_AGENT_LOG_LEAF,
    SERVICE_AGENT_BINARY,
    SERVICE_ENGINE_BINARY,
    SERVICE_AGENT_SOCKET_LEAF,
    SERVICE_ENGINE_SOCKET_LEAF,
];


/// 历史 `LaunchDaemon` 的固定 label（仅用于 bootout；不允许任意 label 输入）。
///
/// 末项 `com.exv.vpn.dev-companion` 是 2026-09-20 退役的开发伴侣 daemon label。
pub const LEGACY_LAUNCHD_LABELS: [&str; 4] = [
    "com.exv.helper",
    "com.exv.dev-service-installer",
    "com.exv.helper.dev",
    "com.exv.vpn.dev-companion",
];

/// 历史 descriptor 叶名（`/Library/LaunchDaemons` 下；含一份 `.previous` 残片）。
///
/// 末项是 2026-09-20 退役的开发伴侣 daemon plist。
pub const LEGACY_DESCRIPTOR_LEAVES: [&str; 5] = [
    "com.exv.helper.plist",
    "com.exv.dev-service-installer.plist",
    "com.exv.helper.dev.plist",
    "com.exv.helper.dev.plist.previous",
    "com.exv.vpn.dev-companion.plist",
];

/// 历史安装目录叶名（`/Library/Application Support/EXV` 下，整目录递归回收）。
///
/// 末项 `DevCompanion` 是 2026-09-20 退役的开发伴侣安装目录（内含其二进制、
/// engine 副本、state 叶与两个 socket）。
pub const LEGACY_INSTALL_DIR_LEAVES: [&str; 4] = ["Helper", "DevServiceInstaller", "DevHelper", "DevCompanion"];

// ---- 2026-09-12：root 属主 runtime 残留的清扫规则 ----
//
// `/private/tmp/exv-vpn-<pid:x>-<16 hex>` 由 Core/Engine 在连接期创建；**正常退出会自行
// 回收**（`ipc::path` 的 `cleanup_sealed_engine_socket_and_runtime`），残留的真实成因是
// **SIGKILL / 崩溃**——进程内无法自清，只能由预卸载的提权段扫掉。
//
// 为什么写在服务代理（而不是壳层）：壳层不得出现受限字面量，且服务代理是唯一
// 有 root 执行权的固定动词入口。本组规则与壳层 `kernel/uninstall.rs` 的形状校验
// **同源**（两处刻意各自实现：壳层卸载必须在本组件缺失或不可用时仍能工作，
// 故不依赖本 crate；改动需同步两边）。

/// runtime 目录的固定父目录。
pub const RUNTIME_RESIDUE_PARENT: &str = "/private/tmp";

/// runtime 目录名允许的最大 pid hex 位数（`{:x}` 格式化的 pid；下界 1 位）。
pub const RUNTIME_RESIDUE_PID_HEX_MAX: usize = 8;

/// runtime 目录内**允许被 unlink 的已知 leaf**；其余条目一律不动（未知条目即保留现场）。
pub const RUNTIME_RESIDUE_KNOWN_LEAVES: [&str; 3] =
    ["engine.sock", "engine.ticket", "control.sock"];

/// 唯一允许传给服务代理的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceAgentAction {
    /// 只读查询固定服务代理状态。
    Status = 1,
    /// 安装固定服务代理对象；必须由 root executor 执行。
    Install = 2,
    /// 卸载固定服务代理对象；必须由 root executor 执行。
    Uninstall = 3,
    /// 清理固定 代理自有 leaf；必须由 root executor 执行。
    Cleanup = 4,
    /// W2.5 root daemon 幂等确保 service engine job 已 bootstrap（`launchctl
    /// bootstrap system <engine plist>`，已加载视成功）；零载荷 action frame。
    EnsureEngine = 6,
}

impl ServiceAgentAction {
    /// 将严格的文字动作转为 V1 枚举。
    ///
    /// # Errors
    ///
    /// 除 `status`、`install`、`uninstall`、`cleanup` 外的任何文字都返回稳定
    /// [`ServiceAgentError::UnknownAction`]，且不会保留调用者输入。
    pub fn parse_literal(value: &str) -> Result<Self, ServiceAgentError> {
        match value {
            "status" => Ok(Self::Status),
            "install" => Ok(Self::Install),
            "uninstall" => Ok(Self::Uninstall),
            "cleanup" => Ok(Self::Cleanup),
            _ => Err(ServiceAgentError::UnknownAction),
        }
    }

    /// 此动作是否会改变固定 代理自有 对象。
    #[must_use]
    pub const fn requires_root(self) -> bool {
        !matches!(self, Self::Status)
    }
}

impl TryFrom<u8> for ServiceAgentAction {
    type Error = ServiceAgentError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Status),
            2 => Ok(Self::Install),
            3 => Ok(Self::Uninstall),
            4 => Ok(Self::Cleanup),
            6 => Ok(Self::EnsureEngine),
            _ => Err(ServiceAgentError::UnknownAction),
        }
    }
}

/// 无泄密的稳定本地错误类别。
///
/// 各 variant 不保存 wire、路径、label、UID 或任何认证材料。未来 executor 可把
/// [`Self::code`] 映射到固定 response，而不会把外部输入回显给调用者。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAgentError {
    /// request 或 response frame 长度不是 V1 固定值。
    FrameLength,
    /// frame magic 不是 V1 固定值。
    FrameMagic,
    /// frame version 不是 V1。
    FrameVersion,
    /// 任何保留字节非零。
    ReservedField,
    /// action tag 不在固定白名单内。
    UnknownAction,
    /// owner uid 为 root 或不符合 V1 owner 范围。
    InvalidOwnerIdentity,
    /// OS/trusted boundary 给出的 owner identity 不等于 frame owner uid。
    OwnerIdentityMismatch,
    /// 可变动作没有 root executor 前置条件。
    RootRequired,
    /// response outcome/code 组合不符合 V1。
    ResponseMalformed,
    /// daemon socket 不允许 install 或 uninstall。
    DaemonActionRejected,
    /// daemon 的固定 state/log cleanup 失败。
    CleanupFailed,
    /// root daemon 无法启动固定开发 Engine。
    EngineLaunchFailed,
    /// W2.5 root daemon 无法幂等确保 service engine job。
    EnsureEngineFailed,
}

impl ServiceAgentError {
    /// 不含秘密或调用者输入的稳定错误码。
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::FrameLength => "DARWIN_SERVICE_AGENT_FRAME_LENGTH",
            Self::FrameMagic => "DARWIN_SERVICE_AGENT_FRAME_MAGIC",
            Self::FrameVersion => "DARWIN_SERVICE_AGENT_FRAME_VERSION",
            Self::ReservedField => "DARWIN_SERVICE_AGENT_RESERVED_FIELD",
            Self::UnknownAction => "DARWIN_SERVICE_AGENT_UNKNOWN_ACTION",
            Self::InvalidOwnerIdentity => "DARWIN_SERVICE_AGENT_INVALID_OWNER_IDENTITY",
            Self::OwnerIdentityMismatch => "DARWIN_SERVICE_AGENT_OWNER_IDENTITY_MISMATCH",
            Self::RootRequired => "DARWIN_SERVICE_AGENT_ROOT_REQUIRED",
            Self::ResponseMalformed => "DARWIN_SERVICE_AGENT_RESPONSE_MALFORMED",
            Self::DaemonActionRejected => "DARWIN_SERVICE_AGENT_DAEMON_ACTION_REJECTED",
            Self::CleanupFailed => "DARWIN_SERVICE_AGENT_CLEANUP_FAILED",
            Self::EngineLaunchFailed => "DARWIN_SERVICE_AGENT_ENGINE_LAUNCH_FAILED",
            Self::EnsureEngineFailed => "DARWIN_SERVICE_AGENT_ENSURE_ENGINE_FAILED",
        }
    }

    const fn wire_code(self) -> u32 {
        match self {
            Self::FrameLength => 1,
            Self::FrameMagic => 2,
            Self::FrameVersion => 3,
            Self::ReservedField => 4,
            Self::UnknownAction => 5,
            Self::InvalidOwnerIdentity => 6,
            Self::OwnerIdentityMismatch => 7,
            Self::RootRequired => 8,
            Self::ResponseMalformed => 9,
            Self::DaemonActionRejected => 10,
            Self::CleanupFailed => 11,
            Self::EngineLaunchFailed => 12,
            Self::EnsureEngineFailed => 13,
        }
    }

    const fn from_wire_code(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::FrameLength),
            2 => Some(Self::FrameMagic),
            3 => Some(Self::FrameVersion),
            4 => Some(Self::ReservedField),
            5 => Some(Self::UnknownAction),
            6 => Some(Self::InvalidOwnerIdentity),
            7 => Some(Self::OwnerIdentityMismatch),
            8 => Some(Self::RootRequired),
            9 => Some(Self::ResponseMalformed),
            10 => Some(Self::DaemonActionRejected),
            11 => Some(Self::CleanupFailed),
            12 => Some(Self::EngineLaunchFailed),
            13 => Some(Self::EnsureEngineFailed),
            _ => None,
        }
    }
}

impl fmt::Display for ServiceAgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ServiceAgentError {}

/// V1 的无自由字段 request。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceAgentRequestV1 {
    action: ServiceAgentAction,
    owner_uid: u32,
}

impl ServiceAgentRequestV1 {
    /// 构造仅包含固定 action 与 owner identity 的 V1 request。
    ///
    /// # Errors
    ///
    /// root uid 不能作为 V1 owner，避免 root executor 把 root 自身误认作发起开发者。
    pub const fn new(
        action: ServiceAgentAction,
        owner_uid: u32,
    ) -> Result<Self, ServiceAgentError> {
        if owner_uid == ROOT_UID {
            return Err(ServiceAgentError::InvalidOwnerIdentity);
        }
        Ok(Self { action, owner_uid })
    }

    /// request 的固定动作。
    #[must_use]
    pub const fn action(self) -> ServiceAgentAction {
        self.action
    }

    /// request 的 declared owner uid；未来 executor 必须同 OS/trusted identity 比对。
    #[must_use]
    pub const fn owner_uid(self) -> u32 {
        self.owner_uid
    }

    /// 编码为精确 V1 request frame。
    #[must_use]
    pub const fn encode(self) -> [u8; SERVICE_AGENT_REQUEST_V1_LEN] {
        let mut bytes = [0_u8; SERVICE_AGENT_REQUEST_V1_LEN];
        bytes[0] = REQUEST_MAGIC[0];
        bytes[1] = REQUEST_MAGIC[1];
        bytes[2] = REQUEST_MAGIC[2];
        bytes[3] = REQUEST_MAGIC[3];
        bytes[4] = FRAME_VERSION;
        bytes[5] = self.action as u8;
        let owner = self.owner_uid.to_be_bytes();
        bytes[8] = owner[0];
        bytes[9] = owner[1];
        bytes[10] = owner[2];
        bytes[11] = owner[3];
        bytes
    }

    /// 解码严格的 V1 request frame。
    ///
    /// # Errors
    ///
    /// 长度、magic、version、action、reserved 或 owner 字段任一不符合时，返回不含
    /// 输入内容的稳定 [`ServiceAgentError`]。
    pub fn decode(bytes: &[u8]) -> Result<Self, ServiceAgentError> {
        if bytes.len() != SERVICE_AGENT_REQUEST_V1_LEN {
            return Err(ServiceAgentError::FrameLength);
        }
        if bytes[0..4] != REQUEST_MAGIC {
            return Err(ServiceAgentError::FrameMagic);
        }
        if bytes[4] != FRAME_VERSION {
            return Err(ServiceAgentError::FrameVersion);
        }
        if bytes[6] != 0
            || bytes[7] != 0
            || bytes[12] != 0
            || bytes[13] != 0
            || bytes[14] != 0
            || bytes[15] != 0
        {
            return Err(ServiceAgentError::ReservedField);
        }

        let action = ServiceAgentAction::try_from(bytes[5])?;
        let owner_uid = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        Self::new(action, owner_uid)
    }
}

/// 经 OS 或已验证上游边界得出的调用进程与原始 owner identity。
///
/// 本类型不来自 wire。未来 root executor 必须从已验证的 peer credential 或已验证的
/// elevation handoff 构造它；不得把 [`ServiceAgentRequestV1::owner_uid`] 当作身份事实。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionIdentity {
    effective_uid: u32,
    verified_owner_uid: u32,
}

impl ExecutionIdentity {
    /// 创建已由 OS/trusted boundary 验证的 identity。
    ///
    /// 非 root caller 的 verified owner 必须等于 effective uid；root caller 可以代表
    /// 一个已验证的非 root owner。此函数不读取系统状态，调用者负责提供真实身份事实。
    #[must_use]
    pub const fn from_verified_os_identity(effective_uid: u32, verified_owner_uid: u32) -> Self {
        Self {
            effective_uid,
            verified_owner_uid,
        }
    }

    /// 该调用是否满足 owner identity 的本地不变量。
    #[must_use]
    pub const fn is_well_formed(self) -> bool {
        self.verified_owner_uid != ROOT_UID
            && (self.effective_uid == ROOT_UID || self.effective_uid == self.verified_owner_uid)
    }

    /// 已验证的开发 owner uid。
    #[must_use]
    pub const fn verified_owner_uid(self) -> u32 {
        self.verified_owner_uid
    }

    /// effective uid 是否为 root。
    #[must_use]
    pub const fn is_root(self) -> bool {
        self.effective_uid == ROOT_UID
    }
}

/// 对无副作用 request 执行固定权限与 owner identity 判断。
///
/// `status` 只需要 owner identity 匹配；`install`、`uninstall` 与 `cleanup` 额外要求
/// root executor。任何拒绝都不触发文件、服务或进程副作用。
///
/// # Errors
///
/// identity 不可信、不匹配，或可变动作不是由 root executor 执行时返回稳定
/// [`ServiceAgentError`]。
pub const fn authorize_request(
    request: ServiceAgentRequestV1,
    identity: ExecutionIdentity,
) -> Result<(), ServiceAgentError> {
    if !identity.is_well_formed() {
        return Err(ServiceAgentError::InvalidOwnerIdentity);
    }
    if request.owner_uid() != identity.verified_owner_uid() {
        return Err(ServiceAgentError::OwnerIdentityMismatch);
    }
    if request.action().requires_root() && !identity.is_root() {
        return Err(ServiceAgentError::RootRequired);
    }
    Ok(())
}

/// 固定 V1 response outcome。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAgentOutcome {
    /// request 通过纯逻辑前置判断；未表示已执行任何系统操作。
    Accepted,
    /// request 被纯逻辑前置判断拒绝。
    Rejected(ServiceAgentError),
}

/// V1 的无自由字段 response。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceAgentResponseV1 {
    action: ServiceAgentAction,
    outcome: ServiceAgentOutcome,
}

impl ServiceAgentResponseV1 {
    /// 构造仅表达已通过前置判断的 response。
    #[must_use]
    pub const fn accepted(action: ServiceAgentAction) -> Self {
        Self {
            action,
            outcome: ServiceAgentOutcome::Accepted,
        }
    }

    /// 构造只携带稳定错误类别的拒绝 response。
    #[must_use]
    pub const fn rejected(action: ServiceAgentAction, error: ServiceAgentError) -> Self {
        Self {
            action,
            outcome: ServiceAgentOutcome::Rejected(error),
        }
    }

    /// response 对应的固定 action。
    #[must_use]
    pub const fn action(self) -> ServiceAgentAction {
        self.action
    }

    /// response 的固定 outcome。
    #[must_use]
    pub const fn outcome(self) -> ServiceAgentOutcome {
        self.outcome
    }

    /// 编码为精确 V1 response frame。
    #[must_use]
    pub const fn encode(self) -> [u8; SERVICE_AGENT_RESPONSE_V1_LEN] {
        let mut bytes = [0_u8; SERVICE_AGENT_RESPONSE_V1_LEN];
        bytes[0] = RESPONSE_MAGIC[0];
        bytes[1] = RESPONSE_MAGIC[1];
        bytes[2] = RESPONSE_MAGIC[2];
        bytes[3] = RESPONSE_MAGIC[3];
        bytes[4] = FRAME_VERSION;
        bytes[5] = self.action as u8;
        let (outcome, code) = match self.outcome {
            ServiceAgentOutcome::Accepted => (0, 0_u32),
            ServiceAgentOutcome::Rejected(error) => (1, error.wire_code()),
        };
        bytes[6] = outcome;
        let code = code.to_be_bytes();
        bytes[8] = code[0];
        bytes[9] = code[1];
        bytes[10] = code[2];
        bytes[11] = code[3];
        bytes
    }

    /// 解码严格的 V1 response frame。
    ///
    /// # Errors
    ///
    /// 任意长度、magic、version、action、reserved 或 outcome/code 不一致均返回不含远端
    /// 内容的稳定 [`ServiceAgentError`]。
    pub fn decode(bytes: &[u8]) -> Result<Self, ServiceAgentError> {
        if bytes.len() != SERVICE_AGENT_RESPONSE_V1_LEN {
            return Err(ServiceAgentError::FrameLength);
        }
        if bytes[0..4] != RESPONSE_MAGIC {
            return Err(ServiceAgentError::FrameMagic);
        }
        if bytes[4] != FRAME_VERSION {
            return Err(ServiceAgentError::FrameVersion);
        }
        if bytes[7] != 0 || bytes[12] != 0 || bytes[13] != 0 || bytes[14] != 0 || bytes[15] != 0 {
            return Err(ServiceAgentError::ReservedField);
        }

        let action = ServiceAgentAction::try_from(bytes[5])?;
        let code = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        match (bytes[6], code) {
            (0, 0) => Ok(Self::accepted(action)),
            (1, _) => ServiceAgentError::from_wire_code(code)
                .map(|error| Self::rejected(action, error))
                .ok_or(ServiceAgentError::ResponseMalformed),
            _ => Err(ServiceAgentError::ResponseMalformed),
        }
    }
}
