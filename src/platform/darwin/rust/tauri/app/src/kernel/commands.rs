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
//!   * `trigger_latency_refresh`：v1 no-op（周期探测由并行车道实现；win32 的
//!     本地标记文件机制属于其 engine 探测循环，darwin 无对应消费方）；
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
    match CoreClient.connect(&state, &intent).await {
        Ok(reply) => Ok(reply),
        Err(AppError::CoreUnreachable(_)) => {
            super::bootstrap::recover_stopped_core_for_connect(&app, &state).await?;
            CoreClient.connect(&state, &intent).await
        }
        Err(error) => Err(error),
    }
}

/// UI 的 Core 两态健康检查。仅由控制管道实际可通与否得出，绝不扫描进程或拉起 Core。
#[tauri::command]
pub(crate) async fn core_status(state: State<'_, CoreState>) -> Result<CoreStatus, AppError> {
    Ok(state.probe_status().await)
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

/// 仅用户主动按住眼睛时读取；使用 Darwin 配置目录解析，与 Core 继承同一环境。
#[tauri::command]
pub(crate) async fn saved_password(
    state: State<'_, CoreState>, username: String, server: String,
) -> Result<Option<super::saved_password::PasswordForDisplay>, String> {
    let current = CoreClient.config_get(&state).await
        .map_err(|_| "无法读取当前配置的已保存密码".to_string())?;
    let value = |key: &str| current.items.iter().find(|item| item.key == key).map(|item| item.value.as_str());
    if value("username") != Some(username.as_str()) || value("server") != Some(server.as_str()) {
        return Err("当前账户已变化，请重新按住查看密码".to_string());
    }
    super::saved_password::read_current_for_display(&username, &server)
}

/// 写入配置（KernelControl.ConfigSet；返回 ok 标志）。
#[tauri::command]
pub(crate) async fn config_set(
    state: State<'_, CoreState>,
    items: Vec<ConfigItem>,
) -> Result<bool, AppError> {
    CoreClient.config_set(&state, items).await
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

/// 触发一次延迟刷新（win32 写本地标记文件，engine 探测循环轮询到即立即 ping）。
///
/// darwin v1 no-op：周期延迟探测由并行车道实现（engine 数据面探测流），
/// 本命令如实受理（Ok）但不产生副作用；延迟仍经 `RuntimeSnapshot.stats.latency_ms`
/// 到达，无 wire 变更。
#[tauri::command]
pub(crate) async fn trigger_latency_refresh() -> Result<(), AppError> {
    Ok(())
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
    // 提权返回**只代表"发起过"**：服务是否真的卸掉由 `pre_uninstall::run` 的后置核对判定，
    // 不以本调用成功为其依据（core payload 用 `;` 串联，退出码只反映最后一项）。
    // [0] 连接前置：用户要卸载，连接必须停。停不下来就**中止整次卸载**（不做任何删除）——
    // 清理会删本连接的运行目录、服务组件与凭据，存在活动会话时删除会与运行中的引擎竞态。
    let connection = stop_active_connection(&state).await;

    // 服务卸载必须延迟到连接前置确认之后；不能先执行再交给 run 报告中止。
    let elevation_entry = elevate_for_stopped_connection(&connection, || async {
        match CoreClient
            .service_control(&state, super::state::ServiceControlAction::Uninstall)
            .await
        {
            Ok(reply) if reply.ok => Ok(super::pre_uninstall::ElevationOutcome::Executed),
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
