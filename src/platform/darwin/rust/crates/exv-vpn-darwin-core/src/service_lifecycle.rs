//! darwin 服务生命周期动作编排（P4 v2：`ServiceControl` install/uninstall/start 实装）。
//!
//! 拓扑与信任边界：Core 是普通用户进程，安装/卸载/启动 dev-service agent 属于 root 操作。
//! 提权机制选型（2026-09 调研拍板）：`/usr/bin/osascript` 的
//! `do shell script … with administrator privileges`——系统密码弹窗授权**整条 payload**，
//! 因此 payload 只允许「固定 argv 的 service agent CLI 动词」（本模块构造，绝不拼接任何
//! 用户输入），与 win32「engine 子命令固定枚举 + 一次 runas」的纪律同构
//! （win32 `service_batch.rs` 的 no-arbitrary-command 原则）。SMAppService 因无签名/
//! ad-hoc 注册不可靠且强制 daemon 留在 bundle 内而排除；SMJobBless 与 Apple 旧
//! Authorization 提权执行 API 均已弃用（守卫对后者 FFI 全禁，本路线不触碰）。
//!
//! **守卫例外说明**：本文件是 darwin workspace 内唯一允许出现
//! `std::process::Command` 与 osascript 的生产文件（`verify-rust-only.sh` 的
//! `has_only_fixed_service_elevation_launcher` 白名单逐字面量审计——与
//! `core_process.rs`/`autostart.rs` 同一例外模式）。除此之外不得引入任何进程启动面。
//!
//! 诚实的语义差异（与 win32 对照，均已在文档登记）：
//! - uninstall 无法回收已在跑的 root Engine 进程（service agent `bootout` 只杀 daemon）；
//!   业务面停机由 Core 前置显式 Stop 尽力完成（见
//!   `LiveConnectionProjection::stop_for_service_transition`），失败不阻断变更。
//! - `scm_orphan` 盲点沿用 P4 v1 声明（`service_status` 模块头）。

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::service_status::{
    ServiceAgentFacts, ServiceAgentHealth, SERVICE_AGENT_BINARY_PATH, derive_health,
};

/// 提权执行等待上界——包含用户阅读弹窗与输入管理员密码的时间（系统弹窗无超时，
/// 超界由本侧 kill 子进程兜底并按失败上报）。300s=5min，std 无更大时间单位构造器。
#[allow(
    clippy::duration_suboptimal_units,
    reason = "5 分钟上界无更大单位的 std 构造器可读形式"
)]
const ELEVATION_BOUND: Duration = Duration::from_secs(300);
/// 动作后就绪/移除等待上界（对齐 win32 `SERVICE_READY_TIMEOUT=15s` 的档位语义：
/// 「双事实达成」在 darwin = socket 可连且 Status 未被拒）。
const READINESS_BOUND: Duration = Duration::from_secs(15);
/// 卸载后移除观察上界（win32 `wait_service_removed` 500ms 上界的 darwin 放宽档：
/// bootout 异步终止 daemon 后 guarded remove 才能生效）。
const REMOVAL_BOUND: Duration = Duration::from_secs(10);
/// 就绪/移除轮询间隔。
const READINESS_POLL: Duration = Duration::from_millis(250);

/// 用户取消管理员授权（系统弹窗「取消」）的稳定码。
pub(crate) const ELEVATION_DENIED_CODE: &str = "DARWIN_CORE_SERVICE_ELEVATION_DENIED";
/// 提权执行失败（payload 非零退出/超时/无法启动 osascript）的稳定码。
pub(crate) const ELEVATION_FAILED_CODE: &str = "DARWIN_CORE_SERVICE_ELEVATION_FAILED";
/// 动作后等待就绪超时的稳定码。
pub(crate) const SERVICE_NOT_READY_CODE: &str = "DARWIN_CORE_SERVICE_NOT_READY";
/// start/ensure 在未安装态发起的稳定码。
pub(crate) const SERVICE_NOT_INSTALLED_CODE: &str = "DARWIN_CORE_SERVICE_NOT_INSTALLED";
/// 安装源二进制缺失（bundle 未随附且无已安装副本）的稳定码。
pub(crate) const SERVICE_SOURCE_MISSING_CODE: &str = "DARWIN_CORE_SERVICE_AGENT_SOURCE_MISSING";

/// 提权执行结果；`Failed` 携带失败细节（service agent 稳定码，或 stderr 有界尾部——
/// 仅固定路径/系统文本，无秘密；供 typed message 与日志聚合诊断使用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ElevationOutcome {
    /// payload 以退出码 0 完成；携带 stdout 原文（oneshot 场景解析 engine pid，
    /// 服务动词忽略）。
    Executed(String),
    /// 用户在系统弹窗取消授权。
    Denied,
    /// payload 非零退出或等待超界。
    Failed(String),
}

/// 提权执行 seam：生产为 osascript；测试注入可编程 fake。
pub(crate) trait ServiceElevator: Send + Sync {
    /// 以管理员权限执行一条固定 payload（同步阻塞直至完成/取消/超界）。
    fn run_admin_payload(&self, prompt: &str, payload: &str) -> ElevationOutcome;
}

/// 一次服务生命周期动作的稳定错误类别（全部携带最终探测事实供上层回填快照）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LifecycleError {
    /// 用户取消管理员授权。
    Denied(ServiceAgentFacts),
    /// 提权执行失败或超界（细节=稳定码/有界 stderr，进 typed message 与日志）。
    Failed(ServiceAgentFacts, String),
    /// 动作后等待就绪超时（携带最后事实）。
    NotReady(ServiceAgentFacts),
    /// start/ensure 在未安装态发起。
    NotInstalled(ServiceAgentFacts),
    /// 安装源二进制缺失。
    SourceMissing(ServiceAgentFacts),
}

/// 探测事实 future 的 seam 类型（kernel 侧 `GetSnapshot`/连接挂钩共用）。
pub(crate) type ServiceProbeFuture =
    std::pin::Pin<std::boxed::Box<dyn std::future::Future<Output = ServiceAgentFacts> + Send>>;
/// 探测 seam：返回当前 service agent 健康事实。
pub(crate) type ServiceStatusProbe = std::sync::Arc<dyn Fn() -> ServiceProbeFuture + Send + Sync>;

/// 服务的配套二进制解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServiceAgentPaths {
    /// 安装/重装 payload 使用的 service agent 二进制（bundle 内随附副本优先；缺席时
    /// 回退已安装副本——service agent 的 install 本就是 `current_exe` 自拷贝）。
    pub source: PathBuf,
    /// 随安装登记进 state 叶的 Engine 绝对路径（Core 同目录兄弟二进制；缺席时
    /// payload 不带 `--engine-path`，daemon 沿用已装 Engine 固定路径回退）。
    pub engine: Option<PathBuf>,
}

/// 固定安装目录中的 service agent 二进制名（bundle 随附副本与已安装副本同名）。
const SERVICE_AGENT_BINARY_NAME: &str = "exv-vpn-darwin-service-agent";
/// Core 同目录的固定 Engine 二进制名（开发检出与 bundle 布局一致：
/// `target/<profile>/` 与 `Contents/MacOS/` 均为平铺四件套：tauri/core/engine/服务代理）。
const ENGINE_BINARY_NAME: &str = "exv-vpn-darwin-engine";

/// 解析安装/重装 payload 的二进制身份（只读 `current_exe` 同目录，无副作用）。
pub(crate) fn resolve_service_agent_paths() -> ServiceAgentPaths {
    let core_directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let sibling = |name: &str| {
        core_directory
            .as_ref()
            .map(|directory| directory.join(name))
            .filter(|path| path.is_file())
    };
    ServiceAgentPaths {
        source: sibling(SERVICE_AGENT_BINARY_NAME)
            .unwrap_or_else(|| PathBuf::from(SERVICE_AGENT_BINARY_PATH)),
        engine: sibling(ENGINE_BINARY_NAME),
    }
}

/// sh 单引号包裹（payload 进入 `do shell script` 的 `/bin/sh`；嵌入单引号用
/// `'\''` 断开重构——对固定常量路径是纵深防御，正常输入不含单引号）。
fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

/// 三类动作的固定 payload（纯函数；路径全部来自 [`ServiceAgentPaths`] 与 Core 自身
/// 身份，绝无用户输入）。
pub(crate) struct ServicePayloads {
    /// `<source> install --owner-uid N --owner-gid N [--engine-path <engine>]`
    pub install: String,
    /// **预卸载合并 payload**（2026-09-12，§5.1 step 2）：一次提权内按
    /// 「先停 → 再删」顺序完成全部 root 动作：
    ///
    /// 1. `<runner> retire-legacy`：历史三个守护（bootout 固定 label + 删 descriptor
    ///    与安装目录）；
    /// 2. `<runner> sweep-runtime-residue`：`/private/tmp` 下 root 属主 runtime 残留
    ///    （活会话跳过）；
    /// 3. `<runner> uninstall --owner-uid N --owner-gid N`：当前服务的 descriptor +
    ///    `ServiceAgent/` 安装目录（退役命名空间由 `retire-legacy` 回收）；
    /// 4. `rm -f` 两个 root 属主 engine 日志——**必须排在停服务之后**：root engine
    ///    运行期间会持续 append（实测 pump.log 持续增长），先删会被立即重建。
    ///
    /// 用 `;` 串联而非 `&&`：单项失败不得阻断后续项。
    ///
    /// **结果判定契约**：本 payload 的退出码**不能**作为成功依据（`;` 串联下它只反映
    /// 最后一项）。分项结果由**调用方**（壳命令 `pre_uninstall`）在提权返回后以
    /// 后置 `lstat` 逐项核对得出（§5.3）；core 只负责按顺序发起这一次提权。
    pub uninstall: String,
    /// `<runner> start`
    pub start: String,
}

/// 固定安装根目录（编译期字面量；W9 的「两段式」白名单根）。
const INSTALL_ROOT_PATH: &str = "/Library/Application Support/EXV";

/// 两个 root 属主 engine 日志的固定绝对路径（编译期字面量，`exv-vpn-darwin-engine`
/// 的 `pump.rs`/`bootstrap_runtime.rs` 硬编码同一路径）。
const ROOT_ENGINE_LOG_PATHS: [&str; 2] = [
    "/private/tmp/exv-engine-pump.log",
    "/private/tmp/exv-engine-panic.log",
];

/// 固定安装根目录是否仍有内容（W9 放宽触发：只要根目录下还有任何条目，就值得提权
/// 跑一次合并 payload）。只读一层目录枚举，不做任何删除。
fn install_root_has_entries() -> bool {
    std::fs::read_dir(INSTALL_ROOT_PATH).is_ok_and(|mut entries| entries.next().is_some())
}

/// 构造三类固定 payload（service agent CLI 的显式 owner 形态：`osascript` 的 root shell
/// 没有 `SUDO_UID`/`SUDO_GID`，owner 由无特权 Core 以自身身份提供）。
///
/// 三类动作统一使用**源二进制**（bundle 随附副本优先，缺席回退已安装副本）：
/// 已安装副本可能是旧版 CLI（不认识显式 owner 形态——2026-09-08 真机首验即因此
/// 报 `ELEVATION_FAILED`：旧二进制对 `uninstall --owner-uid` 回 `CLI_USAGE`，原子
/// 拒绝、无副作用）；service agent 的固定路径常量使任何副本执行装卸等价。
///
/// **W8 payload 纯净性**：本函数（及其调用的 [`shell_quote`]）只由编译期字面量与
/// 参数拼装，**不读任何环境变量**；对应自检
/// `payload_construction_path_never_reads_environment_variables`。
#[allow(
    clippy::similar_names,
    reason = "owner uid/gid 是 service agent CLI 与 serve 描述的既有配对词汇"
)]
pub(crate) fn build_payloads(
    paths: &ServiceAgentPaths,
    owner_uid: u32,
    owner_gid: u32,
) -> ServicePayloads {
    let runner = shell_quote(&paths.source.to_string_lossy());
    let mut install = format!("{runner} install --owner-uid {owner_uid} --owner-gid {owner_gid}");
    if let Some(engine) = paths.engine.as_deref() {
        install.push_str(" --engine-path ");
        install.push_str(&shell_quote(&engine.to_string_lossy()));
    }
    ServicePayloads {
        install,
        uninstall: build_purge_payload(&runner, owner_uid, owner_gid),
        start: format!("{runner} start"),
    }
}

/// 合并后的预卸载 payload（见 [`ServicePayloads::uninstall`] 的顺序契约）。
#[allow(
    clippy::similar_names,
    reason = "owner uid/gid 是 service agent CLI 与 serve 描述的既有配对词汇"
)]
fn build_purge_payload(runner: &str, owner_uid: u32, owner_gid: u32) -> String {
    let logs = ROOT_ENGINE_LOG_PATHS
        .iter()
        .map(|path| shell_quote(path))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{runner} retire-legacy ; {runner} sweep-runtime-residue ; {runner} uninstall --owner-uid {owner_uid} --owner-gid {owner_gid} ; rm -f {logs}"
    )
}

/// W3 oneshot 提权弹窗文案（固定常量纪律：无引号/反斜杠，不进 `AppleScript`
/// 源码解析层）。
pub(crate) const ONESHOT_ELEVATION_PROMPT: &str =
    "EXV 需要管理员权限以一次性运行 VPN 引擎（仅本次连接，不安装系统服务）";

/// W3 oneshot 拉起的稳定错误类别。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OneshotElevationError {
    /// 用户在系统弹窗取消授权。
    Denied,
    /// 提权失败或 stdout pid 解析不合规（细节=稳定码/有界输出）。
    Failed(String),
    /// bundle 内 engine 二进制缺席（无服务形态没有 state 叶回退，直接拒绝）。
    SourceMissing,
}

/// 构造 oneshot 一次性拉起 payload（v3 §二时序第 3 步；路径全部来自
/// [`ServiceAgentPaths`] 与 Core 自身身份，runtime 目录来自刚创建的 `RuntimeDir`）。
///
/// # Errors
///
/// bundle 兄弟 engine 缺席返回 [`OneshotElevationError::SourceMissing`]。
#[allow(
    clippy::similar_names,
    reason = "owner uid/gid 是 service agent CLI 与 serve 描述的既有配对词汇"
)]
pub(crate) fn build_oneshot_payload(
    paths: &ServiceAgentPaths,
    owner_uid: u32,
    owner_gid: u32,
    runtime_dir: &std::path::Path,
    core_pid: u32,
) -> Result<String, OneshotElevationError> {
    let Some(engine) = paths.engine.as_deref() else {
        return Err(OneshotElevationError::SourceMissing);
    };
    let runner = shell_quote(&paths.source.to_string_lossy());
    Ok(format!(
        "{runner} start-engine-once --owner-uid {owner_uid} --owner-gid {owner_gid} --runtime-dir {} --core-pid {core_pid} --engine-path {}",
        shell_quote(&runtime_dir.to_string_lossy()),
        shell_quote(&engine.to_string_lossy()),
    ))
}

/// engine pid 行解析规约（v3 §二）：trim 尾随换行、恰好一行、纯十进制、非零、
/// u32 无溢出；任何偏差返回 `None`（调用方按提权失败处理，稳定码入 message）。
pub(crate) fn parse_engine_pid_line(stdout: &str) -> Option<u32> {
    let trimmed = stdout.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty()
        || trimmed.contains('\n')
        || !trimmed.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    trimmed.parse::<u32>().ok().filter(|pid| *pid != 0)
}

/// W3 oneshot：经 osascript 一次性提权拉起 engine 并解析其 pid。同步阻塞直至
/// 完成/取消/超界（[`ELEVATION_BOUND`]），**调用方必须经 `spawn_blocking` 下沉**
/// （Core 是 `current_thread` runtime）。唯一 root 执行体是 bundle service agent 的
/// `start-engine-once` 守护式 spawn（daemon `StartEngine` 帧已随 E8 退役）。
///
/// # Errors
///
/// 弹窗取消、提权失败、pid 解析不合规或 bundle engine 缺席返回
/// [`OneshotElevationError`]（调用方映射为连接终态，细节码进日志与 message）。
pub(crate) fn run_oneshot_elevation(
    runtime_dir: &std::path::Path,
    core_pid: u32,
) -> Result<u32, OneshotElevationError> {
    let paths = resolve_service_agent_paths();
    let credentials = crate::elevation::current_core_credentials();
    // SAFETY: getgid only reads the current process group id.
    let gid = unsafe { libc::getgid() };
    let payload = build_oneshot_payload(&paths, credentials.uid(), gid, runtime_dir, core_pid)?;
    let elevator = OsascriptElevator;
    match elevator.run_admin_payload(ONESHOT_ELEVATION_PROMPT, &payload) {
        ElevationOutcome::Executed(stdout) => parse_engine_pid_line(&stdout).ok_or_else(|| {
            OneshotElevationError::Failed("DARWIN_CORE_ONESHOT_PID_UNPARSEABLE".to_string())
        }),
        ElevationOutcome::Denied => Err(OneshotElevationError::Denied),
        ElevationOutcome::Failed(detail) => Err(OneshotElevationError::Failed(detail)),
    }
}

/// 事实是否「daemon 在服务」（socket 可连且 Status 未被拒——P4 v1 探测语义）。
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "与 service_status::derive_health 的 &ServiceAgentFacts 取用形态一致"
)]
fn healthy(facts: &ServiceAgentFacts) -> bool {
    derive_health(facts) == ServiceAgentHealth::Healthy
}

/// 事实是否「已安装」（binary 在场；与 `ServiceAgentHealth::PayloadOrphan` 互补）。
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "同 healthy：与 derive_health 的取用形态一致"
)]
fn installed(facts: &ServiceAgentFacts) -> bool {
    facts.binary_present
}

/// 预卸载提权触发判据（纯函数，便于单测钉死）。
///
/// 2026-09-12：早期实现只在 `installed`（binary 在场）时提权，导致两类真实场景被静默
/// 跳过——① `payload_orphan`（binary 已删但 socket/state 残留）；② **只用 oneshot
/// 连接、从未安装过服务**的机器上仍有历史守护在跑（§5.2 W9：用户以为卸干净，launchd
/// 还在尝试拉起已删二进制）。故判据放宽为「任一 service agent 产物在场（binary/socket/
/// state）」或「固定安装根目录非空」——后者由 [`Self::install_root_probe`] 注入，
/// 生产读固定字面量目录，测试注入确定性布尔。
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "与 installed/healthy 的 &ServiceAgentFacts 取用形态一致"
)]
fn purge_needed(facts: &ServiceAgentFacts, install_root_present: bool) -> bool {
    facts.binary_present
        || facts.socket_file_present
        || facts.state_leaf_present
        || install_root_present
}

/// 二进制身份解析 seam（生产读 `current_exe` 同目录；测试注入确定性路径）。
pub(crate) type ServiceAgentPathsResolver = std::sync::Arc<dyn Fn() -> ServiceAgentPaths + Send + Sync>;

/// 安装根目录内容探测 seam（生产读固定字面量目录；测试注入确定性布尔——夹具绝不
/// 触碰宿主 `/Library`）。
pub(crate) type InstallRootProbe = std::sync::Arc<dyn Fn() -> bool + Send + Sync>;

/// 服务生命周期执行器（elevator 与探测 seam 全部可注入；生产用 [`Self::production`]，
/// 测试注入 fake 断言编排顺序）。
pub(crate) struct ServiceLifecycle {
    elevator: std::sync::Arc<dyn ServiceElevator>,
    probe: ServiceStatusProbe,
    paths: ServiceAgentPathsResolver,
    install_root_probe: InstallRootProbe,
    readiness_poll: Duration,
    readiness_bound: Duration,
    removal_bound: Duration,
    owner_uid: u32,
    owner_gid: u32,
}

impl ServiceLifecycle {
    /// 生产执行器：真实 osascript 提权 + 真实 service agent 探测 + Core 自身 owner 身份。
    pub(crate) fn production() -> Self {
        let credentials = crate::elevation::current_core_credentials();
        // SAFETY: getgid only reads the current process group id.
        let gid = unsafe { libc::getgid() };
        Self {
            elevator: std::sync::Arc::new(OsascriptElevator),
            probe: std::sync::Arc::new(|| Box::pin(crate::service_status::probe_service_agent_facts())),
            paths: std::sync::Arc::new(resolve_service_agent_paths),
            install_root_probe: std::sync::Arc::new(install_root_has_entries),
            readiness_poll: READINESS_POLL,
            readiness_bound: READINESS_BOUND,
            removal_bound: REMOVAL_BOUND,
            owner_uid: credentials.uid(),
            owner_gid: gid,
        }
    }

    /// 测试构造：注入 elevator/探测/路径/轮询节奏（毫秒级钉死等待语义）。
    #[allow(
        clippy::similar_names,
        reason = "owner uid/gid 是 service agent CLI 与 serve 描述的既有配对词汇"
    )]
    pub(crate) fn new(
        elevator: std::sync::Arc<dyn ServiceElevator>,
        probe: ServiceStatusProbe,
        paths: ServiceAgentPathsResolver,
        readiness_poll: Duration,
        action_bound: Duration,
        owner_uid: u32,
        owner_gid: u32,
    ) -> Self {
        Self {
            elevator,
            probe,
            paths,
            // 夹具恒「安装根目录为空」：测试要断言该维度时显式
            // `with_install_root_probe` 注入，绝不触碰宿主 `/Library`。
            install_root_probe: std::sync::Arc::new(|| false),
            readiness_poll,
            readiness_bound: action_bound,
            removal_bound: action_bound,
            owner_uid,
            owner_gid,
        }
    }

    async fn facts(&self) -> ServiceAgentFacts {
        (self.probe)().await
    }

    /// 探测 seam 句柄（kernel 侧 `GetSnapshot`/`ServiceControl` query 共用同一
    /// 探测——生产真实探测，夹具恒 healthy，不触宿主）。
    pub(crate) fn probe(&self) -> ServiceStatusProbe {
        std::sync::Arc::clone(&self.probe)
    }

    /// 有界轮询直到谓词成立或超界；返回最后事实（调用方据 `NotReady` 上报）。
    async fn wait_until(
        &self,
        bound: Duration,
        mut predicate: impl FnMut(&ServiceAgentFacts) -> bool,
    ) -> ServiceAgentFacts {
        let deadline = tokio::time::Instant::now() + bound;
        loop {
            let facts = self.facts().await;
            if predicate(&facts) {
                return facts;
            }
            if tokio::time::Instant::now() >= deadline {
                return facts;
            }
            tokio::time::sleep(self.readiness_poll).await;
        }
    }

    /// 提权执行经 `spawn_blocking` 下沉阻塞线程：Core 是 `current_thread` runtime，
    /// 系统密码弹窗最长挂起 [`ELEVATION_BOUND`]，绝不能冻结 gRPC/事件循环。
    async fn run_elevated(
        &self,
        prompt: &str,
        payload: &str,
        facts: ServiceAgentFacts,
    ) -> Result<(), LifecycleError> {
        let elevator = std::sync::Arc::clone(&self.elevator);
        let prompt = prompt.to_owned();
        let payload = payload.to_owned();
        let outcome =
            tokio::task::spawn_blocking(move || elevator.run_admin_payload(&prompt, &payload))
                .await
                .unwrap_or_else(|join_error| {
                    ElevationOutcome::Failed(format!("elevation worker failed: {join_error}"))
                });
        match outcome {
            ElevationOutcome::Executed(_) => Ok(()),
            ElevationOutcome::Denied => Err(LifecycleError::Denied(facts)),
            ElevationOutcome::Failed(detail) => Err(LifecycleError::Failed(facts, detail)),
        }
    }

    /// 安装/修复（win32 `[Install, Start, Verify]` 批量的 darwin 对应：service agent
    /// `install` 单动词内含 bootout→覆写→bootstrap 的替换序列；随后有界等待
    /// daemon 就绪）。
    pub(crate) async fn install(&self) -> Result<ServiceAgentFacts, LifecycleError> {
        let facts = self.facts().await;
        let paths = (self.paths)();
        if !paths.source.is_file() {
            return Err(LifecycleError::SourceMissing(facts));
        }
        let payloads = build_payloads(&paths, self.owner_uid, self.owner_gid);
        self.run_elevated(
            "EXV 需要安装系统级网络服务组件（复制服务并注册为系统守护进程）",
            &payloads.install,
            facts,
        )
        .await?;
        let final_facts = self.wait_until(self.readiness_bound, healthy).await;
        if healthy(&final_facts) {
            Ok(final_facts)
        } else {
            Err(LifecycleError::NotReady(final_facts))
        }
    }

    /// 卸载（幂等；2026-09-12 起语义扩展为**预卸载合并动作**，§5.1 step 2）。
    ///
    /// 触发判据见 [`purge_needed`]：任一 service agent 产物在场或固定安装根目录非空即
    /// 提权一次，,[`build_payloads`] 的合并 payload 内一次完成
    /// 「retire-legacy → sweep-runtime-residue → uninstall → 删 root engine 日志」。
    /// 全无残留时直接 Ok（不弹密码、无副作用）。
    ///
    /// 变更动作调用方（`kernel_control_service`）已在调用前尽力停业务
    /// （`stop_for_service_transition`），对应 §5.1 step 1；`sweep-runtime-residue`
    /// 对活会话另有 W10 活性跳过。
    pub(crate) async fn uninstall(&self) -> Result<ServiceAgentFacts, LifecycleError> {
        let facts = self.facts().await;
        if !purge_needed(&facts, (self.install_root_probe)()) {
            return Ok(facts);
        }
        let payloads = build_payloads(&(self.paths)(), self.owner_uid, self.owner_gid);
        self.run_elevated(
            "EXV 需要管理员权限以移除系统级残留（停止并删除系统服务、历史守护进程、安装文件与运行时残留）",
            &payloads.uninstall,
            facts,
        )
        .await?;
        let final_facts = self
            .wait_until(self.removal_bound, |facts| {
                !facts.binary_present && !facts.socket_file_present
            })
            .await;
        if installed(&final_facts) || final_facts.socket_file_present {
            Err(LifecycleError::NotReady(final_facts))
        } else {
            Ok(final_facts)
        }
    }

    /// 按需启动（win32 `PromptStart` 的 darwin 对应）：healthy 即幂等成功；已装
    /// 未跑则提权 `start`（service agent 单动词内含 bootout-ignore→bootstrap）后等待
    /// 就绪；未安装为稳定拒绝（Core 绝不自动安装——win32 同款，安装由 UI 编排）。
    pub(crate) async fn ensure_running(&self) -> Result<ServiceAgentFacts, LifecycleError> {
        let facts = self.facts().await;
        if healthy(&facts) {
            return Ok(facts);
        }
        if !installed(&facts) {
            return Err(LifecycleError::NotInstalled(facts));
        }
        let payloads = build_payloads(&(self.paths)(), self.owner_uid, self.owner_gid);
        self.run_elevated(
            "EXV 系统服务未在运行，需要管理员权限重新启动它",
            &payloads.start,
            facts,
        )
        .await?;
        let final_facts = self.wait_until(self.readiness_bound, healthy).await;
        if healthy(&final_facts) {
            Ok(final_facts)
        } else {
            Err(LifecycleError::NotReady(final_facts))
        }
    }
}

/// 从 osascript stderr 提取失败细节：优先 service agent 稳定码
/// （`DARWIN_SERVICE_AGENT_*`）+ **同行有界剩余**（≤200 字符）。
///
/// 2026-09-20 问题四第一层放宽：service agent 的守卫拒绝 stderr 已增强为
/// 「稳定码 + artifact contract_key + 不符维度」同行输出（如
/// `DARWIN_SERVICE_AGENT_ARTIFACT_GUARD_REJECTED artifact=state-leaf
/// dimension=parent-guard`）；旧实现取到首个稳定码 token 即截断，会吞掉增强
/// 维度。放宽后捕获稳定码起始的整行（固定常量与系统文本，无秘密），整行限
/// 200 字符防长行 stderr 灌入 typed message 与日志。
fn elevation_failure_detail(stderr: &str) -> String {
    const STABLE_CODE_PREFIX: &str = "DARWIN_SERVICE_AGENT_";
    const DETAIL_BOUND: usize = 200;
    if let Some(start) = stderr.find(STABLE_CODE_PREFIX) {
        let tail = &stderr[start..];
        let line_end = tail.find(['\n', '\r']).unwrap_or(tail.len());
        return tail[..line_end].chars().take(DETAIL_BOUND).collect();
    }
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        "提权命令非零退出（无诊断输出）".to_string()
    } else {
        let bounded = trimmed.chars().rev().take(DETAIL_BOUND).collect::<Vec<_>>();
        bounded.into_iter().rev().collect()
    }
}

/// `osascript` 提权执行器（`on run argv` 形态：payload 作为独立 argv 元素传入，
/// 不经 `AppleScript` 源码解析；弹窗文案 `with prompt` 说明用途）。
struct OsascriptElevator;

impl ServiceElevator for OsascriptElevator {
    fn run_admin_payload(&self, prompt: &str, payload: &str) -> ElevationOutcome {
        // 弹窗文案是本模块固定中文常量（不含引号/反斜杠），直接内插为 AppleScript
        // 字符串字面量；payload 走 argv 不进源码。
        debug_assert!(
            !prompt.contains('"') && !prompt.contains('\\'),
            "osascript prompt must be a fixed literal without quotes"
        );
        let script_do = format!(
            "do shell script (item 1 of argv) with prompt \"{prompt}\" with administrator privileges"
        );
        let Ok(mut child) = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg("on run argv")
            .arg("-e")
            .arg(script_do)
            .arg("-e")
            .arg("end run")
            .arg("--")
            .arg(payload)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return ElevationOutcome::Failed(
                "无法启动系统提权执行程序（/usr/bin/osascript）".to_string(),
            );
        };
        // A9（v3 设计）：成功路径也必须读尽 stdout/stderr——piped 子输出写满 64KB
        // 管道缓冲即写阻塞，会被下方有界等待误判为超时。两管道各由排空线程持有，
        // 进程退出（或超界 kill）后 EOF 自然结束。
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let stdout_drain = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = stdout_pipe {
                let _ = std::io::Read::read_to_string(&mut pipe, &mut text);
            }
            text
        });
        let stderr_drain = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = std::io::Read::read_to_string(&mut pipe, &mut text);
            }
            text
        });
        // 系统弹窗无超时：本侧有界轮询兜底，超界 kill 子进程按失败上报（用户
        // 挂起不输密码最终表现为该动作失败而非 Core 卡死）。
        let deadline = std::time::Instant::now() + ELEVATION_BOUND;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(_) => {
                    let _ = child.kill();
                    return ElevationOutcome::Failed("提权执行进程状态读取失败".to_string());
                }
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return ElevationOutcome::Failed(format!(
                    "等待管理员授权超时（上界 {}s）",
                    ELEVATION_BOUND.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let stdout = stdout_drain.join().unwrap_or_default();
        let stderr = stderr_drain.join().unwrap_or_default();
        if status.success() {
            return ElevationOutcome::Executed(stdout);
        }
        // 取消判定（AppleScript -128/-60006 在 osascript CLI 层表现为非零退出 +
        // stderr 文本）：只读管道判类别，stderr 原文不进 wire。
        let canceled = stderr.contains("User canceled")
            || stderr.contains("-128")
            || stderr.contains("-60006");
        if canceled {
            ElevationOutcome::Denied
        } else {
            ElevationOutcome::Failed(elevation_failure_detail(&stderr))
        }
    }
}

/// 夹具默认 elevator：不启动任何进程，恒返回用户取消——保证测试/fixture 服务
/// 误入变更动作时也不会触发真实提权弹窗（生产路径唯一 elevator 是
/// [`OsascriptElevator`]，只经 [`ServiceLifecycle::production`] 进入）。
pub(crate) struct DeniedElevator;

impl ServiceElevator for DeniedElevator {
    fn run_admin_payload(&self, _prompt: &str, _payload: &str) -> ElevationOutcome {
        ElevationOutcome::Denied
    }
}

/// 夹具默认探测 seam（W3）：恒「未安装」事实——hermetic 世界里连接走 oneshot
/// 夹具路径（`resolve_transport` 判 Oneshot），不触发提权与宿主探测。
pub(crate) fn hermetic_healthy_probe() -> ServiceStatusProbe {
    std::sync::Arc::new(|| {
        Box::pin(std::future::ready(ServiceAgentFacts {
            binary_present: false,
            socket_file_present: false,
            state_leaf_present: false,
            socket_connectable: false,
            status_accepted: None,
            engine_socket_present: false,
            engine_socket_connectable: false,
        }))
    })
}

/// 夹具默认 lifecycle：恒「未安装」探测 + 恒拒绝 elevator（`with_launch`/`new_inner`
/// 的默认服务接缝；W3 起连接默认走 oneshot 夹具路径——服务形态与修复阶梯的
/// 编排断言由测试显式注入 fake seam）。
pub(crate) fn hermetic_lifecycle() -> ServiceLifecycle {
    ServiceLifecycle::new(
        std::sync::Arc::new(DeniedElevator),
        hermetic_healthy_probe(),
        std::sync::Arc::new(resolve_service_agent_paths),
        READINESS_POLL,
        READINESS_BOUND,
        501,
        20,
    )
}
