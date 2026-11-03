//! Tauri Command 定义（P4-b：真实接线，经 CoreClient 接缝）。
//!
//! 映射（计划 §1）：Command ↔ unary。
//!   connect / stop / snapshot / config.get / config.set / logs.list / logs.clear /
//!   respond_interaction
//!
//! P4-b：CoreClient 方法体已替换为真实 gRPC 调用（命令签名不变，仅 async 化）；
//! `logs_list`/`logs_clear`/`config_get`/`config_set` 已真实接线（KernelControl RPC），
//! `stats` 从 snapshot 维护的缓存读取（尚无样本时返回 typed NotWired 占位）。

use tauri::{Emitter, Manager, State};

use super::client::{ConfigItem, ConnectIntent, CoreClient, CoreSession, CoreState, CoreStatus};
use super::error::AppError;
use super::login_attempt_tracker::{LoginAttemptTracker, LoginThrottleWarning};
use super::logs::{LogChunk, LogsClearReply};
use super::state::{OperationReply, RuntimeSnapshot};
use super::stats::RuntimeStats;

#[derive(Clone, serde::Serialize)]
struct BrowserLoginStatus {
    active: bool,
    generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
    /// §7.9：认证阶段（可选）。仅在既有 `exv://browser-login` 通道上做最小增量，
    /// 不新增事件通道；`None` 时字段不序列化，保持旧载荷形状。
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<&'static str>,
    /// 自动恢复轮次；仅恢复事件携带，原阶段载荷保持兼容。
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<u8>,
    /// 登录频次预告（可选，2026-10-07 验证码风险预告）：滚动窗口内已记录 >= 阈值次
    /// 学校表单登录时，随本次认证的**首个** active=true 事件下发一次；前端在认证
    /// 等待模态内渲染警示句。`None` 不序列化，旧载荷形状不变。
    #[serde(skip_serializing_if = "Option::is_none")]
    throttle_warning: Option<LoginThrottleWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<BrowserLoginRecovery>,
}

#[derive(Clone, serde::Serialize)]
struct BrowserLoginRecovery {
    mode: String,
    auto_login: bool,
}

/// §7.9：在既有 `exv://browser-login` 事件上回传认证阶段（最小增量）。
///
/// `active` 恒为 true（阶段回传不结束等待），代际沿用发起本次认证的 owner 代际，
/// 前端据 `stage` 显示自动登录实时阶段；不新增独立事件通道或跨进程状态机。
pub(crate) fn emit_browser_login_stage(app: &tauri::AppHandle, generation: u64, stage: &'static str) {
    let _ = app.emit(
        "exv://browser-login",
        BrowserLoginStatus {
            active: true,
            generation,
            outcome: None,
            stage: Some(stage),
            round: (stage == "recovering").then_some(2),
            throttle_warning: None,
            recovery: None,
        },
    );
}

pub(super) fn emit_browser_authenticated(app: &tauri::AppHandle, generation: u64) {
    let _ = app.emit("exv://browser-login", BrowserLoginStatus {
        active: false, generation, outcome: Some("authenticated"), stage: None, round: None, throttle_warning: None, recovery: None,
    });
}

/// 原认证等待事件供普通 Connect 与 Core 认证续程共同使用。
pub(super) fn emit_browser_login_status(app: &tauri::AppHandle, active: bool, generation: u64) {
    let throttle_warning = if active {
        login_throttle_warning_now(app)
    } else {
        None
    };
    let _ = app.emit(
        "exv://browser-login",
        BrowserLoginStatus {
            active,
            generation,
            outcome: None,
            stage: None,
            round: None,
            throttle_warning,
            recovery: None,
        },
    );
}
/// 后台原 owner 首事件携带冻结显示上下文；后续里程碑继续原 generation。
pub(super) fn emit_browser_recovery_started(
    app: &tauri::AppHandle,
    generation: u64,
    mode: &str,
    auto_login: bool,
) {
    let _ = app.emit(
        "exv://browser-login",
        BrowserLoginStatus {
            active: true,
            generation,
            outcome: None,
            stage: Some("opening"),
            round: None,
            throttle_warning: login_throttle_warning_now(app),
            recovery: Some(BrowserLoginRecovery {
                mode: mode.to_owned(),
                auto_login,
            }),
        },
    );
}

/// 本次明确选择渠道，保留同代等待并更新实际认证方式。
pub(super) fn emit_browser_channel_selected(
    app: &tauri::AppHandle,
    generation: u64,
    mode: &str,
    auto_login: bool,
) {
    #[derive(Clone, serde::Serialize)]
    struct SelectedChannel {
        #[serde(flatten)]
        status: BrowserLoginStatus,
        channel_override: bool,
    }
    let _ = app.emit("exv://browser-login", SelectedChannel {
        status: BrowserLoginStatus {
            active: true, generation, outcome: None, stage: Some("opening"), round: None,
            throttle_warning: login_throttle_warning_now(app),
            recovery: Some(BrowserLoginRecovery { mode: mode.to_owned(), auto_login }),
        },
        channel_override: true,
    });
}

pub(super) fn emit_browser_recovery_finished(
    app: &tauri::AppHandle,
    generation: u64,
    cancelled: bool,
) {
    let _ = app.emit(
        "exv://browser-login",
        BrowserLoginStatus {
            active: false,
            generation,
            outcome: cancelled.then_some("cancelled"),
            stage: None,
            round: None,
            throttle_warning: None,
            recovery: None,
        },
    );
}
/// 浏览器认证开始（active=true）时的验证码风险预告载荷：窗口内已记录次数达到
/// 阈值才返回 `Some`。只读（不消耗/不计数——本次尝试到达表单页时才由
/// `embedded_auth` 计数），因此 core 唤醒重试的第二次 acquire 重算亦幂等。
/// 计数器未 manage（setup 目录不可用）时如实返回 `None` 并不告警刷屏。
fn login_throttle_warning_now(app: &tauri::AppHandle) -> Option<LoginThrottleWarning> {
    app.try_state::<LoginAttemptTracker>()
        .and_then(|tracker| tracker.warning_due(std::time::SystemTime::now()).ok().flatten())
}

/// 发起连接（core KernelControl.Connect）。
///
/// R4：connect 是异步受理（R1w pending）——终态/阶段经 `exv://status` 事件驱动，
/// 不再用 stderr 诊断直出（失败走事件链/命令错误，D3 铁律：日志纯输出、状态走事件）。
#[tauri::command]
pub async fn connect(
    app: tauri::AppHandle,
    state: State<'_, CoreState>,
    session: State<'_, CoreSession>,
    intent: ConnectIntent,
) -> Result<OperationReply, AppError> {
    let browser_status = |active, generation| emit_browser_login_status(&app, active, generation);
    match CoreClient
        .connect_with_auth_status(&state, intent.clone(), app.clone(), browser_status)
        .await
    {
        Ok(reply) => Ok(reply),
        Err(AppError::CoreUnreachable(_)) => {
            // T9（§2.9 业务唤醒）：core 不可达时先确认死亡并重拉（活着则幂等放行），
            // 唤醒失败 → typed CoreWakeFailed（前端弹「尝试重启程序 / 仅退出」模态）。
            super::bootstrap::ensure_core_available(&app, &state, &session).await?;
            CoreClient
                .connect_with_auth_status(&state, intent, app.clone(), browser_status)
                .await
        }
        Err(error) => Err(error),
    }
}

/// 请求当前内嵌 aTrust 登录切换到系统默认 Edge/Chrome。
#[tauri::command]
pub fn atrust_browser_fallback(state: State<'_, CoreState>) -> Result<(), AppError> {
    state
        .connection_requests
        .request_system_browser_fallback()
        .map_err(AppError::Internal)
}

/// 仅当本次 aTrust 登录正在使用内嵌窗口时显示系统浏览器回退按钮。
#[tauri::command]
pub fn atrust_browser_fallback_available(state: State<'_, CoreState>) -> bool {
    state
        .connection_requests
        .system_browser_fallback_available()
}

/// §7.5 手动介入：显示并前置当前隐藏的内嵌 aTrust 认证窗口。
///
/// 内嵌认证窗口以 `atrust-auth-` 前缀创建；页面分类失败（下载页/失败页/停滞）时
/// `embedded_auth` 保留其隐藏状态，由本命令交用户接管。系统浏览器模式没有内嵌窗口，
/// 返回可读错误由前端如实提示（不伪造已前置）。
#[tauri::command]
pub fn atrust_show_auth_window(app: tauri::AppHandle) -> Result<(), AppError> {
    for (label, window) in app.webview_windows() {
        if label.starts_with("atrust-auth-") {
            window
                .show()
                .map_err(|_| AppError::Internal("无法显示内嵌认证窗口".into()))?;
            let _ = window.set_focus();
            return Ok(());
        }
    }
    Err(AppError::Internal(
        "当前没有可显示的内嵌认证窗口；系统浏览器认证请切换到浏览器窗口继续。".into(),
    ))
}

/// UI 的 Core 两态健康检查。仅由控制管道实际可通与否得出，绝不扫描进程或拉起 Core。
#[tauri::command]
pub async fn core_status(state: State<'_, CoreState>) -> Result<CoreStatus, AppError> {
    Ok(state.probe_status().await)
}

/// 一键卸载：按记录的 SHA-1 指纹从**当前用户** Root 删除自建 CA。
///
/// 返回被删除的指纹（大写十六进制）；无持久化 CA 时返回 `None`。只操作 `-user Root`。
/// 命令名沿用 `agent_sim_ca_uninstall`（前端契约不变；证书管理自 C4 起由
/// `local_agent` 接线，CA 生成/信任的平台实现仍在 exv-vpn-win32-agent-sim 平台库）。
#[tauri::command]
pub fn agent_sim_ca_uninstall() -> Result<Option<String>, String> {
    super::local_agent::uninstall_ca()
}

/// 停止连接（core KernelControl.Stop）。
#[tauri::command]
pub async fn stop(
    app: tauri::AppHandle,
    state: State<'_, CoreState>,
) -> Result<OperationReply, AppError> {
    let _ = app; // O3/P4-b：停止成功后通知 core 退出在 lifecycle::notify_core_shutdown。
    CoreClient.stop(&state).await
}

/// 拉取当前运行时快照（core KernelControl.GetSnapshot）。
#[tauri::command]
pub async fn snapshot(state: State<'_, CoreState>) -> Result<RuntimeSnapshot, AppError> {
    CoreClient.snapshot(&state).await
}

/// 拉取日志历史分片（KernelControl.LogsList 真实接线；host 聚合 core/engine 日志）。
#[tauri::command]
pub async fn logs_list(
    state: State<'_, CoreState>,
    after_seq: u64,
    limit: u32,
) -> Result<LogChunk, AppError> {
    CoreClient.logs_list(&state, after_seq, limit).await
}

/// 清空 core 持久化日志（KernelControl.LogsClear；不是只清当前 Vue 视图）。
#[tauri::command]
pub async fn logs_clear(state: State<'_, CoreState>) -> Result<LogsClearReply, AppError> {
    CoreClient.logs_clear(&state).await
}

/// 读取配置（KernelControl.ConfigGet 真实接线；设置页核心配置数据源）。
#[tauri::command]
pub async fn config_get(
    state: State<'_, CoreState>,
) -> Result<super::client::ConfigPayload, AppError> {
    CoreClient.config_get(&state).await
}

/// 拉取当前归一化统计（stats-wire 方案 A：从 `snapshot` 命令维护的缓存读取；
/// 尚无样本返回 typed NotWired 占位，前端保持「暂无统计」不视为失败）。
#[tauri::command]
pub async fn stats(state: State<'_, CoreState>) -> Result<RuntimeStats, AppError> {
    CoreClient.stats(&state).await
}

/// 写入配置（KernelControl.ConfigSet 真实接线；返回 ok 标志）。
///
/// T9（§2.9 业务唤醒）→ A1-1c（2026-10-09 区域 A）：与快速入门提交同一形状的
/// 有界重试环（[`super::retry`]）——仅对 `CoreUnreachable`、仅包裹 ConfigSet
/// （幂等：原子全量重写）、整环 15s 硬上界；core 不可达时先确认死亡并重拉
/// （有界等待就绪），唤醒成功后退避重试；唤醒失败 → typed `CoreWakeFailed`。
#[tauri::command]
pub async fn config_set(
    app: tauri::AppHandle,
    state: State<'_, CoreState>,
    session: State<'_, CoreSession>,
    items: Vec<ConfigItem>,
) -> Result<bool, AppError> {
    super::retry::retry_on_core_unreachable(
        || async { CoreClient.config_set(&state, items.clone()).await },
        Some(|| async {
            super::bootstrap::ensure_core_available(&app, &state, &session)
                .await
                .map(|_| ())
        }),
    )
    .await
}

/// T9（§2.9）弱提示横幅的「重启 core」按钮：确认死亡后重拉；core 活着则幂等成功。
/// 失败返回 typed `CoreWakeFailed`（前端以弱提示 toast 呈现，横幅保留）。
#[tauri::command]
pub async fn core_restart(
    app: tauri::AppHandle,
    state: State<'_, CoreState>,
    session: State<'_, CoreSession>,
) -> Result<(), AppError> {
    super::bootstrap::ensure_core_available(&app, &state, &session)
        .await
        .map(|_| ())
}

/// 读取 EXV Wintun 适配器当前的 IPv4 单播地址。
///
/// 这是只读系统观测：未连接/地址尚未分配时返回 `None`，不把配置中的地址或
/// 路由目标伪装成校内地址。
#[tauri::command]
pub fn tunnel_address() -> Result<Option<String>, String> {
    exv_vpn_win32_resource::adapter_address::first_ipv4_for_adapter("ExvEngine")
        .map(|address| address.map(|value| value.to_string()))
}

/// 应答交互提示（core KernelControl.RespondInteraction）。
#[tauri::command]
pub async fn respond_interaction(
    state: State<'_, CoreState>,
    interaction_id: Vec<u8>,
    response_payload: Vec<u8>,
) -> Result<OperationReply, AppError> {
    CoreClient
        .respond_interaction(&state, interaction_id, response_payload)
        .await
}

/// 触发一次延迟刷新（T1 latency design v2）：写本地标记文件，engine 数据面探测
/// 循环轮询到新值即立即执行一次隧道 ping。无 wire 变更（延迟仍经
/// `RuntimeSnapshot.stats.latency_ms` 到达；host/engine RPC 面在 T1 边界之外）。
#[tauri::command]
pub async fn trigger_latency_refresh() -> Result<(), AppError> {
    super::latency::write_refresh_marker().map_err(AppError::Internal)
}

/// 服务控制（S3/D5：`KernelControl.ServiceControl`）：query 非提权读；install/
/// uninstall/start 变更 action 由 host 经 engine 子命令 runas 提权 seam 执行。
/// 返回 post-action 服务状态 + `ok` + 人类可读信息（无秘密/栈）。
#[tauri::command]
pub async fn service_control(
    state: State<'_, CoreState>,
    action: super::state::ServiceControlAction,
) -> Result<super::state::ServiceControlReply, AppError> {
    CoreClient.service_control(&state, action).await
}

/// 用系统默认浏览器打开外部 URL（Windows 经 cmd /C start）。关于页仓库链接等使用；
/// WebView2 会拦截 `window.open`，必须走本命令。
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---- 测试 ----

/// 只提交本代准备动作；真实phase/busy/取消权限由原ConnectionRequests处理。
#[tauri::command]
pub fn auth_preparation_respond(
    state: State<'_, CoreState>,
    generation: u64,
    action: super::connection_request::AuthPreparationAction,
) -> Result<(), AppError> {
    state.connection_requests.respond_auth_preparation(generation, action).map_err(AppError::Internal)
}

/// 仅由设置页主动展开后调用，保持未连接及切换协议后可用。
#[tauri::command]
pub async fn auth_receiver_status(state: State<'_, CoreState>) -> Result<serde_json::Value, AppError> {
    official_receiver_control(&state, exv_vpn_wire::generated::AuthReceiverAction::Inspect).await
}

/// 固定恢复官方认证接收端首次保存的启动设置，不接受任意服务或路径输入。
#[tauri::command]
pub async fn restore_official_auth_receiver(state: State<'_, CoreState>) -> Result<serde_json::Value, AppError> {
    official_receiver_control(&state, exv_vpn_wire::generated::AuthReceiverAction::Restore).await
}

fn official_receiver_reply(reply: exv_vpn_wire::generated::AuthReceiverControlReply) -> serde_json::Value {
    let config = |value: &exv_vpn_wire::generated::AuthReceiverConfigSummary| serde_json::json!({
        "windows_start_type": value.windows_start_type,
        "windows_delayed_auto_start": value.windows_delayed_auto_start,
        "launchd_targets": value.launchd_targets.iter().map(|target| serde_json::json!({
            "domain": target.domain, "label": target.label, "disabled": target.disabled,
        })).collect::<Vec<_>>(),
    });
    let receiver = reply.receiver.as_ref().map(|value| serde_json::json!({
        "running": value.running, "can_start": value.can_start,
        "recovery_available": value.recovery_available, "source": value.source,
        "original_config": value.original_config.as_ref().map(&config),
        "current_config": value.current_config.as_ref().map(&config),
    }));
    let error = reply.error.as_ref().map(|value| serde_json::json!({
        "message": value.operation_detail.clone().filter(|s| !s.is_empty()).unwrap_or_else(||
            format!("aTrust 启动设置操作失败：{}。请重试。", value.reason)),
        "code": value.code, "stage": value.stage,
        "native": value.native.as_ref().map(|native| serde_json::json!({
            "category": native.category, "namespace": native.namespace, "code": native.code,
        })),
    }));
    serde_json::json!({ "receiver": receiver, "completed_steps": reply.completed_steps, "error": error })
}

async fn official_receiver_control(
    state: &CoreState,
    action: exv_vpn_wire::generated::AuthReceiverAction,
) -> Result<serde_json::Value, AppError> {
    use exv_vpn_win32_ipc::auth_deadline::AuthDeadline;
    use exv_vpn_wire::generated as wire;
    if action == wire::AuthReceiverAction::Restore {
        ensure_official_receiver_restore_allowed(
            state.connection_requests.has_active_request(),
            state.cached_snapshot().as_ref().map(|snapshot| &snapshot.runtime),
        )?;
    }
    let deadline = AuthDeadline::after(std::time::Duration::from_secs(60))
        .map_err(|error| AppError::Internal(format!("准备 aTrust 启动设置控制期限失败：{error:?}")))?;
    let until = deadline.tokio_deadline()
        .map_err(|error| AppError::Internal(format!("aTrust 启动设置操作期限已耗尽：{error:?}")))?;
    let operation_id = uuid::Uuid::new_v4().as_bytes().to_vec();
    CoreClient.prepare_auth_control(state, wire::PrepareAuthControlRequest {
        operation_id: operation_id.clone(), deadline_monotonic_ns: deadline.monotonic_ns(),
    }).await?;
    if action == wire::AuthReceiverAction::Restore {
        let snapshot = CoreClient.snapshot(state).await?;
        ensure_official_receiver_restore_allowed(
            state.connection_requests.has_active_request(), Some(&snapshot.runtime),
        )?;
    }
    let reply = tokio::time::timeout_at(until, CoreClient.auth_receiver_control(state, wire::AuthReceiverControlRequest {
        operation_id, action: action as i32, deadline_monotonic_ns: deadline.monotonic_ns(),
    })).await.map_err(|_| AppError::Internal("aTrust 启动设置操作超过本次期限，请重试。".into()))??;
    Ok(official_receiver_reply(reply))
}


/// 活动认证、连接和清理期间保留官方接收端的禁用设置。
fn ensure_official_receiver_restore_allowed(
    active_request: bool,
    runtime: Option<&super::state::RuntimeState>,
) -> Result<(), AppError> {
    use super::state::RuntimeState;
    let connection_active = matches!(runtime,
        Some(RuntimeState::Connecting { .. }
            | RuntimeState::AwaitingInteraction { .. }
            | RuntimeState::Connected { .. }
            | RuntimeState::Stopping { .. }
            | RuntimeState::Reconciling { .. }
            | RuntimeState::FailedDirty { has_obligation: true, .. }));
    if active_request || connection_active {
        return Err(AppError::Internal("当前连接或认证正在进行，请先断开连接后恢复 aTrust 原启动设置。".into()));
    }
    Ok(())
}
