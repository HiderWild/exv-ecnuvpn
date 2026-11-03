//! Tauri Command 定义（win32 命令面的 darwin 实现；win32 名字 + win32 DTO 形状）。
//!
//! 映射：Command ↔ unary。
//!   connect / stop / snapshot / core_status / stats / logs_list / logs_clear /
//!   config_get / config_set / respond_interaction / trigger_latency_refresh /
//!   service_control / tunnel_address / open_external / quick_start_apply（独立模块）
//!
//! darwin 平台旁路（诚实语义，不伪造 win32 状态）：
//!   * `respond_interaction`：darwin core 该 RPC 返回 typed `Unimplemented` →
//!     原样透传为 typed 错误，前端走既有认证失败/内部错误面；
//!   * `trigger_latency_refresh`：经已认证 Core 转交当前 Cisco Engine 探测队列；
//!   * `service_control` 已非旁路（W3-4/P4 v1 起透传 core 的 query 实装与变更
//!     动作 typed 指引，见下文命令文档）；
//!   * `open_external` 已实装（非旁路）：壳侧 AppKit `NSWorkspace openURL:` 打开
//!     外部 URL（`kernel::external_open`，见命令文档）——不经 shell/进程/插件，
//!     无守卫红线；修复 darwin「关于页 GitHub 链接点击无反应」（原 stub 报错 +
//!     WKWebView 内 `window.open` 静默无效）。
//!
//! `tunnel_address` 已实装（W2-A/P5，非旁路）：壳侧 getifaddrs 只读观测 engine
//! utun（[`super::adapter_address`]），判据与 None 语义对齐 win32 `adapter_address`
//! 本地 seam 契约——未连接/未分配/候选歧义诚实返回 `None`。

use exv_vpn_darwin_ipc::auth_deadline::AuthDeadline;
use tauri::State;

use super::client::{ConfigItem, ConnectIntent, CoreClient, CoreState, CoreStatus};
use super::error::AppError;
use super::logs::{LogChunk, LogsClearReply};
use super::state::{
    OperationReply, RuntimeSnapshot, RuntimeState, ServiceControlAction, ServiceControlReply,
};
use super::stats::RuntimeStats;

/// 发起连接（core KernelControl.Connect）。
///
/// R4：connect 是异步受理（R1w pending）——终态/阶段经 `exv://status` 事件驱动。
/// CoreUnreachable 时先走会话恢复（darwin：整会话替换），再重试原请求一次。
#[tauri::command]
pub(crate) async fn connect(
    app: tauri::AppHandle,
    state: State<'_, CoreState>,
    intent: ConnectIntent,
) -> Result<OperationReply, AppError> {
    // 在恢复Core等任何异步准备之前建立本次用户意图；后续认证只会交候选给此入口。
    let request = state.connection_requests.begin()?;
    let deadline = AuthDeadline::after(super::browser_auth::AUTH_BUDGET)
        .map_err(super::client::auth_deadline_app_error)?;
    connect_with_recovery(
        request,
        deadline,
        |request, deadline| {
            super::atrust_connect::connect(&app, &state, &intent, request, deadline)
        },
        || super::bootstrap::recover_stopped_core_for_connect(&app, &state),
    )
    .await
}

/// 命令的真实恢复编排：同一请求、同一绝对期限；已启动恢复等待真实收束。
pub(super) async fn connect_with_recovery<A, AF, R, RF>(
    request: std::sync::Arc<super::connection_request::ConnectionRequest>,
    deadline: AuthDeadline,
    mut attempt: A,
    recover: R,
) -> Result<OperationReply, AppError>
where
    A: FnMut(std::sync::Arc<super::connection_request::ConnectionRequest>, AuthDeadline) -> AF,
    AF: std::future::Future<Output = Result<OperationReply, AppError>>,
    R: FnOnce() -> RF,
    RF: std::future::Future<Output = Result<(), AppError>>,
{
    request.begin_submit()?;
    deadline
        .remaining()
        .map_err(super::client::auth_deadline_app_error)?;
    let result = attempt(std::sync::Arc::clone(&request), deadline).await;
    request.begin_submit()?;
    match result {
        Err(AppError::CoreUnreachable(_)) => {
            request.begin_submit()?;
            deadline
                .remaining()
                .map_err(super::client::auth_deadline_app_error)?;
            recover().await?;
            request.begin_submit()?;
            deadline
                .remaining()
                .map_err(super::client::auth_deadline_app_error)?;
            let result = attempt(std::sync::Arc::clone(&request), deadline).await;
            request.begin_submit()?;
            result
        }
        result => result,
    }
}

/// UI 控制面读取；失败沿原 AppError 返回，不扫描进程或推导已停止。
#[tauri::command]
pub(crate) async fn core_status(state: State<'_, CoreState>) -> Result<CoreStatus, AppError> {
    state.probe_status().await
}

/// 停止连接（core KernelControl.Stop）。
#[tauri::command]
pub(crate) async fn stop(state: State<'_, CoreState>) -> Result<OperationReply, AppError> {
    CoreClient.stop(&state).await
}

/// 拉取当前运行时快照（core KernelControl.GetSnapshot）。
#[tauri::command]
pub(crate) async fn snapshot(state: State<'_, CoreState>) -> Result<RuntimeSnapshot, AppError> {
    CoreClient.snapshot(&state).await
}

/// 拉取日志历史分片（KernelControl.LogsList；core 聚合日志）。
#[tauri::command]
pub(crate) async fn logs_list(
    state: State<'_, CoreState>,
    after_seq: u64,
    limit: u32,
) -> Result<LogChunk, AppError> {
    CoreClient.logs_list(&state, after_seq, limit).await
}

/// 清空 core 持久化日志（KernelControl.LogsClear；不是只清当前 Vue 视图）。
#[tauri::command]
pub(crate) async fn logs_clear(state: State<'_, CoreState>) -> Result<LogsClearReply, AppError> {
    CoreClient.logs_clear(&state).await
}

/// 读取配置（KernelControl.ConfigGet；设置页核心配置数据源）。
#[tauri::command]
pub(crate) async fn config_get(
    state: State<'_, CoreState>,
) -> Result<super::client::ConfigPayload, AppError> {
    CoreClient.config_get(&state).await
}

/// 写入配置（KernelControl.ConfigSet；返回 ok 标志）。
///
/// A1-1e（2026-10-09 区域 A）：ConfigSet 阶段的有界重试环（[`super::retry`]，
/// win32 `commands.rs::config_set` 同构参数化——仅对 `CoreUnreachable`、仅包裹
/// 幂等的 ConfigSet、整环 15s 硬上界；darwin 无唤醒设施，`wake` 恒 `None`
/// 纯退避，core 若死则环耗尽后快速失败交给连接路径恢复语义）。darwin 宿主
/// 未运行，本改动为参数化文本改动，未经本地编译/测试验证，如实登记。
#[tauri::command]
pub(crate) async fn config_set(
    state: State<'_, CoreState>,
    items: Vec<ConfigItem>,
) -> Result<bool, AppError> {
    super::retry::retry_on_core_unreachable(
        || async { CoreClient.config_set(&state, items.clone()).await },
        super::retry::NO_WAKE,
    )
    .await
}

/// 拉取当前归一化统计（stats-wire 方案 A：从 `snapshot` 命令维护的缓存读取；
/// 尚无样本返回 typed NotWired 占位，前端保持「暂无统计」不视为失败）。
#[tauri::command]
pub(crate) async fn stats(state: State<'_, CoreState>) -> Result<RuntimeStats, AppError> {
    CoreClient.stats(&state).await
}

/// 读取 EXV 隧道（engine utun 接口）当前的 IPv4 地址（win32 读 Wintun 适配器的
/// darwin 实装对应物：[`super::adapter_address`] 壳侧 getifaddrs 只读观测）。
///
/// 只读、无缓存、无 State、可重入；判据与 None 语义（未连接/未分配/候选歧义均
/// 诚实 `None`，不把配置中的地址伪装成已分配的隧道地址；前端「校内地址」渲染
/// 「—」有口值缺档）见 [`super::adapter_address`] 模块文档。仅 `getifaddrs` 枚举
/// 失败时返回 typed `Internal` 错误。
#[tauri::command]
pub(crate) fn tunnel_address() -> Result<Option<String>, AppError> {
    super::adapter_address::first_ipv4_for_exv_utun()
        .map(|address| address.map(|value| value.to_string()))
        .map_err(|error| AppError::Internal(format!("Darwin tunnel_address 观测失败：{error}")))
}

/// 应答交互提示（core KernelControl.RespondInteraction）。
///
/// 诚实语义：darwin core 该 RPC 当前返回 typed `Unimplemented`——本命令把该拒绝
/// 原样透传为 typed `Internal` 错误（携带稳定码），前端按既有认证失败/内部错误面
/// 处理，不伪造成功回复。
#[tauri::command]
pub(crate) async fn respond_interaction(
    state: State<'_, CoreState>,
    interaction_id: Vec<u8>,
    response_payload: Vec<u8>,
) -> Result<OperationReply, AppError> {
    CoreClient
        .respond_interaction(&state, interaction_id, response_payload)
        .await
}

/// 请求当前已接纳 operation 的时延刷新；受理与匹配回包分别沿 RPC/Stats 返回。
#[tauri::command]
pub(crate) async fn trigger_latency_refresh(
    state: State<'_, CoreState>,
    operation_id: String,
) -> Result<(), AppError> {
    CoreClient
        .trigger_latency_refresh(&state, &operation_id)
        .await
}

/// 服务控制（core `KernelControl.ServiceControl`；W3-4/P4 v1 起透传 core）。
///
/// * `query`：core 的 服务代理健康探测——固定安装目录 binary stat × control.sock
///   可连 ×（可选）Status RPC，映射**既有**五态词汇（healthy/
///   installed_unavailable/payload_orphan/not_installed；scm_orphan 是 darwin v1
///   盲点，见 core `service_status` 模块声明），ServicePanel 点亮真实档位；
/// * `install`/`uninstall`/`start`：core typed `Unimplemented` 携带 root CLI 指引
///   （服务代理安装/卸载是 root CLI 形态，Core 不自动安装——未装如实报
///   not_installed）；经 `map_status` 透传为 `Internal`，前端 ServicePanel 走既有
///   错误面；
/// * `rotate_key`：core typed 拒绝（一次性 ticket 模型无持久服务密钥，语义不适用）。
///
/// 历史旁路（query 返回 `ok=true`+`service_status=None` 占位）由此退役。
#[tauri::command]
pub(crate) async fn service_control(
    state: State<'_, CoreState>,
    action: ServiceControlAction,
) -> Result<ServiceControlReply, AppError> {
    service_control_via_core(&CoreClient, &state, action).await
}

/// 可在真实 Core gRPC 边界测试的透传实现（`quick_start::apply` 同款接缝形态）。
pub(crate) async fn service_control_via_core(
    client: &CoreClient,
    state: &CoreState,
    action: ServiceControlAction,
) -> Result<ServiceControlReply, AppError> {
    client.service_control(state, action).await
}

/// 用系统默认浏览器打开外部 URL（macOS 经 AppKit `NSWorkspace openURL:`，见
/// [`super::external_open`]；win32 壳对应实现是 `cmd /C start`）。darwin 前端
/// AboutPage 仓库链接等经此命令打开（经壳注入的全局 adapter 单接缝）。
///
/// `NSWorkspace` 只能在主线程触碰，而 async 命令运行在 tokio worker 线程 → 先经
/// `AppHandle::run_on_main_thread` 派发，再用 oneshot 回传结果：打开失败如实返回
/// typed 错误，不伪造成功。URL 只放行 http/https（打开面约束见 `external_open`）。
#[tauri::command]
pub(crate) async fn open_external(app: tauri::AppHandle, url: String) -> Result<(), AppError> {
    super::external_open::open_in_default_browser(&app, &url)
        .await
        .map_err(AppError::Internal)
}

/// 预卸载：删除 EXV 自己的全部本地产物。
///
/// 编排顺序见计划 §5.1：停连接 → 管理员提权段（core 侧：卸服务 + 历史守护 + `Library` 产物 +
/// root 属主 runtime 残留）→ 用户态段 → 应用本体。**分项结果**由此函数返回，逐项状态以上
/// 删除尝试之后的后置核对为准（提权调用的退出码不能作为分项成功依据）。
///
/// **调用方（前端）必须在展示结果之后再退出应用**——本命令自身不退出，因为"先退出后渲染"
/// 会让用户看不到逐项结果与手动处理指引。
///
/// `app_path` 缺省取当前可执行文件所在 bundle；测试可注入沙箱路径。
#[tauri::command]
pub(crate) async fn pre_uninstall(
    state: State<'_, CoreState>,
    home: Option<String>,
    app_path: Option<String>,
) -> Result<super::pre_uninstall::PreUninstallReply, AppError> {
    let home = match home {
        Some(value) => std::path::PathBuf::from(value),
        None => std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| AppError::Internal("无法解析用户主目录".to_owned()))?,
    };
    let bundle = match app_path {
        Some(value) => std::path::PathBuf::from(value),
        None => current_bundle_path()?,
    };
    // 提权段：复用 core 既有 `service_control(Uninstall)` 动作——该动作在 core 侧已扩展为
    // 一次管理员提权内完成「卸服务 + 历史守护 + `/Library` 产物 + root 属主 runtime 残留」。
    // 壳层不拼任何提权 payload、不 exec 进程（守卫约束）。
    //
    // 按原 Core Uninstall 结果报告：Core 已保存各原清理动作的首个失败，原错误继续上抛。
    // [0] 连接前置：用户要卸载，连接必须停。停不下来就**中止整次卸载**（不做任何删除）——
    // 清理会删本连接的运行目录、服务组件与凭据，存在活动会话时删除会与运行中的引擎竞态。
    let connection = stop_active_connection(&state).await;

    // 服务卸载必须延迟到连接前置确认之后；不能先执行再交给 run 报告中止。
    let elevation_entry = elevate_for_stopped_connection(&connection, || async {
        match CoreClient
            .full_uninstall(&state)
            .await
        {
            Ok(reply) if reply.ok => {
                let detail = reply.message.lines().filter(|line| line.starts_with("已保留包含其他文件的目录："))
                    .collect::<Vec<_>>().join("\n");
                if detail.is_empty() { Ok(super::pre_uninstall::ElevationOutcome::Executed) }
                else { Ok(super::pre_uninstall::ElevationOutcome::ExecutedWithDetail(detail)) }
            },
            Ok(reply) => classify_elevation_failure(&reply.message),
            Err(error) => classify_elevation_failure(&error.to_string()),
        }
    })
    .await;
    let Some(elevation_entry) = elevation_entry else {
        // 复用原编排的分项中止结果；NotStopped 会在调用 launcher 或任何删除前返回。
        let never_launch: super::pre_uninstall::ElevationLauncher =
            std::sync::Arc::new(|| Err("当前连接未停止，未发起服务卸载".to_owned()));
        return Ok(super::pre_uninstall::run(
            &home,
            &bundle,
            connection,
            &never_launch,
        ));
    };
    // 提权段只发起一次：闭包把同一结果交给编排（保持 seam 形状以便测试注入其他结果）。
    let launcher: super::pre_uninstall::ElevationLauncher =
        std::sync::Arc::new(move || elevation_entry.clone());
    Ok(super::pre_uninstall::run(
        &home, &bundle, connection, &launcher,
    ))
}

/// 在调用服务卸载之前检查连接结果；闭包延迟创建请求，便于验证中止时零调用。
async fn elevate_for_stopped_connection<F, Fut>(
    connection: &super::pre_uninstall::ConnectionOutcome,
    launch: F,
) -> Option<Result<super::pre_uninstall::ElevationOutcome, String>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<super::pre_uninstall::ElevationOutcome, String>>,
{
    if matches!(
        connection,
        super::pre_uninstall::ConnectionOutcome::NotStopped(_)
    ) {
        return None;
    }
    Some(launch().await)
}

/// core 稳定码：用户取消了管理员授权（`service_lifecycle::ELEVATION_DENIED_CODE`）。
///
/// 壳层以字符串匹配分类（core 经 gRPC `Status::failed_precondition` 返回该码，壳的
/// `map_status` 归入 `Internal` 并保留原文）。**故意不新增错误变体**：改动共享的
/// `AppError` 面会影响其它命令的错误契约。
const ELEVATION_DENIED_CODE: &str = "DARWIN_CORE_SERVICE_ELEVATION_DENIED";

/// 把提权段失败分类成"用户取消（跳过）"或"真实失败"。
///
/// **语义差异是刻意的**（计划 §5.3）：用户取消密码框不等于卸载失败——报告里应显示
/// "已跳过 / 系统级残留仍在"而不是"失败"，两者都必须带上手动清理指引。把它一律归为
/// 失败会让用户以为程序出了故障。
fn classify_elevation_failure(
    detail: &str,
) -> Result<super::pre_uninstall::ElevationOutcome, String> {
    if detail.contains(ELEVATION_DENIED_CODE) {
        return Ok(super::pre_uninstall::ElevationOutcome::Skipped(
            "用户取消了管理员授权；系统级残留仍在".to_owned(),
        ));
    }
    Err(detail.to_owned())
}

/// 由当前可执行文件位置推导 `.app` bundle 根（`.../EXV.app/Contents/MacOS/<exe>` → `.../EXV.app`）。
fn current_bundle_path() -> Result<std::path::PathBuf, AppError> {
    let exe = std::env::current_exe()
        .map_err(|error| AppError::Internal(format!("无法解析当前可执行文件：{error}")))?;
    // 期望形态：<bundle>/Contents/MacOS/<exe>；非 bundle 运行（裸二进制）时如实报错，
    // 由前端提示用户手动删除，不做猜测。
    let macos_dir = exe
        .parent()
        .ok_or_else(|| AppError::Internal("可执行文件无父目录".to_owned()))?;
    let contents = macos_dir
        .parent()
        .ok_or_else(|| AppError::Internal("可执行文件不在 Contents/MacOS 下".to_owned()))?;
    let bundle = contents
        .parent()
        .ok_or_else(|| AppError::Internal("无法定位 .app bundle".to_owned()))?;
    if contents.file_name().and_then(|name| name.to_str()) != Some("Contents") {
        return Err(AppError::Internal(
            "当前不在 .app bundle 内运行，应用本体需手动删除".to_owned(),
        ));
    }
    Ok(bundle.to_path_buf())
}

/// 退出应用（预卸载流程的收尾步骤专用）。
///
/// **为什么不复用窗口关闭**：`chrome.control("close")` 的行为受 `close_preference`
/// 驱动（smart 宽限 / tray 隐藏 / quit 退出）——卸载后必须**确定退出**，不能因用户
/// 的关闭偏好而变成"隐藏到托盘"，否则用户以为卸载完了但进程还在。
///
/// 复用 [`crate::lifecycle::notify_core_shutdown`] 这一既有唯一退出入口（托盘退出、
/// 关闭按钮共用同一实现），不新增停机路径。
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri command 签名要求 AppHandle 按值注入；本函数只读取它"
)]
pub(crate) fn pre_uninstall_quit(app: tauri::AppHandle) {
    crate::lifecycle::notify_core_shutdown(&app);
}

/// 卸载前的连接处置：有活动连接就停掉，并**确认回到 Idle** 才允许继续卸载。
///
/// 判定口径（与前端凭据取消收束同一原则）：`Stop` RPC 返回**不等于**已回到 Idle；
/// 必须以后续快照的权威状态为准。轮询有界（[`STOP_IDLE_DEADLINE`]），超时即
/// [`super::pre_uninstall::ConnectionOutcome::NotStopped`]，由编排中止卸载。
async fn stop_active_connection(state: &CoreState) -> super::pre_uninstall::ConnectionOutcome {
    use super::pre_uninstall::ConnectionOutcome;

    let idle = |snapshot: &RuntimeSnapshot| matches!(snapshot.runtime, RuntimeState::Idle { .. });

    // 先看当前状态：本来空闲就不必发 Stop。
    match CoreClient.snapshot(state).await {
        Ok(snapshot) if idle(&snapshot) => return ConnectionOutcome::Idle,
        Ok(_) => {}
        // 快照都取不到：无法证明"没有活动连接"，保守判为未停（宁可中止也不在未知状态下删）。
        Err(error) => return ConnectionOutcome::NotStopped(format!("无法读取运行状态：{error}")),
    }

    if let Err(error) = CoreClient.stop(state).await {
        return ConnectionOutcome::NotStopped(format!("停止请求失败：{error}"));
    }

    // 等权威 Idle（Stop 返回不代表已停）。
    let deadline = tokio::time::Instant::now() + STOP_IDLE_DEADLINE;
    loop {
        match CoreClient.snapshot(state).await {
            Ok(snapshot) if idle(&snapshot) => return ConnectionOutcome::Stopped,
            Ok(_) => {}
            Err(error) => {
                return ConnectionOutcome::NotStopped(format!("等待停止时读取状态失败：{error}"));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return ConnectionOutcome::NotStopped(format!(
                "等待 {} 秒仍未回到「未连接」",
                STOP_IDLE_DEADLINE.as_secs()
            ));
        }
        tokio::time::sleep(STOP_IDLE_POLL_INTERVAL).await;
    }
}

/// 等待连接回到 Idle 的上限。取 30 秒与「连接/断开各自的产品时限」同量级：既覆盖引擎
/// 完整拆除（组装取消 + 收尾），又不至于让卸载长时间无反馈。
const STOP_IDLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Idle 轮询间隔。
const STOP_IDLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// 浏览器发现与会话内选择只影响本地认证原语，连接提交仍由用户请求持有。
/// （B3 起选择按阶梯+缓存自动求值；`atrust_browser_choose` 为 wire 兼容保留，
/// 前端选择器与等待流已退场。）
#[tauri::command]
pub(crate) async fn atrust_browser_status(app: tauri::AppHandle, state: State<'_, CoreState>) -> Result<super::browser_auth::BrowserStatus, AppError> {
    super::browser_auth::browser_status(&app, &state.browser_selection).await.map_err(AppError::Internal)
}
#[tauri::command]
pub(crate) fn atrust_browser_choose(state: State<'_, CoreState>, browser: String) -> Result<bool, AppError> {
    state.browser_selection.choose(&browser).map_err(AppError::Internal)?; Ok(true)
}

/// 只提交本代准备动作；真实phase/busy/取消权限由原ConnectionRequests处理。
#[tauri::command]
pub(crate) fn auth_preparation_respond(
    state: State<'_, CoreState>,
    generation: u64,
    action: super::connection_request::AuthPreparationAction,
) -> Result<(), AppError> {
    state.connection_requests.respond_auth_preparation(generation, action).map_err(AppError::Internal)
}

/// 仅由设置页主动展开后调用，保持未连接及切换协议后可用。
#[tauri::command]
pub(crate) async fn auth_receiver_status(state: State<'_, CoreState>) -> Result<serde_json::Value, AppError> {
    official_receiver_control(&state, exv_vpn_wire::generated::AuthReceiverAction::Inspect).await
}

/// 固定恢复官方认证接收端首次保存的启动设置，不接受任意服务或路径输入。
#[tauri::command]
pub(crate) async fn restore_official_auth_receiver(state: State<'_, CoreState>) -> Result<serde_json::Value, AppError> {
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
        "code": value.code, "stage": value.stage, "reason": value.reason,
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
    let mut operation_id = vec![0u8; 16];
    getrandom::fill(&mut operation_id)
        .map_err(|error| AppError::Internal(format!("生成 aTrust 启动设置操作标识失败：{error}")))?;
    CoreClient.prepare_auth_control(state, deadline).await?;
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
