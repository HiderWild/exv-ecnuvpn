//! CoreClient —— UI 侧访问 core 的唯一接缝（P4-b：真实 tonic 调用）。
//!
//! 架构（计划 §1）：core 是独立进程，唯一语义网关；UI 不直连 engine。
//! P4-b 接线方式（见 README「P4-b IPC 接线说明」）：
//!   * Tauri host 进程 spawn/attach core 进程（[`super::core_process`]），
//!     经 [`super::core_transport::connect_core_channel`] 拨号 core 的
//!     `KernelControl` Named Pipe gRPC 端点；
//!   * [`CoreHandle::Dialed`] 持有 tonic `Channel`；[`CoreClient`] 方法经
//!     `KernelControlClient` 做真实 unary / server-streaming 调用；
//!   * wire 消息 → UI 镜像类型的机械映射在 [`super::wire`]。
//!
//! 2026-08-18：`logs_list`/`config_get`/`config_set` 已真实接线（KernelControl
//! 新增 LogsList/LogsClear/ConfigGet/ConfigSet RPC；host 聚合日志 + ExvConfig
//! 服务，LogEvent 迁至 common.proto 共享）。stats 经 RuntimeSnapshot.stats 携带，
//! `stats` 命令从 snapshot 缓存读取。`stats` 缓存无样本时返回 NotWired 占位。

use std::{fmt, sync::RwLock};

use exv_vpn_wire::generated::kernel_control_client::KernelControlClient;
use exv_vpn_wire::generated::{self as wire};
use serde::{Deserialize, Serialize};
use tonic::transport::Channel;
use tonic::{Code, Request, Status};

const STOP_TOTAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

fn stop_unconfirmed_reply(operation_id: Option<&[u8]>, stage: &str) -> OperationReply {
    let error = super::state::VpnError {
        code: "STOP_DEADLINE_EXCEEDED".into(),
        message: format!("停止等待超时，尚未确认清理完成（{stage}）；可重试停止或修复服务。"),
        reason: "stop/deadline_exceeded".into(),
        stage: Some("ERROR_STAGE_TEARDOWN".into()),
    };
    OperationReply {
        operation_id: operation_id.map(wire_map::hex),
        stop_completion: Some(super::state::StopCompletion { stopped: false, blocks_new_connect: false, errors: vec![error.clone()] }),
        result: super::state::OperationResult::Failed { error: Some(error) },
    }
}

use super::core_transport::{CorePeer, GrpcPipeError, connect_core_channel};
use super::error::AppError;
use super::logs::{LogChunk, LogsClearReply};
use super::state::{
    OperationReply, OperationResult, RuntimeSnapshot, ServiceControlAction, ServiceControlReply,
};
use super::stats::RuntimeStats;
use super::wire::{self as wire_map};

/// 前端提交的本次连接凭据。密码只会在 Tauri→Core 的当前请求中编码，不记录到错误或日志。
#[derive(Clone, Serialize, Deserialize)]
pub struct ConnectCredentials {
    pub username: String,
    pub password: String,
    pub persist: bool,
}

impl fmt::Debug for ConnectCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("persist", &self.persist)
            .finish()
    }
}

/// 连接/身份意图。Tauri 命令边界只接收结构化 `credentials`，由 wire 层编码为已有
/// `secret_payload`；保留 `secret_payload` 仅供既有 Rust 内部调用路径兼容。
///
/// R1：安装/连接拆分为独立路由——`auto_install_service` 已从 wire 移除，connect 只携带
/// `profile_ref` + `secret_payload`；安装服务由 UI 独立发 ServiceControl install 请求。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectIntent {
    pub profile_ref: String,
    #[serde(default)]
    pub credentials: Option<ConnectCredentials>,
    #[serde(default)]
    pub secret_payload: Option<String>,
}

/// 配置项（设置页骨架；core config 契约不在冻结 wire 上，P4-b 保留 seam）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigPayload {
    pub items: Vec<ConfigItem>,
    /// 本次 ConfigGet 是否因配置缺失、空白、非 object 或不可解析而 bootstrap 默认配置。
    /// 不携带原始配置、异常细节或任何秘密。
    pub requires_quick_start: bool,
    /// R-D3-C：当次启动损坏修复的一次性报告（core config_hygiene 损坏分支产出，
    /// 不持久化）；缺失文件 bootstrap 与合法启动为 `None`。仅布尔 + 备份文件名。
    #[serde(default)]
    pub config_recovery: Option<ConfigRecovery>,
    /// S4（2026-10）：config.json 读失败（三档自愈后仍不可读）的结构化报告；
    /// 读取成功（含自愈成功）为 `None`。读失败绝不触发 bootstrap/覆盖（禁令）。
    #[serde(default)]
    pub read_failure: Option<ConfigReadFailure>,
}

/// R-D3-C 损坏恢复报告的 UI 侧镜像（wire `ConfigRecovery` → serde → 前端）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigRecovery {
    pub corrupted: bool,
    pub backup_file: String,
}

/// S4 读失败报告的 UI 侧镜像（wire `ConfigReadFailure` → serde → 前端）。
/// 前端据此区分「损坏已恢复」（config_recovery）与「读不了」（本报告），
/// 并对 ACL 类失败给出「以管理员身份修复权限」runas 入口。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigReadFailure {
    /// Win32 原始错误码（5=拒绝访问、32=占用…；0=未知）。
    pub win32_code: i32,
    /// 读取失败的 config.json 全路径。
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigItem {
    pub key: String,
    pub value: String,
}

/// 与 core 的通道形态。P4-a 恒为 `NotWired`；P4-b 增加 `Dialed`。
#[derive(Debug, Clone)]
pub enum CoreHandle {
    /// 尚未建立到 core 的通道（core 未启动 / 拨号失败）。
    NotWired,
    /// 已拨号并验证 core server 的通道。
    Dialed { channel: Channel },
}

/// UI 对 Core 控制面的两态结论。日常探测只依据已验证管道上的 RPC 是否可通，
/// 不把 Windows 进程表当作健康来源；进程句柄只允许在用户点击连接后的恢复分支读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreStatus {
    Normal,
    Stopped,
}

impl Default for CoreHandle {
    fn default() -> Self {
        Self::NotWired
    }
}

impl CoreHandle {
    /// 已拨号通道的引用（`NotWired` → `None`）。
    #[must_use]
    pub fn channel(&self) -> Option<&Channel> {
        match self {
            Self::Dialed { channel, .. } => Some(channel),
            Self::NotWired => None,
        }
    }
}

/// Tauri managed state：UI 侧 core 会话状态 + 快照缓存。
#[derive(Debug, Default)]
pub struct CoreState {
    /// 当前经过身份验证的 Core 控制通道。Core 意外退出后，只有连接恢复分支可以替换它；
    /// 其余命令仅克隆读取，不会触发进程探测或拉起。
    pub handle: RwLock<CoreHandle>,
    /// 最近一次 RuntimeSnapshot 缓存（前端可即时渲染，无需等下一事件）。
    /// `RwLock`：命令层读，事件订阅 task 写。
    pub last_snapshot: RwLock<Option<RuntimeSnapshot>>,
    /// 最近一次归一化统计缓存（stats-wire 方案 A：由 `snapshot` 命令从快照携带的
    /// `stats` 写入；`stats` 命令读取）。`RwLock`：命令层读/写。
    pub last_stats: RwLock<Option<RuntimeStats>>,
    pub connection_requests: crate::kernel::connection_request::ConnectionRequests,
}

impl CoreState {
    /// 以给定 handle 构造（其余字段取默认）。集成测试（crate 外）无法用
    /// `..Default::default()` 字面量初始化（请求提交 gate 归 connection_request 管理），
    /// 经本构造绕过字段可见性（clippy 债务清偿 2026-09-27 顺带修复既有编译破损）。
    #[must_use]
    pub fn with_handle(handle: CoreHandle) -> Self {
        Self {
            handle: RwLock::new(handle),
            ..Self::default()
        }
    }

    /// 取得当前已验证通道的副本。`None` 即 UI 所见「Core 已停止」。
    #[must_use]
    pub fn channel(&self) -> Option<Channel> {
        self.handle
            .read()
            .ok()
            .and_then(|handle| handle.channel().cloned())
    }

    /// 原子替换为恢复后重新认证的通道。
    pub fn replace_handle(&self, handle: CoreHandle) {
        if let Ok(mut current) = self.handle.write() {
            *current = handle;
        }
    }

    /// 将 UI 的 Core 两态结论置为已停止（管道不可用）。
    pub fn mark_stopped(&self) {
        self.replace_handle(CoreHandle::NotWired);
    }

    /// 日常管道健康结论。这里不观察进程，仅执行既有轻量 snapshot RPC。
    pub async fn probe_status(&self) -> CoreStatus {
        match CoreClient.snapshot(self).await {
            Ok(_) => CoreStatus::Normal,
            Err(_) => CoreStatus::Stopped,
        }
    }

    /// 最近一次缓存快照（P4-b 接入点：snapshot 命令写入；P5-c UI 展示统计/即时渲染读取）。
    #[must_use]
    #[allow(dead_code)]
    pub fn cached_snapshot(&self) -> Option<RuntimeSnapshot> {
        self.last_snapshot.read().map(|g| g.clone()).unwrap_or(None)
    }

    /// 更新缓存快照。
    pub fn update_snapshot(&self, snapshot: RuntimeSnapshot) {
        if let Ok(mut g) = self.last_snapshot.write() {
            *g = Some(snapshot);
        }
    }

    /// 最近一次缓存统计（stats-wire 方案 A：`snapshot` 命令写入，`stats` 命令读取）。
    #[must_use]
    pub fn cached_stats(&self) -> Option<RuntimeStats> {
        self.last_stats.read().map(|g| g.clone()).unwrap_or(None)
    }

    /// 更新缓存统计（`snapshot` 命令从快照携带的 `stats` 写入）。
    pub fn update_stats(&self, stats: RuntimeStats) {
        if let Ok(mut g) = self.last_stats.write() {
            *g = Some(stats);
        }
    }
}

/// Tauri managed state：core 进程生命周期句柄（O3 强绑定）。
///
/// - `child`：core 子进程句柄（UI 宿主 spawn；Drop → 强制终止，core 不得遗留）。
/// - `subscriptions`：事件订阅 task 句柄（停机先中止，停止 emit）。
///
/// 由 [`super::bootstrap::bootstrap`] 在 setup 时管理；lifecycle 停机路径
/// （[`crate::lifecycle::notify_core_shutdown`]）读取。
#[derive(Default)]
pub struct CoreSession {
    /// core 子进程（`None` = 未拉起 / 已退出）。O3 生命周期句柄：core 由 pipe-close
    /// 感知自行退出（不做 Drop 强杀）；本句柄供挂死兜底（`terminate`）与观测。
    #[allow(dead_code)]
    pub child: std::sync::Mutex<Option<super::core_process::CoreChild>>,
    /// 事件订阅 task 句柄（WatchEvents 等；停机先中止）。用 tauri 自带 runtime 的
    /// JoinHandle（setup 主线程非 tokio 上下文，`tokio::spawn` 会 panic）。
    pub subscriptions: std::sync::Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    /// 连接恢复的串行闸门。并发点击连接只允许一个调用检查子进程/重拉 Core，避免
    /// 同一 UI PID 下竞争同一个控制管道。
    pub recovery: tokio::sync::Mutex<()>,
    /// T9（§2.9）：core 崩溃事件的边沿去重标志。`true` = 已向前端广播过「core 已崩溃」，
    /// 待恢复广播（重拉成功/管道复通）时复位——避免订阅循环的退避重试重复广播。
    pub crash_announced: std::sync::atomic::AtomicBool,
}

pub(super) fn encode_browser_candidate(session: &exv_vpn_win32_ipc::browser_session::BrowserSession, profile: &std::path::Path) -> Result<Vec<u8>, AppError> {
    #[derive(serde::Serialize)]
    struct Candidate<'a> {
        #[serde(flatten)] session: &'a exv_vpn_win32_ipc::browser_session::BrowserSession,
        authenticated_username: String,
    }
    serde_json::to_vec(&Candidate {
        authenticated_username: exv_core::atrust_session_cache::username_for(profile, session).unwrap_or_default(),
        session,
    }).map_err(|_| AppError::browser_login_failed("无法编码浏览器登录会话"))
}

/// aTrust 浏览器认证结果及其待提交到 Core 的 owner lease。
type BrowserAcquisition = (
    Vec<u8>,
    Option<crate::kernel::connection_request::ConnectionRequest>,
);

/// UI 侧对 core 的命令客户端（无状态 facade；真实会话状态在 CoreState）。
#[derive(Debug, Default)]
pub struct CoreClient;

/// 拨号并连接 core 控制面管道（P4-b：UI 宿主 spawn core 后调用）。
///
/// `expected_core_pid` 是 core 进程 pid（UI 侧 spawn 时获得）；`expected_user_sid`
/// 是当前用户 SID（core 与 UI 同用户）。core 侧验证 UI 的 pid+SID 由 server 完成
/// （host `verify_ui_peer`）。
///
/// # Errors
/// 拨号失败 → `AppError::CoreUnreachable`；身份不匹配 → `AppError::CoreUnreachable`；
/// channel 构建失败 → `AppError::CoreUnreachable`。
pub async fn dial_core(
    pipe_name: &str,
    expected_core_pid: u32,
    expected_user_sid: &str,
) -> Result<(Channel, CorePeer), AppError> {
    connect_core_channel(pipe_name, expected_core_pid, expected_user_sid)
        .await
        .map_err(pipe_error_to_app)
}

/// 把传输层错误映射为 UI 错误（全部归 `CoreUnreachable`——core 不可达）。
fn pipe_error_to_app(e: GrpcPipeError) -> AppError {
    AppError::CoreUnreachable(e.to_string())
}

/// R5 服务连接失败消息的稳定前缀（host `kernel_control_service` 路由失败时置于
/// `failed_precondition` 消息开头；前缀后为可读信息）。
const SERVICE_NOT_RUNNING_PREFIX: &str = "service_not_running|";
const SERVICE_START_FAILED_PREFIX: &str = "service_start_failed|";
const SERVICE_CONNECT_FAILED_PREFIX: &str = "service_connect_failed|";
const CREDENTIAL_REQUIRED_PREFIX: &str = "credential_required|";
const COMPATIBILITY_MODE_UNAVAILABLE_PREFIX: &str = "compatibility_mode_unavailable|";
/// T02 协议不可用的稳定前缀（host `ensure_selected_protocol_is_serviceable`）。
const PROTOCOL_UNAVAILABLE_PREFIX: &str = "protocol_unavailable|";

/// 把 tonic `Status` 映射为 UI 错误：transport 类 → `CoreUnreachable`；
/// 服务路由失败前缀 → typed `ServiceNotRunning` / `ServiceConnectFailed`（R5 modal
/// 触发）；其余 → `Internal`（携带稳定 code + 精简 message）。
fn map_status(s: Status) -> AppError {
    // 已返回的业务来源优先于通用传输分类，保留服务原操作阶段与原因。
    if let Some(rest) = s.message().strip_prefix(SERVICE_NOT_RUNNING_PREFIX)
        .or_else(|| s.message().strip_prefix(SERVICE_START_FAILED_PREFIX)) {
        return AppError::ServiceNotRunning(rest.trim().to_string());
    }
    if let Some(rest) = s.message().strip_prefix(SERVICE_CONNECT_FAILED_PREFIX) {
        return AppError::ServiceConnectFailed(rest.trim().to_string());
    }
    if s.code() == Code::DeadlineExceeded
        && ["auth_control/", "auth_receiver_control/", "atrust_auth_acquisition|"]
            .iter().any(|prefix| s.message().starts_with(prefix))
    {
        return AppError::BrowserLoginFailed {
            code: Some("auth_deadline_exceeded"),
            message: "本次认证总期限已耗尽，请重新连接。",
            server_detail: Some(exv_core::browser_auth::page_facts::sanitize_server_detail(s.message())),
        };
    }
    match s.code() {
        Code::Unavailable | Code::Cancelled | Code::Unknown | Code::DeadlineExceeded => {
            AppError::CoreUnreachable(format!("{}: {}", s.code(), s.message()))
        }
        _ => {
            let message = s.message();
            if let Some(rest) = message.strip_prefix(CREDENTIAL_REQUIRED_PREFIX) {
                let code = rest.trim();
                AppError::CredentialRequired {
                    code: (!code.is_empty()).then(|| code.to_string()),
                    message: "credential_required".to_string(),
                }
            } else if let Some(rest) = message.strip_prefix(SERVICE_NOT_RUNNING_PREFIX) {
                AppError::ServiceNotRunning(rest.trim().to_string())
            } else if let Some(rest) = message.strip_prefix(SERVICE_START_FAILED_PREFIX) {
                AppError::ServiceNotRunning(rest.trim().to_string())
            } else if let Some(rest) = message.strip_prefix(SERVICE_CONNECT_FAILED_PREFIX) {
                AppError::ServiceConnectFailed(rest.trim().to_string())
            } else if let Some(rest) = message.strip_prefix(COMPATIBILITY_MODE_UNAVAILABLE_PREFIX) {
                AppError::CompatibilityModeUnavailable(rest.trim().to_string())
            } else if let Some(rest) = message.strip_prefix(PROTOCOL_UNAVAILABLE_PREFIX) {
                AppError::ProtocolUnavailable(rest.trim().to_string())
            } else if let Some(rest) = message.strip_prefix("preflight_blocked|") {
                let reasons = rest.split('|').next().unwrap_or_default();
                AppError::PreflightBlocked(reasons.to_string())
            } else {
                AppError::Internal(format!("{}: {}", s.code(), s.message()))
            }
        }
    }
}

/// R4（ui-connect-stop-responsiveness 复审 P1-1）：把壳本地生成的 16 字节
/// operation_id 以小写 hex 回注 UI `OperationReply`。
///
/// 硬契约（C1 三方 id 锚的壳侧一环）：host wire `OperationReply` **没有**
/// operation_id 字段（common.proto:430-433 仅 `terminal`）——前端看到的回复 id
/// 完全由壳在本地生成并回注；host 受理快照/状态事件携带同一 id
/// （`OperationLookupKey.operation_id`），前端据此关联「当前用户操作」。
/// connect 与 stop 两处回注共用本实现，壳层单测钉死（前端 mock gateway
/// 覆盖不到的测试空洞）。
fn ui_operation_reply_with_local_id(
    reply: wire::OperationReply,
    operation_id: &[u8],
) -> OperationReply {
    let mut ui = wire_map::operation_reply_from_wire(&reply);
    ui.operation_id = Some(wire_map::hex(operation_id));
    ui
}

/// 活动 Owned/HTTP 驱动只把显式系统回退交给此处；期限、代际和控制沿用原尝试。
pub(super) async fn acquire_with_system_fallback<'a, P, S, SFut>(
    primary: P,
    generation: u64,
    action: &'a mut tokio::sync::watch::Receiver<exv_core::atrust_browser::LoginAction>,
    attempt: &'a exv_core::browser_auth::AuthAttempt,
    system: S,
) -> Result<
    exv_vpn_win32_ipc::browser_session::BrowserSession,
    exv_core::atrust_browser::BrowserAuthError,
>
where
    P: std::future::Future<
            Output = Result<
                exv_vpn_win32_ipc::browser_session::BrowserSession,
                exv_core::atrust_browser::BrowserAuthError,
            >,
        >,
    S: FnOnce(
        u64,
        &'a mut tokio::sync::watch::Receiver<exv_core::atrust_browser::LoginAction>,
        &'a exv_core::browser_auth::AuthAttempt,
    ) -> SFut,
    SFut: std::future::Future<
            Output = Result<
                exv_vpn_win32_ipc::browser_session::BrowserSession,
                exv_core::atrust_browser::BrowserAuthError,
            >,
        >,
{
    use exv_core::atrust_browser::BrowserAuthError;
    use exv_core::browser_auth::{AuthControl, AuthErrorCode};
    match primary.await {
        Err(BrowserAuthError::Control {
            control: AuthControl::UseSystemBrowser,
            ..
        }) => system(generation, action, attempt)
            .await
            .map_err(|mut error| {
                if let BrowserAuthError::Auth {
                    error: auth,
                    compatibility_message,
                } = &mut error
                    && auth.code == AuthErrorCode::AcquisitionTimeout
                {
                    *compatibility_message = compatibility_message.replacen(
                        "浏览器登录超时，请重试",
                        "内嵌登录超时，请重试",
                        1,
                    );
                }
                error
            }),
        result => result,
    }
}

/// 普通连接与 Core 认证续程共用原 owner、渠道、凭据与一次预算。
pub(super) async fn acquire_browser_once<F: Fn(bool, u64)>(
    requests: &super::connection_request::ConnectionRequests,
    app: tauri::AppHandle,
    server: String,
    mode: String,
    lease: super::connection_request::ConnectionRequest,
    school_auto_login: Option<exv_core::school_auto_login::SchoolAutoLogin>,
    automatic: bool,
    silent: bool,
    attempt: exv_core::browser_auth::AuthAttempt,
    profiles: exv_core::browser_cache_profile::BrowserDataDirs,
    status: F,
) -> (
    Result<
        exv_vpn_win32_ipc::browser_session::BrowserSession,
        exv_core::atrust_browser::BrowserAuthError,
    >,
    super::connection_request::ConnectionRequest,
) {
    let auth_intent = exv_core::browser_auth::AuthIntent::from_auto_login(automatic);
    let generation = lease.generation();
    let success_app = app.clone();
    let interaction = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let success_interaction = interaction.clone();
    let acquired = requests
        .acquire_outcome_with_lease(
            lease,
            move |active| status(active, generation),
            move |generation| {
                if silent { return; }
                super::commands::emit_browser_authenticated(&success_app, generation);
                if success_interaction.load(std::sync::atomic::Ordering::Acquire) {
                    crate::lifecycle::show_main_window(&success_app);
                }
            },
            move |mut action| async move {
                let mut selected = mode;
                let mut selected_intent = auth_intent;
                let mut credentials = school_auto_login;
                loop {
                    let result = if matches!(selected.as_str(), "headless" | "embedded") {
                        super::embedded_auth::acquire_channels(
                            &app, &server, generation, action.clone(), credentials.as_ref(), requests,
                            &attempt, true, selected_intent, interaction.clone(), Some(&profiles), silent,
                        ).await
                    } else {
                        super::system_auth::acquire(
                            &app, &server, profiles.cache_profile(), generation, &mut action, &attempt,
                            selected_intent, interaction.clone(),
                        ).await
                    };
                    match result {
                        Err(exv_core::atrust_browser::BrowserAuthError::Control { control: exv_core::browser_auth::AuthControl::UseSystemBrowser, .. }) if !silent => {
                            selected = "browser".into();
                            super::commands::emit_browser_channel_selected(&app, generation, &selected, false);
                        }
                        Err(exv_core::atrust_browser::BrowserAuthError::FallBackEmbedded) if !silent => {
                            let saved_requested = requests.auth_preparation_actions(generation).ok()
                                .is_some_and(|actions| *actions.borrow() == Some(super::connection_request::AuthPreparationAction::TrySavedLogin));
                            if saved_requested {
                                credentials = school_auto_login_saved(&server, true, &exv_vpn_win32_config::config_dir());
                                if credentials.is_none() { return Err(exv_core::atrust_browser::BrowserAuthError::Host("未保存完整的学校用户名和密码".into())); }
                            }
                            selected_intent = exv_core::browser_auth::AuthIntent::from_auto_login(saved_requested);
                            selected = if saved_requested { "headless" } else { "embedded" }.into();
                            super::commands::emit_browser_channel_selected(&app, generation, &selected, saved_requested);
                        }
                        result => return result,
                    }
                }
            },
        )
        .await;
    acquired
}

impl CoreClient {
    pub(super) async fn prepare_auth_control(&self, state: &CoreState, request: wire::PrepareAuthControlRequest) -> Result<(), AppError> {
        let deadline = exv_vpn_win32_ipc::auth_deadline::AuthDeadline::from_monotonic_ns(request.deadline_monotonic_ns)
            .and_then(|deadline| deadline.tokio_deadline())
            .map_err(|_| AppError::browser_login_failed("认证准备已超过本次截止时间"))?;
        let channel = state.channel().ok_or_else(|| AppError::CoreUnreachable("auth control: core not dialed".into()))?;
        let mut client = KernelControlClient::new(channel);
        tokio::time::timeout_at(deadline, client.prepare_auth_control(request)).await
            .map_err(|_| AppError::browser_login_failed("准备本地认证控制会话超过本次截止时间"))?
            .map_err(map_status)?;
        Ok(())
    }

    pub(super) async fn auth_receiver_control(&self, state: &CoreState, request: wire::AuthReceiverControlRequest) -> Result<wire::AuthReceiverControlReply, AppError> {
        let channel = state.channel().ok_or_else(|| AppError::CoreUnreachable("auth receiver: core not dialed".into()))?;
        KernelControlClient::new(channel).auth_receiver_control(request).await.map(tonic::Response::into_inner).map_err(map_status)
    }

    /// 通用 Core/手动驱动接线：非 aTrust 保持原 wire；aTrust 使用纯手动系统浏览器。
    /// 正式 Tauri 命令通过 connect_with_auth_status 选择配置的认证通道。
    pub async fn connect_with_system_browser(
        &self,
        state: &CoreState,
        intent: ConnectIntent,
    ) -> Result<OperationReply, AppError> {
        self.connect_with_browser_mode(
            state,
            intent,
            |server, _mode, lease, _school_auto_login, automatic, attempt| async move {
                let (session, lease) = state
                    .connection_requests
                    .acquire_with_typed_lease_held(
                        lease,
                        |_| {},
                        move |mut action| async move {
                            let profile = super::browser_cache_profile::browser_data_dirs()?
                                .cache_profile()
                                .to_path_buf();
                            // §7.10-3 渠道边界：系统浏览器是纯手动渠道，不承担自动填交。
                            tokio::time::timeout_at(
                                attempt.deadline(std::time::Duration::from_secs(300)),
                                exv_core::atrust_browser::acquire_typed_with_intent(
                                    &server, &profile, &mut action, None,
                                    exv_core::browser_auth::AuthIntent::from_auto_login(automatic),
                                ),
                            ).await.map_err(|_| exv_core::atrust_browser::BrowserAuthError::Host("认证已超过本次截止时间".into()))?
                        },
                    )
                    .await
                    .map_err(AppError::browser_auth_failed)?;
                let cache_profile = super::browser_cache_profile::browser_data_dirs().map_err(AppError::browser_login_failed)?.cache_profile().to_path_buf();
                let payload = encode_browser_candidate(&session, &cache_profile)?;
                Ok((payload, Some(lease)))
            },
        )
        .await
    }

    /// 按当前配置的 aTrust 登录模式选择受控 Chrome 或应用管理的独立 WebView。
    pub async fn connect_with_auth_status<F: Fn(bool, u64)>(
        &self,
        state: &CoreState,
        intent: ConnectIntent,
        app: tauri::AppHandle,
        status: F,
    ) -> Result<OperationReply, AppError> {
        self.connect_with_browser_mode(
            state,
            intent,
            |server, mode, lease, school_auto_login, automatic, attempt| async move {
                let profiles = super::browser_cache_profile::browser_data_dirs()
                    .map_err(AppError::browser_login_failed)?;
                let cache_profile = profiles.cache_profile().to_path_buf();
                let (session, lease) = acquire_browser_once(
                    &state.connection_requests, app, server, mode, lease, school_auto_login,
                    automatic, false, attempt, profiles, status,
                ).await;
                let session = session.map_err(AppError::browser_auth_failed)?;
                let payload = encode_browser_candidate(&session, &cache_profile)?;
                Ok((payload, Some(lease)))
            },
        )
        .await
    }

    pub(super) async fn connect_with_browser_mode<F, Fut>(
        &self,
        state: &CoreState,
        mut intent: ConnectIntent,
        acquire: F,
    ) -> Result<OperationReply, AppError>
    where
        F: FnOnce(
            String,
            String,
            crate::kernel::connection_request::ConnectionRequest,
            Option<exv_core::school_auto_login::SchoolAutoLogin>,
            bool,
            exv_core::browser_auth::AuthAttempt,
        ) -> Fut,
        Fut: std::future::Future<Output = Result<BrowserAcquisition, AppError>>,
    {
        // 只记录固定阶段；不格式化 intent、Cookie、令牌或服务端响应。
        log::info!(target: "exv.connect", "connect.request_started");
        if let Some(mut finished) = state.connection_requests.auth_acquisitions.clear(&state.connection_requests) {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !*finished.borrow() {
                    if finished.changed().await.is_err() { break; }
                }
            }).await.map_err(|_| AppError::browser_login_failed("取消浏览器登录超时；当前认证窗口仍在关闭，请稍后重试"))?;
        }
        let lease = state
            .connection_requests
            .begin(false)
            .map_err(AppError::browser_login_failed)?;
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("connect: core not dialed".to_string()))?;
        let config = tokio::select! {
            biased;
            () = lease.cancelled() => return Err(AppError::browser_login_failed("浏览器登录已取消")),
            result = self.config_get(state) => result?,
        };
        log::info!(target: "exv.connect", "connect.config_received");
        let atrust = config
            .items
            .iter()
            .any(|item| item.key == "protocol" && item.value == "atrust");
        let operation_id = uuid::Uuid::new_v4().as_bytes().to_vec();
        // 同一个绝对期限覆盖 Engine 准备、全部认证渠道和最终 Core 提交。
        let auth_deadline = if atrust {
            Some(exv_vpn_win32_ipc::auth_deadline::AuthDeadline::after(std::time::Duration::from_secs(300))
                .map_err(|_| AppError::browser_login_failed("创建本次认证期限失败"))?)
        } else { None };
        let mut browser_lease = Some(lease);
        let mut auth_binding = None;
        let browser_payload = if atrust {
            let server = config
                .items
                .iter()
                .find(|item| item.key == "server")
                .map(|item| item.value.clone())
                .ok_or_else(|| AppError::Internal("aTrust 服务器尚未配置".into()))?;
            // Core 归一模式决定本次渠道，兼容字段不再覆盖。
            let (mode, _) = resolved_atrust_auth_mode(&config)?;
            let mode = mode.to_owned();
            auth_binding = Some(wire::AuthBinding {
                protocol: "atrust".into(),
                gateway: exv_core::atrust_browser::validated_gateway(&server)
                    .map_err(AppError::browser_login_failed)?,
                mode: mode.clone(),
            });
            // T4c（2026-09-30 用户授权）：aTrust 自动登录读取与 Cisco 完全共享的单对
            // 凭据（本次显式输入优先，其次本机已保存对），只用于学校登录页一次填交；
            // 发往 Core 的连接请求仍不携带任何密码——浏览器会话是唯一认证材料。
            let school_auto_login = school_auto_login_attempt(&config, &intent);
            // 冻结授权和本次凭据来源；缺材料只在复用会话耗尽、确需填交时返回既有入口。
            let automatic = mode == "headless";
            intent.credentials = None;
            intent.secret_payload = None;
            let lease = browser_lease.take().expect("本请求持有唯一 lease");
            lease
                .set_auth_mode(matches!(mode.as_str(), "embedded" | "headless"))
                .map_err(AppError::browser_login_failed)?;
            let deadline = auth_deadline.expect("aTrust request has one deadline");
            lease.set_auth_control_deadline(deadline.monotonic_ns());
            let local_deadline = deadline.tokio_deadline().map_err(|_| AppError::browser_login_failed("认证准备已超过本次截止时间"))?;
            tokio::select! {
                biased;
                () = lease.cancelled() => return Err(AppError::browser_login_failed("浏览器登录已取消")),
                result = self.prepare_auth_control(state, wire::PrepareAuthControlRequest {
                    operation_id: operation_id.clone(), deadline_monotonic_ns: deadline.monotonic_ns(),
                }) => result?,
            }
            log::info!(target: "exv.connect", "connect.browser_wait_started");
            let candidate = acquire(server, mode, lease, school_auto_login, automatic,
                exv_core::browser_auth::AuthAttempt::with_deadline(local_deadline)).await;
            if candidate.is_err() {
                log::warn!(target: "exv.connect", "connect.browser_wait_failed");
            }
            let (payload, lease) = candidate?;
            browser_lease =
                Some(lease.ok_or_else(|| AppError::browser_login_failed("连接请求交接已失效"))?);
            log::info!(target: "exv.connect", "connect.browser_payload_received");
            Some(payload)
        } else {
            None
        };
        let mut body = wire_map::connect_request(&intent, operation_id.clone())?;
        if let Some(payload) = browser_payload {
            body.secret_payload = payload;
        }
        body.auth_binding = auth_binding;
        let mut request = Request::new(body);
        if let Some(deadline) = auth_deadline {
            deadline.remaining().map_err(|_| AppError::browser_login_failed("认证已超过本次截止时间"))?;
            request.metadata_mut().insert("x-exv-auth-deadline-monotonic-ns", deadline.monotonic_ns().to_string().parse()
                .map_err(|_| AppError::Internal("编码本次认证期限失败".into()))?);
        }
        // `persist` 是 host 配置写入的非秘密选择；engine 只接收固定的
        // `{version,username,password}` payload，绝不看到此元数据。
        if intent
            .credentials
            .as_ref()
            .is_some_and(|credentials| credentials.persist)
        {
            request.metadata_mut().insert(
                "x-exv-persist-credentials",
                tonic::metadata::MetadataValue::from_static("true"),
            );
        }
        let browser_commit_guard = if let Some(lease) = browser_lease.as_ref() {
            let guard = tokio::select! {
                biased;
                () = lease.cancelled() => return Err(AppError::browser_login_failed("浏览器登录已取消")),
                guard = state.connection_requests.commit_gate.clone().lock_owned() => guard,
            };
            if !lease.begin_core_connect() {
                return Err(AppError::browser_login_failed("浏览器登录已取消"));
            }
            Some(guard)
        } else {
            None
        };
        // RPC 一旦取得提交权就独立持有 lease/gate，调用方 drop 不代表远端撤销。
        // 不建立队列；Stop 沿用五秒有界等待，超时只报告尚未确认停止。
        tokio::spawn(async move {
            let _guard = browser_commit_guard;
            let _lease = browser_lease;
            let mut client = KernelControlClient::new(channel);
            log::info!(target: "exv.connect", "connect.core_rpc_started");
            let reply = client.connect(request).await.map_err(map_status)?;
            log::info!(target: "exv.connect", "connect.core_rpc_returned");
            Ok(ui_operation_reply_with_local_id(
                reply.into_inner(),
                &operation_id,
            ))
        })
        .await
        .map_err(|_| AppError::Internal("连接请求执行任务异常结束；请断开后重试".into()))?
    }

    /// 停止连接（P4-b: KernelControl.Stop）。operation_id 关联契约同 connect。
    pub async fn stop(&self, state: &CoreState) -> Result<OperationReply, AppError> {
        self.stop_with_deadline(state, tokio::time::Instant::now() + STOP_TOTAL_BUDGET).await
    }

    pub(super) async fn stop_with_deadline(&self, state: &CoreState, deadline: tokio::time::Instant) -> Result<OperationReply, AppError> {
        let deadline = deadline.min(tokio::time::Instant::now() + STOP_TOTAL_BUDGET);
        let recovery_auth_owner = state.connection_requests.has_auth_acquisition_owner();
        if recovery_auth_owner { let _ = state.connection_requests.auth_acquisitions.clear(&state.connection_requests); }
        let cancelled_browser_login = state.connection_requests.cancel_with_completion();
        if let Some(mut finished) = cancelled_browser_login {
            let closed = tokio::time::timeout_at(deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(5)), async {
                loop {
                    if *finished.borrow() { return true; }
                    if finished.changed().await.is_err() { return *finished.borrow(); }
                }
            }).await.unwrap_or(false);
            if !closed { return Err(AppError::browser_login_failed("取消浏览器登录超时；当前认证窗口仍在关闭，请稍后重试")); }
            if !recovery_auth_owner {
                return Ok(OperationReply { stop_completion: None, result: OperationResult::Succeeded { effect_id: None, authority_epoch: None }, operation_id: None });
            }
        }
        let gate = std::sync::Arc::clone(&state.connection_requests.commit_gate);
        let permit = match tokio::time::timeout_at(deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(5)), gate.lock_owned()).await {
            Ok(permit) if tokio::time::Instant::now() < deadline => permit,
            _ => return Ok(stop_unconfirmed_reply(None, "commit_gate")),
        };
        let channel = state.channel().ok_or_else(|| AppError::CoreUnreachable("stop: core not dialed".into()))?;
        let operation_id = uuid::Uuid::new_v4().as_bytes().to_vec();
        let mut request = Request::new(wire_map::stop_request(operation_id.clone())?);
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() { return Ok(stop_unconfirmed_reply(None, "before_dispatch")); }
        request.set_timeout(remaining);
        let reply_id = operation_id.clone();
        // 用户不再等待不会撤销已提交 Stop 或释放原提交许可。
        let mut task = tokio::spawn(async move {
            let _permit = permit;
            let mut client = KernelControlClient::new(channel);
            match client.stop(request).await {
                Ok(reply) => Ok(ui_operation_reply_with_local_id(reply.into_inner(), &reply_id)),
                Err(status) if status.code() == Code::DeadlineExceeded || status.code() == Code::Cancelled =>
                    Ok(stop_unconfirmed_reply(Some(&reply_id), &format!("rpc: {status}"))),
                Err(status) => Err(map_status(status)),
            }
        });
        match tokio::time::timeout_at(deadline, &mut task).await {
            Ok(result) => result.map_err(|_| AppError::Internal("停止控制任务异常退出，停止未确认。".into()))?,
            Err(_) => Ok(stop_unconfirmed_reply(Some(&operation_id), "completion")),
        }
    }

    /// 拉取当前运行时快照（P4-b: KernelControl.GetSnapshot），并更新缓存。
    ///
    /// stats-wire 方案 A：快照携带最新统计（`GetSnapshot` 从 EventBus 统计 lane
    /// 附加）——`snapshot` 同时更新 `last_snapshot` 与 `last_stats` 缓存。
    pub async fn snapshot(&self, state: &CoreState) -> Result<RuntimeSnapshot, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("snapshot: core not dialed".to_string()))?;
        let mut client = KernelControlClient::new(channel);
        let reply = match client
            .get_snapshot(wire::SnapshotRequest::default())
            .await
            .map_err(map_status)
        {
            Ok(reply) => reply,
            Err(error) => {
                if matches!(error, AppError::CoreUnreachable(_)) {
                    state.mark_stopped();
                }
                return Err(error);
            }
        };
        let ui = wire_map::snapshot_from_wire(&reply.into_inner(), 0);
        state.update_snapshot(ui.clone());
        if let Some(stats) = ui.stats {
            state.update_stats(stats);
        }
        Ok(ui)
    }

    /// 查询先前操作 disposition（P4-b: KernelControl.GetOperation）。
    /// P4-3 重试 UI（Reconcile）接入后启用——当前命令层未暴露。
    #[allow(dead_code)]
    pub async fn get_operation(
        &self,
        state: &CoreState,
        key: wire::OperationLookupKey,
    ) -> Result<Option<wire::OperationState>, AppError> {
        let channel = state.channel().ok_or_else(|| {
            AppError::CoreUnreachable("get_operation: core not dialed".to_string())
        })?;
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .get_operation(wire::GetKernelOperationRequest { key: Some(key) })
            .await
            .map_err(map_status)?;
        Ok(reply.into_inner().state)
    }

    /// 拉取日志历史分片（KernelControl.LogsList 真实接线）。
    pub(crate) async fn record_auth_diagnostic(&self, state: &CoreState, event: wire::LogEvent) -> Result<bool, AppError> {
        let channel = state.channel().ok_or_else(|| AppError::CoreUnreachable("auth diagnostic: core not dialed".into()))?;
        KernelControlClient::new(channel).record_auth_diagnostic(event).await
            .map(|reply| reply.into_inner().recorded).map_err(map_status)
    }

    pub async fn logs_list(
        &self,
        state: &CoreState,
        after_seq: u64,
        limit: u32,
    ) -> Result<LogChunk, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("logs_list: core not dialed".to_string()))?;
        let request = wire::LogsListRequest {
            after_seq,
            limit,
            filter: String::new(),
        };
        let mut client = KernelControlClient::new(channel);
        let reply = client.logs_list(request).await.map_err(map_status)?;
        let inner = reply.into_inner();
        let effective_limit = if limit == 0 { 100 } else { limit as usize };
        let events = inner
            .entries
            .iter()
            .map(wire_map::log_event_from_wire)
            .collect::<Vec<_>>();
        let has_more = !inner.entries.is_empty() && inner.entries.len() >= effective_limit;
        // W1-B（P9）：core 的增量过滤是严格 `seq > after_seq`，且 `next_seq` =
        // 末条 seq + 1——把 `next_seq` 原样回传会让前端下一轮恰漏 seq ==
        // next_seq 的那条（每轮游标推进漏一条）。翻译成 last-seen 游标
        // （末条 seq），钳制不回退（对齐 `LogChunk::next_after_seq` 既有文档
        // 「本分片末条的事件序号」）。
        let next_after_seq = inner.next_seq.saturating_sub(1).max(after_seq);
        Ok(LogChunk {
            events,
            next_after_seq,
            has_more,
        })
    }

    /// 清空 core 的持久化日志（KernelControl.LogsClear）。
    pub async fn logs_clear(&self, state: &CoreState) -> Result<LogsClearReply, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("logs_clear: core not dialed".to_string()))?;
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .logs_clear(wire::LogsClearRequest {})
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(LogsClearReply {
            cleared: reply.cleared,
            removed_entries: reply.removed_entries,
        })
    }

    /// 读取配置（KernelControl.ConfigGet 真实接线）。
    pub async fn config_get(&self, state: &CoreState) -> Result<ConfigPayload, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("config_get: core not dialed".to_string()))?;
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .config_get(wire::ConfigGetRequest {})
            .await
            .map_err(map_status)?;
        let inner = reply.into_inner();
        Ok(ConfigPayload {
            items: inner
                .items
                .into_iter()
                .map(|i| ConfigItem {
                    key: i.key,
                    value: i.value,
                })
                .collect(),
            requires_quick_start: inner.requires_quick_start,
            config_recovery: inner.config_recovery.map(|r| ConfigRecovery {
                corrupted: r.corrupted,
                backup_file: r.backup_file,
            }),
            read_failure: inner.read_failure.map(|f| ConfigReadFailure {
                win32_code: f.win32_code,
                path: f.path,
            }),
        })
    }

    /// 写入配置（KernelControl.ConfigSet 真实接线）。
    pub async fn config_set(
        &self,
        state: &CoreState,
        items: Vec<ConfigItem>,
    ) -> Result<bool, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("config_set: core not dialed".to_string()))?;
        let request = wire::ConfigSetRequest {
            items: items
                .into_iter()
                .map(|i| wire::ConfigItem {
                    key: i.key,
                    value: i.value,
                })
                .collect(),
        };
        let mut client = KernelControlClient::new(channel);
        let reply = client.config_set(request).await.map_err(map_status)?;
        Ok(reply.into_inner().ok)
    }

    /// 拉取当前归一化统计（stats-wire 方案 A）。
    ///
    /// 统计不再走独立 RPC：`RuntimeSnapshot` 携带统计（`GetSnapshot`/`WatchEvents`
    /// 一并下发），本命令从 `snapshot` 命令维护的 `last_stats` 缓存读取——最小改动的
    /// unary 拉取路径。尚无样本（未连接/无快照）→ `NotWired` 占位错误（前端
    /// 保持「暂无统计」占位，不视为失败）。
    pub async fn stats(&self, state: &CoreState) -> Result<RuntimeStats, AppError> {
        state.cached_stats().ok_or_else(|| {
            AppError::NotWired(
                "stats: no statistics yet (snapshot carries none; connect to start the data plane)"
                    .into(),
            )
        })
    }

    /// 应答交互提示（P4-b: KernelControl.RespondInteraction）。
    pub async fn respond_interaction(
        &self,
        state: &CoreState,
        interaction_id: Vec<u8>,
        response_payload: Vec<u8>,
    ) -> Result<OperationReply, AppError> {
        let channel = state.channel().ok_or_else(|| {
            AppError::CoreUnreachable("respond_interaction: core not dialed".to_string())
        })?;
        let request = wire_map::interaction_response(interaction_id, response_payload);
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .respond_interaction(request)
            .await
            .map_err(map_status)?;
        Ok(wire_map::operation_reply_from_wire(&reply.into_inner()))
    }

    /// 执行服务控制（S3/D5: `KernelControl.ServiceControl`）。query 非提权读；
    /// install/uninstall/start 变更 action 由 host 经 engine 子命令 runas 提权 seam
    /// 执行（D4——host 非提权，SCM 操作在 engine 内）。
    pub async fn service_control(
        &self,
        state: &CoreState,
        action: ServiceControlAction,
    ) -> Result<ServiceControlReply, AppError> {
        let channel = state.channel().ok_or_else(|| {
            AppError::CoreUnreachable("service_control: core not dialed".to_string())
        })?;
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .service_control(wire_map::service_control_request(action))
            .await
            .map_err(map_status)?;
        Ok(wire_map::service_control_reply_from_wire(
            &reply.into_inner(),
        ))
    }

    /// 打开 `KernelControl.WatchEvents` server-streaming 订阅（P4-b 事件订阅源）。
    ///
    /// `resume_tick == 0` 表示从当前快照开始。返回的流由事件订阅 task 驱动
    /// （`stream.next()` → `app.emit`）。流 EOF = core 断开。
    ///
    /// # Errors
    /// RPC 拒绝 / 掉线 → 相应 `AppError`（channel 由调用方保证已拨号）。
    pub async fn open_watch_stream(
        &self,
        channel: &Channel,
        resume_tick: u64,
    ) -> Result<tonic::Streaming<wire::RuntimeEvent>, AppError> {
        let mut client = KernelControlClient::new(channel.clone());
        let reply = client
            .watch_events(wire::WatchEventsRequest { resume_tick })
            .await
            .map_err(map_status)?;
        Ok(reply.into_inner())
    }
}

fn configured_atrust_auth_mode(config: &ConfigPayload) -> Result<&str, AppError> {
    let mode = config
        .items
        .iter()
        .find(|item| item.key == "atrust_auth_mode")
        .map(|item| item.value.as_str())
        .unwrap_or("browser");
    // S7 无头认证：新合法值 "headless"（独立新线，见 headless_auth 模块）。
    if matches!(mode, "browser" | "embedded" | "headless") {
        Ok(mode)
    } else {
        Err(AppError::Internal("aTrust 登录方式配置无效".into()))
    }
}

/// Core 已完成旧配置迁移；渠道只消费归一后的模式。
fn resolved_atrust_auth_mode(config: &ConfigPayload) -> Result<(&str, bool), AppError> {
    Ok((configured_atrust_auth_mode(config)?, false))
}

/// T4c（2026-09-30 用户授权）：在 `atrust_auto_login` 开启时构建单次学校自动填交
/// 尝试。凭据来源是与 Cisco 完全共享的单对账密——本次连接显式输入优先，没有显式
/// 输入时读取本机已保存对（`username` + `remember_password` 封存的 `password`）。
/// 任一来源缺失都返回 `None`；自动登录开启时调用方（C2 缺凭据前置门）据此以
/// `credential_required` 拒绝连接并弹统一凭据模态，而非回落人工登录。
/// 不记录任何凭据内容。
fn school_auto_login_attempt(
    config: &ConfigPayload,
    intent: &ConnectIntent,
) -> Option<exv_core::school_auto_login::SchoolAutoLogin> {
    let value = |name: &str| {
        config
            .items
            .iter()
            .find(|item| item.key == name)
            .map(|item| item.value.as_str())
    };
    if value("atrust_auth_mode") != Some("headless") {
        return None;
    }
    let server = value("server")?.to_owned();
    if let Some(credentials) = intent.credentials.as_ref()
        && !credentials.username.trim().is_empty()
        && !credentials.password.is_empty()
    {
        return exv_core::school_auto_login::SchoolAutoLogin::new(
            &server,
            credentials.username.trim().to_owned(),
            credentials.password.clone(),
        )
        .ok();
    }
    school_auto_login_saved(&server, true, &exv_vpn_win32_config::config_dir())
}

/// 恢复仅沿已冻结授权、网关和原凭据目录加载；不重读最新设置。
pub(super) fn school_auto_login_saved(
    server: &str,
    automatic: bool,
    config_dir: &std::path::Path,
) -> Option<exv_core::school_auto_login::SchoolAutoLogin> {
    if !automatic {
        return None;
    }
    let bundle =
        exv_core::credential::load_credentials(config_dir, exv_vpn_win32_config::Protocol::Atrust)
            .ok()?;
    let username = bundle.config.username.trim().to_owned();
    if username.is_empty() {
        return None;
    }
    let password = bundle.password.as_str().to_owned();
    exv_core::school_auto_login::SchoolAutoLogin::new(&server, username, password).ok()
}

