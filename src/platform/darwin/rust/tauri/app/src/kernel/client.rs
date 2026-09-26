//! CoreClient —— UI 侧访问 core 的唯一接缝（win32 `kernel/client.rs` 的 darwin 移植）。
//!
//! 架构与 win32 同构：core 是独立进程，唯一语义网关；UI 不直连 engine。darwin 差异：
//!   * 通道形态：win32 是 Named Pipe gRPC，darwin 是既有 authenticated UDS session
//!     （[`super::core_process`] 负责进程管理与 UDS 认证拨号；gRPC-over-UDS）；
//!   * 会话恢复：WatchEvents 支持多订阅与 resume 重放（W3-1/P1），事件订阅任务
//!     自行退避重连；core 死亡的正路恢复是连接时的整会话重建
//!     （[`super::bootstrap::recover_stopped_core_for_connect`]）；
//!   * wire 消息 → UI 镜像类型的机械映射在 [`super::wire`]（与 win32 同源）。
//!
//! `service_control`（W3-4/P4 v1 起透传 core）：darwin core 的 query 已实装
//! 服务代理健康探测（五态词汇复用），变更动作 typed 拒绝携带 CLI 指引——命令层
//! 不再做平台旁路（见 `commands.rs`）。

use std::sync::RwLock;

use exv_vpn_wire::generated::kernel_control_client::KernelControlClient;
use exv_vpn_wire::generated::{self as wire};
use serde::{Deserialize, Serialize};
use tonic::transport::Channel;
use tonic::{Code, Request, Status};

use super::core_process;
use super::error::AppError;
use super::logs::{LogChunk, LogsClearReply};
use super::state::{OperationReply, RuntimeSnapshot, ServiceControlAction, ServiceControlReply};
use super::stats::RuntimeStats;
use super::wire::{self as wire_map};

/// 前端提交的本次连接凭据。密码只会在 Tauri→Core 的当前请求中编码，不记录到错误或日志。
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ConnectCredentials {
    pub username: String,
    pub password: String,
    pub persist: bool,
}

impl std::fmt::Debug for ConnectCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("persist", &self.persist)
            .finish()
    }
}

/// 连接/身份意图。Tauri 命令边界只接收结构化 `credentials`，由 wire 层编码为已有
/// `secret_payload`；保留 `secret_payload` 仅供既有 Rust 内部调用路径兼容。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ConnectIntent {
    pub profile_ref: String,
    #[serde(default)]
    pub credentials: Option<ConnectCredentials>,
    #[serde(default)]
    pub secret_payload: Option<String>,
}

/// 配置读取的命令回复（core `ConfigGet` 透传形状）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ConfigPayload {
    pub items: Vec<ConfigItem>,
    /// 本次 ConfigGet 是否因配置缺失而 bootstrap 默认配置（darwin core 当前恒 false，
    /// 如实透传；前端据此决定是否弹快速入门）。
    pub requires_quick_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConfigItem {
    pub key: String,
    pub value: String,
}

/// 与 core 的通道形态。
#[derive(Debug, Clone, Default)]
pub(crate) enum CoreHandle {
    /// 尚未建立到 core 的通道（core 未启动 / 拨号失败）。
    #[default]
    NotWired,
    /// 已认证的 UDS tonic 通道。
    Dialed { channel: Channel },
}

impl CoreHandle {
    /// 已拨号通道的引用（`NotWired` → `None`）。
    #[must_use]
    pub(crate) fn channel(&self) -> Option<&Channel> {
        match self {
            Self::Dialed { channel, .. } => Some(channel),
            Self::NotWired => None,
        }
    }
}

/// UI 对 Core 控制面的两态结论（win32 同款：只依据已验证管道上的 RPC 是否可通）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoreStatus {
    Normal,
    Stopped,
}

/// Tauri managed state：UI 侧 core 会话状态 + 快照缓存（win32 同款）。
#[derive(Debug, Default)]
pub(crate) struct CoreState {
    /// 当前经过身份验证的 Core 控制通道。Core 意外退出后，只有恢复分支可以替换它；
    /// 其余命令仅克隆读取，不会触发进程探测或拉起。
    pub(crate) handle: RwLock<CoreHandle>,
    /// 最近一次 RuntimeSnapshot 缓存（前端可即时渲染，无需等下一事件）。
    /// `RwLock`：命令层读，事件订阅 task 写。
    pub(crate) last_snapshot: RwLock<Option<RuntimeSnapshot>>,
    /// 最近一次归一化统计缓存（stats-wire 方案 A：由 `snapshot` 命令从快照携带的
    /// `stats` 写入；`stats` 命令读取）。`RwLock`：命令层读/写。
    pub(crate) last_stats: RwLock<Option<RuntimeStats>>,
}

impl CoreState {
    /// 取得当前已验证通道的副本。`None` 即 UI 所见「Core 已停止」。
    #[must_use]
    pub(crate) fn channel(&self) -> Option<Channel> {
        self.handle
            .read()
            .ok()
            .and_then(|handle| handle.channel().cloned())
    }

    /// 原子替换为恢复后重新认证的通道。
    pub(crate) fn replace_handle(&self, handle: CoreHandle) {
        if let Ok(mut current) = self.handle.write() {
            *current = handle;
        }
    }

    /// 将 UI 的 Core 两态结论置为已停止（管道不可用）。
    pub(crate) fn mark_stopped(&self) {
        self.replace_handle(CoreHandle::NotWired);
    }

    /// 日常管道健康结论。这里不观察进程，仅执行既有轻量 snapshot RPC。
    pub(crate) async fn probe_status(&self) -> CoreStatus {
        match CoreClient.snapshot(self).await {
            Ok(_) => CoreStatus::Normal,
            Err(_) => CoreStatus::Stopped,
        }
    }

    /// 最近一次缓存快照（事件订阅/快照命令写入；诊断/测试读取面，win32 同款保留）。
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn cached_snapshot(&self) -> Option<RuntimeSnapshot> {
        self.last_snapshot.read().ok().and_then(|g| g.clone())
    }

    /// 更新缓存快照。
    pub(crate) fn update_snapshot(&self, snapshot: RuntimeSnapshot) {
        if let Ok(mut g) = self.last_snapshot.write() {
            *g = Some(snapshot);
        }
    }

    /// 最近一次缓存统计（stats-wire 方案 A：`snapshot` 命令写入，`stats` 命令读取）。
    #[must_use]
    pub(crate) fn cached_stats(&self) -> Option<RuntimeStats> {
        self.last_stats.read().ok().and_then(|g| *g)
    }

    /// 更新缓存统计（`snapshot` 命令从快照携带的 `stats` 写入）。
    pub(crate) fn update_stats(&self, stats: RuntimeStats) {
        if let Ok(mut g) = self.last_stats.write() {
            *g = Some(stats);
        }
    }
}

/// Tauri managed state：core 进程生命周期句柄（darwin：整会话容器）。
///
/// * `inner`：当前 core 进程会话（`core_process::CoreSession`，持有 child/runtime/
///   watch 流；`None` = 未拉起 / 已回收）。由 [`super::bootstrap`] 在 setup 时与恢复
///   路径写入；进程 teardown drop 该会话 → UDS/stdin 关闭 → core 感知自行停机；
/// * `subscriptions`：事件订阅 task 句柄（会话替换时先中止）；
/// * `recovery`：会话恢复的串行闸门（并发 connect / 断流收敛只允许一个替换在途）。
#[derive(Default)]
pub(crate) struct CoreSession {
    pub(crate) inner: tokio::sync::Mutex<Option<core_process::CoreSession>>,
    pub(crate) subscriptions: std::sync::Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    pub(crate) recovery: tokio::sync::Mutex<()>,
}

impl CoreSession {
    /// best-effort 中止全部事件订阅 task（UI 退出加速 EOF 感知；会话替换前调用）。
    pub(crate) fn abort_subscriptions(&self) {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            for subscription in subscriptions.drain(..) {
                subscription.abort();
            }
        }
    }

    /// UI 正常退出：取出唯一会话所有权（只成功一次）。
    ///
    /// 退出入口不可等待，故用 `try_lock`；锁被占用（理论不可达：退出时命令层已停）
    /// 返回 `None`，调用方只记录并继续退出——泄漏一个 runtime 目录不改变退出语义。
    pub(crate) fn take_inner_for_shutdown(&self) -> Option<core_process::CoreSession> {
        self.inner.try_lock().ok().and_then(|mut slot| slot.take())
    }
}

/// UI 侧对 core 的命令客户端（无状态 facade；真实会话状态在 CoreState/CoreSession）。
#[derive(Debug, Default)]
pub(crate) struct CoreClient;

/// darwin core 的稳定 typed 拒绝码（`failed_precondition` message；见 core
/// `kernel_control_service.rs`）。凭据缺失 → 前端凭据模态；engine 会话/提权失败 →
/// typed Internal（携带稳定码，前端按内部错误展示）。
const CODE_CREDENTIALS_MISSING: &str = "DARWIN_CORE_CONNECT_CREDENTIALS_MISSING";
const CODE_ENGINE_ELEVATION_FAILED: &str = "DARWIN_CORE_ENGINE_ELEVATION_FAILED";
const CODE_ENGINE_SESSION_FAILED: &str = "DARWIN_CORE_ENGINE_SESSION_FAILED";
const CODE_SERVICE_NOT_INSTALLED: &str = "DARWIN_CORE_SERVICE_NOT_INSTALLED";
/// P4 v2：服务路由失败前缀（win32 既有词汇；core 在 服务代理未就绪/会话建立
/// 失败时发出，壳层据此分流恢复 modal）。
const SERVICE_NOT_RUNNING_PREFIX: &str = "service_not_running|";
const SERVICE_CONNECT_FAILED_PREFIX: &str = "service_connect_failed|";

/// 把 tonic `Status` 映射为 UI 错误（win32 `map_status` 的 darwin 对应）：
/// 服务路由前缀 → `ServiceNotRunning`/`ServiceConnectFailed`（恢复 modal）；
/// transport 类 → `CoreUnreachable`；darwin 稳定拒绝码 → typed 变体
/// （凭据缺失触发前端凭据模态）；其余（含 Unimplemented，如 `respond_interaction`）
/// → `Internal`（携带稳定码 + 精简 message，按 win32 同款错误面透传）。
fn map_status(s: Status) -> AppError {
    let message = s.message();
    if let Some(rest) = message.strip_prefix(SERVICE_NOT_RUNNING_PREFIX) {
        return AppError::ServiceNotRunning(rest.to_string());
    }
    if let Some(rest) = message.strip_prefix(SERVICE_CONNECT_FAILED_PREFIX) {
        return AppError::ServiceConnectFailed(rest.to_string());
    }
    match s.code() {
        Code::Unavailable | Code::Cancelled | Code::Unknown | Code::DeadlineExceeded => {
            AppError::CoreUnreachable(format!("{}: {}", s.code(), message))
        }
        _ => {
            if message.contains(CODE_CREDENTIALS_MISSING) {
                AppError::CredentialRequired {
                    code: Some(CODE_CREDENTIALS_MISSING.to_string()),
                    message: "credential_required".to_string(),
                }
            } else if message.contains(CODE_SERVICE_NOT_INSTALLED) {
                AppError::ServiceNotInstalled(message.to_string())
            } else if message.contains(CODE_ENGINE_ELEVATION_FAILED) {
                AppError::Internal(CODE_ENGINE_ELEVATION_FAILED.to_string())
            } else if message.contains(CODE_ENGINE_SESSION_FAILED) {
                AppError::Internal(CODE_ENGINE_SESSION_FAILED.to_string())
            } else {
                AppError::Internal(format!("{}: {}", s.code(), message))
            }
        }
    }
}

/// R4：把壳本地生成的 16 字节 operation_id 以小写 hex 回注 UI `OperationReply`。
///
/// 硬契约（win32 同源）：host wire `OperationReply` 没有 operation_id 字段——前端
/// 看到的回复 id 完全由壳在本地生成并回注；core 受理快照/状态事件携带同一 id，
/// 前端据此关联「当前用户操作」。connect 与 stop 两处回注共用本实现。
fn ui_operation_reply_with_local_id(
    reply: wire::OperationReply,
    operation_id: &[u8],
) -> OperationReply {
    let mut ui = wire_map::operation_reply_from_wire(&reply);
    ui.operation_id = Some(wire_map::hex(operation_id));
    ui
}

impl CoreClient {
    /// 发起连接（`KernelControl.Connect`）。
    ///
    /// R4 事件关联：调用方在组装 intent 时生成 operation_id（16 字节随机），
    /// core 以 lookup_key.operation_id 关联状态事件；同一 id 随 `OperationReply`
    /// 回传前端，前端据此只让「当前用户操作」的事件驱动连接 UI。
    pub(crate) async fn connect(
        &self,
        state: &CoreState,
        intent: &ConnectIntent,
    ) -> Result<OperationReply, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("connect: core not dialed".to_string()))?;
        let operation_id = random_16()?;
        let mut request = Request::new(wire_map::connect_request(intent, operation_id.clone())?);
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
        let mut client = KernelControlClient::new(channel);
        let reply = match client.connect(request).await.map_err(map_status) {
            Ok(reply) => reply,
            Err(error) => {
                if matches!(error, AppError::CoreUnreachable(_)) {
                    state.mark_stopped();
                }
                return Err(error);
            }
        };
        Ok(ui_operation_reply_with_local_id(
            reply.into_inner(),
            &operation_id,
        ))
    }

    /// 停止连接（`KernelControl.Stop`）。operation_id 关联契约同 connect。
    pub(crate) async fn stop(&self, state: &CoreState) -> Result<OperationReply, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("stop: core not dialed".to_string()))?;
        let operation_id = random_16()?;
        let request = Request::new(wire_map::stop_request(operation_id.clone())?);
        let mut client = KernelControlClient::new(channel);
        let reply = client.stop(request).await.map_err(map_status)?;
        Ok(ui_operation_reply_with_local_id(
            reply.into_inner(),
            &operation_id,
        ))
    }

    /// 拉取当前运行时快照（`KernelControl.GetSnapshot`），并更新缓存。
    ///
    /// stats-wire 方案 A：快照携带最新统计——`snapshot` 同时更新 `last_snapshot`
    /// 与 `last_stats` 缓存。
    pub(crate) async fn snapshot(&self, state: &CoreState) -> Result<RuntimeSnapshot, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("snapshot: core not dialed".to_string()))?;
        let mut client = KernelControlClient::new(channel);
        let reply = match client
            .get_snapshot(wire::SnapshotRequest {
                runtime_epoch: Vec::new(),
            })
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

    /// 拉取日志历史分片（`KernelControl.LogsList`）。
    pub(crate) async fn logs_list(
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

    /// 清空 core 的持久化日志（`KernelControl.LogsClear`）。
    pub(crate) async fn logs_clear(&self, state: &CoreState) -> Result<LogsClearReply, AppError> {
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

    /// 读取配置（`KernelControl.ConfigGet`）。
    pub(crate) async fn config_get(&self, state: &CoreState) -> Result<ConfigPayload, AppError> {
        let channel = state
            .channel()
            .ok_or_else(|| AppError::CoreUnreachable("config_get: core not dialed".to_string()))?;
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .config_get(wire::ConfigGetRequest {})
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(ConfigPayload {
            items: inner_config_items(reply.items),
            requires_quick_start: reply.requires_quick_start,
        })
    }

    /// 写入配置（`KernelControl.ConfigSet`）。
    pub(crate) async fn config_set(
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

    /// 拉取当前归一化统计（stats-wire 方案 A：从 `snapshot` 命令维护的缓存读取；
    /// 尚无样本返回 typed NotWired 占位，前端保持「暂无统计」不视为失败）。
    pub(crate) async fn stats(&self, state: &CoreState) -> Result<RuntimeStats, AppError> {
        state.cached_stats().ok_or_else(|| {
            AppError::NotWired(
                "stats: no statistics yet (snapshot carries none; connect to start the data plane)"
                    .into(),
            )
        })
    }

    /// 应答交互提示（`KernelControl.RespondInteraction`）。
    ///
    /// 诚实语义：darwin core 该 RPC 当前返回 typed `Unimplemented`——原样透传为
    /// typed `Internal` 错误，前端走既有认证失败/内部错误面，不伪造成功。
    pub(crate) async fn respond_interaction(
        &self,
        state: &CoreState,
        interaction_id: Vec<u8>,
        response_payload: Vec<u8>,
    ) -> Result<OperationReply, AppError> {
        let channel = state.channel().ok_or_else(|| {
            AppError::CoreUnreachable("respond_interaction: core not dialed".to_string())
        })?;
        let request = Request::new(wire_map::interaction_response(
            interaction_id,
            response_payload,
        ));
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .respond_interaction(request)
            .await
            .map_err(map_status)?;
        Ok(wire_map::operation_reply_from_wire(&reply.into_inner()))
    }

    /// 服务控制（`KernelControl.ServiceControl`；W3-4/P4 v1 起透传 core）。
    ///
    /// query 返回 core 服务代理健康探测的五态事实（既有 wire 词汇）；变更动作
    /// （install/uninstall/start/rotate_key）由 core typed 拒绝并携带 CLI 指引，
    /// 经 [`map_status`] 透传为 `Internal`（稳定码 + message，前端 ServicePanel 走
    /// 既有错误面）——壳层不再旁路伪造。
    pub(crate) async fn service_control(
        &self,
        state: &CoreState,
        action: ServiceControlAction,
    ) -> Result<ServiceControlReply, AppError> {
        let channel = state.channel().ok_or_else(|| {
            AppError::CoreUnreachable("service_control: core not dialed".to_string())
        })?;
        let mut client = KernelControlClient::new(channel);
        let reply = client
            .service_control(wire_map::service_control_request(&action))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(wire_map::service_control_reply_from_wire(&reply))
    }

    /// 打开 `KernelControl.WatchEvents` server-streaming 订阅（事件订阅源）。
    ///
    /// `resume_tick == 0` 表示从当前快照开始；`> 0` 时 core 按断线重放语义先补发
    /// 当前快照再转发现场事件（W3-1/P1 对齐 win32 `EventBus::subscribe`）。
    pub(crate) async fn open_watch_stream(
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

/// 16 字节随机 id（与 `wire::random_16` 同源；命令层独立入口便于测试）。
fn random_16() -> Result<Vec<u8>, AppError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| AppError::Internal("entropy unavailable".to_string()))?;
    Ok(bytes.to_vec())
}

/// wire 配置项 → UI 配置项（机械映射）。
fn inner_config_items(items: Vec<wire::ConfigItem>) -> Vec<ConfigItem> {
    items
        .into_iter()
        .map(|i| ConfigItem {
            key: i.key,
            value: i.value,
        })
        .collect()
}
