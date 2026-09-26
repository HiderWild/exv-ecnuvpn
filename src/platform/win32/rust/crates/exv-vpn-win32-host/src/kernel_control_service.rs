

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::sync::{Mutex, watch};
use tokio_stream::{Stream, StreamExt};
use tonic::{Code, Request, Response, Status};
use uuid::Uuid;

use sha2::{Digest, Sha256};

use exv_engine::service::{SERVICE_CONTROL_PIPE, SERVICE_NAME};
use exv_engine::service_batch::{
    BatchStep, MAX_REQUEST_BYTES, ServiceBatchRequest, ServiceBatchResult,
};
use exv_vpn_data_plane::teardown::TeardownSide;
use exv_vpn_domain::error::VpnError;
use exv_vpn_domain::identity::{ConnectionBindingDigest, OperationMethod, PrincipalDigest};
use exv_vpn_domain::ports::{AuthorityEpoch, MonotonicTick};
use exv_vpn_resource::authority::{
    ConnectionBinding, PeerCapability, PeerContext, VerifiedConnectionMetadata,
};
use exv_vpn_win32_config::{
    ConnectionMode, ExvConfig, load_for_startup, save_after_user_submission,
};
use exv_vpn_win32_ipc::peer_auth::{SYSTEM_SID, VerifiedPipePeer, current_user_sid};
use exv_vpn_win32_ipc::pipe_security::PipeSecurity;
use exv_vpn_win32_ipc::service_key::read_service_psk;
use exv_vpn_win32_resource::native_error::NativeError;
use exv_vpn_win32_resource::proxy_tun::{self, ProxyTunDetection};
use exv_vpn_win32_resource::system_proxy::{
    self, SystemProxyMode, SystemProxySnapshot, TopologyKind,
};
use exv_vpn_wire::generated::kernel_control_server::{KernelControl, KernelControlServer};
use exv_vpn_wire::generated::{
    self as wire, ApplyTunnelReply, ApplyTunnelRequest, ConfigGetRequest, ConfigItem,
    ConfigPayload, ConfigReply, ConfigSetRequest, ConnectPhase, ConnectRequest,
    GetKernelOperationRequest, GetOperationRequest, InteractionResponse, KernelOperationReply,
    LogsClearReply, LogsClearRequest, LogsListReply, LogsListRequest, ObserveOwnedStateRequest,
    OperationReply, OperationState, ReconcileRequest, RuntimeEvent, RuntimeSnapshot,
    ServiceControlReply, ServiceControlRequest, ServiceSelfReport, SnapshotRequest, StopRequest,
    StopTunnelRequest, WatchEventsRequest, service_control_request,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    SetFileSecurityW,
};
use windows::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, WriteFile,
};

use crate::composition::{HostComposition, HostEffect, HostEvent, HostPhase};
use crate::credential::{
    CredentialError, CredentialPackage, SECRET_PAYLOAD_VERSION, build_connect_request,
    load_credentials, parse_secret_payload, zeroize_connect_secret,
};
use crate::engine_lifecycle::EngineSlot;
use crate::grpc_control::{
    EngineStatusEvent, GrpcClientError, KernelEngineControl, StatsErrorKind, StatsStreamItem,
};
use crate::kernel_control::ClearableSecret;
use crate::log_aggregator::LogAggregator;
use crate::network_diagnostics::{NetworkMonitor, SnapshotContext, request_snapshot};
use crate::process_lifecycle::{
    ENGINE_ADAPTER_NAME, EngineChild, engine_bin_path, spawn_engine_elevated,
};
use crate::service_status::{
    HealthState, RealServiceStatusSource, ServiceState, ServiceStatusSnapshot, ServiceStatusSource,
    derive_health, derive_health_with_self_report, query_service_status,
    service_health_from_snapshot,
};
use crate::stats::{RuntimeStats, TrafficSample, normalize_stats};
use zeroize::Zeroize;

/// 状态订阅事实只对它所绑定的 engine 代次成立。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusAttachment {
    generation: u64,
    attached: bool,
}

impl StatusAttachment {
    /// 撤销旧流时不能覆盖已发布的新代次。
    fn revoke(sender: &watch::Sender<Self>, generation: u64) {
        sender.send_if_modified(|state| {
            if state.generation != generation || !state.attached {
                return false;
            }
            state.attached = false;
            true
        });
    }

    /// 路由换点和转发器都可以报告新目标；同代次已完成的 attach 不被重复撤销。
    fn advance(sender: &watch::Sender<Self>, generation: u64) {
        sender.send_if_modified(|state| {
            if state.generation >= generation {
                return false;
            }
            *state = Self {
                generation,
                attached: false,
            };
            true
        });
    }
}

static NEXT_STATUS_FORWARDER_ID: AtomicU64 = AtomicU64::new(1);

/// `run_connect` 在自动重连 admission 线性化点发现原会话已失效时的内部标记。
/// 仅供同进程 worker 分流：该路径没有改变状态机或 dispatch，不能按连接失败收束。
const AUTO_RECONNECT_SUPERSEDED: &str = "auto_reconnect_superseded";

/// 只描述 host 本次连接的派发边界，不把 SCM 生命周期并入连接状态机。
struct ConnectDispatch {
    id: Uuid,
    cancelled: watch::Sender<bool>,
    target_generation: Option<u64>,
    /// 当前 attempt 自己是否已经跨过 Apply 派发边界。
    attempt_applied: bool,
    /// 当前生命周期是否仍欠真实停止；可以继承前一个重叠 attempt 的义务。
    stop_required: bool,
}

async fn connect_cancelled(cancelled: &mut watch::Receiver<bool>) {
    while !*cancelled.borrow_and_update() {
        if cancelled.changed().await.is_err() {
            return;
        }
    }
}

/// permit 在任务 future 被取消或销毁时释放；所有服务克隆共享同一 owner 锁。
struct StatusForwarderOwner {
    permit: Option<tokio::sync::OwnedMutexGuard<()>>,
    attachment: watch::Sender<StatusAttachment>,
    logs: Arc<LogAggregator>,
    selected_mode: Arc<AtomicU8>,
    core_session_id: Uuid,
    forwarder_id: u64,
    engine_generation: u64,
    attach_attempt: u64,
    reattach_reason: &'static str,
    backoff: Duration,
}

impl StatusForwarderOwner {
    fn log(&self, level: &str, code: &str, message: &str) {
        let _ = self.logs.append_core(
            level,
            "kernel",
            code,
            message,
            &BTreeMap::from([
                ("core_pid".to_string(), std::process::id().to_string()),
                (
                    "core_session_id".to_string(),
                    self.core_session_id.to_string(),
                ),
                ("forwarder_id".to_string(), self.forwarder_id.to_string()),
                (
                    "engine_generation".to_string(),
                    self.engine_generation.to_string(),
                ),
                (
                    "attach_attempt".to_string(),
                    self.attach_attempt.to_string(),
                ),
                (
                    "reattach_reason".to_string(),
                    self.reattach_reason.to_string(),
                ),
                (
                    "backoff_ms".to_string(),
                    self.backoff.as_millis().to_string(),
                ),
                (
                    "route_mode".to_string(),
                    ServiceMode::from_u8(self.selected_mode.load(Ordering::Relaxed))
                        .as_wire_str()
                        .to_string(),
                ),
                (
                    "failure_category".to_string(),
                    match code {
                        "kernel.forward.status_attach_failed" => "status_subscribe",
                        "kernel.forward.status_eof" => "stream_eof",
                        _ => "none",
                    }
                    .to_string(),
                ),
            ]),
        );
    }

    fn slot_swapped(&mut self, generation: u64) {
        StatusAttachment::revoke(&self.attachment, self.engine_generation);
        StatusAttachment::advance(&self.attachment, generation);
        self.reattach_reason = "slot_swap";
        self.backoff = ENGINE_EVENT_RECONNECT_BASE;
        self.log(
            "info",
            "kernel.forward.status_slot_swapped",
            "status forwarder observed engine slot swap; retrying immediately",
        );
    }

    async fn wait_for_retry(
        &mut self,
        swaps: &mut watch::Receiver<u64>,
        reason: &'static str,
    ) -> bool {
        self.reattach_reason = reason;
        tokio::select! {
            biased;
            changed = swaps.changed() => {
                if changed.is_err() {
                    return false;
                }
                self.slot_swapped(*swaps.borrow_and_update());
            }
            _ = tokio::time::sleep(self.backoff) => {
                self.backoff = (self.backoff * 2).min(ENGINE_EVENT_RECONNECT_MAX);
            }
        }
        true
    }
}

impl Drop for StatusForwarderOwner {
    fn drop(&mut self) {
        if self.permit.is_some() {
            StatusAttachment::revoke(&self.attachment, self.engine_generation);
            self.reattach_reason = "owner_exit";
            self.backoff = Duration::ZERO;
            self.log(
                "info",
                "kernel.forward.status_exited",
                "status forwarder owner exited",
            );
        }
    }
}

/// `KernelControl` 服务。
///
/// 持有唯一的 host composition、共享 engine 控制面（写路径派发；测试注入 fake）、
/// 凭据目录、共享日志聚合器与 `WatchEvents` 事件总线（[`EventBus`]）。构造为
/// [`KernelControlServer`] 后即可挂到 tonic `Server`。
///
/// `Clone` = 轻量共享句柄：所有字段为 `Arc`/`Copy`/克隆，克隆体与本体共享同一内部
/// 状态（C3a 重连 worker 持一个克隆调用 `run_connect`/路由等 `&self` 方法）。
#[derive(Clone)]
pub struct KernelControlService {
    /// 唯一的 host composition（写路径经 gate；读路径查 phase）。
    composition: Arc<Mutex<HostComposition>>,
    /// 共享 engine 控制面槽（P3 崩溃自愈换点；写路径派发经 [`EngineSlot::current`]
    /// 取当前 engine——respawn/provision 换入新 client 后写路径自动指向新 engine）。
    /// 按需拉起模型（2026-09-08）：空态 = 规范 detached 占位，无隐藏常驻 oneshot。
    engine: EngineSlot,
    /// oneshot engine 按需拉起编排（2026-09-08 计划批 2）：connect 路由 Oneshot 分支
    /// 经它「复用或提权拉起」。`None` = 未接线（测试注入槽内 fake 的路径——保持
    /// 既有语义，槽内即目标 engine）。
    engine_provisioner: Option<Arc<crate::engine_provisioner::EngineProvisioner>>,
    /// 凭据目录（测试可注入；产品默认目录由 `ExvConfig::load()` 语义给出）。
    config_dir: PathBuf,
    /// 共享日志聚合器（组合时与 `LogControlService` 共享 Arc；写路径产出 core 事件）。
    logs: Arc<LogAggregator>,
    /// `WatchEvents` 事件总线（P3-c1 真实订阅；engine 转发器发布，UI 订阅）。
    events: Arc<EventBus>,
    /// C5-wire：上游 proxy TUN 检测探针（默认真实枚举；测试注入确定性结果）。
    proxy_tun_probe: ProxyTunProbe,
    /// EXV_UNFREEZE 系统代理感知：系统代理探测探针（默认真实 WinINET 注册表捕获 +
    /// 拓扑分类；测试注入确定性结果）。
    system_proxy_probe: SystemProxyProbe,
    /// 当前状态流的 engine 代次与挂接事实。
    status_attachment: watch::Sender<StatusAttachment>,
    /// 同一个 core 会话（包括服务克隆）只允许一个活动状态转发器。
    status_forwarder_owner: Arc<Mutex<()>>,
    /// 同一段查询失联仅记一次告警；成功后恢复，避免每秒轮询刷屏。
    snapshot_query_failed: Arc<AtomicBool>,
    core_session_id: Uuid,
    /// 同步短锁；只在 composition 或 engine 之后取得，持锁期间没有 await。
    connect_dispatch: Arc<std::sync::Mutex<Option<ConnectDispatch>>>,
    /// 已取消的路由允许完成启动，但必须先退出才能开始下一次路由，避免迟到换槽。
    connect_route_lock: Arc<Mutex<()>>,
    /// R3-C1 `await_status_ready` 的有界等待上界（默认 [`STATUS_READY_WAIT`]；测试
    /// 注入短时长验证不悬挂语义——连接超时明确失败；已派发隧道的 Stop 仍负责清理）。
    status_ready_wait: Duration,
    /// S3/D5：非提权 SCM 服务状态源（默认真实 `OpenSCManagerW`；测试注入确定性结果）。
    service_status_source: Arc<dyn ServiceStatusSource>,
    /// S3/D4：服务变更操作 seam（install/uninstall/start/stop——engine 子命令 runas；
    /// 测试注入 fake 记录语义）。
    service_ops: Arc<dyn ServiceControlOps>,
    /// S3/D3：service engine 连接 seam（真实实现 = 读 PSK + 拨号稳定服务管道 +
    /// SID-only 验证 + PSK-HMAC 挑战 + 返回客户端；测试注入 fake 换入 fake engine，
    /// 避免单测依赖真实 PSK 文件与服务管道）。
    service_connector: Arc<dyn ServiceEngineConnector>,
    /// R2：keepalive 就绪探活 seam（服务 Start 后 `wait_for_service_ready` 用；默认 =
    /// [`RealServiceProbe`]，测试注入 fake 返回确定性结果）。
    service_probe: Arc<dyn ServiceProbe>,
    /// R2：服务就绪轮询上界（默认 [`SERVICE_READY_TIMEOUT`]；测试注入短时长）。
    service_ready_timeout: Duration,
    /// R2：服务就绪轮询间隔（默认 [`SERVICE_READY_POLL`]；测试注入短间隔）。
    service_ready_poll: Duration,
    /// S3/D3：最近一次连接路由的实际模式（0=auto / 1=service / 2=oneshot；快照 `mode`
    /// 展示用，缺省 auto）。
    selected_mode: Arc<AtomicU8>,
    /// S3/D5：`DeleteService` 成功后的语义状态覆盖。
    ///
    /// Windows SCM 在删除成功后可能短暂保留 marked-for-delete 条目；在这段窗口内
    /// `OpenServiceW` 仍可能成功。Core 已经完成删除事务时，不能把这个中间 SCM 事实
    /// 再回传成"已安装"，否则 UI 会显示 completed 但仍提供卸载按钮。
    service_removed_override: Arc<AtomicBool>,
    /// 服务生命周期变更串行门，防止重复点击/并发 RPC 交叉执行 stop、install、uninstall。
    service_control_lock: Arc<tokio::sync::Mutex<()>>,
    /// C3a 自动重连：per-connection 重连尝试计数（每次自动重连派发递增；Connected 成功
    /// 后由状态转发器清零；`auto_reconnect_max_attempts` 耗尽即停止）。
    reconnect_attempts: Arc<AtomicU32>,
    /// C3a 自动重连：单次重连尝试在途标记（worker 派发后置 true；终态事件（Connected/
    /// Failed/Stopped）由状态转发器清 false——在途期间新的可重试掉线不重复触发，防重入）。
    reconnect_active: Arc<AtomicBool>,
    /// C3a 自动重连：状态转发器 → 重连 worker 的触发通道发送端（engine 掉线事件为
    /// 触发源；worker 拉起时换入自己的发送端并串行消费——天然杜绝并发重连；未拉起
    /// worker 时 `None`，信号被丢弃）。
    reconnect_tx: Arc<std::sync::Mutex<Option<mpsc::UnboundedSender<()>>>>,
    /// 已受理连接的重连策略。它在用户点击连接时从已保存配置读取并冻结；连接期间的
    /// 设置保存只影响下一次用户连接，绝不能改变本会话的重连行为或 UI 投影。
    reconnect_session_policy: Arc<std::sync::Mutex<Option<ReconnectSessionPolicy>>>,
    /// 旧配置缓存仍由凭据持久化路径维护，供历史兼容测试使用；运行中重连不再读取它。
    reconnect_config_cache: Arc<ReconnectConfigCache>,
    /// reconnect-backoff（2026-09-05）：自动重连退避 base（产品默认
    /// [`RECONNECT_BACKOFF_BASE`] = 2s；测试经 [`Self::with_reconnect_backoff`] 注入
    /// 短时长——生产路径不改默认）。
    reconnect_backoff_base: Duration,
    /// reconnect-backoff cap（产品默认 [`RECONNECT_BACKOFF_CAP`] = 30s；同上 seam）。
    reconnect_backoff_cap: Duration,
    /// RT-DIAG-03：统计转发器首样本期限（码 6 预算；默认
    /// [`STATS_FIRST_SAMPLE_TIMEOUT`] = 3000ms；测试经 builder seam 注入短时长）。
    stats_first_sample_timeout: Duration,
}

/// `WatchEvents` 的 server-streaming 返回类型（P3-c1：真实订阅流，非单事件骨架）。
type WatchEventsStream = Pin<Box<dyn Stream<Item = Result<RuntimeEvent, Status>> + Send>>;

/// C5-wire：可注入的上游 proxy TUN 检测探针（默认真实 `GetAdaptersAddresses` 枚举；
/// 单测注入确定性结果，避免依赖真实 Win32 适配器状态）。
type ProxyTunProbe = Arc<dyn Fn() -> Result<ProxyTunDetection, NativeError> + Send + Sync>;

/// 默认真实探针：以 EXV 引擎适配器名（`ExvEngine`）为排除基准检测上游 proxy TUN。
fn default_proxy_tun_probe() -> ProxyTunProbe {
    Arc::new(|| proxy_tun::detect_upstream_proxy_tun(ENGINE_ADAPTER_NAME))
}

/// EXV_UNFREEZE 系统代理感知：可注入的系统代理探测探针（默认真实 WinINET 注册表
/// 捕获 + 拓扑分类；单测注入确定性结果，避免依赖真实注册表状态）。返回完整的
/// wire [`SystemProxyDetection`]（mode / endpoint_count / bypass_merged / 四态 topology）。
type SystemProxyProbe =
    Arc<dyn Fn() -> Result<wire::SystemProxyDetection, NativeError> + Send + Sync>;

/// 默认真实探针：当前进程用户 SID → `capture_for_user`（`HKU\<sid>\…\Internet Settings`
/// 五原始值）→ `snapshot_from_raw`（规范化快照）→ 转 wire。
///
/// 拓扑是 `(proxy_present, tunnel_present)` 的纯函数分类（设计 §2）；TUN 维度复用
/// `proxy_tun::detect_upstream_proxy_tun` 同源探测（与 proxy TUN 探针同一排除基准；
/// 探测失败按「无 TUN」保守处理，拓扑退 T0/T1 不虚报）。探测/解析失败 → typed 错误，
/// 由刷新点按 `None` 处理（状态上报不因探测失败而失败）。
fn default_system_proxy_probe() -> SystemProxyProbe {
    Arc::new(|| {
        let sid = current_user_sid().ok_or_else(|| {
            NativeError::from_win32(0, "当前进程用户 SID 不可解析（系统代理探测）")
        })?;
        let raw = system_proxy::capture_for_user(&sid)?;
        let snapshot = system_proxy::snapshot_from_raw(&raw)?;
        let tunnel_present = proxy_tun::detect_upstream_proxy_tun(ENGINE_ADAPTER_NAME)
            .map(|d| d.detected)
            .unwrap_or(false);
        Ok(system_proxy_snapshot_to_wire(&snapshot, tunnel_present))
    })
}

/// S3/D3 + M3：auto 决策表（core 侧路由连接）。服务 bootstrap 由 Core 统一维护，
/// 因此 `connect` 可在已安装但 stopped 时自动拉起服务。
///
/// 三态：
/// - 已装 + 在跑 → [`RouteDecision::Service`]（连接服务 engine）；
/// - 已装 + 未跑 → [`RouteDecision::PromptStart`]（需要 Core bootstrap 服务）；
/// - 未装 → [`RouteDecision::Oneshot`]（维持既有 oneshot 路径）。
/// 连接路由决策收编于 [`crate::guards`]（正交守卫层）——`RouteDecision`/`decide_route`
/// 单一权威，本模块 re-export 保持既有调用点与测试不变。
pub use crate::guards::{RouteDecision, ServiceMode, decide_route};

// ---------------------------------------------------------------------------
// R2 + 双采纳（2026-08-21 计划 Task 3 合同，dual-adoption 计划 4.1 冻结）：keepalive
// 就绪探活（服务 Start 后判定 engine 是否业务就绪）。就绪判定 = SCM running **且**
// keepalive 回复同一拍联言成立；任一单方事实只继续轮询，不产生 ready。
// ---------------------------------------------------------------------------

/// R2 默认服务就绪等待上界（`wait_for_service_ready`；服务 Start 后探活）。
pub const SERVICE_READY_TIMEOUT: Duration = Duration::from_secs(15);
/// R2 默认服务就绪轮询间隔。
///
/// SCM 查询是非提权的廉价本地调用；100ms 能把"服务已在 services.msc 出现/进入
/// Running"与 UI 反馈之间的观察窗口压到亚秒级，同时仍由 `SERVICE_READY_TIMEOUT`
/// 提供失败上界。
pub const SERVICE_READY_POLL: Duration = Duration::from_millis(100);
/// R2 单次 keepalive 探活 RPC 超时上界（`RealServiceProbe`）。
pub const SERVICE_PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);

/// R2 就绪达成来源（`ServiceReadiness.source`）。
///
/// 双采纳合同下值域收缩为两值：`ready=true ⇔ source==Both`。单方事实（仅 SCM
/// running、或仅 keepalive 回复）不再有专属枚举值——它们不产生 ready，只进入轮询
/// 下一拍或 `Timeout`（编译期暴露旧「先到先采纳」消费点，防止旧语义复活）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessSource {
    /// 同一拍观察到 SCM Running **且** keepalive 回复（唯一 ready 形态）。
    Both,
    /// 有界轮询到期，双事实从未在同一拍成立。
    Timeout,
}

/// R2 服务就绪判定结果（`wait_for_service_ready` 的产出；`ServiceControlReply` 的输入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceReadiness {
    /// engine 是否业务就绪（= 某一拍「SCM Running 且 keepalive 回复」联言达成）。
    pub ready: bool,
    /// 达成 / 失败来源（`ready=true ⇔ Both`；`ready=false` 恒为 `Timeout`）。
    pub source: ReadinessSource,
    /// 轮询窗口内最后一次 SCM 快照状态（`None` = SCM 查询失败 / 未查到）。
    pub scm_state: Option<ServiceState>,
    /// 轮询窗口内是否曾收到 keepalive 回复（诊断字段，**不参与 ready 判定**）。
    pub keepalive: bool,
    /// 轮询经过的毫秒数。
    pub elapsed_ms: u64,
}

/// 单次业务操作（一次 connect / 一次显式 Start）的修复子进程预算（2026-09-08 计划
/// 批 4）：bootstrap 或 service 连接观察到服务客观问题时，至多升级**一次** runas 修复
/// 批量（1 次 UAC）——预算耗尽后原样上抛真实错误，不做修复循环。
#[derive(Debug, Default)]
struct RepairBudget {
    spent: bool,
}

impl RepairBudget {
    /// 尝试消耗一次修复名额：已消耗 → `false`（不再修复）。
    fn try_spend(&mut self) -> bool {
        if self.spent {
            false
        } else {
            self.spent = true;
            true
        }
    }
}

/// R2 keepalive 就绪探活 seam（测试注入 fake 返回确定性结果；产品 =
/// [`RealServiceProbe`]）。
#[tonic::async_trait]
pub trait ServiceProbe: Send + Sync {
    /// 执行一次 keepalive 探活：`true` = engine 已回复（业务就绪）。
    async fn probe(&self) -> bool;

    /// S3-B：执行一次 ServiceManage.query 深度自述探活（Tier 2 零 UAC 通道，健康加深的
    /// 源）。返回 engine 自述报告；`None` = 探针失败/未达成（拨号/PSK/超时/RPC 拒绝）——
    /// **保守保留 SCM 派生**，不把探针失败当引擎失败。
    async fn probe_self_report(&self) -> Option<ServiceSelfReport>;
}

/// 真实探活：读 `%ProgramData%\exv\service.key` → 拨号稳定服务管道 → 一条 `KeepAlive` RPC。
///
/// `true` = keepalive 已回复。PSK 不可读 / 拨号失败 / RPC 失败 / 超时 → `false`（本次
/// 探活未达成，由轮询下一拍重试——服务刚启动未绑管道的竞窗由
/// [`probe_service_engine`](crate::grpc_control::probe_service_engine) 内部的有界重试覆盖）。
pub struct RealServiceProbe;

#[tonic::async_trait]
impl ServiceProbe for RealServiceProbe {
    async fn probe(&self) -> bool {
        let Ok(psk) = read_service_psk() else {
            return false;
        };
        crate::grpc_control::probe_service_engine(
            SERVICE_CONTROL_PIPE,
            SYSTEM_SID,
            &psk,
            SERVICE_PROBE_ATTEMPT_TIMEOUT,
        )
        .await
        .is_ok()
    }

    async fn probe_self_report(&self) -> Option<ServiceSelfReport> {
        // 复用 keepalive 探活的同一通道（`SERVICE_CONTROL_PIPE` + `SYSTEM_SID` + PSK +
        // `SERVICE_PROBE_ATTEMPT_TIMEOUT`）；失败保守（`query_service_self` 内部不抛错）。
        let psk = read_service_psk().ok()?;
        crate::grpc_control::query_service_self(
            SERVICE_CONTROL_PIPE,
            SYSTEM_SID,
            &psk,
            SERVICE_PROBE_ATTEMPT_TIMEOUT,
        )
        .await
    }
}

/// R2 服务就绪判定（双采纳合同，4.1 真值表冻结）：SCM running **且** keepalive 回复
/// **同一拍**联言成立才 ready；有界轮询。
///
/// 每拍同时采集两事实：先查询 SCM（廉价、非提权；查询失败按「非 Running」处理，仍进入
/// 探活分支），再执行一次有界 keepalive 探活。任一单方事实都不返回 ready——SCM Running
/// 而管道未被证明、或探活先行而 SCM 未收敛，都继续下一拍（管道可能下一拍就绪 / SCM
/// 可能下一拍收敛）。单次探活不会占住整个 3s 的底层拨号重试：外层以一个轮询拍长为上界
///（`max(poll_interval, 1ms).min(SERVICE_PROBE_ATTEMPT_TIMEOUT)`），悬挂探活不阻塞 SCM
/// 轮询。`timeout` 到期仍未同拍达成 → `ready=false`（`source=Timeout`，携带最后一次
/// SCM 状态与 keepalive 事实，供调用方按 4.2 分类产出可读信息）。
pub async fn wait_for_service_ready(
    source: &dyn ServiceStatusSource,
    probe: &dyn ServiceProbe,
    timeout: Duration,
    poll_interval: Duration,
) -> ServiceReadiness {
    let start = std::time::Instant::now();
    let mut scm_state: Option<ServiceState> = None;
    let mut keepalive = false;
    loop {
        let elapsed = start.elapsed();
        // 事实 1：SCM 快照（查询失败 Err 按「非 Running」处理，进入探活分支）。
        let scm_running = match query_service_status(source, SERVICE_NAME) {
            Ok(snap) => {
                scm_state = Some(snap.state);
                snap.state == ServiceState::Running
            }
            Err(_) => false,
        };
        // 事实 2：一次有界 keepalive 探活（拍长为硬上界，悬挂不阻塞下一拍 SCM 查询）。
        let probe_deadline = poll_interval
            .max(Duration::from_millis(1))
            .min(SERVICE_PROBE_ATTEMPT_TIMEOUT);
        let replied_this_tick = tokio::time::timeout(probe_deadline, probe.probe())
            .await
            .unwrap_or(false);
        if replied_this_tick {
            keepalive = true;
        }
        // 4.1 真值表：仅「同一拍 SCM Running 且探活回复」联言成立才 ready。
        if scm_running && replied_this_tick {
            return ServiceReadiness {
                ready: true,
                source: ReadinessSource::Both,
                scm_state,
                keepalive,
                elapsed_ms: elapsed.as_millis() as u64,
            };
        }
        if elapsed >= timeout {
            return ServiceReadiness {
                ready: false,
                source: ReadinessSource::Timeout,
                scm_state,
                keepalive,
                elapsed_ms: elapsed.as_millis() as u64,
            };
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// 4.2 失败分类（冻结）：`source=Timeout` 的 [`ServiceReadiness`] → 稳定机器前缀 +
/// 中文事实的可读信息（无秘密、无栈）。前缀按超时时事实组合分流：
///
/// - `scm_state == Running` 且 `keepalive == false` → `service_not_ready|`（SCM 在跑
///   但控制面探活未响应——管道未被证明）；
/// - SCM 非 Running / 未知 且 `keepalive == false` → `service_not_running|`；
/// - `keepalive == true`（曾回复但从未与 Running 同拍）→ `service_not_running|`
///  （SCM 侧未收敛；message 如实携带最后 SCM 状态与 keepalive 事实）。
///
/// 稳定前缀唯一产生层 = 本函数（[`KernelControlService::ensure_service_ready`] 与
/// `service_control_start_reply` 共用）；任何外层不得再叠加前缀。
fn readiness_failure_message(readiness: &ServiceReadiness) -> String {
    let scm_desc = match readiness.scm_state {
        Some(state) => format!("最后 SCM 状态={state:?}"),
        None => "最后 SCM 状态未知".to_string(),
    };
    let elapsed = readiness.elapsed_ms;
    if readiness.keepalive {
        format!(
            "service_not_running|服务未就绪（{elapsed}ms）：keepalive 探活曾回复，\
             但未与 SCM Running 同拍达成（{scm_desc}）"
        )
    } else if readiness.scm_state == Some(ServiceState::Running) {
        format!(
            "service_not_ready|服务业务面未就绪（{elapsed}ms）：SCM 状态=Running，\
             但控制面探活未响应"
        )
    } else {
        format!("service_not_running|服务未运行（{elapsed}ms）：{scm_desc}，keepalive 未响应")
    }
}

/// S3/D4：服务变更操作 seam（install/uninstall/start/stop）。
///
/// 真实实现 = engine 子命令经 runas 提权（D4：host 非提权，SCM 操作在 engine 内）；
/// 测试注入 fake 记录调用并返回确定性结果。
#[tonic::async_trait]
pub trait ServiceControlOps: Send + Sync {
    /// 安装/修复 engine SCM 服务（重装轮换 PSK，M14）。
    async fn install(&self) -> Result<String, String>;
    /// 卸载 engine SCM 服务。
    async fn uninstall(&self) -> Result<String, String>;
    /// 启动 engine SCM 服务。
    async fn start(&self) -> Result<String, String>;
    /// 轮换（撤销）服务 PSK（2026-09-05 撤销计划：提权批量 `[RotateKey, Verify]`，
    /// 写新密钥 + 确认 SCM 仍 Running，**无需重启服务**）。
    ///
    /// 成功 = 携带 RotateKey 步骤消息（`rotated fingerprint=<16hex>`，非秘密摘要）。
    ///
    /// # Errors
    /// 批量失败（UAC 拒绝 / 写入失败 / Verify 失败）→ 携带原因的字符串（旧 key
    /// 仍有效，诚实失败）。
    async fn rotate_key(&self) -> Result<String, String>;
}

/// 真实服务操作：以 runas 提权拉起 engine 批量执行完整服务操作序列并等待退出
///（D4 边界——host 非提权，engine 是唯一特权进程；一次 runas = 1 次 UAC，S2-B）。
pub struct EngineSubcommandServiceOps {
    /// 批量提权 spawn（生产 = ShellExecuteExW runas；测试注入 fake 记录请求并写回结果）。
    spawn: Arc<dyn ServiceBatchSpawn>,
}

impl EngineSubcommandServiceOps {
    /// 真实批量 spawner（ShellExecuteExW runas）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            spawn: Arc::new(RealServiceBatchSpawn),
        }
    }
}

impl Default for EngineSubcommandServiceOps {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl ServiceControlOps for EngineSubcommandServiceOps {
    async fn install(&self) -> Result<String, String> {
        // 安装/修复 = 一次 runas 完成 [Install,Start,Verify]（含 REPAIR + SCM 启动 +
        // keepalive 探活）。config_dir 经批量请求 JSON 传递（engine `RealServiceOps`
        // 从请求读并写入 SCM 启动参数）。
        run_service_batch(
            vec![BatchStep::Install, BatchStep::Start, BatchStep::Verify],
            self.spawn.as_ref(),
        )
        .await
    }
    async fn uninstall(&self) -> Result<String, String> {
        run_service_batch(
            vec![BatchStep::Uninstall, BatchStep::VerifyRemoved],
            self.spawn.as_ref(),
        )
        .await
    }
    async fn start(&self) -> Result<String, String> {
        run_service_batch(
            vec![BatchStep::Start, BatchStep::Verify],
            self.spawn.as_ref(),
        )
        .await
    }
    async fn rotate_key(&self) -> Result<String, String> {
        // 轮换（撤销）= 一次 runas 完成 [RotateKey, Verify]：批量进程覆盖写新 PSK
        //（`write_service_psk`，CREATE_ALWAYS + DACL 重建）+ 确认 SCM 仍 Running
        //（无需重启服务；端到端生效证明由后续 core 连接承担）。
        let result = run_service_batch_steps(
            vec![BatchStep::RotateKey, BatchStep::Verify],
            self.spawn.as_ref(),
        )
        .await?;
        // fingerprint 由 RotateKey 步骤消息携带（`rotated fingerprint=<16hex>`）；
        // 形状异常时回退总述（诚实透传，不伪造 fingerprint）。
        Ok(result
            .steps
            .iter()
            .find(|step| step.step == BatchStep::RotateKey)
            .map(|step| step.message.clone())
            .unwrap_or(result.message))
    }
}

// ---------------------------------------------------------------------------
// 服务批量提权入口（服务进程生命周期由批量内 SCM 控制）
// ---------------------------------------------------------------------------


/// 批量提权 spawn seam（生产 = ShellExecuteExW runas；测试注入 fake 记录请求并模拟结果）。
///
/// 返回 `Ok(exit_code)` = 引擎进程已退出（0=ok，非零=fail）；`Err` = 提权 spawn 失败 /
/// 等待超时（调用方按其语义报失败）。
#[tonic::async_trait]
pub trait ServiceBatchSpawn: Send + Sync {
    /// 提权拉起 engine 执行 `--service-batch` 并等待退出，返回退出码。
    ///
    /// # Errors
    /// 提权 spawn 失败 / 超时 → 携带原因的字符串。
    async fn spawn_batch(&self, exe: &Path, args: &[String]) -> Result<i32, String>;
}

/// 批量等待上界（计划文档阶段 2：`SendChild::wait_exit_code(60_000)`）。
const SERVICE_BATCH_TIMEOUT_MS: u32 = 60_000;

/// 真实批量 spawner：`ShellExecuteExW(runas)` 提权拉起 engine `--service-batch`（1 次
/// UAC），有界等待退出（60s）。
pub struct RealServiceBatchSpawn;

#[tonic::async_trait]
impl ServiceBatchSpawn for RealServiceBatchSpawn {
    async fn spawn_batch(&self, exe: &Path, args: &[String]) -> Result<i32, String> {
        let (pid, handle) = spawn_engine_elevated(exe, args)?;
        // EngineChild 含 HANDLE（`*mut c_void`，非 Send）——经 SendChild 移入
        // spawn_blocking（等待/终止/关闭在闭包线程内串行，无并发关闭；镜像原
        // run_service_subcommand 的同一模式）。
        let child = SendChild(EngineChild::new(pid, handle));
        // 必须经方法调用触发 whole-struct 捕获——`move || child.0.wait_exit_code(...)`
        // 的 disjoint capture 会捕获 `child.0`（EngineChild，非 Send），绕过 SendChild
        // 的 Send impl（镜像 engine_lifecycle `WaitHandle` 的同一陷阱）。
        let exit_code =
            tokio::task::spawn_blocking(move || child.wait_exit_code(SERVICE_BATCH_TIMEOUT_MS))
                .await
                .map_err(|e| format!("service batch join: {e}"))?;
        exit_code
            .ok_or_else(|| format!("service batch timed out after {SERVICE_BATCH_TIMEOUT_MS}ms"))
    }
}

/// 批量临时文件清理：host 无论成功/失败都删除 req/res（engine 消费 req 后自删，
/// result 是 host 交付结果的唯一通道——读毕由 host 删；spawn 失败时 req 也可能残留）。
struct BatchFileCleanup<'a> {
    request: &'a Path,
    result: &'a Path,
}

impl Drop for BatchFileCleanup<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.request);
        let _ = std::fs::remove_file(self.result);
    }
}

/// 一次 runas 拉起 engine 批量执行完整服务操作序列（1 次 UAC）。
///
/// 经临时文件传请求/收结果（ShellExecuteExW 无 stdio，计划文档阶段 2）：
/// - **req 文件**：随机 uuid 文件名 + DACL（SYSTEM + 当前用户 SID，复用
///   [`PipeSecurity`] 冻结形状，随 `CreateFileW` 生效并保护继承）；
/// - engine 读 req → 执行 → 写 result → **finally 删 req**；result 文件保留——它是
///   ShellExecuteExW（无 stdio）下向 host 交付结果的唯一通道；
/// - host 在 `wait_exit_code` 后读 result、校验 JSON 形状（`deny_unknown_fields`）、
///   由 host 删除。
///
/// 语义映射：`result.ok && exit == 0` → 成功；否则失败（message 透传）。`--host-pid`
/// 携带自身 pid（孤儿 watchdog 接线由 S2-C 集成测试处理）。
///
/// # Errors
/// bin 缺失 / 请求写入或 DACL 失败 / spawn 失败或超时 / result 缺失或形状非法 /
/// 引擎报告失败 → 携带原因的字符串。
async fn run_service_batch(
    steps: Vec<BatchStep>,
    spawn: &dyn ServiceBatchSpawn,
) -> Result<String, String> {
    run_service_batch_steps(steps, spawn)
        .await
        .map(|result| result.message)
}

/// [`run_service_batch`] 的全结果变体：成功时返回完整 [`ServiceBatchResult`]。
///
/// rotate 需要从 RotateKey 步骤消息提取 fingerprint（总述只有
/// `"batch completed"`）；其余调用方走 [`run_service_batch`] 只消费总述。
///
/// # Errors
/// 同 [`run_service_batch`]：bin 缺失 / 请求写入或 DACL 失败 / spawn 失败或超时 /
/// result 缺失或形状非法 / 引擎报告失败 → 携带原因的字符串。
async fn run_service_batch_steps(
    steps: Vec<BatchStep>,
    spawn: &dyn ServiceBatchSpawn,
) -> Result<ServiceBatchResult, String> {
    let request = ServiceBatchRequest {
        version: 1,
        sequence: steps,
        config_dir: exv_vpn_win32_config::config_dir()
            .to_string_lossy()
            .into_owned(),
    };
    let dir = std::env::temp_dir();
    let req = dir.join(format!("exv-batch-req-{}.json", Uuid::new_v4().simple()));
    let res = dir.join(format!("exv-batch-res-{}.json", Uuid::new_v4().simple()));
    write_batch_request_file(&req, &request)?;
    let _cleanup = BatchFileCleanup {
        request: &req,
        result: &res,
    };
    let exe = engine_bin_path().ok_or_else(|| "engine bin not found".to_string())?;
    let args = vec![
        "--service-batch".to_string(),
        "--request".to_string(),
        req.display().to_string(),
        "--result".to_string(),
        res.display().to_string(),
        "--host-pid".to_string(),
        std::process::id().to_string(),
    ];
    let t0 = Instant::now();
    let exit_code = spawn.spawn_batch(&exe, &args).await?;
    let spawn_elapsed_ms = t0.elapsed().as_millis();
    let result = read_batch_result_file(&res)?;
    let total_elapsed_ms = t0.elapsed().as_millis();
    eprintln!(
        "[exv-host-batch] spawn+wait: steps={:?} exit={} spawn={}ms total={}ms",
        request.sequence, exit_code, spawn_elapsed_ms, total_elapsed_ms
    );
    if result.ok && exit_code == 0 {
        Ok(result)
    } else {
        Err(result.message)
    }
}

/// 写批量请求文件（随机 uuid 路径；DACL = SYSTEM + 当前用户 SID）。
///
/// DACL 复用 [`PipeSecurity`] 冻结形状（WSP1 §4：`D:(A;;GA;;;SY)(A;;GA;;;<user>)`），
/// 随 `CreateFileW` 的 `SECURITY_ATTRIBUTES` 在创建时生效（镜像
/// `write_service_psk` 的文件 DACL 模式）。`CreateFileW` 只复制 descriptor 的显式 DACL，
/// 父目录（temp）的可继承 ACE 仍会合并进来——再用 `SetFileSecurityW` +
/// `PROTECTED_DACL_SECURITY_INFORMATION` 去掉继承 ACE，使文件 DACL 恰为
/// SYSTEM + 当前用户（其余拒绝），且在任何内容写入前完成。
///
/// # Errors
/// 当前用户 SID 不可得 / DACL 构造失败 / `CreateFileW` / `SetFileSecurityW` /
/// `WriteFile` 失败 → 携带原因的字符串。
fn write_batch_request_file(path: &Path, request: &ServiceBatchRequest) -> Result<(), String> {
    let json = serde_json::to_vec(request).map_err(|e| format!("serialize batch request: {e}"))?;
    if json.len() > MAX_REQUEST_BYTES {
        return Err(format!("batch request exceeds {MAX_REQUEST_BYTES} bytes"));
    }
    let sid = current_user_sid().ok_or_else(|| "current user SID unavailable".to_string())?;
    let security = PipeSecurity::new(&sid, true)
        .map_err(|code| format!("batch request DACL build failed (code {code})"))?;
    let attributes = security.as_attributes();
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` 是活的 NUL 结尾宽字符串；`attributes` 是活 `SECURITY_ATTRIBUTES`，
    // 其 `lpSecurityDescriptor` 指向 `security` 拥有的活 descriptor（同作用域存活）。
    let handle = unsafe {
        CreateFileW(
            windows::core::PCWSTR(wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_READ,
            Some(&raw const attributes as *const _),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|e| format!("create batch request file {}: {e}", path.display()))?;
    // 保护 DACL 不受父目录继承（见函数注释）。descriptor 来自 PipeSecurity（活于本作用域）。
    // SAFETY: `wide` 仍存活；`PSECURITY_DESCRIPTOR` 指向 `security` 拥有的活 descriptor。
    let protect = unsafe {
        SetFileSecurityW(
            windows::core::PCWSTR(wide.as_ptr()),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR(attributes.lpSecurityDescriptor),
        )
    };
    if !protect.as_bool() {
        // SAFETY: 无指针参数，读线程错误码。
        let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
        // SAFETY: `handle` 是已打开句柄，使用后关闭。
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Err(format!("protect batch request DACL failed (code {code})"));
    }
    let result = write_all_to_handle(handle, &json);
    // SAFETY: `handle` 是 `CreateFileW` 返回的已打开句柄，使用后关闭。
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.map_err(|e| format!("write batch request {}: {e}", path.display()))
}

/// 一次性写满 `buf` 到句柄（`WriteFile`；短写视为失败——请求必须完整落盘）。
fn write_all_to_handle(
    handle: windows::Win32::Foundation::HANDLE,
    buf: &[u8],
) -> Result<(), String> {
    let mut written = 0u32;
    // SAFETY: handle 是已打开可写句柄；written 是活 out-param。
    unsafe { WriteFile(handle, Some(buf), Some(&raw mut written), None) }
        .map_err(|e| format!("write batch request: {e}"))?;
    if written as usize != buf.len() {
        return Err(format!(
            "short write: wrote {written}, expected {}",
            buf.len()
        ));
    }
    Ok(())
}

/// 读批量结果文件并校验 JSON 形状（`deny_unknown_fields`——旧字段/拼写错误在解析即拒）。
///
/// # Errors
/// 文件缺失 / 读取失败 / 形状非法 → 携带原因的字符串。
fn read_batch_result_file(path: &Path) -> Result<ServiceBatchResult, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("read batch result {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("batch result malformed: {e}"))
}

/// `EngineChild`（含 `windows` HANDLE）的 Send 封装：进程句柄是独立句柄值，可跨线程
/// `WaitForSingleObject`/`CloseHandle`——等待/终止/关闭在同一闭包线程串行完成，无并发
/// 关闭。镜像 `engine_lifecycle` 的 `WaitHandle` 模式。
struct SendChild(EngineChild);

impl SendChild {
    /// 有界等待子进程退出（whole-struct 方法——disjoint capture 绕过 Send impl）。
    fn wait_exit(mut self, timeout_ms: u32) -> bool {
        self.0.wait_exit(timeout_ms)
    }

    /// 有界等待子进程退出并读取退出码（`Some(code)`=已退出；`None`=超时）。
    fn wait_exit_code(mut self, timeout_ms: u32) -> Option<i32> {
        self.0.wait_exit_code(timeout_ms)
    }
}
// SAFETY: 句柄操作（wait_exit/terminate/Drop）在单一线程内串行；句柄值本身跨线程安全。
unsafe impl Send for SendChild {}

/// S3/D3：service engine 连接 seam（真实实现 = 读 PSK + 拨号稳定服务管道 + SID-only 验证
/// + PSK-HMAC 双向挑战 + 返回客户端；测试注入 fake 换入 fake engine）。
#[tonic::async_trait]
pub trait ServiceEngineConnector: Send + Sync {
    /// 连接 service-mode engine 并返回控制面客户端（后续写路径经 [`EngineSlot`] 换入）。
    ///
    /// # Errors
    /// 用户 SID / PSK 不可读 / 拨号 / 认证 / channel 构建失败 → 携带原因的字符串。
    async fn connect_service_engine(
        &self,
    ) -> Result<Arc<tokio::sync::Mutex<dyn KernelEngineControl>>, String>;
}

/// 真实 service engine 连接器（S3/D2/D7/S6）：读 `%ProgramData%\exv\service.key` → 拨号
/// 稳定服务管道 → engine 自报身份验证（**服务 engine 以 LocalSystem 运行 → 期望 SID 是
/// [`SYSTEM_SID`]**，非安装用户；S6 起改验 engine 自报身份，不再 `OpenProcess` 读 SYSTEM
/// 进程）+ PSK-HMAC 双向挑战 → 客户端。仅 Service 分支调用（幂等性由调用方保证）。
pub struct RealServiceEngineConnector;

#[tonic::async_trait]
impl ServiceEngineConnector for RealServiceEngineConnector {
    async fn connect_service_engine(
        &self,
    ) -> Result<Arc<tokio::sync::Mutex<dyn KernelEngineControl>>, String> {
        let psk = read_service_psk()?;
        let client = crate::grpc_control::EngineControlGrpcClient::connect_service(
            SERVICE_CONTROL_PIPE,
            SYSTEM_SID,
            &psk,
        )
        .await
        .map_err(|e| format!("service engine connect failed: {e:?}"))?;
        Ok(Arc::new(tokio::sync::Mutex::new(client)))
    }
}

impl KernelControlService {
    /// 构造服务：composition + engine 控制面 + 凭据目录 + 共享日志聚合器 + 事件总线。
    #[must_use]
    pub fn new(
        composition: Arc<Mutex<HostComposition>>,
        engine: EngineSlot,
        config_dir: PathBuf,
        logs: Arc<LogAggregator>,
    ) -> Self {
        let generation = *engine.subscribe_swaps().borrow();
        let (status_attachment, _) = watch::channel(StatusAttachment {
            generation,
            attached: false,
        });
        Self {
            composition,
            engine,
            engine_provisioner: None,
            config_dir,
            logs,
            events: Arc::new(EventBus::new()),
            proxy_tun_probe: default_proxy_tun_probe(),
            system_proxy_probe: default_system_proxy_probe(),
            status_attachment,
            status_forwarder_owner: Arc::new(Mutex::new(())),
            snapshot_query_failed: Arc::new(AtomicBool::new(false)),
            core_session_id: Uuid::new_v4(),
            connect_dispatch: Arc::new(std::sync::Mutex::new(None)),
            connect_route_lock: Arc::new(Mutex::new(())),
            status_ready_wait: STATUS_READY_WAIT,
            service_status_source: Arc::new(RealServiceStatusSource),
            service_ops: Arc::new(EngineSubcommandServiceOps::new()),
            service_connector: Arc::new(RealServiceEngineConnector),
            service_probe: Arc::new(RealServiceProbe),
            service_ready_timeout: SERVICE_READY_TIMEOUT,
            service_ready_poll: SERVICE_READY_POLL,
            selected_mode: Arc::new(AtomicU8::new(ServiceMode::Auto.as_u8())),
            service_removed_override: Arc::new(AtomicBool::new(false)),
            service_control_lock: Arc::new(tokio::sync::Mutex::new(())),
            reconnect_attempts: Arc::new(AtomicU32::new(0)),
            reconnect_active: Arc::new(AtomicBool::new(false)),
            reconnect_tx: Arc::new(std::sync::Mutex::new(None)),
            reconnect_session_policy: Arc::new(std::sync::Mutex::new(None)),
            reconnect_config_cache: Arc::new(ReconnectConfigCache::new()),
            reconnect_backoff_base: RECONNECT_BACKOFF_BASE,
            reconnect_backoff_cap: RECONNECT_BACKOFF_CAP,
            stats_first_sample_timeout: STATS_FIRST_SAMPLE_TIMEOUT,
        }
    }

    /// 注入统计转发器首样本期限（RT-DIAG-03 码 6 预算；默认 3000ms，测试注入短时长，
    /// 生产路径不改默认）。
    #[must_use]
    pub fn with_stats_first_sample_timeout(mut self, timeout: Duration) -> Self {
        self.stats_first_sample_timeout = timeout;
        self
    }

    /// 注入非提权 SCM 服务状态源（S3/D5；测试注入确定性结果，避免依赖真实服务状态）。
    #[must_use]
    pub fn with_service_status_source(mut self, source: Arc<dyn ServiceStatusSource>) -> Self {
        self.service_status_source = source;
        self
    }

    /// 注入服务变更操作 seam（S3/D4；测试注入 fake 记录 install/start/stop）。
    #[must_use]
    pub fn with_service_ops(mut self, ops: Arc<dyn ServiceControlOps>) -> Self {
        self.service_ops = ops;
        self
    }

    /// 注入 service engine 连接 seam（S3/D3；测试注入 fake 换入 fake engine，避免真实
    /// PSK 文件与服务管道依赖）。
    #[must_use]
    pub fn with_service_connector(mut self, connector: Arc<dyn ServiceEngineConnector>) -> Self {
        self.service_connector = connector;
        self
    }

    /// 注入 keepalive 就绪探活 seam（R2；测试注入 fake 返回确定性结果，避免真实 PSK
    /// 文件与服务管道依赖）。
    #[must_use]
    pub fn with_service_probe(mut self, probe: Arc<dyn ServiceProbe>) -> Self {
        self.service_probe = probe;
        self
    }

    /// 注入服务就绪轮询上界（R2；测试注入短时长验证先到先采纳/超时语义，避免真实 15s）。
    #[must_use]
    pub fn with_service_ready_timeout(mut self, timeout: Duration) -> Self {
        self.service_ready_timeout = timeout;
        self
    }

    /// 注入服务就绪轮询间隔（R2；测试注入短间隔加快轮询收敛）。
    #[must_use]
    pub fn with_service_ready_poll(mut self, poll: Duration) -> Self {
        self.service_ready_poll = poll;
        self
    }

    /// 查询当前 SCM 服务状态（非提权；查询失败 → `Ok(None)`——状态上报不因查询失败而失败）。
    fn query_service_status_snapshot(&self) -> Option<ServiceStatusSnapshot> {
        let snapshot =
            query_service_status(self.service_status_source.as_ref(), SERVICE_NAME).ok()?;
        if !self.service_removed_override.load(Ordering::Acquire) {
            return Some(snapshot);
        }

        if !snapshot.state.is_installed() {
            // SCM 已经完全清场；从此恢复真实查询。
            self.clear_service_removed_override();
            return Some(snapshot);
        }

        // DeleteService 已经成功，SCM 的 marked-for-delete 观察窗口不再代表业务
        // 安装状态。返回确定性的"未安装"事实，避免 UI 让用户重复点击卸载。
        Some(ServiceStatusSnapshot {
            state: ServiceState::NotInstalled,
            binary_path: None,
        })
    }

    /// 查询当前 SCM 服务状态并派生 wire `ServiceStatus`（含 R3 `health_state`）。
    ///
    /// 廉价健康事实（`service_health_from_snapshot`：SCM 注册 / 二进制路径 / 引擎二进制
    /// 存在性 / PSK 可读）随快照计算；昂贵探针（控制面管道 / keepalive）不在此路径。
    fn query_service_status_wire(&self) -> Option<wire::ServiceStatus> {
        let snap = self.query_service_status_snapshot()?;
        let health = derive_health(&service_health_from_snapshot(&snap));
        Some(service_status_to_wire(&snap, health))
    }

    /// 记录最近一次连接路由的实际模式（快照 `mode` 展示用；缺省 auto）。`ServiceMode`
    /// 是 typed 编码（与 `ENGINE_KIND_*` 共享 0/1/2），存储沿用 `AtomicU8` 供 keepalive
    /// ticker 后台线程无锁读取。
    fn record_mode(&self, mode: ServiceMode) {
        self.selected_mode.store(mode.as_u8(), Ordering::Relaxed);
    }

    /// 当前展示模式字符串（`"auto"` / `"service"` / `"oneshot"`；未知码 fail-closed → auto）。
    fn mode_string(&self) -> String {
        ServiceMode::from_u8(self.selected_mode.load(Ordering::Relaxed))
            .as_wire_str()
            .to_string()
    }

    /// 共享维护形态句柄（`CoreRuntime` run 循环按形态分流崩溃自愈：仅 oneshot 维护
    /// 形态 respawn；service/auto 不为无对象或服务形态 engine 弹 UAC——2026-09-08
    /// 计划批 3）。
    #[must_use]
    pub fn selected_mode_handle(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.selected_mode)
    }

    /// 接线 oneshot engine 按需拉起编排（main 在 serve 前调用；测试注入槽内 fake 的
    /// 路径不接线——route Oneshot 分支保持「槽内即目标 engine」的既有语义）。
    pub fn set_engine_provisioner(
        &mut self,
        provisioner: Arc<crate::engine_provisioner::EngineProvisioner>,
    ) {
        self.engine_provisioner = Some(provisioner);
    }

    /// 回填已验证的 UI peer（serve 接受并验证 UI 连接后调用）——provisioner 的
    /// composition 重建需要它做 gate 重授权（fail closed：未回填时 provision 拒绝）。
    pub fn note_verified_ui_peer(&self, peer: &VerifiedPipePeer) {
        if let Some(provisioner) = &self.engine_provisioner {
            provisioner.set_ui_peer(peer.clone());
        }
    }

    /// 卸载成功后标记「SCM marked-for-delete 观察窗口」覆盖：`DeleteService` 已成功但 SCM
    /// 可能短暂保留条目，此后 `query_service_status_snapshot` 强制报告 NotInstalled。
    fn mark_service_removed(&self) {
        self.service_removed_override.store(true, Ordering::Release);
    }

    /// 清除覆盖（安装成功 / 查询观察到 SCM 已完全清场）——恢复真实 SCM 查询。
    fn clear_service_removed_override(&self) {
        self.service_removed_override
            .store(false, Ordering::Release);
    }

    /// Core-owned service bootstrap：确保服务正在运行且业务面已经可用。
    ///
    /// 这是 install、显式 Start 和 connect-after-install 的共同入口。仅在本次操作
    /// 观察到 Stopped/Other/未知状态时提交一次提权 start；Running、StartPending、
    /// StopPending 均不重复提交，由 SCM 自己收敛。提交发生竞态时，若随后查询已经
    /// 变成 Running，仍按幂等成功处理。
    async fn ensure_service_ready(&self) -> Result<ServiceReadiness, String> {
        let initial_state = self
            .query_service_status_snapshot()
            .map(|snapshot| snapshot.state);
        // 这是一次连接/显式 Start 内的 bootstrap，不是后台守护：StartPending/StopPending
        // 由 SCM 自己收敛，重复提交 start 只会制造 ERROR_SERVICE_ALREADY_RUNNING 竞态。
        let needs_start = matches!(
            initial_state,
            None | Some(
                ServiceState::Stopped | ServiceState::NotInstalled | ServiceState::Other(_)
            )
        );
        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.service_bootstrap.observed",
                "service bootstrap checked SCM state",
                &BTreeMap::from([
                    ("state".to_string(), format!("{:?}", initial_state)),
                    ("start_submitted".to_string(), needs_start.to_string()),
                ]),
            )
            .ok();
        if needs_start {
            if let Err(error) = self.service_ops.start().await {
                let running_after_race = self
                    .query_service_status_snapshot()
                    .is_some_and(|snapshot| snapshot.state == ServiceState::Running);
                if !running_after_race {
                    return Err(format!("start service: {error}"));
                }
            }
        }

        let readiness = wait_for_service_ready(
            self.service_status_source.as_ref(),
            self.service_probe.as_ref(),
            self.service_ready_timeout,
            self.service_ready_poll,
        )
        .await;
        if readiness.ready {
            Ok(readiness)
        } else {
            // 4.2：失败分类与可读信息（稳定前缀 `service_not_running|` /
            // `service_not_ready|`）在本层产生；外层透传，禁止叠前缀。
            Err(readiness_failure_message(&readiness))
        }
    }

    /// Connect 内的服务启动门禁（含修复子进程补洞升级，2026-09-08 计划批 4）。
    ///
    /// 服务启动阶段的 readiness 只允许完成一次；真正的业务面门禁由后面的
    /// `connect_service_engine_with_bootstrap_retry` 完成（服务身份/PSK 握手 + owner
    /// lease）。这里不能在 SCM 已 Running 后再次开一条 KeepAlive 管道，否则会与服务
    /// accept-loop 的上一条 gRPC 管道释放形成竞态：探活成功而第二次 Dial(2)。
    ///
    /// **repair 补洞**：bootstrap 失败 = 服务客观事实有问题（start 提交失败 / 就绪
    /// 不达成），且调用方正处于「需要业务」的上下文（connect / 显式 Start）→ 消耗
    /// `budget` 的一次修复名额，runas 派 engine 修复子进程（`install` 批量 =
    /// create-or-repair + Start + Verify，1 次 UAC）后重做 readiness。每次业务操作
    /// 至多修复一次（budget 由 route/Start 建立）。
    async fn ensure_service_ready_for_connect(
        &self,
        budget: &mut RepairBudget,
    ) -> Result<ServiceReadiness, String> {
        match self.ensure_service_ready().await {
            Ok(readiness) => Ok(readiness),
            Err(first) => {
                if !budget.try_spend() {
                    return Err(first);
                }
                let _ = self.logs.append_core(
                    "warn",
                    "kernel",
                    "kernel.service.repair_escalated",
                    &format!(
                        "service bootstrap failed during a business request; escalating to repair batch (1 UAC): {first}"
                    ),
                    &BTreeMap::new(),
                );
                match self.service_ops.install().await {
                    Ok(_) => self.ensure_service_ready().await.map_err(|second| {
                        format!(
                            "{first}; repair batch completed but service still not ready: {second}"
                        )
                    }),
                    Err(repair) => Err(format!("{first}; repair batch failed: {repair}")),
                }
            }
        }
    }

    /// S3/D3 + M3：auto 决策表路由（connect 派发前调用）。服务已安装但 stopped 时，
    /// 由 Core 自动完成 start + readiness，再继续连接。
    ///
    /// - **Service**：服务已装且在跑 → 把 engine 槽换到服务 engine（SCM 常驻）后返回。
    /// - **PromptStart**：服务已装未跑 → Core bootstrap 后继续走 service engine。
    /// - **Oneshot**：未装 → 按需拉起 oneshot engine（2026-09-08 计划批 2：首次业务
    ///   连接此刻才提权 spawn；槽内已有存活 engine 则复用——非「启动即拉」）。
    ///
    /// 服务状态查询失败（`None`）= **未知**（S5/MED[3] 加固）：安全兜底 oneshot（spawn
    /// engine 直连，不触碰 SCM、不因查询失败阻塞连接）。PSK 不可读 / 服务连接失败 →
    /// `failed_precondition`（服务模式必须持有共享秘密）。
    async fn route_connect(&self) -> Result<RouteDecision, Status> {
        let snap = self.query_service_status_snapshot();
        let Some(snap) = snap else {
            self.logs
                .append_core(
                    "warn",
                    "kernel",
                    "kernel.route.service_query_failed",
                    "service status query failed; treating as unknown and falling back to oneshot",
                    &BTreeMap::new(),
                )
                .ok();
            return Ok(RouteDecision::Oneshot);
        };
        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.route.scm_state",
                &format!(
                    "service scm state={:?} -> route={:?}",
                    snap.state,
                    decide_route(snap.state)
                ),
                &BTreeMap::new(),
            )
            .ok();
        let mut repair_budget = RepairBudget::default();
        match decide_route(snap.state) {
            RouteDecision::Service => {
                // SCM 已经观察到 Running：不要在 connect 上重复执行 install/start 的
                // readiness gate。真正的业务面门禁是 service pipe + owner lease；若服务
                // 在这一个快照之后停止，下面的有界 bootstrap retry 会处理该竞态。
                self.connect_service_engine_with_bootstrap_retry(&mut repair_budget)
                    .await?;
                Ok(RouteDecision::Service)
            }
            RouteDecision::PromptStart => {
                self.ensure_service_ready_for_connect(&mut repair_budget)
                    .await
                    // 4.2 前缀唯一层：bootstrap 错误已由 ensure 层携带稳定前缀
                    //（`service_not_running|` / `service_not_ready|`），此处原样上抛——
                    // 任何外层禁止叠前缀（不得再包 `service_start_failed|`）。
                    .map_err(Status::failed_precondition)?;
                self.connect_service_engine_with_bootstrap_retry(&mut repair_budget)
                    .await?;
                Ok(RouteDecision::Service)
            }
            RouteDecision::Oneshot => {
                // 2026-09-08 计划批 2：oneshot engine 按需拉起——首次 oneshot 连接
                //（或服务卸载后的下一次连接）此刻才提权 spawn（UAC 归属本次业务请求）；
                // 槽内已有存活 engine 则复用（热启动，免重复提权）。无 provisioner
                //（测试注入槽内 fake）时保持既有语义：槽内即目标 engine。
                if let Some(provisioner) = &self.engine_provisioner {
                    provisioner.ensure_oneshot_engine().await?;
                }
                Ok(RouteDecision::Oneshot)
            }
        }
    }

    /// 换点后发布新目标代次；转发器若已完成同代次挂接，不能再被路由撤销。
    async fn swap_engine_for_route(
        &self,
        engine: Arc<tokio::sync::Mutex<dyn KernelEngineControl>>,
    ) {
        let swaps = self.engine.subscribe_swaps();
        if !self.engine.swap(engine).await {
            return;
        }
        StatusAttachment::advance(&self.status_attachment, *swaps.borrow());
        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.route.engine_swapped",
                "engine slot swapped; published target generation for status attachment",
                &BTreeMap::from([("target_generation".to_string(), swaps.borrow().to_string())]),
            )
            .ok();
    }

    /// 把 engine 槽换到 service-mode engine（S3/D2/D7）：经 [`ServiceEngineConnector`] seam
    /// 连接（真实实现 = 读 PSK + 拨号稳定服务管道 + SID-only 验证 + PSK-HMAC 双向挑战）→
    /// 换入槽（后续写路径自动指向服务 engine）。幂等性由调用方（路由决策）保证——仅在
    /// Service 分支调用。
    ///
    /// R2：对 connect-after-start 竞窗做**有界重试**（`connect_service_engine` 内部的
    /// 拨号重试覆盖未绑管道的竞窗，此处兜底 SCM running 后 service engine 尚未就绪的
    /// 窗口——少数尝试 + 小退避）。最终失败仍保持既有 `failed_precondition` 语义。
    async fn ensure_service_engine(&self) -> Result<(), Status> {
        // 断开后立即重连：若槽内已是 service client（上次服务连接后未切换），复用其
        // owner lease（host 侧幂等）——否则新建连接 acquire 会被引擎以「peer already
        // owns」拒绝（旧连接 owner 未释放；peer 含 per-connection digest，新连接=新
        // peer）→ ConnectionLost（实测：断开后立刻重连必现，等 3s 后旧 owner 释放才恢复）。
        // 复用失败（旧 client 已失效/服务重启）→ 落回下方新建路径。
        if self.selected_mode.load(Ordering::SeqCst) == ServiceMode::Service.as_u8() {
            let current = self.engine.current().await;
            let mut guard = current.lock().await;
            let reusable = guard.ensure_owner_lease().await.is_ok();
            drop(guard);
            if reusable {
                self.logs
                    .append_core(
                        "info",
                        "kernel",
                        "kernel.route.service_reuse",
                        "reusing existing service engine client (idempotent owner lease)",
                        &BTreeMap::new(),
                    )
                    .ok();
                return Ok(());
            }
        }
        const ATTEMPTS: u32 = 3;
        const BACKOFF: Duration = Duration::from_millis(300);
        let mut last_err = "no attempt".to_string();
        for attempt in 0..ATTEMPTS {
            match self.service_connector.connect_service_engine().await {
                Ok(engine) => {
                    // PSK/pipe 握手成功不等于 mutation 可用；先建立 owner lease，把
                    // 不可用尽早收敛为 service_connect_failed，而不是等 ApplyTunnel 才
                    // 映射成笼统的 engine control unavailable。
                    {
                        let mut guard = engine.lock().await;
                        if let Err(error) = guard.ensure_owner_lease().await {
                            last_err = format!("service engine owner lease: {error:?}");
                            self.logs
                                .append_core(
                                    "warn",
                                    "kernel",
                                    "kernel.route.service_lease_failed",
                                    &format!("service engine lease attempt {attempt}: {error:?}"),
                                    &BTreeMap::new(),
                                )
                                .ok();
                            if attempt + 1 < ATTEMPTS {
                                tokio::time::sleep(BACKOFF).await;
                            }
                            continue;
                        }
                    }
                    self.logs
                        .append_core(
                            "info",
                            "kernel",
                            "kernel.route.service_dial_ok",
                            &format!("service engine dial+lease ok on attempt {attempt}; swapping to service engine"),
                            &BTreeMap::new(),
                        )
                        .ok();
                    self.swap_engine_for_route(engine).await;
                    // 先切换维护策略，再让后续 connect 派发继续运行，避免 ticker 在
                    // service engine 已换入、快照 mode 尚未更新的竞窗内发送 KeepAlive。
                    self.record_mode(ServiceMode::Service);
                    return Ok(());
                }
                Err(e) => {
                    last_err = e.clone();
                    self.logs
                        .append_core(
                            "warn",
                            "kernel",
                            "kernel.route.service_dial_failed",
                            &format!("service engine dial attempt {attempt} failed: {e}"),
                            &BTreeMap::new(),
                        )
                        .ok();
                    if attempt + 1 < ATTEMPTS {
                        tokio::time::sleep(BACKOFF).await;
                    }
                }
            }
        }
        // R5：稳定前缀 `service_connect_failed|` ——UI 侧 map_status 映射为
        // `AppError::ServiceConnectFailed`（modal 触发）；人读信息保持在后缀。
        Err(Status::failed_precondition(format!(
            "service_connect_failed|service engine connect: {last_err}"
        )))
    }

    /// Service pipe 连接的一次性恢复边界：如果 engine 在本次连接刚开始时从 Running
    /// 崩掉并已经回到 stopped，补做一次 bootstrap，再重试一次 service pipe。这里不
    /// 开启常驻监控，也不对持续 Running 但业务面异常的服务无限重启。
    /// Service pipe 连接的一次性恢复边界：如果 engine 在本次连接刚开始时从 Running
    /// 崩掉并已经回到 stopped，补做一次 bootstrap，再重试一次 service pipe。这里不
    /// 开启常驻监控，也不对持续 Running 但业务面异常的服务无限重启。
    ///
    /// **Running-but-unreachable 补洞（2026-09-08 计划批 4）**：服务 SCM 观察 Running
    /// 但业务面连接失败（PSK 缺失/管道死/损坏——SCM restart 无法恢复的客观问题）→
    /// 消耗 `budget` 的一次修复名额（runas 修复子进程）后重试一次。已 Stopped 的
    /// 既有路径走 bootstrap（含同 budget 的 repair 升级）。
    async fn connect_service_engine_with_bootstrap_retry(
        &self,
        budget: &mut RepairBudget,
    ) -> Result<(), Status> {
        match self.ensure_service_engine().await {
            Ok(()) => Ok(()),
            Err(first_error) => {
                let stopped = self
                    .query_service_status_snapshot()
                    .is_some_and(|snapshot| {
                        snapshot.state.is_installed() && snapshot.state != ServiceState::Running
                    });
                if stopped {
                    self.ensure_service_ready_for_connect(budget)
                        .await
                        // 4.2 前缀唯一层：同 PromptStart 分支——ensure 层错误已带稳定
                        // 前缀，原样上抛，禁止叠前缀。
                        .map_err(Status::failed_precondition)?;
                    return self.ensure_service_engine().await;
                }
                // SCM Running 但业务面三次拨号+lease 全失败：客观损坏 → 修复子进程
                // 补洞（1 次 UAC，消耗 budget）→ 重试一次；budget 已耗尽则原样上抛。
                if !budget.try_spend() {
                    return Err(first_error);
                }
                let _ = self.logs.append_core(
                    "warn",
                    "kernel",
                    "kernel.service.repair_escalated",
                    &format!(
                        "service engine unreachable while SCM reports Running; escalating to repair batch (1 UAC): {first_error:?}"
                    ),
                    &BTreeMap::new(),
                );
                self.service_ops.install().await.map_err(|repair| {
                    Status::failed_precondition(format!(
                        "service_connect_failed|service engine connect: {first_error}; repair batch failed: {repair}"
                    ))
                })?;
                self.ensure_service_engine().await
            }
        }
    }


    /// 在 service install/uninstall 前清理当前 engine 的业务连接。
    ///
    /// service 与 oneshot 不能在同一 core 生命周期中并存：安装服务前清理 oneshot，
    /// 卸载服务前清理 service。`StopTunnel` 成功后把 host composition 收敛回 Idle，并
    /// 暂时将维护类型置为 auto，确保旧 oneshot 不再收到 KeepAlive；调用方在服务子命令
    /// 成功后再写入目标类型。
    async fn stop_engine_for_transition(&self, _source_mode: ServiceMode) -> Result<(), String> {
        let active = {
            let composition = self.composition.lock().await;
            !matches!(composition.phase(), HostPhase::Idle | HostPhase::Stopped)
        };
        if !active {
            // Stopped 是 teardown 已完成但 admission 可能仍关闭的合法中间观测态。
            // 服务变更是下一次业务操作的边界，必须在这里补齐显式 reopen，不能因为
            // selected_mode 与 transition 来源不一致而提前返回并把后续 connect 永久锁死。
            let mut composition = self.composition.lock().await;
            if composition.phase() == HostPhase::Stopped {
                composition.apply(HostEvent::ReopenAdmission);
            }
            drop(composition);
            self.record_mode(ServiceMode::Auto);
            return Ok(());
        }

        let runtime_epoch = {
            let composition = self.composition.lock().await;
            composition.runtime_epoch_bytes().to_vec()
        };
        let operation_id = Uuid::new_v4().as_bytes().to_vec();
        let request = StopTunnelRequest {
            lookup_key: Some(wire::OperationLookupKey {
                principal_digest: vec![0u8; 32],
                method: wire::OperationMethod::StopTunnel as i32,
                runtime_epoch,
                operation_id,
            }),
            request_digest: vec![0u8; 32],
        };
        let engine = self.engine.current().await;
        let mut engine = engine.lock().await;
        let lease_available = match engine.ensure_owner_lease().await {
            Ok(()) => true,
            Err(GrpcClientError::ConnectionLost) => {
                // 旧 engine 已经断开时，业务连接实际上已经消失；继续做本地 teardown，
                // 否则 admission 会永远保持 closed，后续 install/uninstall 也无法重试。
                false
            }
            Err(e) => {
                return Err(format!(
                    "ensure owner lease before service transition: {e:?}"
                ));
            }
        };
        if lease_available {
            match engine.stop_tunnel(request).await {
                Ok(_) => {}
                Err(GrpcClientError::ConnectionLost) => {
                    // 与 lease 阶段掉线相同：旧 engine 已不再可用，本地收敛仍必须继续。
                }
                Err(e) => {
                    return Err(format!("stop engine before service transition: {e:?}"));
                }
            }
        }
        drop(engine);

        let mut composition = self.composition.lock().await;
        composition.apply(HostEvent::Disconnect);
        composition.apply(HostEvent::TeardownSideJoined(TeardownSide::ProtocolControl));
        composition.apply(HostEvent::TeardownSideJoined(TeardownSide::PacketData));
        composition.apply(HostEvent::ReopenAdmission);
        drop(composition);
        self.record_mode(ServiceMode::Auto);
        Ok(())
    }

    /// 从一个已组合的 host 构造 (composition 句柄, 服务)。
    ///
    /// 调用方持有返回的 `Arc`，以便在服务之外驱动同一 composition（例如 P3 的
    /// engine 事件转发）。
    #[must_use]
    pub fn from_composition(
        composition: HostComposition,
        engine: EngineSlot,
        config_dir: PathBuf,
        logs: Arc<LogAggregator>,
    ) -> (Arc<Mutex<HostComposition>>, Self) {
        let shared = Arc::new(Mutex::new(composition));
        let service = Self::new(Arc::clone(&shared), engine, config_dir, logs);
        (shared, service)
    }

    /// 注入上游 proxy TUN 检测探针（C5-wire；默认 [`default_proxy_tun_probe`]）。
    ///
    /// 单测注入确定性结果（探测失败/无检测/检测到），避免 `GetSnapshot` 与事件转发器
    /// 依赖真实 Win32 适配器状态；产品路径保持默认真实枚举。
    #[must_use]
    pub fn with_proxy_tun_probe(mut self, probe: ProxyTunProbe) -> Self {
        self.proxy_tun_probe = probe;
        self
    }

    /// 注入系统代理探测探针（EXV_UNFREEZE；默认 [`default_system_proxy_probe`]）。
    ///
    /// 单测注入确定性结果（探测失败/disabled/manual…），避免 `GetSnapshot` 与事件转发器
    /// 依赖真实 WinINET 注册表状态；产品路径保持默认真实捕获。
    #[must_use]
    pub fn with_system_proxy_probe(mut self, probe: SystemProxyProbe) -> Self {
        self.system_proxy_probe = probe;
        self
    }

    /// 注入 `await_status_ready` 的有界等待上界（R3-C1；测试注入短时长验证"不悬挂、
    /// 放行派发报真实错误"语义）。产品路径保持默认 [`STATUS_READY_WAIT`]。
    #[must_use]
    pub fn with_status_ready_wait(mut self, wait: Duration) -> Self {
        self.status_ready_wait = wait;
        self
    }

    /// 注入自动重连退避参数（reconnect-backoff，2026-09-05）：`drive_reconnect` 在
    /// 派发前 sleep `min(base * 2^(attempt-1), cap)`。产品路径保持冻结默认
    /// base=2s/cap=30s（[`RECONNECT_BACKOFF_BASE`]/[`RECONNECT_BACKOFF_CAP`]）；
    /// 测试注入毫秒级短时长，让白盒重连节奏测试不依赖真实 2s/30s 等待。
    #[must_use]
    pub fn with_reconnect_backoff(mut self, base: Duration, cap: Duration) -> Self {
        self.reconnect_backoff_base = base;
        self.reconnect_backoff_cap = cap;
        self
    }

    /// 转为 tonic `KernelControlServer`（P4 将其加入 `Server::builder()`）。
    #[must_use]
    pub fn into_server(self) -> KernelControlServer<KernelControlService> {
        KernelControlServer::new(self)
    }

    /// `WatchEvents` 事件总线句柄（测试可注入事件；`spawn_status_forwarder` 发布）。
    #[must_use]
    pub fn events(&self) -> Arc<EventBus> {
        Arc::clone(&self.events)
    }

    /// 状态订阅接收端：`attached` 仅描述其 `generation` 对应的状态流。
    #[must_use]
    pub fn status_ready(&self) -> watch::Receiver<StatusAttachment> {
        self.status_attachment.subscribe()
    }

    /// 仅等待本次路由的代次；错误在连接层收束，绝不进入服务安装/修复路径。
    async fn await_status_ready(&self, target_generation: u64) -> Result<(), Status> {
        let mut ready = self.status_ready();
        let result = tokio::time::timeout(self.status_ready_wait, async {
            loop {
                let attachment = *ready.borrow_and_update();
                if attachment.generation == target_generation && attachment.attached {
                    return Ok(());
                }
                if attachment.generation > target_generation {
                    return Err("engine generation changed");
                }
                if ready.changed().await.is_err() {
                    return Err("status forwarder closed");
                }
            }
        })
        .await;
        match result {
            Ok(Ok(())) => Ok(()),
            failure => Err(Status::unavailable(format!(
                "status_stream_not_ready|连接已完成引擎路由，但状态订阅未就绪；未发送连接请求，请重试。target_generation={target_generation}; reason={}",
                match failure {
                    Ok(Err(reason)) => reason,
                    _ => "attachment timed out",
                }
            ))),
        }
    }

    fn log_status_gate(
        &self,
        attempt_id: Option<Uuid>,
        target_generation: Option<u64>,
        outcome: &str,
    ) {
        let (attempt_applied, stop_required) = {
            let dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match dispatch
                .as_ref()
                .filter(|state| Some(state.id) == attempt_id)
            {
                Some(state) => (
                    state.attempt_applied.to_string(),
                    state.stop_required.to_string(),
                ),
                None => ("not_recorded".to_string(), "not_recorded".to_string()),
            }
        };
        let _ = self.logs.append_core(
            "info",
            "kernel",
            "kernel.connect.status_gate",
            "connection status subscription gate",
            &BTreeMap::from([
                (
                    "target_generation".to_string(),
                    target_generation
                        .map_or_else(|| "not_routed".to_string(), |value| value.to_string()),
                ),
                ("outcome".to_string(), outcome.to_string()),
                (
                    "attempt_id".to_string(),
                    attempt_id.map_or_else(|| "not_recorded".to_string(), |id| id.to_string()),
                ),
                ("attempt_applied".to_string(), attempt_applied),
                ("stop_required".to_string(), stop_required),
            ]),
        );
    }

    /// composition → dispatch，保证旧 attempt 的失败不能跨过 Stop 或新的受理。
    async fn apply_connect_failed_for_route(&self, attempt_id: Uuid, status: &Status) {
        let mut composition = self.composition.lock().await;
        let current = {
            let dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            dispatch
                .as_ref()
                .is_some_and(|state| state.id == attempt_id && !*state.cancelled.borrow())
        };
        if !current {
            return;
        }
        let error = route_failure_wire_error(status);
        composition.set_last_wire_error(error.clone());
        composition.apply(HostEvent::ConnectFailed(wire_vpn_error_to_domain(&error)));
        let snapshot = snapshot_for_phase(composition.phase(), &composition);
        // 仍持有 composition：身份校验、状态提交与 event tick 发布之间没有 await，
        // Stop 或新 Connect 不能先发布新态后再被本次旧 Failed 覆盖。
        self.events
            .publish(wire::RuntimeEventKind::Transition, snapshot);
    }

    /// 没有数据面或已有 Stop 确认回执时，收束屏障并重开连接受理。
    async fn finish_local_stop(&self, attempt_id: Option<Uuid>) {
        let mut composition = self.composition.lock().await;
        {
            let mut dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if dispatch.as_ref().map(|state| state.id) != attempt_id {
                return;
            }
            if let Some(state) = dispatch.as_mut() {
                state.stop_required = false;
            }
        }
        composition.synthesize_data_plane_join();
        if composition.phase() == HostPhase::Stopped {
            composition.apply(HostEvent::ReopenAdmission);
        }
        // 同一有序边界内发布已确认的 Idle。显式 Stop 已结束重连会话，故无重连伴生。
        let snapshot = snapshot_for_phase(composition.phase(), &composition);
        self.events
            .publish(wire::RuntimeEventKind::Transition, snapshot);
    }

    /// 在用户连接受理时读取持久化配置，形成仅属于本次连接生命周期的重连策略。
    /// 设置保存不会覆盖它；下一次用户连接才会创建新的策略。
    fn reconnect_policy_for_new_session(&self) -> ReconnectSessionPolicy {
        match ExvConfig::load_from_dir(&self.config_dir) {
            Ok(config) => ReconnectSessionPolicy::from_config(&config),
            Err(_) => ReconnectSessionPolicy::disabled(),
        }
    }

    /// 未建立连接会话时不投影重连设置，避免把持久化配置误呈现为运行事实。
    fn session_reconnect_status(&self) -> Option<wire::ReconnectStatus> {
        self.reconnect_session_policy
            .lock()
            .ok()
            .and_then(|policy| *policy)
            .map(|policy| {
                reconnect_status_from_state(
                    self.reconnect_attempts.load(Ordering::Relaxed),
                    self.reconnect_active.load(Ordering::Acquire),
                    policy.auto_reconnect,
                    policy.max_attempts,
                )
            })
    }

    /// C1（ui-connect-stop-responsiveness）：connect/stop 受理即发布受理快照。
    ///
    /// 受理快照是「相位 + operation_id 的即时通报」：admission 落相位后**短暂重取**
    /// composition 锁组装 `snapshot_for_phase`（Connecting/Stopping 携带刚登记的
    /// operation_id），经 `EventBus::publish` 发布 `Transition`。总线在发布点自动附加
    /// 缓存伴生值（stats/proxy_tun/system_proxy/service_status/mode），因此本路径
    /// **零探测、零 SCM 查询**；reconnect 伴生只取连接受理时已冻结的会话策略，
    /// 不重新读取设置。发布失败（broadcast
    /// 无订阅者等）静默容忍——`EventBus::publish` 已按 `let _ =` 处理，不阻塞
    /// connect/stop 主路径。
    ///
    /// 锁纪律：持有 composition 直到同步 publish 返回；期间没有 await。
    async fn publish_admission_snapshot(&self) {
        // 受理发布也保持 composition 到同步 publish 返回，避免快照释放锁后迟到。
        let composition = self.composition.lock().await;
        let snapshot = snapshot_for_phase(composition.phase(), &composition);
        let snapshot = snapshot_with_reconnect(snapshot, self.session_reconnect_status());
        self.events
            .publish(wire::RuntimeEventKind::Transition, snapshot);
    }

    /// 经 transport peer 认证路径授权 `KernelControl` gate（P3-c1）。
    ///
    /// host 侧核对 transport peer 身份（`VerifiedPipePeer` 携带进程 pid + user SID +
    /// account name，缺任一项即拒绝）后授权 gate——engine 侧 `peer_auth`（P1-b）已
    /// 落地，host 侧把已验 peer 身份接到 gate 的授权路径。进程生命周期（P3-c2）在
    /// 传输层认证后调用本方法；已授权后写 RPC 通过 gate。
    ///
    /// # Errors
    /// peer 无可用身份事实（pid/SID/account 缺失）→ `VpnError::Unauthorized`。
    pub async fn authorize_transport_peer(&self, peer: &VerifiedPipePeer) -> Result<(), VpnError> {
        let mut composition = self.composition.lock().await;
        composition.kernel_gate().authorize(peer)?;
        // R1：控制器（KernelControl transport peer）授权即绑定到唯一 runtime actor——
        // connect 受理（`HostEvent::Connect`）需要 actor 已绑定 peer+capability 才会
        // admitted。绑定用确定性的 peer+capability（固定字面量构造，镜像 acceptance /
        // portable H80 同款；真实绑定推导属 P3-c2 进程生命周期接线）。
        let (actor_peer, capability) = controller_peer_and_capability();
        composition.bind_controller(actor_peer, capability);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // C3a：自动重连 host 侧重连驱动。
    // -----------------------------------------------------------------------

    /// 一次 Connect 的核心（admission → route → attach-before-apply → execute_connect）。
    ///
    /// UI `KernelControl.Connect` 与 C3a 自动重连共用。凭据始终从磁盘（`config_dir`）
    /// 重新组装（本方法不持有 UI 提供的一次性 secret）；状态机受理（Idle/Connected/
    /// Failed → Connecting）、路由决策、SCM 状态刷新、attach-before-apply 与发送后
    /// 零化语义与既有 connect 完全一致。
    ///
    /// # Errors
    /// admission 拒绝 → `failed_precondition`；路由失败 → 对应 `Status`（已补发
    /// ConnectFailed）；凭据缺失/引擎不可达 → `execute_connect` 的相应 `Status`。
    async fn run_connect(
        &self,
        intent: wire::ConnectIntent,
        ui_credentials: Option<CredentialPackage>,
        persist_credentials: bool,
        new_session_policy: Option<ReconnectSessionPolicy>,
        expected_dispatch_id: Option<Uuid>,
        connection_mode: ConnectionMode,
    ) -> Result<Response<OperationReply>, Status> {
        // R1：connect 受理即驱动状态机（Idle/Connected/Failed → Connecting）。
        // 失败（admission closed / teardown in progress）→ 明确拒绝，不进 Connecting。
        let operation_id = intent
            .lookup_key
            .as_ref()
            .and_then(|k| <[u8; 16]>::try_from(k.operation_id.as_slice()).ok());
        let attempt_id = Uuid::new_v4();
        let (cancel_sender, mut cancelled) = watch::channel(false);
        let mut composition = self.composition.lock().await;
        {
            let mut dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // 自动重连退避期间可能已经受理了新的手动连接，或用户已经 Stop。
            // 在既有 composition -> dispatch 线性化边界内核对原 attempt，避免旧
            // timer 在检查后到状态机受理之间发生 TOCTOU 并覆盖新会话。
            if let Some(expected_id) = expected_dispatch_id {
                let still_current = dispatch.as_ref().is_some_and(|state| {
                    state.id == expected_id && !*state.cancelled.borrow()
                });
                if !still_current {
                    return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED));
                }
            }
            let effect = composition.apply(HostEvent::Connect);
            if let HostEffect::ConnectRefused(reason) = effect {
                return Err(Status::failed_precondition(format!("connect: {reason}")));
            }
            if let Some(id) = operation_id {
                composition.set_operation_id(id);
            }
            let stop_required = dispatch.as_ref().is_some_and(|state| state.stop_required);
            *dispatch = Some(ConnectDispatch {
                id: attempt_id,
                cancelled: cancel_sender,
                target_generation: None,
                attempt_applied: false,
                stop_required,
            });
        }
        if let Some(policy) = new_session_policy {
            if let Ok(mut current) = self.reconnect_session_policy.lock() {
                *current = Some(policy);
            }
            self.reconnect_attempts.store(0, Ordering::Relaxed);
            self.reconnect_active.store(false, Ordering::Release);
        }
        self.logs
            .append_core(
                "info",
                "kernel",
                if new_session_policy.is_some() {
                    "kernel.connect.connection_mode_frozen"
                } else {
                    "kernel.reconnect.connection_mode_reused"
                },
                &format!(
                    "{} connection mode={}",
                    if new_session_policy.is_some() {
                        "manual connect accepted with frozen"
                    } else {
                        "auto reconnect reusing frozen"
                    },
                    connection_mode.as_str()
                ),
                &BTreeMap::new(),
            )
            .ok();
        // EXV_UNFREEZE 2026-09-05（自愈发布点表·冻结）：用户发起连接受理 → 清除自愈
        // 上下文（lane `None`）。随下一次自然事件生效（不发空刷新）——GetSnapshot 每次
        // 组装读当前缓存即已即时反映清除。
        self.events.refresh_self_heal(None);
        drop(composition);
        // C1（ui-connect-stop-responsiveness）：受理即发布 Connecting 受理快照——
        // 发布点在 admission 之后、route_connect 之前，UI 事件链的第一手真相先于
        // 任何路由/SCM/bootstrap 工作（bootstrap 慢路径下 UI 也即时离开乐观态）。
        self.publish_admission_snapshot().await;

        request_snapshot(
            Arc::clone(&self.logs),
            SnapshotContext::new(
                "connect_admitted",
                operation_id.map(Uuid::from_bytes),
                Some(attempt_id),
                self.reconnect_attempts.load(Ordering::Relaxed),
                *self.engine.subscribe_swaps().borrow(),
            ),
        );

        let _route_guard = tokio::select! {
            biased;
            () = connect_cancelled(&mut cancelled) => return Err(Status::cancelled("connect cancelled before route")),
            guard = self.connect_route_lock.lock() => guard,
        };

        // S3/D3 + M3：auto 决策表路由（core 侧）——服务在跑 → service；已装未跑 →
        // 本次 connect 内由 Core 自动启动并等待就绪；未装 → oneshot（connect 不触发
        // install）。服务启动只发生在本次连接操作内，不是后台监控。
        // 路由决策记录实际模式（快照 `mode` 展示用）；Service 分支把 engine 槽换到服务
        // engine（SCM 常驻，非提权拉起）；Oneshot 维持既有 spawn 路径。
        //
        // R5：路由失败（PromptStart / service engine connect 失败）时补发 ConnectFailed
        // ——connect 已把状态机推进到 Connecting，但路由在派发前失败、engine 状态流永不
        // 携带 Failed → 不补发则快照卡死 Connecting（R5 根因，modal 触发依赖 Failed 相态）。
        // 已开始的 SCM 操作不强行丢弃；Stop 可独立收束，结果回来后只允许原操作继续。
        let route_result = self.route_connect().await;
        if *cancelled.borrow() || cancelled.has_changed().is_err() {
            self.log_status_gate(Some(attempt_id), None, "cancelled_after_route");
            return Err(Status::cancelled("connect cancelled during route"));
        }
        let route = match route_result {
            Ok(route) => route,
            Err(status) => {
                self.apply_connect_failed_for_route(attempt_id, &status)
                    .await;
                return Err(status);
            }
        };
        // 路由锁覆盖到 Apply 完成；同一次 host 连接不能被另一次路由换点。
        // 额外代次校验覆盖生命周期模块发起的外部换点。
        let slot_swaps = self.engine.subscribe_swaps();
        let target_generation = *slot_swaps.borrow();
        let target_engine = self.engine.current().await;
        if *slot_swaps.borrow() != target_generation {
            let status = Status::unavailable(
                "status_stream_not_ready|engine changed while capturing route target",
            );
            self.apply_connect_failed_for_route(attempt_id, &status)
                .await;
            return Err(status);
        }
        {
            let mut dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(state) = dispatch
                .as_mut()
                .filter(|state| state.id == attempt_id && !*state.cancelled.borrow())
            else {
                return Err(Status::cancelled("connect cancelled during route"));
            };
            state.target_generation = Some(target_generation);
        }
        self.record_mode(ServiceMode::from_route(route));
        self.events.refresh_service_mode(Some(self.mode_string()));
        // S3/D5 修复：connect 路由（含 ensure_service_ready 内部 bootstrap start 把
        // 服务从 Stopped 拉到 Running）后立即刷新 SCM 快照缓存——否则 WatchEvents
        // 发布的快照持续携带 connect 前的旧服务状态（UI 显示「已停止」而实际 Running）。
        {
            let service_status = self.query_service_status_wire();
            self.events.refresh_service_status(service_status.clone());
            self.logs
                .append_core(
                    "info",
                    "kernel",
                    "kernel.service.status_refreshed_on_route",
                    &format!(
                        "service status cache refreshed after connect route: installed={} state={}",
                        service_status
                            .as_ref()
                            .map(|s| s.installed)
                            .unwrap_or(false),
                        service_status
                            .as_ref()
                            .map(|s| s.state.clone())
                            .unwrap_or_default(),
                    ),
                    &BTreeMap::new(),
                )
                .ok();
        }

        // R1 attach-before-apply（硬约束）：先等 status 流挂接再派发 ApplyTunnel——
        // engine `StatusPublisher` 无快照，open 前事件丢弃；挂接后才不会丢阶段事件。
        self.log_status_gate(Some(attempt_id), Some(target_generation), "waiting");
        let ready_result = tokio::select! {
            biased;
            () = connect_cancelled(&mut cancelled) => {
                self.log_status_gate(Some(attempt_id), Some(target_generation), "cancelled");
                return Err(Status::cancelled("connect cancelled before apply"));
            }
            result = self.await_status_ready(target_generation) => result,
        };

        if let Err(status) = ready_result {
            self.log_status_gate(Some(attempt_id), Some(target_generation), "failed");
            self.apply_connect_failed_for_route(attempt_id, &status)
                .await;
            return Err(status);
        }

        // engine 可能被慢 RPC 占用；此等待和后续 lease 准备均可取消，不占 dispatch。
        let mut engine = tokio::select! {
            biased;
            () = connect_cancelled(&mut cancelled) => {
                self.log_status_gate(Some(attempt_id), Some(target_generation), "cancelled_waiting_engine");
                return Err(Status::cancelled("connect cancelled waiting for engine"));
            }
            engine = target_engine.lock() => engine,
        };
        let connect_result = execute_connect_tracked(
            &mut *engine,
            &self.config_dir,
            intent,
            ui_credentials,
            persist_credentials,
            Some(connection_mode),
            &self.reconnect_config_cache,
            &mut cancelled,
            || {
                // 极短线性化点：engine → dispatch；没有 await。Stop 若先标记取消，
                // 此处禁止 Apply；Apply 若先登记义务，Stop 随后等待 engine 再清理。
                let mut dispatch = self.connect_dispatch.lock().unwrap_or_else(|error| error.into_inner());
                let Some(state) = dispatch.as_mut().filter(|state| state.id == attempt_id && !*state.cancelled.borrow()) else {
                    return Err(Status::cancelled("connect cancelled before apply"));
                };
                let attachment = *self.status_attachment.borrow();
                if *slot_swaps.borrow() == target_generation
                    && attachment.generation == target_generation && attachment.attached {
                    state.attempt_applied = true;
                    state.stop_required = true;
                    Ok(())
                } else {
                    Err(Status::unavailable("status_stream_not_ready|status attachment changed during connect preparation"))
                }
            },
        )
        .await;
        drop(engine);
        self.log_status_gate(
            Some(attempt_id),
            Some(target_generation),
            if connect_result.is_ok() {
                "dispatched"
            } else {
                "dispatch_failed"
            },
        );
        let (reply, zeroized) = match connect_result {
            Ok(result) => result,
            Err(status) => {
                self.apply_connect_failed_for_route(attempt_id, &status)
                    .await;
                return Err(status);
            }
        };

        // 发送完成：`execute_connect` 已确定性零化 KernelControl wire 副本（明文不再驻留）。
        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.connect.dispatched",
                "connect dispatched to engine (secret one-shot, wire zeroized)",
                &BTreeMap::new(),
            )
            .ok();
        let _ = zeroized;
        // R1：engine 的 ApplyTunnel 立即回 `pending`（ApplyAccepted，带 operation_id）；
        // 终态/阶段走 StreamConnectStatus（状态转发器驱动状态机 + UI 事件）。
        Ok(Response::new(operation_reply_from_apply(&reply)))
    }

    /// 当前未取消的连接派发身份。自动重连用它把退避 timer 绑定到触发该 timer 的
    /// 会话；新手动连接会换 id，Stop 会设置 cancelled，两者都使旧 timer 失效。
    fn current_reconnect_dispatch_id(&self) -> Option<Uuid> {
        self.connect_dispatch
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|state| !*state.cancelled.borrow())
            .map(|state| state.id)
    }

    /// C3a 自动重连 worker：串行消费状态转发器发来的可重试掉线信号，驱动一次重连。
    ///
    /// 每个信号按序处理（天然防并发重连）；在途标记（`reconnect_active`）期间的新信号
    /// 直接跳过（防重入）。重连前判定：`auto_reconnect` 未开启 → 跳过；`max_attempts`
    /// 耗尽 → 跳过。每次实际重跑 connect 递增 `reconnect_attempts`（per-connection，
    /// Connected 成功后由状态转发器清零）。重连凭据从磁盘重新组装（`run_connect` 内部
    /// `execute_connect` load config + key.bin），不持有 engine 凭据。
    ///
    /// reconnect-backoff（2026-09-05）：`auto_reconnect_backoff` 开启时，每次派发
    /// connect 前 sleep 退避延时（由 `reconnect_attempts` 派生：2s→4s→8s→16s→30s
    /// cap；attempts 在 Connected 清零的既有语义使成功自动重置回 base）；关闭时走现状
    /// 立即重连路径（不 sleep、行为零变化）。sleep 在占用在途标记之后、派发之前——
    /// 睡眠期间到达的新掉线信号按防重入跳过；停机 abort worker 句柄（shutdown.rs
    /// `set_reconnect_worker_task`）会取消进行中的 sleep。
    async fn drive_reconnect(&self) {
        // 防重入：上一次重连尝试仍在途（已派发、终态未到）→ 不重复触发。
        if self.reconnect_active.load(Ordering::Acquire) {
            self.logs
                .append_core(
                    "info",
                    "kernel",
                    "kernel.reconnect.skip_in_flight",
                    "auto reconnect skipped: a reconnect attempt is still in flight",
                    &BTreeMap::new(),
                )
                .ok();
            return;
        }
        // 重连判定只读本会话冻结策略：设置保存只为下一次用户连接准备，绝不在
        // 当前连接掉线后改变重连预算、退避或 UI 显示。
        let Some(policy) = self
            .reconnect_session_policy
            .lock()
            .ok()
            .and_then(|policy| *policy)
        else {
            return;
        };
        let ReconnectSessionPolicy {
            auto_reconnect,
            max_attempts: max,
            backoff,
            connection_mode,
        } = policy;
        if !auto_reconnect {
            self.logs
                .append_core(
                    "info",
                    "kernel",
                    "kernel.reconnect.disabled",
                    "retryable data-plane drop observed but auto_reconnect is disabled",
                    &BTreeMap::new(),
                )
                .ok();
            return;
        }
        let attempts = self.reconnect_attempts.load(Ordering::Relaxed);
        if max != 0 && attempts >= max {
            self.logs
                .append_core(
                    "warn",
                    "kernel",
                    "kernel.reconnect.exhausted",
                    &format!("auto reconnect stopped: attempts={attempts} exhausted (max={max})"),
                    &BTreeMap::new(),
                )
                .ok();
            return;
        }

        let Some(origin_dispatch_id) = self.current_reconnect_dispatch_id() else {
            self.logs
                .append_core(
                    "info",
                    "kernel",
                    "kernel.reconnect.skip_superseded",
                    "auto reconnect skipped: no current originating session",
                    &BTreeMap::new(),
                )
                .ok();
            return;
        };

        // 重连执行：占用在途标记 + 计数 + 复用 run_connect（凭据从磁盘重新组装）。
        self.reconnect_active.store(true, Ordering::Release);
        self.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
        let attempt = attempts + 1;
        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.reconnect.attempt",
                &format!(
                    "auto reconnect attempt {attempt} (max={}, mode={})",
                    if max == 0 {
                        "unlimited".to_string()
                    } else {
                        max.to_string()
                    },
                    connection_mode.as_str()
                ),
                &BTreeMap::new(),
            )
            .ok();
        // 自动重连只允许读取已持久化的凭据；绝不复用先前 UI 的一次性密码。
        // reconnect-backoff：开启时在派发前 sleep 退避延时（占用在途标记之后——睡眠
        // 期间新掉线信号按防重入跳过；停机 abort worker 会取消进行中的 sleep）。
        // backoff 关闭（默认）→ 零 sleep，与现状立即重连完全一致。
        if backoff {
            let delay = reconnect_backoff_delay(
                self.reconnect_backoff_base,
                self.reconnect_backoff_cap,
                attempt,
            );
            self.logs
                .append_core(
                    "info",
                    "kernel",
                    "kernel.reconnect.backoff",
                    &format!(
                        "auto reconnect backoff: sleeping {} ms before attempt {attempt}",
                        delay.as_millis()
                    ),
                    &BTreeMap::new(),
                )
                .ok();
            tokio::time::sleep(delay).await;
        }
        match self
            .run_connect(
                reconnect_intent(),
                None,
                false,
                None,
                Some(origin_dispatch_id),
                connection_mode,
            )
            .await
        {
            Ok(_reply) => {
                // 已派发：保持 in-flight（终态事件由状态转发器清 reconnect_active）。
                self.logs
                    .append_core(
                        "info",
                        "kernel",
                        "kernel.reconnect.dispatched",
                        &format!("auto reconnect attempt {attempt} dispatched (credentials re-assembled from disk)"),
                        &BTreeMap::new(),
                    )
                    .ok();
            }
            Err(status) => {
                // 新手动会话或 Stop 已替换/取消旧 dispatch：旧 timer 只退出，不能
                // 清掉新会话的 reconnect_active，也不能把旧失败投影到新会话。
                if status.code() == tonic::Code::Cancelled
                    && status.message() == AUTO_RECONNECT_SUPERSEDED
                {
                    self.logs
                        .append_core(
                            "info",
                            "kernel",
                            "kernel.reconnect.skip_superseded",
                            "auto reconnect skipped: the originating session was replaced or stopped",
                            &BTreeMap::new(),
                        )
                        .ok();
                    return;
                }
                // 同步失败：上报失败（ConnectFailed → 状态机 Failed），释放在途标记。
                self.reconnect_active.store(false, Ordering::Release);
                // run_connect 在持有本次派发锁时发布失败；这里不能让旧 worker 的迟到
                // 错误（尤其 Cancel）覆盖新的连接或已经收束的 Idle。
                self.logs
                    .append_core(
                        "warn",
                        "kernel",
                        "kernel.reconnect.failed",
                        &format!(
                            "auto reconnect attempt {attempt} failed before dispatch: code={:?} msg={}",
                            status.code(),
                            status.message()
                        ),
                        &BTreeMap::new(),
                    )
                    .ok();
            }
        }
    }

    /// 拉起自动重连 worker 任务（C3a 生产接线）。worker 持服务克隆（轻量共享句柄，
    /// 内部全部 Arc/Copy），可调 `run_connect`/路由等 `&self` 方法；停机时必须 abort
    /// 本句柄释放 worker（否则服务被 worker 持有的克隆永久引用）。
    #[must_use]
    pub fn spawn_reconnect_worker(&self) -> tokio::task::JoinHandle<()> {
        // 换入绑定本 worker 接收端的发送端：此后状态转发器的信号经该通道到达本 worker。
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        *self.reconnect_tx.lock().unwrap() = Some(tx);
        let service = self.clone();
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                service.drive_reconnect().await;
            }
        })
    }

    /// 订阅 engine connect-status 流并驱动状态机 + 事件总线（R1 真实订阅的 host 侧接线）。
    ///
    /// 后台任务循环：acquire engine `stream_connect_status`（R1w 独立 status 通道，
    /// **非 StreamLogs**——日志已退役为纯输出，绝不回流状态）→ 逐事件
    /// （[`EngineStatusEvent`]）驱动 composition 状态机（阶段推进 / 真实 Connected /
    /// 失败带 err / 停止收敛）并发布 `RuntimeEvent` → 状态流 EOF（engine 断开）→
    /// 发布断线过渡事件（Reconciling 表示）→ 退避重连（断线重放语义：核心状态机为
    /// 单一事实源，重连后从当前 phase 继续）。调用方（P3-c2 进程生命周期）在 engine
    /// 就绪后调用；返回 `JoinHandle` 供测试保持/终止。
    ///
    /// **attach-before-apply 硬约束**：本转发器持有唯一挂接的 status 流；connect/stop
    /// 写路径在派发前依赖它已挂接（engine `StatusPublisher` 无快照、open 前事件丢弃）。
    #[must_use]
    pub fn spawn_status_forwarder(&self) -> tokio::task::JoinHandle<()> {
        let slot = self.engine.clone();
        let mut slot_swaps = slot.subscribe_swaps();
        let owner = StatusForwarderOwner {
            permit: Arc::clone(&self.status_forwarder_owner)
                .try_lock_owned()
                .ok(),
            attachment: self.status_attachment.clone(),
            logs: Arc::clone(&self.logs),
            selected_mode: Arc::clone(&self.selected_mode),
            core_session_id: self.core_session_id,
            forwarder_id: NEXT_STATUS_FORWARDER_ID.fetch_add(1, Ordering::Relaxed),
            engine_generation: *slot_swaps.borrow(),
            attach_attempt: 0,
            reattach_reason: "initial",
            backoff: Duration::ZERO,
        };
        if owner.permit.is_none() {
            owner.log(
                "warn",
                "kernel.forward.status_already_active",
                "status forwarder already active in this core session; duplicate start rejected",
            );
            return tokio::spawn(async {});
        }
        owner.log(
            "info",
            "kernel.forward.status_started",
            "status forwarder owner started",
        );
        let events = Arc::clone(&self.events);
        let composition = Arc::clone(&self.composition);
        let reconnect_tx = Arc::clone(&self.reconnect_tx);
        let reconnect_attempts = Arc::clone(&self.reconnect_attempts);
        let reconnect_active = Arc::clone(&self.reconnect_active);
        // 发布路径只读连接受理时冻结的策略，永不随 settings 保存重新取值。
        let reconnect_session_policy = Arc::clone(&self.reconnect_session_policy);
        let connect_dispatch = Arc::clone(&self.connect_dispatch);
        let proxy_tun_probe = Arc::clone(&self.proxy_tun_probe);
        let system_proxy_probe = Arc::clone(&self.system_proxy_probe);
        let logs = Arc::clone(&self.logs);
        tokio::spawn(async move {
            // owner 在第一次 poll 前已取得 permit；即使任务尚未运行就被 abort 也会释放。
            let mut owner = owner;
            // 监测生命周期附着于这个 forwarder；任务被销毁时自动取消 timer。
            let mut network_monitor = NetworkMonitor::default();
            owner.backoff = ENGINE_EVENT_RECONNECT_BASE;
            loop {
                // generation 前后夹取 current：若 await 期间换点，就丢弃不匹配的 engine。
                owner.engine_generation = *slot_swaps.borrow_and_update();
                StatusAttachment::advance(&owner.attachment, owner.engine_generation);
                let engine = slot.current().await;
                let current_generation = *slot_swaps.borrow();
                if current_generation != owner.engine_generation {
                    owner.slot_swapped(current_generation);
                    continue;
                }
                owner.attach_attempt += 1;
                owner.log(
                    "info",
                    "kernel.forward.status_attach_attempt",
                    "status forwarder subscribing to engine",
                );
                let attach = tokio::select! {
                    biased;
                    changed = slot_swaps.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        owner.slot_swapped(*slot_swaps.borrow_and_update());
                        continue;
                    }
                    result = async {
                        let mut guard = engine.lock().await;
                        guard.stream_connect_status().await
                    } => result,
                };
                let stream = {
                    match attach {
                        Ok(stream) => stream,
                        Err(_) => {
                            // 无法订阅（断线）：撤销就绪（R3-C3 防 stale-true——窗口内
                            // 派发不误判已挂接）+ R2 收敛双保险（engine 不可达时若本机
                            // 正在停机收敛,补数据面侧加入→Stopped/Idle）+ 发布断线过渡
                            // + 退避后重试。
                            StatusAttachment::revoke(&owner.attachment, owner.engine_generation);
                            owner.log(
                                "warn",
                                "kernel.forward.status_attach_failed",
                                "status forwarder failed to subscribe current engine (will backoff)",
                            );
                            refresh_bus_proxy_tun(&events, &proxy_tun_probe);
                            refresh_bus_system_proxy(&events, &system_proxy_probe);
                            {
                                let mut composition_guard = composition.lock().await;
                                let generation = slot_swaps.borrow();
                                if *generation == owner.engine_generation {
                                    composition_guard.synthesize_data_plane_join();
                                    let snapshot = snapshot_with_reconnect(
                                        disconnected_snapshot(&composition_guard),
                                        reconnect_status_from_session(
                                            &reconnect_attempts,
                                            &reconnect_active,
                                            &reconnect_session_policy,
                                        ),
                                    );
                                    events.publish(wire::RuntimeEventKind::Transition, snapshot);
                                }
                            }
                            if !owner
                                .wait_for_retry(&mut slot_swaps, "attach_failed_backoff")
                                .await
                            {
                                return;
                            }
                            continue;
                        }
                    }
                };
                // attach-before-apply 就绪：status 流已挂接（engine `StatusPublisher`
                // 无快照、open 前事件丢弃）→ 写路径派发前可放心 apply。
                // 持有 swap 的只读借用直到发布结束；换点不能夹进检查和 attached=true 之间。
                let attached = {
                    let generation = slot_swaps.borrow();
                    if *generation == owner.engine_generation {
                        owner.attachment.send_if_modified(|state| {
                            if state.generation > owner.engine_generation {
                                return false;
                            }
                            *state = StatusAttachment {
                                generation: owner.engine_generation,
                                attached: true,
                            };
                            true
                        })
                    } else {
                        false
                    }
                };
                if !attached {
                    owner.slot_swapped(*slot_swaps.borrow_and_update());
                    continue;
                }
                owner.backoff = Duration::ZERO;
                owner.log(
                    "info",
                    "kernel.forward.status_attached",
                    "status forwarder attached to current engine",
                );
                owner.backoff = ENGINE_EVENT_RECONNECT_BASE;
                let mut stream = stream;
                let mut engine_swapped = false;
                loop {
                    tokio::select! {
                        biased;
                        changed = slot_swaps.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            // 当前 status stream 属于旧 engine；不要把旧流的 EOF 当作
                            // engine 断线，也不要让旧流继续吞掉新 engine 的状态事件。
                            owner.slot_swapped(*slot_swaps.borrow_and_update());
                            engine_swapped = true;
                            network_monitor.cancel();
                            break;
                        }
                        status_event = stream.next() => {
                            let Some(status_event) = status_event else {
                                break;
                            };
                            let status_received = Instant::now();
                            // R3-C2：终态事件（Connected/Failed/Idle）的 operation_id 与当前
                            // 在途操作不符 → 丢弃（防旧操作迟到终态驱动相态）；`None` = 丢弃。
                            // RT-SESSION-04：Connected 缺正值起点时在此归一化内采纳本地
                            // 时钟并落码 11（诊断只进日志聚合器）。
                            let mut composition_guard = composition.lock().await;
                            let composition_wait = status_received.elapsed();
                            // 排队等待 composition 时可能已经换代；从核对到状态提交与
                            // publish 一直持有代次借用和 composition，禁止旧事件迟到投影。
                            let generation = slot_swaps.borrow();
                            if *generation != owner.engine_generation {
                                continue;
                            }
                            let status_event = normalize_unexpected_stop(status_event, &composition_guard, &logs);
                            if let Some((kind, snapshot)) =
                                runtime_event_from_status_locked(&status_event, &mut composition_guard, &logs)
                            {
                                let operation_id = Uuid::from_slice(&snapshot.operation_id).ok();
                                network_monitor.invalidate_if_different(operation_id, owner.engine_generation);
                                match &status_event {
                                    EngineStatusEvent::Connected { .. } => {
                                        if let Some(operation_id) = operation_id {
                                            let attempt_id = connect_dispatch.lock()
                                                .unwrap_or_else(|error| error.into_inner())
                                                .as_ref().map(|dispatch| dispatch.id);
                                            network_monitor.start(Arc::clone(&logs), SnapshotContext::new(
                                                "connected_baseline", Some(operation_id), attempt_id,
                                                reconnect_attempts.load(Ordering::Relaxed), owner.engine_generation,
                                            ), operation_id, owner.engine_generation);
                                        }
                                    }
                                    EngineStatusEvent::Failed { .. } => {
                                        let attempt_id = connect_dispatch.lock()
                                            .unwrap_or_else(|error| error.into_inner())
                                            .as_ref().map(|dispatch| dispatch.id);
                                        network_monitor.record_disconnect(&logs, &SnapshotContext::new(
                                            "connection_failed", operation_id, attempt_id,
                                            reconnect_attempts.load(Ordering::Relaxed), owner.engine_generation,
                                        ));
                                    }
                                    EngineStatusEvent::Stopped { .. } => network_monitor.cancel(),
                                    _ => {}
                                }
                                // 调度只读后台任务，不在 composition 锁内执行任何网络探测。
                                if let EngineStatusEvent::Failed { error, .. } = &status_event {
                                    if is_retryable_disconnect(error) {
                                        let attempt_id = connect_dispatch.lock()
                                            .unwrap_or_else(|error| error.into_inner())
                                            .as_ref().map(|dispatch| dispatch.id);
                                        request_snapshot(Arc::clone(&logs), SnapshotContext::new(
                                            "retryable_disconnect", operation_id, attempt_id,
                                            reconnect_attempts.load(Ordering::Relaxed), owner.engine_generation,
                                        ));
                                    }
                                }
                                let probe_started = Instant::now();
                                refresh_bus_proxy_tun(&events, &proxy_tun_probe);
                                let proxy_tun_duration = probe_started.elapsed();
                                let system_proxy_started = Instant::now();
                                refresh_bus_system_proxy(&events, &system_proxy_probe);
                                let system_proxy_duration = system_proxy_started.elapsed();
                                // S3/D5 修复：Connected 过渡时刷新 SCM 服务状态缓存——
                                // 服务可能由外部/bootstrap 启动，缓存若停留在 connect 前
                                // 旧值，WatchEvents 快照会让 UI 显示「已停止」而实际 Running。
                                //
                                // C3a：Connected = 一次真实成功连接（新连接生命周期）——
                                // 清零 per-connection 重连计数 + 释放重连在途标记（预算回到 0）。
                                if let EngineStatusEvent::Connected { .. } = &status_event {
                                    reconnect_attempts.store(0, Ordering::Relaxed);
                                    reconnect_active.store(false, Ordering::Release);
                                    if let Some(service_status) =
                                        query_service_status(&RealServiceStatusSource, SERVICE_NAME)
                                            .ok()
                                            .map(|snap| {
                                                let health = derive_health(
                                                    &service_health_from_snapshot(&snap),
                                                );
                                                service_status_to_wire(&snap, health)
                                            })
                                    {
                                        events.refresh_service_status(Some(service_status));
                                    }
                                }
                                // C3a：任何终态（Failed/Stopped）都释放重连在途标记——
                                // 本次重连尝试已结束（成功/失败），允许下一次可重试掉线触发。
                                if matches!(
                                    &status_event,
                                    EngineStatusEvent::Failed { .. }
                                        | EngineStatusEvent::Stopped { .. }
                                ) {
                                    reconnect_active.store(false, Ordering::Release);
                                }
                                // 发布前附加当前连接冻结的重连状态；配置保存不会改变它。
                                events.publish(
                                    kind,
                                    snapshot_with_reconnect(
                                        snapshot,
                                        reconnect_status_from_session(
                                            &reconnect_attempts,
                                            &reconnect_active,
                                            &reconnect_session_policy,
                                        ),
                                    ),
                                );
                                // C3a：可重试数据面掉线（stage=DataPlane + retry=
                                // RetrySameOperation——C2 唯一来源）→ 触发自动重连。
                                // 异步信号给重连 worker（worker 内再判 auto_reconnect 与
                                // 预算/在途）；不阻塞事件循环。未开启 auto_reconnect 时
                                // 由 worker 记「已掉线但未开启」日志；断线状态已先发布。
                                if let EngineStatusEvent::Failed { error, .. } = &status_event {
                                    if is_retryable_disconnect(error) {
                                        let mut timing_fields = BTreeMap::from([
                                            ("operation_id".into(), operation_id.map_or_else(|| "unknown".into(), |id| id.to_string())),
                                            ("engine_generation".into(), owner.engine_generation.to_string()),
                                            ("reconnect_attempt".into(), reconnect_attempts.load(Ordering::Relaxed).to_string()),
                                            ("composition_wait_ms".into(), composition_wait.as_millis().to_string()),
                                            ("proxy_tun_probe_ms".into(), proxy_tun_duration.as_millis().to_string()),
                                            ("system_proxy_probe_ms".into(), system_proxy_duration.as_millis().to_string()),
                                            ("status_to_signal_ms".into(), status_received.elapsed().as_millis().to_string()),
                                        ]);
                                        if let Some(policy) = *reconnect_session_policy.lock()
                                            .unwrap_or_else(|error| error.into_inner()) {
                                            timing_fields.insert("effective_auto_reconnect".into(), policy.auto_reconnect.to_string());
                                            timing_fields.insert("effective_max_attempts".into(), policy.max_attempts.to_string());
                                            timing_fields.insert("effective_backoff".into(), policy.backoff.to_string());
                                        }
                                        let _ = logs.append_core(
                                            "info",
                                            "kernel",
                                            "kernel.reconnect.trigger",
                                            "retryable data-plane drop detected; signaling reconnect worker",
                                            &timing_fields,
                                        );
                                        // worker 未拉起时 `None`，信号丢弃（无重连语义）。
                                        if let Some(tx) = reconnect_tx.lock().unwrap().as_ref() {
                                            let _ = tx.send(());
                                        }
                                        timing_fields.insert("status_to_signal_ms".into(), status_received.elapsed().as_millis().to_string());
                                        let _ = logs.append_core("debug", "kernel", "kernel.reconnect.trigger_timing",
                                            "状态接收至重连信号的宿主耗时", &timing_fields);
                                    }
                                }
                            }
                        }
                    }
                }
                if engine_swapped {
                    // 换点前已撤销 status_ready；回到循环后从 EngineSlot 重新挂接。
                    continue;
                }
                // 状态流 EOF = engine 断开：撤销就绪（R3-C3 防 stale-true——退避窗口内
                // 派发不误判已挂接）→ R2 收敛双保险（engine 侧 Idle 终态可能随断线丢失
                // → 若本机正在停机收敛,补数据面侧加入→Stopped/Idle）→ 发布断线过渡 +
                // 退避重连（无 resume tick；状态机为单一事实源，重连后从当前 phase 继续）。
                network_monitor.record_stream_disconnect(&logs, owner.engine_generation);
                StatusAttachment::revoke(&owner.attachment, owner.engine_generation);
                owner.log(
                    "warn",
                    "kernel.forward.status_eof",
                    "status stream ended; forwarder will retry",
                );
                refresh_bus_proxy_tun(&events, &proxy_tun_probe);
                refresh_bus_system_proxy(&events, &system_proxy_probe);
                {
                    let mut composition_guard = composition.lock().await;
                    let generation = slot_swaps.borrow();
                    if *generation == owner.engine_generation {
                        composition_guard.synthesize_data_plane_join();
                        let snapshot = snapshot_with_reconnect(
                            disconnected_snapshot(&composition_guard),
                            reconnect_status_from_session(
                                &reconnect_attempts,
                                &reconnect_active,
                                &reconnect_session_policy,
                            ),
                        );
                        events.publish(wire::RuntimeEventKind::Transition, snapshot);
                    }
                }
                if !owner.wait_for_retry(&mut slot_swaps, "eof_backoff").await {
                    return;
                }
            }
        })
    }

    /// 订阅 engine 统计流并转发到事件总线（P5-b：core 统计归一化接入 EventBus）。
    ///
    /// 后台任务循环：acquire engine `stream_stats`（seam；真实通道 = `StreamStats`）→
    /// 逐 `StatsEvent` 经 [`TrafficSample`] 以累计字节增量归一化速度（权威口径，
    /// engine `rx_rate`/`tx_rate` convenience 不使用）→ [`EventBus::publish_stats`] 发布
    /// 到统计 lane → 统计流 EOF（engine 断开）→ 退避重连并**重建采样状态**（累计
    /// 计数即天然断点，重连后首条样本速率为 0，不跨断点虚构流量）。调用方（P3-c2 进程
    /// 生命周期 / `serve_kernel_control_pipe`）在 engine 就绪后与事件转发器一并拉起；
    /// 返回 `JoinHandle` 供测试保持/终止。
    ///
    /// RT-DIAG-03：转发器消费 [`StatsStreamItem`]（错误类别保留）与 [`SampleOutcome`]
    /// （回退事实），按 §4.1 冻结码表写 `kernel.stats.*` 诊断码（60s 同码节流；码
    /// 1/2/7 不节流）并维护 [`EventBus::stats_diagnostic`] 状态机。attempt 语义冻结：
    /// per 转发器生命周期从 1 计。重试中是状态而非事件——周期性重试不逐次记码。
    #[must_use]
    pub fn spawn_stats_forwarder(&self) -> tokio::task::JoinHandle<()> {
        let slot = self.engine.clone();
        let events = Arc::clone(&self.events);
        let logs = Arc::clone(&self.logs);
        let first_sample_timeout = self.stats_first_sample_timeout;
        tokio::spawn(async move {
            let mut backoff = ENGINE_EVENT_RECONNECT_BASE;
            // 与状态转发器一样监听 route/respawn 的 engine 换点：仅重连到旧 engine
            // 的 stats 流会使 UI 在已连接时永久拿不到新数据面的累计计数和速率。
            let mut slot_swaps = slot.subscribe_swaps();
            // attempt 语义（§4.1 冻结）：per 转发器生命周期从 1 计；每次订阅尝试 +1
            //（含退避重订与 slot swap 立即重订），不随 engine 代重置。
            let mut attempt: u32 = 0;
            let mut throttle = StatsDiagThrottle::new();
            // 本次订阅前实际等待过的退避时长（码 1 的 backoff_ms fields；首订与
            // slot swap 立即重订为 0）。
            let mut waited_backoff = Duration::ZERO;
            loop {
                // ---- 订阅尝试（码 1 / 码 3）----
                attempt = attempt.saturating_add(1);
                let current_attempt = attempt;
                let waited_backoff_ms =
                    u64::try_from(waited_backoff.as_millis()).unwrap_or(u64::MAX);
                let stream = {
                    // 从槽取当前 engine（P3 respawn 后自动指向新 engine）。
                    let engine = slot.current().await;
                    let mut guard = engine.lock().await;
                    guard.stream_stats(0).await
                };
                let stream = match stream {
                    Ok(stream) => {
                        events.update_stats_diagnostic(|d| {
                            d.state = StatsForwarderState::Subscribed;
                            d.attempt = current_attempt;
                            d.last_error = None;
                            d.last_transition_ms = Some(unix_now_ms());
                            d.suppressed_since_last = 0;
                        });
                        // 状态变化 → 码 3/4/5 连续重复节流复位（冻结规则）。
                        throttle.reset_retry_lane();
                        emit_stats_diag(
                            &logs,
                            StatsDiagCode::Subscribed,
                            [
                                ("attempt", current_attempt.to_string()),
                                ("backoff_ms", waited_backoff_ms.to_string()),
                            ],
                        );
                        stream
                    }
                    Err(err) => {
                        let kind = StatsErrorKind::from(&err);
                        let backoff_ms = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX);
                        // 先落码后更新状态：诊断可见即日志已落盘（测试无竞窗）。
                        if throttle.allow_retry(StatsDiagCode::SubscribeFailed, Some(kind)) {
                            emit_stats_diag(
                                &logs,
                                StatsDiagCode::SubscribeFailed,
                                [
                                    ("error_kind", kind.as_str().to_string()),
                                    ("attempt", current_attempt.to_string()),
                                    ("backoff_ms", backoff_ms.to_string()),
                                ],
                            );
                            events.update_stats_diagnostic(|d| d.suppressed_since_last = 0);
                        } else {
                            events.update_stats_diagnostic(|d| {
                                d.suppressed_since_last = d.suppressed_since_last.saturating_add(1);
                            });
                        }
                        events.update_stats_diagnostic(|d| {
                            d.state = StatsForwarderState::Retrying;
                            d.attempt = current_attempt;
                            d.last_error = Some(StatsForwarderError::SubscribeFailed);
                            d.last_transition_ms = Some(unix_now_ms());
                        });
                        // 无法订阅（断线）：退避后重试。
                        tokio::time::sleep(backoff).await;
                        waited_backoff = backoff;
                        backoff = (backoff * 2).min(ENGINE_EVENT_RECONNECT_MAX);
                        continue;
                    }
                };
                backoff = ENGINE_EVENT_RECONNECT_BASE;
                // 每次订阅重建采样状态：StreamStats 从当前累计计数开始，重连后首条
                // 样本速率 0，不把断点间隔误算为流量。
                let mut sample = TrafficSample::new();
                // 本订阅已收样本数（码 4/5 fields；每次订阅重置——断点语义）。
                let mut sample_count: u64 = 0;
                let mut stream = stream;
                let mut engine_swapped = false;
                // 首样本期限（码 6）：一次订阅生命周期内至多触发一次；首样本到达或
                // 超时落码后即解除（guard），state 保持 Subscribed 直至样本到达转
                // Receiving（U4 语义）。
                let mut first_sample_settled = false;
                let first_deadline = tokio::time::sleep(first_sample_timeout);
                tokio::pin!(first_deadline);
                loop {
                    tokio::select! {
                        changed = slot_swaps.changed() => {
                            // 所有 sender 已释放时没有可重连的 engine，结束任务。
                            if changed.is_err() {
                                return;
                            }
                            // 码 7（info，无字段）：引擎换点 → 立即重订阅（attempt 继续递增）。
                            emit_stats_diag(&logs, StatsDiagCode::SlotSwapped, []);
                            events.update_stats_diagnostic(|d| {
                                d.last_transition_ms = Some(unix_now_ms());
                            });
                            engine_swapped = true;
                            break;
                        }
                        _ = &mut first_deadline, if !first_sample_settled => {
                            // 码 6：订阅成功后预算内无任何样本（warn，wait_ms）。
                            first_sample_settled = true;
                            let wait_ms = u64::try_from(first_sample_timeout.as_millis())
                                .unwrap_or(u64::MAX);
                            emit_stats_diag(&logs, StatsDiagCode::FirstSampleTimeout, [
                                ("wait_ms", wait_ms.to_string()),
                            ]);
                            events.update_stats_diagnostic(|d| {
                                d.last_error = Some(StatsForwarderError::FirstSampleTimeout);
                                d.last_transition_ms = Some(unix_now_ms());
                                d.suppressed_since_last = 0;
                            });
                        }
                        stats_item = stream.next() => {
                            match stats_item {
                                Some(StatsStreamItem::Sample(ev)) => {
                                    first_sample_settled = true;
                                    sample_count = sample_count.saturating_add(1);
                                    // RT-SAMPLE-02：速率语义不变，回退事实按冻结表落码。
                                    let (stats, outcome) = normalize_stats(&ev, &mut sample);
                                    // 码 2：本订阅生命周期内第一条样本（info；不节流）。
                                    // 订阅成功把 state 置 Subscribed，故 state 尚为
                                    // Subscribed/Starting 即本订阅首样本（超时落码后迟到
                                    // 的首样本同样转 Receiving——U4 语义）。
                                    let mut is_first = false;
                                    events.update_stats_diagnostic(|d| {
                                        is_first = d.state != StatsForwarderState::Receiving;
                                        if is_first {
                                            d.state = StatsForwarderState::Receiving;
                                            d.last_error = None;
                                            d.last_transition_ms = Some(unix_now_ms());
                                            d.suppressed_since_last = 0;
                                        }
                                    });
                                    if is_first {
                                        // 状态变化 → 重试族节流复位。
                                        throttle.reset_retry_lane();
                                        emit_stats_diag(&logs, StatsDiagCode::FirstSample, [
                                            ("phase", stats_phase_diag_str(stats.phase).to_string()),
                                            ("engine_sequence", ev.sequence.to_string()),
                                        ]);
                                    }
                                    // 码 8：phase 判别值 ∉ 1..=5（归一化为 Unspecified 并
                                    // 照常透传；样本级 60s 节流）。
                                    if !(1..=5).contains(&ev.phase) {
                                        if throttle.allow_sample(DIAG_LANE_PHASE) {
                                            emit_stats_diag(&logs, StatsDiagCode::PhaseUnspecified, [
                                                ("engine_sequence", ev.sequence.to_string()),
                                            ]);
                                            events.update_stats_diagnostic(|d| {
                                                d.suppressed_since_last = 0;
                                            });
                                        } else {
                                            events.update_stats_diagnostic(|d| {
                                                d.suppressed_since_last =
                                                    d.suppressed_since_last.saturating_add(1);
                                            });
                                        }
                                    }
                                    if outcome.timestamp_regression {
                                        // 码 9：样本时间戳回退（info；样本级 60s 节流）。
                                        if throttle.allow_sample(DIAG_LANE_TIMESTAMP) {
                                            emit_stats_diag(&logs, StatsDiagCode::TimestampRegression, [
                                                ("engine_sequence", ev.sequence.to_string()),
                                            ]);
                                            events.update_stats_diagnostic(|d| {
                                                d.suppressed_since_last = 0;
                                            });
                                        } else {
                                            events.update_stats_diagnostic(|d| {
                                                d.suppressed_since_last =
                                                    d.suppressed_since_last.saturating_add(1);
                                            });
                                        }
                                    }
                                    if outcome.counter_regression {
                                        // 码 10：同一订阅流内累计计数回退（warn；节流同上）。
                                        if throttle.allow_sample(DIAG_LANE_COUNTER) {
                                            emit_stats_diag(&logs, StatsDiagCode::CounterRegression, [
                                                ("engine_sequence", ev.sequence.to_string()),
                                            ]);
                                            events.update_stats_diagnostic(|d| {
                                                d.suppressed_since_last = 0;
                                            });
                                        } else {
                                            events.update_stats_diagnostic(|d| {
                                                d.suppressed_since_last =
                                                    d.suppressed_since_last.saturating_add(1);
                                            });
                                        }
                                    }
                                    events.update_stats_diagnostic(|d| {
                                        d.last_sample_phase = Some(stats.phase);
                                        d.last_sample_sequence = Some(stats.engine_sequence);
                                    });
                                    events.publish_stats(stats);
                                }
                                Some(StatsStreamItem::StreamError(kind)) => {
                                    // 码 4：统计流中途传输错误项（warn；同码同类别 60s 节流）。
                                    if throttle.allow_retry(StatsDiagCode::StreamError, Some(kind)) {
                                        emit_stats_diag(&logs, StatsDiagCode::StreamError, [
                                            ("error_kind", kind.as_str().to_string()),
                                            ("sample_count", sample_count.to_string()),
                                        ]);
                                        events.update_stats_diagnostic(|d| {
                                            d.suppressed_since_last = 0;
                                        });
                                    } else {
                                        events.update_stats_diagnostic(|d| {
                                            d.suppressed_since_last =
                                                d.suppressed_since_last.saturating_add(1);
                                        });
                                    }
                                    events.update_stats_diagnostic(|d| {
                                        d.state = StatsForwarderState::Retrying;
                                        d.last_error = Some(StatsForwarderError::StreamError);
                                        d.last_transition_ms = Some(unix_now_ms());
                                    });
                                    break;
                                }
                                Some(StatsStreamItem::Ended) | None => {
                                    // 码 5：统计流干净 EOF（warn；sample_count 精确）。
                                    if throttle.allow_retry(StatsDiagCode::StreamEnded, None) {
                                        emit_stats_diag(&logs, StatsDiagCode::StreamEnded, [
                                            ("sample_count", sample_count.to_string()),
                                        ]);
                                        events.update_stats_diagnostic(|d| {
                                            d.suppressed_since_last = 0;
                                        });
                                    } else {
                                        events.update_stats_diagnostic(|d| {
                                            d.suppressed_since_last =
                                                d.suppressed_since_last.saturating_add(1);
                                        });
                                    }
                                    events.update_stats_diagnostic(|d| {
                                        d.state = StatsForwarderState::Retrying;
                                        d.last_error = Some(StatsForwarderError::StreamEnded);
                                        d.last_transition_ms = Some(unix_now_ms());
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
                if engine_swapped {
                    // 新 engine 已可用；跳过退避，立即对当前槽重新订阅（无退避等待）。
                    waited_backoff = Duration::ZERO;
                    continue;
                }
                // 统计流 EOF/stream_error = engine 断开：退避重连（无 resume tick；
                // 累计计数为断点）。
                tokio::time::sleep(backoff).await;
                waited_backoff = backoff;
                backoff = (backoff * 2).min(ENGINE_EVENT_RECONNECT_MAX);
            }
        })
    }

    /// 拉起 core 侧 KeepAlive 心跳 tick 任务（P2 有界存留）：按 `period` 周期发
    /// `KeepAlive` 到 engine——engine 侧刷新 `last_heartbeat` 单调计时，15s 未收到即
    /// 自清理+自退出（hung-core / 进程句柄路径故障兜底）。持共享 engine 控制面（与
    /// 事件/统计/日志转发器一致）；停机先中止（`CoreRuntime` 在 `shutdown_core` 前
    /// abort 本任务，释放 engine client 引用）。
    ///
    /// 失败（engine 掉线）best-effort：继续 tick，engine 恢复后自动续心跳——链接终止由
    /// liveness 监视/状态转发器驱动（本任务不改变业务状态）。
    #[must_use]
    pub fn spawn_keepalive_ticker(&self, period: Duration) -> tokio::task::JoinHandle<()> {
        crate::grpc_control::spawn_keepalive_ticker(
            self.engine.clone(),
            Arc::clone(&self.selected_mode),
            period,
        )
    }

    /// 订阅 engine 结构化日志流并聚合落盘（R3：日志纯单向输出链路）。
    ///
    /// 后台任务循环：acquire engine `stream_logs`（`resume_tick = 0`，从当前流位置
    /// 开始、不补拉——断线缺口由 engine raw 文件离线对账，O4）→ 逐条经
    /// [`ingest_engine_stream`] 落盘到共享聚合服务（磁盘为唯一真相源）→ 流 EOF /
    /// transport 错误（engine 断开）→ 退避重连。**纯单向输出**（D3 铁律）：本转发器
    /// 只写日志文件，绝不发布状态事件、绝不驱动状态机——`logs_never_drive_state`
    /// 不变量由此保持。调用方（P3-c2 进程生命周期 / `serve_kernel_control_pipe`）在
    /// engine 就绪后与事件/统计转发器一并拉起；返回 `JoinHandle` 供测试保持/终止。
    #[must_use]
    pub fn spawn_log_forwarder(&self) -> tokio::task::JoinHandle<()> {
        let slot = self.engine.clone();
        let logs = Arc::clone(&self.logs);
        let mut slot_swaps = slot.subscribe_swaps();
        tokio::spawn(async move {
            let mut backoff = ENGINE_LOG_RECONNECT_BASE;
            loop {
                let stream = {
                    // 从槽取当前 engine（P3 respawn / 路由换点后自动指向新 engine）。
                    let engine = slot.current().await;
                    let mut guard = engine.lock().await;
                    match guard.stream_logs(0).await {
                        Ok(stream) => stream,
                        Err(_) => {
                            // 无法订阅（断线）：退避后重试（日志缺口由 raw 离线对账）。
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(ENGINE_LOG_RECONNECT_MAX);
                            continue;
                        }
                    }
                };
                backoff = ENGINE_LOG_RECONNECT_BASE;
                let mut stream = std::pin::pin!(stream);
                let mut engine_swapped = false;
                loop {
                    tokio::select! {
                        changed = slot_swaps.changed() => {
                            // 路由换点（oneshot→service 等）：当前 log 流属于旧 engine。
                            // 中断本流、回到外层重新挂接新 engine——修复此前日志转发器
                            // 不监听换点、服务引擎日志从不进入聚合器的问题。
                            if changed.is_err() {
                                return;
                            }
                            engine_swapped = true;
                            break;
                        }
                        next = stream.next() => {
                            match next {
                                Some(Ok(event)) => {
                                    if let Err(e) = logs.append_engine(&event) {
                                        tracing::warn!(error = %e, "aggregate engine log failed");
                                    }
                                }
                                Some(Err(status)) => {
                                    tracing::warn!(error = %status, "engine log stream transport error; reconnect");
                                    break;
                                }
                                None => break, // EOF：退避重连。
                            }
                        }
                    }
                }
                if engine_swapped {
                    continue;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(ENGINE_LOG_RECONNECT_MAX);
            }
        })
    }
}

#[tonic::async_trait]
impl KernelControl for KernelControlService {
    type WatchEventsStream = WatchEventsStream;

    async fn connect(
        &self,
        request: Request<ConnectRequest>,
    ) -> Result<Response<OperationReply>, Status> {
        let persist_credentials = persist_credentials_requested(&request);
        let mut req = request.into_inner();
        self.connect_request(&mut req, persist_credentials).await
    }

    async fn respond_interaction(
        &self,
        request: Request<InteractionResponse>,
    ) -> Result<Response<OperationReply>, Status> {
        self.require_authorized().await?;
        let response = request.into_inner();
        validate_interaction_response(&response)?;

        // P3-c1：经 seam 转发 engine。engine 冻结 wire 无 interaction RPC → 真实实现
        // 返回 typed `Unimplemented`；2026-09-05 裁决维持现状（engine 无交互提示产生
        // 点，当前无触发场景，重开条件见 engine-obligation-wire 计划 §9.3）；fake
        // 观测转发语义。
        let engine = self.engine.current().await;
        let mut engine = engine.lock().await;
        let reply = engine
            .respond_interaction(response)
            .await
            .map_err(grpc_error_to_status)?;
        drop(engine);

        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.interaction.responded",
                "interaction response forwarded to engine",
                &BTreeMap::new(),
            )
            .ok();
        Ok(Response::new(reply))
    }

    async fn stop(
        &self,
        request: Request<StopRequest>,
    ) -> Result<Response<OperationReply>, Status> {
        self.require_authorized().await?;
        let req = request.into_inner();
        let intent = req
            .intent
            .ok_or_else(|| Status::invalid_argument("stop: missing stop intent"))?;
        validate_stop_intent(&intent)?;

        // composition → dispatch；发出取消后即释放同步短锁，随后才等待 engine。
        let mut composition = self.composition.lock().await;
        let (attempt_id, stop_required, target_generation) = {
            let mut dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match dispatch.as_mut() {
                Some(state) => {
                    state.cancelled.send_replace(true);
                    (Some(state.id), state.stop_required, state.target_generation)
                }
                // 无本会话记录时，保留真实清理可能存在的隧道的兼容路径。
                None => (None, true, None),
            }
        };

        // 用户显式停止即结束本连接生命周期；下一次连接必须重新读取已保存的重连设置。
        if let Ok(mut policy) = self.reconnect_session_policy.lock() {
            *policy = None;
        }
        self.reconnect_active.store(false, Ordering::Release);

        // R1：stop 受理即驱动状态机（Connected/Connecting/Failed → Stopping）。
        // 状态机单一事实源：用户 Stop 是唯一业务取消，立即反映到 phase。控制面（host）
        // 侧即加入 teardown 屏障；engine 状态流先发 Idle（数据面侧）→ 双侧齐 → Stopped
        //（R1 收敛；R2 再补 core 合成 Idle 双保险）。
        let operation_id = intent
            .lookup_key
            .as_ref()
            .and_then(|k| <[u8; 16]>::try_from(k.operation_id.as_slice()).ok());
        {
            composition.apply(HostEvent::Disconnect);
            composition.apply(HostEvent::TeardownSideJoined(TeardownSide::ProtocolControl));
            if let Some(id) = operation_id {
                composition.set_operation_id(id);
            }
        }
        drop(composition);
        // C1（ui-connect-stop-responsiveness）：受理即发布 Stopping 受理快照——发布点
        // 在 admission 之后、`await_status_ready` 之前，UI 即时离开乐观态（后续断开的
        // 真实收敛仍由状态转发器端到端发布）。
        self.publish_admission_snapshot().await;

        if !stop_required {
            // 本次连接没有创建数据面；控制面取消即完成，不向 engine 发送虚构的 Stop。
            self.finish_local_stop(attempt_id).await;
            self.log_status_gate(attempt_id, target_generation, "cancelled_without_apply");
            return Ok(Response::new(OperationReply { terminal: None }));
        }

        let stop_generation = *self.engine.subscribe_swaps().borrow();
        let readiness = self.await_status_ready(stop_generation).await;
        self.log_status_gate(
            attempt_id,
            Some(stop_generation),
            if readiness.is_ok() {
                "stop_attached"
            } else {
                "stop_without_status"
            },
        );
        // 已 Apply 的隧道始终需要真实停止；状态流不可达时依靠 Stop 的确认回执收束，
        // 不因展示通道故障留下网络资源，也不进入 service repair。

        // engine 派发：StopTunnel 携带 stop 意图的 lookup_key + request_digest。
        let engine = self.engine.current().await;
        let mut engine = engine.lock().await;
        // P1-b lease 前置：StopTunnel 同样经 `bind_mutation` 要求 `core.owner`（幂等
        // ensure——即使无 prior connect 或 engine 重连后也满足，避免
        // `failed_precondition("no owner lease established")`）。
        engine
            .ensure_owner_lease()
            .await
            .map_err(grpc_error_to_status)?;
        // Bug E：engine-facing `StopTunnelRequest` 的 `wire_key.method` 必须是 engine
        // `stop_tunnel` 期待的 `StopTunnel`——UI Stop 意图的 method=Stop 直接传给 engine
        // 会让 `kernel_request_to_operation` 判 "kernel: operation: out of scope"。转换只改
        // method，保留 runtime_epoch/operation_id。
        let mut engine_key = intent.lookup_key.clone();
        if let Some(key) = engine_key.as_mut() {
            key.method = wire::OperationMethod::StopTunnel as i32;
        }
        let reply = engine
            .stop_tunnel(StopTunnelRequest {
                lookup_key: engine_key,
                request_digest: intent.request_digest.clone(),
            })
            .await
            .map_err(grpc_error_to_status)?;
        drop(engine);

        if matches!(
            reply.result,
            Some(wire::stop_tunnel_reply::Result::Stopped(_))
        ) {
            self.finish_local_stop(attempt_id).await;
        }

        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.stop.dispatched",
                "stop dispatched to engine",
                &BTreeMap::new(),
            )
            .ok();
        Ok(Response::new(operation_reply_from_stop(&reply)))
    }

    async fn reconcile(
        &self,
        request: Request<ReconcileRequest>,
    ) -> Result<Response<OperationReply>, Status> {
        self.require_authorized().await?;
        let req = request.into_inner();
        let key = req
            .key
            .ok_or_else(|| Status::invalid_argument("reconcile: missing operation key"))?;
        let intent = ReconcileIntent::from_wire(key.clone(), req.request_digest.clone())
            .map_err(|e| Status::invalid_argument(e))?;

        // engine 派发：HelperControl 无 Reconcile RPC，经 GetOperation 观察当前义务的
        // disposition。
        let engine = self.engine.current().await;
        let mut engine = engine.lock().await;
        let op = engine
            .get_operation(GetOperationRequest {
                lookup_key: Some(intent.key.clone()),
            })
            .await
            .map_err(grpc_error_to_status)?;

        if reconcile_retry_plan(&op.state) == ReconcileRetry::Retry {
            match engine
                .retry_obligation(ReconcileRequest {
                    key: Some(intent.key),
                    request_digest: intent.request_digest.clone(),
                })
                .await
            {
                Ok(_) => {}
                Err(GrpcClientError::Rpc(Code::Unimplemented, _)) => {
                    self.logs
                        .append_core(
                            "warn",
                            "kernel",
                            "kernel.reconcile.retry.unwired",
                            "engine obligation retry not on frozen wire (P3-c2 seam)",
                            &BTreeMap::new(),
                        )
                        .ok();
                }
                // 其余错误（掉线/超时）：保留观测 disposition，不掩盖 RPC 回执。
                Err(_) => {}
            }
        }
        drop(engine);

        self.logs
            .append_core(
                "info",
                "kernel",
                "kernel.reconcile.observed",
                "reconcile observed obligation disposition",
                &BTreeMap::new(),
            )
            .ok();
        Ok(Response::new(operation_reply_from_operation_state(
            op.state,
        )))
    }

    async fn get_operation(
        &self,
        request: Request<GetKernelOperationRequest>,
    ) -> Result<Response<KernelOperationReply>, Status> {
        // 读路径不 gate（与 GetSnapshot/WatchEvents 一致）：路由到 engine GetOperation
        // 真实读取义务/操作 disposition。
        let key = request
            .into_inner()
            .key
            .ok_or_else(|| Status::invalid_argument("get_operation: missing operation key"))?;
        let engine = self.engine.current().await;
        let mut engine = engine.lock().await;
        let op = engine
            .get_operation(GetOperationRequest {
                lookup_key: Some(key),
            })
            .await
            .map_err(grpc_error_to_status)?;
        drop(engine);
        Ok(Response::new(KernelOperationReply { state: op.state }))
    }

    async fn get_snapshot(
        &self,
        _request: Request<SnapshotRequest>,
    ) -> Result<Response<RuntimeSnapshot>, Status> {
        // 查询失败只说明失去确认能力，不能把旧 Connected 继续作为现场事实。
        let query_basis = {
            let guard = self.composition.lock().await;
            (guard.phase(), guard.operation_id())
        };
        let engine_result = tokio::time::timeout(Duration::from_secs(3), async {
            let engine = self.engine.current().await;
            let mut engine = engine.lock().await;
            engine
                .observe_owned_state(ObserveOwnedStateRequest { lookup_key: None })
                .await
        }).await.unwrap_or(Err(GrpcClientError::Timeout));
        let engine_snapshot = match engine_result {
            Ok(reply) if reply.snapshot.is_some() => {
                if self.snapshot_query_failed.swap(false, Ordering::AcqRel) {
                    let _ = self.logs.append_core("debug", "connection", "kernel.snapshot.query_recovered",
                        "引擎状态查询已恢复", &BTreeMap::new());
                }
                reply.snapshot
            }
            result => {
                if !self.snapshot_query_failed.swap(true, Ordering::AcqRel) {
                    let category = match result {
                        Err(GrpcClientError::ConnectionLost) => "connection_lost",
                        Err(GrpcClientError::Timeout) => "timeout",
                        Err(GrpcClientError::Rpc(..)) => "rpc_error",
                        Err(GrpcClientError::Transport(_)) => "transport_error",
                        _ => "missing_snapshot",
                    };
                    let _ = self.logs.append_core("warn", "connection", "kernel.snapshot.query_failed",
                        "无法确认引擎当前状态，暂停已连接展示并等待状态恢复",
                        &BTreeMap::from([("error_category".into(), category.into())]));
                }
                None
            }
        };
        let mut composition = self.composition.lock().await;
        // 同名服务管道可重拨新进程而不换 EngineSlot；新引擎的空会话不能复活旧连接。
        // 仅裁决查询前后仍属同一已连接操作的事实，避免查询途中建立新会话的竞态。
        if query_basis.0 == HostPhase::Connected && composition.phase() == HostPhase::Connected
            && query_basis.1 == composition.operation_id()
            && engine_snapshot.as_ref().is_some_and(|snapshot| snapshot.operation_id.is_empty()
                && matches!(snapshot.state, Some(wire::runtime_snapshot::State::Idle(_)))) {
            let operation_id = composition.operation_id().map(|id| id.to_vec()).unwrap_or_default();
            let _ = self.logs.append_core("warn", "connection", "kernel.connection.engine_session_missing",
                "引擎报告没有活动会话，旧连接已经失效", &BTreeMap::new());
            let event = EngineStatusEvent::Failed { operation_id, error: wire::VpnError {
                code: wire::ErrorCode::EffectUnknown as i32, stage: wire::ErrorStage::DataPlane as i32,
                certainty: wire::EffectCertainty::NoEffect as i32, retry: wire::RetryAdvice::RetrySameOperation as i32,
                ..Default::default()
            }};
            if let Some((kind, snapshot)) = runtime_event_from_status_locked(&event, &mut composition, &self.logs) {
                self.events.publish(kind, snapshot_with_reconnect(snapshot, self.session_reconnect_status()));
                request_snapshot(Arc::clone(&self.logs), SnapshotContext::new("engine_session_missing",
                    composition.operation_id().map(Uuid::from_bytes), None,
                    self.reconnect_attempts.load(Ordering::Relaxed), *self.engine.subscribe_swaps().borrow()));
                if let Some(tx) = self.reconnect_tx.lock().unwrap().as_ref() { let _ = tx.send(()); }
            }
        }
        let composition_snapshot = if engine_snapshot.is_none() {
            disconnected_snapshot(&composition)
        } else {
            snapshot_for_phase(composition.phase(), &composition)
        };
        let confirmed_session_start = composition.session_established_at_ms();
        let selected_snapshot = prefer_snapshot(engine_snapshot, composition_snapshot);
        let cached = self.events.current_snapshot();
        let uncertain = matches!(selected_snapshot.state, Some(wire::runtime_snapshot::State::Reconciling(_)));
        let recovered = matches!(selected_snapshot.state, Some(wire::runtime_snapshot::State::Connected(_)));
        if (uncertain && cached.as_ref().is_some_and(|snapshot| matches!(snapshot.state, Some(wire::runtime_snapshot::State::Connected(_)))))
            || (recovered && cached.as_ref().is_some_and(|snapshot| matches!(snapshot.state, Some(wire::runtime_snapshot::State::Reconciling(_))))) {
            // 统计事件复用 EventBus 的状态，查询与推送必须一起撤下或恢复 Connected。
            self.events.publish(wire::RuntimeEventKind::Transition,
                snapshot_with_reconnect(selected_snapshot.clone(), self.session_reconnect_status()));
        }
        drop(composition);
        // P5-wire 方案 A：把 EventBus 统计 lane 的最新样本附加到快照（无样本 → None）。
        let stats = self.events.current_stats();
        // C5-wire：GetSnapshot 是 UI 拉取点——每次调用探测一次上游 proxy TUN 并刷新
        // 进事件总线缓存（启动即带共存状态，badge 无需等首次 engine 事件）。探测失败
        // → 缓存 None（`proxy_tun` 留空，状态上报不因探测失败而失败）。
        refresh_bus_proxy_tun(&self.events, &self.proxy_tun_probe);
        let proxy_tun = self.events.current_proxy_tun();
        // EXV_UNFREEZE：同点探测一次系统代理并刷新进缓存（启动即带系统代理感知状态，
        // 无需等首次 engine 事件）。探测失败 → 缓存 None（`system_proxy` 留空）。
        refresh_bus_system_proxy(&self.events, &self.system_proxy_probe);
        let system_proxy = self.events.current_system_proxy();
        // S3/D5：服务感知——GetSnapshot 是服务状态的拉取点（非提权查询；失败 → None，
        // 状态上报不因查询失败而失败）+ 模式展示（缺省 auto）。查询/模式同时刷新进
        // 事件总线缓存（`WatchEvents` 随快照附加同一事实）。
        let service_status = self.query_service_status_wire();
        let mode = self.mode_string();
        self.events.refresh_service_status(service_status.clone());
        self.events.refresh_service_mode(Some(mode.clone()));
        // GetSnapshot 只投影连接受理时冻结的重连策略，不读取当前设置。
        let reconnect = self.session_reconnect_status();
        // EXV_UNFREEZE 2026-09-05：GetSnapshot 与事件通道同源附加自愈进展（读 EventBus
        // lane 当前缓存——拉取通道每次组装都读缓存，对 `refresh_self_heal` 即时生效，
        // 包括 Connect 受理的清除；事件订阅者则要等下一次自然 publish）。
        let self_heal = self.events.current_self_heal();
        Ok(Response::new(snapshot_with_reconnect(
            snapshot_with_self_heal(
                snapshot_with_service_context(
                    snapshot_with_system_proxy(
                        snapshot_with_proxy_tun(
                            snapshot_with_stats(
                                snapshot_with_confirmed_session_start(
                                    selected_snapshot,
                                    confirmed_session_start,
                                ),
                                stats,
                            ),
                            proxy_tun,
                        ),
                        system_proxy,
                    ),
                    service_status,
                    Some(mode),
                ),
                self_heal,
            ),
            reconnect,
        )))
    }

    async fn watch_events(
        &self,
        request: Request<WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        // 读路径不 gate（与 GetSnapshot 一致）。真实订阅（P3-c1）：resume_tick 语义见
        // [`EventBus::subscribe`]——0 = 从当前快照开始；落后 → 重放当前快照；随后
        // 转发 tick 递增的现场事件。事件由 engine 状态转发器（`spawn_status_forwarder`）
        // 与写路径发布。
        let resume_tick = request.into_inner().resume_tick;
        let stream = self
            .events
            .subscribe(resume_tick)
            .map(Ok::<RuntimeEvent, Status>);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn logs_list(
        &self,
        request: Request<LogsListRequest>,
    ) -> Result<Response<LogsListReply>, Status> {
        // 读路径不 gate（与 GetSnapshot 一致）：拉聚合日志历史（磁盘文件为真相源）。
        // 磁盘读取是阻塞 I/O：放 blocking pool，避免卡住 async worker——否则日志加载期间
        // 同 runtime 上的其它 RPC（连接/停止/设置）全部排队（并发响应被阻断）。
        let req = request.into_inner();
        let limit = if req.limit == 0 {
            100
        } else {
            req.limit as usize
        };
        let logs = Arc::clone(&self.logs);
        let page = tokio::task::spawn_blocking(move || logs.list(req.after_seq, limit))
            .await
            .map_err(|e| Status::internal(format!("logs_list: join {e}")))?
            .map_err(|e| Status::internal(format!("logs_list: {e:?}")))?;
        let entries = page
            .entries
            .into_iter()
            .map(|e| e.into_wire_event())
            .collect();
        Ok(Response::new(LogsListReply {
            entries,
            next_seq: page.next_seq,
        }))
    }

    async fn logs_clear(
        &self,
        _request: Request<LogsClearRequest>,
    ) -> Result<Response<LogsClearReply>, Status> {
        // 写路径 gate：清空聚合日志（运维操作，mutates 磁盘）。磁盘写放 blocking pool。
        self.require_authorized().await?;
        let removed = self.logs.last_seq();
        let logs = Arc::clone(&self.logs);
        tokio::task::spawn_blocking(move || logs.clear())
            .await
            .map_err(|e| Status::internal(format!("logs_clear: join {e}")))?
            .map_err(|e| Status::internal(format!("logs_clear: {e:?}")))?;
        Ok(Response::new(LogsClearReply {
            cleared: true,
            removed_entries: removed,
        }))
    }

    async fn config_get(
        &self,
        _request: Request<ConfigGetRequest>,
    ) -> Result<Response<ConfigPayload>, Status> {
        // 读路径不 gate：返回非 secret 配置项。
        let startup = load_for_startup(&self.config_dir)
            .map_err(|e| Status::internal(format!("config_get: {e:?}")))?;
        let cfg = startup.config;
        // 开关与凭据可用性分开：记住开关可能开启但密文/密钥缺失。验证所得明文由
        // CredentialsBundle 的零化 guard 当场释放，ConfigGet 只返回布尔状态。
        let has_stored_password = cfg.remember_password
            && !cfg.password.is_empty()
            && crate::credential::load_credentials(&self.config_dir).is_ok();
        let items = vec![
            ConfigItem {
                key: "has_stored_password".into(),
                value: has_stored_password.to_string(),
            },
            ConfigItem {
                key: "server".into(),
                value: cfg.server,
            },
            ConfigItem {
                key: "username".into(),
                value: cfg.username,
            },
            ConfigItem {
                key: "remember_password".into(),
                value: cfg.remember_password.to_string(),
            },
            ConfigItem {
                key: "routes".into(),
                value: cfg.routes.join(","),
            },
            ConfigItem {
                key: "user_agent".into(),
                value: cfg.user_agent,
            },
            ConfigItem {
                key: "mtu".into(),
                value: cfg.mtu.to_string(),
            },
            ConfigItem {
                key: "connection_mode".into(),
                value: cfg.connection_mode.as_str().into(),
            },
            ConfigItem {
                key: "auto_reconnect".into(),
                value: cfg.auto_reconnect.to_string(),
            },
            ConfigItem {
                key: "auto_reconnect_max_attempts".into(),
                value: cfg.auto_reconnect_max_attempts.to_string(),
            },
            ConfigItem {
                key: "auto_reconnect_backoff".into(),
                value: cfg.auto_reconnect_backoff.to_string(),
            },
        ];
        Ok(Response::new(ConfigPayload {
            items,
            requires_quick_start: startup.requires_quick_start,
        }))
    }

    async fn config_set(
        &self,
        request: Request<ConfigSetRequest>,
    ) -> Result<Response<ConfigReply>, Status> {
        // 写路径 gate：应用并持久化配置。
        self.require_authorized().await?;
        let mut cfg = ExvConfig::load_from_dir(&self.config_dir)
            .map_err(|e| Status::internal(format!("config_set: load {e:?}")))?;
        let previous_username = cfg.username.clone();
        let mut password = None;
        for item in request.into_inner().items {
            match item.key.as_str() {
                "server" => cfg.server = item.value,
                "username" => cfg.username = item.value,
                "remember_password" => {
                    cfg.remember_password = item.value.parse().map_err(|_| {
                        Status::invalid_argument("config_set: remember_password must be true/false")
                    })?;
                }
                "routes" => {
                    cfg.routes = item
                        .value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
                "user_agent" => cfg.user_agent = item.value,
                "mtu" => {
                    cfg.mtu = item
                        .value
                        .parse()
                        .map_err(|_| Status::invalid_argument("config_set: mtu must be a number"))?
                }
                "connection_mode" => {
                    cfg.connection_mode = item.value.parse().map_err(|()| {
                        Status::invalid_argument(
                            "config_set: connection_mode must be standard/compatibility",
                        )
                    })?
                }
                "auto_reconnect" => {
                    cfg.auto_reconnect = item.value.parse().map_err(|_| {
                        Status::invalid_argument("config_set: auto_reconnect must be true/false")
                    })?
                }
                "auto_reconnect_max_attempts" => {
                    cfg.auto_reconnect_max_attempts = item.value.parse().map_err(|_| {
                        Status::invalid_argument(
                            "config_set: auto_reconnect_max_attempts must be a number",
                        )
                    })?
                }
                "auto_reconnect_backoff" => {
                    cfg.auto_reconnect_backoff = item.value.parse().map_err(|_| {
                        Status::invalid_argument(
                            "config_set: auto_reconnect_backoff must be true/false",
                        )
                    })?
                }
                "password" => {
                    password = Some(zeroize::Zeroizing::new(item.value));
                }
                _ => {
                    return Err(Status::invalid_argument(format!(
                        "config_set: unknown key '{}'",
                        item.key
                    )));
                }
            }
        }
        // 整批字段决定最终选择；加密函数会置 remember=true，不能让字段顺序覆盖遗忘。
        // 单独更换账户不能继承旧账户密码。同账户显式切换 VPN 线路继续复用凭据。
        if cfg.username != previous_username && password.as_ref().is_none_or(|value| value.is_empty()) {
            cfg.password.zeroize();
            cfg.password.clear();
        }
        if !cfg.remember_password {
            cfg.password.zeroize();
            cfg.password.clear();
        } else if let Some(password) = password.filter(|value| !value.is_empty()) {
            let mut key = match ExvConfig::load_key(&self.config_dir) {
                Ok(Some(key)) => key,
                Ok(None) => {
                    return Err(Status::internal(
                        "config_set: password key unavailable (reinstall?)",
                    ));
                }
                Err(e) => {
                    return Err(Status::internal(format!("config_set: password key read {e:?}")));
                }
            };
            let encrypted = cfg.set_password_encrypted(&password, &key);
            key.zeroize();
            encrypted.map_err(|e| {
                Status::internal(format!("config_set: password encrypt {e:?}"))
            })?;
        }
        save_after_user_submission(&self.config_dir, &cfg)
            .map_err(|e| Status::internal(format!("config_set: save {e:?}")))?;
        // 保存配置仅供下一次用户连接读取，不改变当前会话策略。
        // 历史配置缓存同步失效；运行时发布和重连决策已经不再使用它。
        self.reconnect_config_cache.invalidate();
        Ok(Response::new(ConfigReply { ok: true }))
    }

    async fn service_control(
        &self,
        request: Request<ServiceControlRequest>,
    ) -> Result<Response<ServiceControlReply>, Status> {
        let req = request.into_inner();
        let action = req
            .action
            .ok_or_else(|| Status::invalid_argument("service_control: no action"))?;
        // UI 已经有 busy 门，但 Core 仍必须把服务生命周期操作串行化：重复点击、
        // 多窗口或迟到 RPC 不能让 install/uninstall/start 交叉修改同一个 SCM 条目。
        let _service_transition_guard =
            if matches!(&action, service_control_request::Action::Query(_)) {
                None
            } else {
                Some(self.service_control_lock.lock().await)
            };
        // 变更动作（install/uninstall/start）走 write path gate；query 为非提权读。
        match action {
            service_control_request::Action::Query(_) => {
                // S3-B：query 含健康加深（SCM Running/StartPending 时有界调 engine
                // 深度自述，把报告折进 health_state）。
                self.service_control_query_reply().await
            }
            service_control_request::Action::Install(_) => {
                self.require_authorized().await?;
                if let Err(error) = self.stop_engine_for_transition(ServiceMode::Oneshot).await {
                    return self.service_control_reply(true, Err(error)).await;
                }
                // 业务隧道已收束；服务进程的停止由批量内 SCM Stop 唯一负责，避免
                // Shutdown RPC 自退出与 SCM 停止/失败恢复交叉。
                let outcome = self.service_ops.install().await;
                match outcome {
                    Ok(_) => {
                        // 安装成功后清除上一次卸载的 SCM 语义覆盖。安装阶段仍是
                        // auto：只有真正换入 service engine 的 connect 才记录 service，
                        // 避免把"安装完成"误当成"当前连接已经使用服务"。
                        self.clear_service_removed_override();
                        // 存在性互斥（2026-09-08 计划批 2）：服务形态成为唯一权威——
                        // 退役 oneshot engine（业务停机已由 transition stop 完成，此处
                        // 收进程）+ 槽换回 detached 占位。下一次 connect 走 Service
                        // 分支拨号服务管道；卸载后的 oneshot 连接则按需重新 provision。
                        if let Some(provisioner) = &self.engine_provisioner {
                            provisioner.retire_oneshot_engine().await;
                        }
                        match self.ensure_service_ready().await {
                            Ok(readiness) => self.service_control_start_reply(readiness).await,
                            Err(error) => self.service_control_reply(true, Err(error)).await,
                        }
                    }
                    Err(error) => self.service_control_reply(true, Err(error)).await,
                }
            }
            service_control_request::Action::Uninstall(_) => {
                self.require_authorized().await?;
                if let Err(error) = self.stop_engine_for_transition(ServiceMode::Service).await {
                    return self.service_control_reply(true, Err(error)).await;
                }
                // 业务清理先完成，再由批量 SCM Stop 控制进程退出与删除注册。
                let outcome = self.service_ops.uninstall().await;
                if outcome.is_ok() {
                    // DeleteService 已返回成功；SCM marked-for-delete 的短暂残留不再
                    // 影响本次事务的业务状态回传。
                    self.mark_service_removed();
                    // 卸载完成的边界（2026-09-08 计划批 2）：槽换回 detached 占位、
                    // 维护形态记录 oneshot。不再保留预拉 oneshot——下一次一次性连接
                    // 由 provisioner 按需重新提权拉起（每 core 会话内仍只在首次需要
                    // 时 1 次 UAC）。无 provisioner（测试）时仅换占位。
                    if let Some(provisioner) = &self.engine_provisioner {
                        provisioner.retire_oneshot_engine().await;
                    } else {
                        self.swap_engine_for_route(crate::grpc_control::detached_engine())
                            .await;
                    }
                    self.record_mode(ServiceMode::Oneshot);
                }
                let outcome = outcome.map(|msg| friendly_service_message("uninstall", &msg));
                self.service_control_reply(true, outcome).await
            }
            service_control_request::Action::Start(_) => {
                self.require_authorized().await?;
                // 显式 Start 是用户业务手势：bootstrap 失败 = 服务客观问题 → 允许一次
                // 修复子进程补洞（2026-09-08 计划批 4）。
                let mut repair_budget = RepairBudget::default();
                match self
                    .ensure_service_ready_for_connect(&mut repair_budget)
                    .await
                {
                    Ok(readiness) => self.service_control_start_reply(readiness).await,
                    Err(error) => self.service_control_reply(true, Err(error)).await,
                }
            }
            service_control_request::Action::RotateKey(_) => {
                self.require_authorized().await?;
                let _ = self.logs.append_core(
                    "info",
                    "kernel",
                    "service.rotate.requested",
                    "service PSK rotation requested (revoke leaked key copies)",
                    &BTreeMap::new(),
                );
                let outcome = match self.service_ops.rotate_key().await {
                    Ok(step_message) => {
                        // 4.4 审计：fingerprint（SHA-256 前 8 字节 hex，非秘密）入日志，
                        // 与批量侧/engine accept 侧可关联；禁 PSK 原文/HMAC。
                        let _ = self.logs.append_core(
                            "info",
                            "kernel",
                            "service.rotate.ok",
                            "service PSK rotated; the previous key is rejected from the next connection on",
                            &BTreeMap::from([(
                                "fingerprint".to_string(),
                                rotate_fingerprint_of(&step_message).unwrap_or_default(),
                            )]),
                        );
                        Ok(friendly_service_message("rotate", &step_message))
                    }
                    Err(error) => {
                        // 诚实失败：旧 key 仍有效，服务不受影响。
                        let _ = self.logs.append_core(
                            "warn",
                            "kernel",
                            "service.rotate.failed",
                            "service PSK rotation failed; the previous key remains valid",
                            &BTreeMap::from([("error_class".to_string(), error.clone())]),
                        );
                        Err(error)
                    }
                };
                self.service_control_reply(true, outcome).await
            } // 服务停止 = 错误态（A5），修复对策是启动；wire 已删 Stop（tag 5 reserved），
              // match 五个显式 action 完备即穷尽，未知 action 由外层校验拒绝。
        }
    }
}

/// 把引擎侧硬编码的 `"batch completed"` 替换为面向用户的人类可读完成信息。
///
/// rotate 先行：成功消息由批量步骤消息（`rotated fingerprint=<16hex>`）改写为
/// 4.3 冻结全文（含 4.5 兼容矩阵的旧 engine 生效时点注明）；fingerprint 形状不符
/// 时原样透传（诚实呈现，不伪造摘要）。`msg` 非 `"batch completed"` 时原样返回
///（引擎失败信息、非标准路径均不拦截）。
fn friendly_service_message(action: &str, msg: &str) -> String {
    if action == "rotate" {
        return match msg.strip_prefix("rotated fingerprint=") {
            Some(fingerprint) => format!(
                "服务密钥已轮换（fingerprint {fingerprint}），旧密钥自下一次连接起失效；\
                 旧版本引擎（启动读一次密钥）将立即失去控制面连接，需 stop/start 或修复（各一次 UAC）恢复"
            ),
            None => msg.to_string(),
        };
    }
    if msg == "batch completed" {
        match action {
            "install" => "服务安装完成并已启动。".to_string(),
            "uninstall" => "服务卸载完成。".to_string(),
            "start" => "服务已启动。".to_string(),
            _ => msg.to_string(),
        }
    } else {
        msg.to_string()
    }
}

/// 从批量 RotateKey 步骤消息（`rotated fingerprint=<16hex>`）提取 fingerprint
/// （审计日志字段用；形状不符 → `None`，调用方落空串并保留原样消息）。
fn rotate_fingerprint_of(step_message: &str) -> Option<String> {
    step_message
        .strip_prefix("rotated fingerprint=")
        .map(str::to_string)
}

impl KernelControlService {
    /// 组装 ServiceControl 回复：post-action 服务状态 + `ok` + 人类可读信息。
    ///
    /// 查询失败（SCM 打开/读取异常）→ `ok=false` + 错误信息，`service_status` 留空；
    /// 变更动作成功 → `ok=true` + 携带的完成信息；失败 → `ok=false` + 原因。
    async fn service_control_reply(
        &self,
        _is_mutation: bool,
        outcome: Result<String, String>,
    ) -> Result<Response<ServiceControlReply>, Status> {
        let (ok, message) = match outcome {
            Ok(msg) => (true, msg),
            Err(e) => (false, e),
        };
        let service_status = self.query_service_status_wire();
        self.events.refresh_service_status(service_status.clone());
        Ok(Response::new(ServiceControlReply {
            service_status,
            ok,
            message,
        }))
    }

    /// S3-B：组装 ServiceControl **query** 回复——SCM 状态 + **健康加深**。
    ///
    /// 与既有 [`Self::service_control_reply`] 的差异：query 是非提权读，且在 SCM 状态为
    /// Running/StartPending（服务在途/运行）时有界调用 engine 深度自述
    /// （`ServiceProbe::probe_self_report` → `ServiceManage.query`，Tier 2 零 UAC 通道），
    /// 把报告折进 `health_state`：self-not-ready → `InstalledUnavailable`（SCM Running 但
    /// 引擎控制面未就绪 / PSK 缺席）；探针失败（拨号/PSK/超时）→ 保守保留 SCM 派生。
    /// 非 Running/StartPending 不触发探针（A6）。
    async fn service_control_query_reply(&self) -> Result<Response<ServiceControlReply>, Status> {
        let service_status = self.query_service_status_deepened().await;
        self.events.refresh_service_status(service_status.clone());
        Ok(Response::new(ServiceControlReply {
            service_status,
            ok: true,
            message: "ok".to_string(),
        }))
    }

    /// S3-B：SCM 快照 → 健康加深后的 wire `ServiceStatus`。
    ///
    /// 廉价事实（`service_health_from_snapshot`）随快照计算；Running/StartPending 时
    /// 追加一次 engine 深度自述探活，报告折进 [`derive_health_with_self_report`]。
    async fn query_service_status_deepened(&self) -> Option<wire::ServiceStatus> {
        let snap = self.query_service_status_snapshot()?;
        let health = service_health_from_snapshot(&snap);
        let deepened = if matches!(
            health.state,
            ServiceState::Running | ServiceState::StartPending
        ) {
            // 有界探活（复用 keepalive 探针时限）；失败保守（None → 保留 SCM 派生）。
            let report = self.service_probe.probe_self_report().await;
            derive_health_with_self_report(&health, report.as_ref())
        } else {
            derive_health(&health)
        };
        Some(service_status_to_wire(&snap, deepened))
    }

    /// 组装 ServiceControl **Start** 回复（R2 双采纳）：以 [`wait_for_service_ready`] 的
    /// 就绪事实驱动 `ok`/`message`（+ 最新 SCM 状态）。
    ///
    /// - 就绪（同一拍 SCM running 且 keepalive 回复，`source=Both`）→ `ok=true`，
    ///   message 标注双事实达成来源；
    /// - 超时（双事实从未同拍成立）→ `ok=false`，message 按 4.2 失败分类产出
    ///  （稳定前缀 + 最后 SCM 状态 + keepalive 事实 + 耗时，前端可读）。
    async fn service_control_start_reply(
        &self,
        readiness: ServiceReadiness,
    ) -> Result<Response<ServiceControlReply>, Status> {
        let (ok, message) = if readiness.ready {
            // 双采纳下 ready 恒为 Both——双事实标注即达成来源（`ReadinessSource`
            // 收缩后的机械改写：单方事实分支已不存在）。
            (
                true,
                "服务已就绪（SCM running 且 keepalive 回复，同拍达成）".to_string(),
            )
        } else {
            (false, readiness_failure_message(&readiness))
        };
        let service_status = self.query_service_status_wire();
        self.events.refresh_service_status(service_status.clone());
        Ok(Response::new(ServiceControlReply {
            service_status,
            ok,
            message,
        }))
    }
}

impl KernelControlService {
    /// 写 RPC 的前置 gate 检查：transport peer 未授权 → `unauthenticated`。
    ///
    /// `KernelControlGate` 在 composition 内；授权由 transport peer 认证路径驱动
    /// （P3-c1 `authorize_transport_peer`，进程生命周期 P3-c2 在传输层认证后调用）。
    /// 本实现执行 fail-closed 的拒绝路径——任何未经授权的写 RPC 一律拒绝。
    async fn require_authorized(&self) -> Result<(), Status> {
        let mut composition = self.composition.lock().await;
        if !composition.kernel_gate().is_authorized() {
            return Err(Status::unauthenticated(
                "KernelControl transport peer not authorized (gate wiring lands in P3-c)",
            ));
        }
        Ok(())
    }

    /// 消费已解包的 Connect 请求。请求 carrier 从入口到返回始终由此方法持有；任何在
    /// 解析 UI 载荷之前的早退都先原位清零秘密字节，避免普通 `Vec` drop 留下明文。
    async fn connect_request(
        &self,
        request: &mut ConnectRequest,
        persist_credentials: bool,
    ) -> Result<Response<OperationReply>, Status> {
        if let Err(error) = self.require_authorized().await {
            zeroize_connect_secret(request);
            return Err(error);
        }

        let intent = match request.intent.take() {
            Some(intent) => intent,
            None => {
                zeroize_connect_secret(request);
                return Err(Status::invalid_argument("connect: missing connect intent"));
            }
        };
        if let Err(error) = validate_intent(&intent) {
            zeroize_connect_secret(request);
            return Err(error);
        }
        let ui_credentials = match take_ui_credentials(request) {
            Ok(credentials) => credentials,
            Err(error) => {
                zeroize_connect_secret(request);
                return Err(error);
            }
        };

        // 用户点击连接这一刻读取已经保存的设置并冻结为会话策略；自动重连复用该
        // 策略，期间任何 ConfigSet 都只为下一次用户连接准备。
        let session_policy = self.reconnect_policy_for_new_session();
        // 纯逻辑核心（admission → route → attach-before-apply → execute_connect）与
        // C3a 自动重连共用；只有当前 RPC 能携带一次性 UI 凭据，重连显式传 None。
        self.run_connect(
            intent,
            ui_credentials,
            persist_credentials,
            Some(session_policy),
            None,
            session_policy.connection_mode,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Connect：凭据生命周期 + engine 派发 + 发送后零化（纯逻辑，测试可注入 fake engine）。
// ---------------------------------------------------------------------------

async fn execute_connect_tracked(
    engine: &mut dyn KernelEngineControl,
    config_dir: &Path,
    intent: wire::ConnectIntent,
    ui_credentials: Option<CredentialPackage>,
    persist_credentials: bool,
    connection_mode_override: Option<ConnectionMode>,
    reconnect_cache: &ReconnectConfigCache,
    cancelled: &mut watch::Receiver<bool>,
    before_apply: impl FnOnce() -> Result<(), Status> + Send,
) -> Result<(ApplyTunnelReply, ConnectRequest), Status> {
    let (config, mut payload) = match ui_credentials {
        Some(mut credentials) => {
            let mut config = ExvConfig::load_from_dir(config_dir)
                .map_err(|e| Status::internal(format!("connect: config load: {e}")))?;
            persist_ui_credentials(
                config_dir,
                &mut config,
                &credentials,
                persist_credentials,
                reconnect_cache,
            )?;
            let payload = credentials
                .to_bytes()
                .map_err(|e| Status::internal(format!("connect: secret assembly: {e}")))?;
            credentials.zeroize();
            (config, payload)
        }
        None => {
            let mut bundle = load_credentials(config_dir).map_err(credential_error_to_status)?;
            let payload = bundle
                .assemble_secret_payload()
                .map_err(|e| Status::internal(format!("connect: secret assembly: {e}")))?;
            (bundle.config, payload)
        }
    };

    // KernelControl wire 副本（`secret_payload` 即一次性明文载体；发送后零化）。
    let mut connect_req = build_connect_request(intent.clone(), std::mem::take(&mut payload));
    payload.zeroize();
    // engine 请求：lookup_key/request_digest 派生自 connect 意图；plan 从 config 近似
    // 组装（真实协商地址/路由由 CSTP 协商 P5 替换）；`secret_payload` 由 `apply_connect`
    // 从 one-shot 槽移入。
    let connection_mode = connection_mode_override.unwrap_or(config.connection_mode);
    let apply = match apply_request_from_connect(&connect_req, &config, connection_mode) {
        Ok(apply) => apply,
        Err(error) => {
            zeroize_connect_secret(&mut connect_req);
            return Err(error);
        }
    };

    // protobuf 旧 peer 会忽略未知 tag=5。兼容模式先走既有只读 ServiceManage 自述，
    // 没有明确能力声明便拒绝，避免 RPC 成功但 engine 实际仍按标准模式装配。
    let mode_support = tokio::select! {
        biased;
        () = connect_cancelled(cancelled) => Err(Status::cancelled("connect cancelled checking connection mode support")),
        result = ensure_engine_supports_connection_mode(engine, connection_mode) => result,
    };
    if let Err(error) = mode_support {
        zeroize_connect_secret(&mut connect_req);
        return Err(error);
    }

    // P1-b lease 前置：engine 的 `ApplyTunnel` 经 `bind_mutation` 要求 `core.owner` 已建立
    // （MaintainOwnerLease 授权握手）——缺失时 engine 返回 `failed_precondition("no owner
    // lease established")`。`ensure_owner_lease` 幂等（已握手则直接返回）。
    let lease_result = tokio::select! {
        biased;
        () = connect_cancelled(cancelled) => Err(Status::cancelled("connect cancelled preparing owner lease")),
        result = engine.ensure_owner_lease() => result.map_err(grpc_error_to_status),
    };
    if let Err(error) = lease_result {
        zeroize_connect_secret(&mut connect_req);
        return Err(error);
    }

    // engine 派发：槽持有明文，`apply_connect` 移入 wire 并发送后零化槽（成败两路径）。
    if let Err(status) = before_apply() {
        zeroize_connect_secret(&mut connect_req);
        return Err(status);
    }
    let mut secret = ClearableSecret::new(&connect_req.secret_payload);
    let reply = engine
        .apply_connect(apply, &mut secret)
        .await
        .map_err(grpc_error_to_status);

    // 无论 engine 成功或失败均确定性零化 KernelControl wire 副本（明文不再驻留）。
    zeroize_connect_secret(&mut connect_req);
    let reply = reply?;
    Ok((reply, connect_req))
}

async fn ensure_engine_supports_connection_mode(
    engine: &mut dyn KernelEngineControl,
    connection_mode: ConnectionMode,
) -> Result<(), Status> {
    if connection_mode == ConnectionMode::Standard {
        return Ok(());
    }
    let request = wire::ServiceManageRequest {
        action: Some(wire::service_manage_request::Action::Query(
            wire::ServiceSelfQuery {},
        )),
    };
    let reply = match engine.service_manage(request).await {
        Ok(reply) => reply,
        Err(GrpcClientError::Rpc(tonic::Code::Unimplemented, _)) => {
            return Err(compatibility_mode_unavailable());
        }
        Err(error) => return Err(grpc_error_to_status(error)),
    };
    let supported = reply
        .self_report
        .as_ref()
        .is_some_and(|report| {
            report.supported_windows_connection_modes.contains(
                &(wire::WindowsConnectionMode::Compatibility as i32),
            )
        });
    if !supported {
        return Err(compatibility_mode_unavailable());
    }
    Ok(())
}

fn compatibility_mode_unavailable() -> Status {
    Status::failed_precondition(
        "compatibility_mode_unavailable|当前 EXV 网络服务版本不支持兼容模式，请更新或修复 EXV 后重试",
    )
}

/// 解析本次 RPC 的 UI 一次性凭据。先取走并归零 wire bytes，再对零化类型解析副本做
/// 严格版本/空值校验；任一失败均不进入 engine。
fn take_ui_credentials(request: &mut ConnectRequest) -> Result<Option<CredentialPackage>, Status> {
    let mut payload = std::mem::take(&mut request.secret_payload);
    if payload.is_empty() {
        return Ok(None);
    }
    let parsed = parse_secret_payload(&payload)
        .map_err(|_| Status::invalid_argument("connect: invalid credential payload"));
    payload.zeroize();
    let mut credentials = parsed?;
    if credentials.version != SECRET_PAYLOAD_VERSION
        || credentials.username.trim().is_empty()
        || credentials.password.is_empty()
    {
        credentials.zeroize();
        return Err(Status::invalid_argument(
            "connect: invalid credential payload",
        ));
    }
    Ok(Some(credentials))
}

/// `persist=true` 是 Tauri→Core 请求元数据，刻意不进入 engine 固定 payload。只有精确
/// `true` 才允许持久化；缺失、格式不符或旧客户端全部保守视为本次使用。
fn persist_credentials_requested(request: &Request<ConnectRequest>) -> bool {
    request
        .metadata()
        .get("x-exv-persist-credentials")
        .and_then(|value| value.to_str().ok())
        == Some("true")
}

/// UI 凭据的唯一落盘边界：只有显式 `persist=true` 才保存用户名和 AES 密文。
/// `persist=false` 只更新本次连接内存中的用户名，不改磁盘身份、密文或记住开关。
/// C5（ui-connect-stop-responsiveness）：整文件重写成功后主动失效重连配置缓存
///（本函数不改 auto_reconnect 键，但失效廉价且无脑正确——缓存与磁盘的因果线
/// 只有写点）。
fn persist_ui_credentials(
    config_dir: &Path,
    config: &mut ExvConfig,
    credentials: &CredentialPackage,
    persist_credentials: bool,
    reconnect_cache: &ReconnectConfigCache,
) -> Result<(), Status> {
    config.username.clone_from(&credentials.username);
    // 临时凭据只参与当前内存连接请求；失败时磁盘仍是原账户及其完整保存凭据。
    if !persist_credentials {
        return Ok(());
    }
    if persist_credentials {
        let mut key = ExvConfig::ensure_key(config_dir)
            .map_err(|e| Status::internal(format!("connect: credential key: {e}")))?;
        let encrypted = config.set_password_encrypted(&credentials.password, &key);
        key.zeroize();
        encrypted.map_err(|e| Status::internal(format!("connect: credential save: {e}")))?;
    }
    save_after_user_submission(config_dir, config)
        .map_err(|e| Status::internal(format!("connect: credential save: {e}")))?;
    reconnect_cache.invalidate();
    Ok(())
}

/// 缺凭据是前端恢复流的稳定类别，其他配置/序列化错误保持原有内部错误语义。
fn credential_error_to_status(error: CredentialError) -> Status {
    let code = match error {
        CredentialError::KeyMissing => Some("key_missing"),
        CredentialError::KeyRead(_) => Some("key_read"),
        CredentialError::PasswordEmpty => Some("password_not_remembered"),
        CredentialError::PasswordDecrypt(_) => Some("password_decrypt"),
        CredentialError::Config(_) | CredentialError::Serialize(_) => None,
    };
    match code {
        Some(code) => Status::failed_precondition(format!("credential_required|{code}")),
        None => Status::internal(format!("connect: credential load: {error}")),
    }
}

/// 校验 connect 意图：`request_digest` 32 字节 + `lookup_key.method` == CONNECT。
///
/// # Errors
/// digest 长度非法 / 缺 lookup_key / method 不匹配 → `Status::invalid_argument`。
fn validate_intent(intent: &wire::ConnectIntent) -> Result<(), Status> {
    if intent.request_digest.len() != 32 {
        return Err(Status::invalid_argument(
            "connect: request digest must be 32 bytes",
        ));
    }
    let key = intent
        .lookup_key
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("connect: missing lookup key"))?;
    if key.method != wire::OperationMethod::Connect as i32 {
        return Err(Status::invalid_argument(
            "connect: lookup key method must be CONNECT",
        ));
    }
    Ok(())
}

/// 从 KernelControl `ConnectRequest` 组装 engine `ApplyTunnelRequest`。
///
/// `lookup_key`/`request_digest` 派生自 connect 意图；`plan` 从 config 近似组装
/// （P3-b2：真实隧道地址/路由由 CSTP 协商 P5 替换，此处用确定性的 config 派生计划）；
/// `secret_payload` 留空，由 [`crate::grpc_control::KernelEngineControl::apply_connect`]
/// 从 one-shot 槽移入（P3-b1 契约）。
///
/// # Errors
/// 缺 connect 意图 / 缺 lookup_key → `Status::invalid_argument`。
fn apply_request_from_connect(
    connect_req: &wire::ConnectRequest,
    config: &ExvConfig,
    connection_mode: ConnectionMode,
) -> Result<wire::ApplyTunnelRequest, Status> {
    let intent = connect_req
        .intent
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("connect: missing connect intent"))?;
    let mut lookup_key = intent
        .lookup_key
        .clone()
        .ok_or_else(|| Status::invalid_argument("connect: missing lookup key"))?;
    // Bug E：engine-facing `ApplyTunnelRequest` 的 `wire_key.method` 必须是 engine
    // `apply_tunnel` 期待的 `ApplyTunnel`——UI Connect 意图的 method=Connect 直接传给
    // engine 会让 `kernel_request_to_operation` 判 `method != expected_method ->
    // "kernel: operation: out of scope"`。转换只改 method，保留 runtime_epoch/operation_id
    //（engine 侧以认证 peer 覆盖 principal，操作身份 = peer + ApplyTunnel + 该 key）。
    lookup_key.method = wire::OperationMethod::ApplyTunnel as i32;
    let request_digest = connection_mode_bound_digest(&intent.request_digest, connection_mode);
    let plan = plan_from_config(config, &request_digest);
    Ok(ApplyTunnelRequest {
        lookup_key: Some(lookup_key),
        plan: Some(plan),
        request_digest,
        secret_payload: vec![],
        windows_connection_mode: match connection_mode {
            ConnectionMode::Standard => wire::WindowsConnectionMode::Standard as i32,
            ConnectionMode::Compatibility => wire::WindowsConnectionMode::Compatibility as i32,
        },
    })
}

/// 把一次连接冻结的模式纳入 engine mutation 的 canonical digest。这样相同 lookup key
/// 若携带不同模式，不会被 engine 的幂等表误认成前一次装配结果。
fn connection_mode_bound_digest(
    connect_request_digest: &[u8],
    connection_mode: ConnectionMode,
) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(b"exv.windows.apply-tunnel.connection-mode.v1\0");
    digest.update((connect_request_digest.len() as u64).to_be_bytes());
    digest.update(connect_request_digest);
    digest.update(connection_mode.as_str().as_bytes());
    digest.finalize().to_vec()
}

/// 从用户 config 近似组装 `TunnelPlan`（P3-b2）。
///
/// `ipv4_routes` 从 `config.routes`（CIDR）解析；`mtu` 取自 config（夹到 ≥ 576）；
/// `ipv4_address` 用确定性占位（真实客户端隧道地址来自 CSTP 协商，P5 替换）；
/// `opaque_intent` 用 connect 的 `request_digest`（32 字节，标识该意图）。
///
/// `control_bypass` **恒空**（2026-09-08 计划 I6）：网关 /32 物理旁路由 engine 在
/// 装配点以 VGDC 解析地址驱动安装（`exv-vpn-win32-resource::routes::gateway_bypass_row`，
/// 见 engine `platform_tunnel`/`tunnel_runtime`），不经 wire plan——历史
/// `server_bypass_ips`（手填网关 IP → control_bypass）链路整体退役（config 字段
/// 读丢弃、写禁止）。wire 字段保留仅为冻结面兼容（`convert::tunnel_plan_from_wire`
/// 语义不变，空表合法）。
fn plan_from_config(config: &ExvConfig, intent_digest: &[u8]) -> wire::TunnelPlan {
    let ipv4_routes = config
        .routes
        .iter()
        .filter_map(|r| {
            parse_cidr(r).map(|(network, prefix_len)| wire::Ipv4Route {
                network,
                prefix_len,
            })
        })
        .collect();
    wire::TunnelPlan {
        // P3-b2 近似：客户端隧道地址来自 CSTP 协商（P5）；此处确定性占位。
        ipv4_address: vec![0, 0, 0, 0],
        ipv4_prefix_len: 32,
        mtu: config.mtu.max(576),
        ipv4_routes,
        dns_servers: vec![],
        // 恒空（2026-09-08 计划 I6）：网关 bypass 由 engine 以 VGDC 解析地址驱动，
        // 不经 wire plan 下发。
        control_bypass: Vec::new(),
        proxy_exempt: Vec::new(),
        opaque_intent: Some(wire::TunnelIntentRef {
            identity_digest: intent_digest.to_vec(),
        }),
    }
}

/// 解析 CIDR（`a.b.c.d/prefix`）→ (4 字节 network, prefix_len)；非法输入 → `None`。
fn parse_cidr(cidr: &str) -> Option<(Vec<u8>, u32)> {
    let (addr, prefix) = cidr.split_once('/')?;
    let prefix_len: u32 = prefix.parse().ok()?;
    if prefix_len > 32 {
        return None;
    }
    let octets: Vec<u8> = addr
        .split('.')
        .map(|o| o.parse::<u8>().ok())
        .collect::<Option<Vec<_>>>()?;
    (octets.len() == 4).then_some((octets, prefix_len))
}

// ---------------------------------------------------------------------------
// Stop / Reconcile：完整意图组装 + engine 派发。
// ---------------------------------------------------------------------------

/// 校验 stop 意图：`request_digest` 32 字节 + `lookup_key.method` == STOP。
///
/// # Errors
/// digest 长度非法 / 缺 lookup_key / method 不匹配 → `Status::invalid_argument`。
fn validate_stop_intent(intent: &wire::StopIntent) -> Result<(), Status> {
    if intent.request_digest.len() != 32 {
        return Err(Status::invalid_argument(
            "stop: request digest must be 32 bytes",
        ));
    }
    let key = intent
        .lookup_key
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("stop: missing lookup key"))?;
    if key.method != wire::OperationMethod::Stop as i32 {
        return Err(Status::invalid_argument(
            "stop: lookup key method must be STOP",
        ));
    }
    Ok(())
}

/// Reconcile 的完整重试意图（lookup_key + request_digest）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileIntent {
    /// 义务的完整 lookup key（method 必须为 RECONCILE）。
    pub key: wire::OperationLookupKey,
    /// 重试请求摘要（32 字节）。
    pub request_digest: Vec<u8>,
}

impl ReconcileIntent {
    /// 从 wire `ReconcileRequest` 组装完整意图：校验 `key.method` == RECONCILE 与
    /// `request_digest` 长度（32 字节）。
    ///
    /// # Errors
    /// method 不匹配或 digest 长度非法 → `&'static str`。
    pub fn from_wire(
        key: wire::OperationLookupKey,
        request_digest: Vec<u8>,
    ) -> Result<Self, &'static str> {
        if key.method != wire::OperationMethod::Reconcile as i32 {
            return Err("reconcile: lookup key method must be RECONCILE");
        }
        if request_digest.len() != 32 {
            return Err("reconcile: request digest must be 32 bytes");
        }
        Ok(Self {
            key,
            request_digest,
        })
    }
}

// ---------------------------------------------------------------------------
// engine 错误 → gRPC Status；engine reply → OperationReply。
// ---------------------------------------------------------------------------

/// 把 engine 控制面错误映射为 gRPC `Status`（transport 类 → unavailable；
/// 超时 → deadline_exceeded；其余透传 code）。
fn grpc_error_to_status(e: GrpcClientError) -> Status {
    match e {
        GrpcClientError::Transport(_) | GrpcClientError::ConnectionLost => {
            Status::unavailable("engine control unavailable")
        }
        GrpcClientError::Timeout => Status::deadline_exceeded("engine control timed out"),
        GrpcClientError::Rpc(code, message) => Status::new(code, message),
    }
}

/// 确定性 controller peer + capability（R1：authorize 时绑定到 runtime actor）。
///
/// 固定字面量构造，镜像 acceptance `crash_matrix::controller_peer_and_capability` 与
/// portable H80 `peer_and_capability` 同款（portable host 测试的确定性构造）。真实
/// 绑定推导（从 `VerifiedPipePeer` 派生 PrincipalDigest/Binding）属 P3-c2 进程生命周期
/// 接线；此处为 connect 受理路径提供确定性的已绑定 actor（非伪造外部身份）。
///
/// `pub(crate)`：P3 崩溃自愈（`crash_recovery`）重建 composition 后复用同一确定性
/// actor 绑定。
pub(crate) fn controller_peer_and_capability() -> (PeerContext, PeerCapability) {
    let principal = PrincipalDigest::try_from([0u8; 32]).expect("principal digest");
    let binding = ConnectionBindingDigest::try_from([1u8; 32]).expect("binding digest");
    let metadata = VerifiedConnectionMetadata::try_from((principal.clone(), binding.clone()))
        .expect("verified metadata");
    let peer = PeerContext::try_from(metadata).expect("peer context");
    let connection = ConnectionBinding::try_from(binding).expect("connection binding");
    let capability = PeerCapability::try_from((
        connection,
        principal,
        AuthorityEpoch::try_from(7u64).expect("authority epoch"),
        OperationMethod::Connect,
        MonotonicTick::try_from(42u64).expect("monotonic tick"),
    ))
    .expect("peer capability");
    (peer, capability)
}

/// `ApplyTunnelReply` → `OperationReply`（KernelControl 通用 mutation 回执）。
fn operation_reply_from_apply(reply: &ApplyTunnelReply) -> OperationReply {
    match reply.result.as_ref() {
        Some(wire::apply_tunnel_reply::Result::Applied(receipt)) => OperationReply {
            terminal: Some(wire::OperationTerminal {
                result: Some(wire::operation_terminal::Result::Succeeded(receipt.clone())),
            }),
        },
        Some(wire::apply_tunnel_reply::Result::Failed(failed)) => OperationReply {
            terminal: Some(wire::OperationTerminal {
                result: Some(wire::operation_terminal::Result::Failed(failed.clone())),
            }),
        },
        // R1w 机械适配（仅编译通过，不做 host 逻辑重构——那是 R1 范围）：
        // engine 的 ApplyTunnel 现为异步，先回 `pending` ApplyAccepted；终态走独立
        // StreamConnectStatus 通道。OperationReply 无 pending 分支，故映射为
        // `terminal: None`（尚无终局）；host 侧的 pending→status-channel 语义映射
        // 属 R1/R4 范围，此处仅保持编译绿。
        Some(wire::apply_tunnel_reply::Result::Pending(_)) | None => {
            OperationReply { terminal: None }
        }
    }
}

/// `StopTunnelReply` → `OperationReply`。
fn operation_reply_from_stop(reply: &wire::StopTunnelReply) -> OperationReply {
    let terminal = reply.result.as_ref().map(|r| match r {
        wire::stop_tunnel_reply::Result::Stopped(receipt) => wire::OperationTerminal {
            result: Some(wire::operation_terminal::Result::Succeeded(receipt.clone())),
        },
        wire::stop_tunnel_reply::Result::Failed(failed) => wire::OperationTerminal {
            result: Some(wire::operation_terminal::Result::Failed(failed.clone())),
        },
    });
    OperationReply { terminal }
}

/// `OperationState`（Reconcile 观察到的义务 disposition）→ `OperationReply`。
///
/// 仅 `Terminal` 状态透传为终局；Pending/Unknown/Absent/Rejected 无终局 →
/// `terminal: None`（UI 视作操作仍在进行）。
fn operation_reply_from_operation_state(state: Option<OperationState>) -> OperationReply {
    let terminal = match state.and_then(|s| s.state) {
        Some(wire::operation_state::State::Terminal(t)) => Some(t),
        _ => None,
    };
    OperationReply { terminal }
}

// ---------------------------------------------------------------------------
// GetSnapshot：phase → 完整 wire snapshot（attempt/lease/proof 结构性组装）。
// ---------------------------------------------------------------------------

/// `HostPhase` → `RuntimeSnapshot` 的完整组装。
///
/// phase 对应的 state 分支携带 attempt/lease/proof 结构：身份 ref 为纯 composition 的
/// 确定性 stand-in（runtime epoch = 能力签发 epoch，packet lease = W23B 附加的 lease），
/// 真实 attempt/lease/proof 数据由 P3-c 从 engine 事件源化替换。`Stopped` 无对应
/// Kernel 状态分支（连接已终结）回落 `Idle`。
///
/// R1：`ConnectingState.phase` 用状态机登记的当前细粒度 ConnectPhase（不再硬编码）；
/// `Failed` → `FailedDirtyState`（携带最后一次失败的结构化 err，不静默）；快照携带
/// 当前在途操作 operation_id（R1 契约）。
/// 组装给定相位的确定性 wire 快照（相位 → 状态；engine 是状态权威时 `prefer_snapshot`
/// 优先 engine 快照）。
///
/// `pub(crate)`（2026-09-05 host 自愈进展计划 §4.3）：自愈阶段变化点（`shutdown.rs`
/// respawn 分支）在宿主进程内直接组装相位快照，供 `EventBus::publish_self_heal_refresh`
/// 显式发布——自愈窗口内没有自然事件，等下一次 publish 会把 UI 卡在旧快照上。
pub(crate) fn snapshot_for_phase(
    phase: HostPhase,
    composition: &HostComposition,
) -> RuntimeSnapshot {
    let state = match phase {
        HostPhase::Idle => Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
            last_cleanup: None,
        })),
        HostPhase::Connecting => Some(wire::runtime_snapshot::State::Connecting(
            wire::ConnectingState {
                attempt: Some(attempt_standin(composition)),
                phase: composition
                    .connect_phase()
                    .map(wire_connect_phase_from_domain)
                    .unwrap_or(wire::ConnectPhase::ObservingOwnedState)
                    as i32,
            },
        )),
        HostPhase::Connected => Some(wire::runtime_snapshot::State::Connected(
            wire::ConnectedState {
                session: Some(connected_session_standin(composition)),
                session_established_at_ms: composition.session_established_at_ms().unwrap_or(0),
            },
        )),
        HostPhase::Reconciling => Some(wire::runtime_snapshot::State::Reconciling(
            wire::ReconcilingState {
                context: Some(recovery_context_standin(composition)),
                obligation: Some(recovery_obligation_standin(composition)),
            },
        )),
        HostPhase::Stopping => Some(wire::runtime_snapshot::State::Stopping(
            wire::StoppingState {
                attempt: Some(attempt_standin(composition)),
                // host 不保留 stop 意图（P3-c 从 engine 事件源化）。
                stop: None,
                queued_connect: None,
            },
        )),
        // Stopped：连接已终结，无 FailedClean/Dirty 上下文 → 回落 Idle。
        HostPhase::Stopped => Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
            last_cleanup: None,
        })),
        // 登录前的本地依赖检查没有网络副作用，不伪造清理义务；后续阶段保留恢复语义。
        HostPhase::Failed if composition.last_wire_error().is_some_and(|error| {
            error.code == wire::ErrorCode::PlatformDependencyUnavailable as i32
                && error.stage == wire::ErrorStage::ObservingOwnedState as i32
                && error.certainty == wire::EffectCertainty::NoEffect as i32
        }) => Some(wire::runtime_snapshot::State::FailedClean(wire::FailedCleanState {
            last_error: composition.last_wire_error().cloned(),
            proof: None,
        })),
        HostPhase::Failed => Some(wire::runtime_snapshot::State::FailedDirty(
            wire::FailedDirtyState {
                last_error: composition.last_wire_error().cloned(),
                context: Some(recovery_context_standin(composition)),
                obligation: Some(recovery_obligation_standin(composition)),
            },
        )),
    };
    // stats / proxy_tun 由 `snapshot_with_stats` / `snapshot_with_proxy_tun` 在
    // 发布/GetSnapshot 时附加（本函数只组装状态）。S3：service_status/mode 由
    // `snapshot_with_service_context` 在 GetSnapshot/发布时附加（缺省 None/空）。
    RuntimeSnapshot {
        state,
        stats: None,
        proxy_tun: None,
        // EXV_UNFREEZE 2026-08-24：系统代理感知状态（设计 §5.5），由感知链路附加；占位 None。
        system_proxy: None,
        // EXV_UNFREEZE 2026-08-25：自动重连状态（C3a host 驱动），由状态转发器附加；占位 None。
        reconnect: None,
        // EXV_UNFREEZE 2026-09-05：host 自愈进展，由 EventBus lane 在发布/GetSnapshot
        // 时附加；占位 None。
        self_heal: None,
        // R1：当前在途操作 operation_id（空 = 无在途操作）。
        operation_id: composition
            .operation_id()
            .map(|id| id.to_vec())
            .unwrap_or_default(),
        service_status: None,
        mode: String::new(),
    }
}

/// 把已由 engine 状态事件确认的会话起点补入后续快照。
///
/// `ObserveOwnedState` 允许返回只含相位/统计的 Connected 快照；该类完整性较低的
/// 刷新不能把已经展示的在线时长清成横线。若 engine 这次提供了正值，保留它作为
/// 最新权威值；只有缺失或 0 时才回退到当前会话已确认的起点。
#[must_use]
fn snapshot_with_confirmed_session_start(
    mut snapshot: RuntimeSnapshot,
    confirmed_session_start: Option<i64>,
) -> RuntimeSnapshot {
    let Some(confirmed_session_start) = confirmed_session_start.filter(|value| *value > 0) else {
        return snapshot;
    };
    if let Some(wire::runtime_snapshot::State::Connected(connected)) = snapshot.state.as_mut()
        && connected.session_established_at_ms <= 0
    {
        connected.session_established_at_ms = confirmed_session_start;
    }
    snapshot
}

/// domain `ConnectPhase` → wire `ConnectPhase`（8 级一一对应；状态机登记为单一来源）。
fn wire_connect_phase_from_domain(
    phase: exv_vpn_domain::model::ConnectPhase,
) -> wire::ConnectPhase {
    use exv_vpn_domain::model::ConnectPhase as P;
    match phase {
        P::ObservingOwnedState => wire::ConnectPhase::ObservingOwnedState,
        P::AcquiringPlatformLease => wire::ConnectPhase::AcquiringPlatformLease,
        P::ConnectingControl => wire::ConnectPhase::ConnectingControl,
        P::AwaitingInteraction => wire::ConnectPhase::AwaitingInteraction,
        P::NegotiatingTunnel => wire::ConnectPhase::NegotiatingTunnel,
        P::ApplyingPlatformTunnel => wire::ConnectPhase::ApplyingPlatformTunnel,
        P::AttachingPacketBoundary => wire::ConnectPhase::AttachingPacketBoundary,
        P::StartingDataPlane => wire::ConnectPhase::StartingDataPlane,
    }
}

/// host `RuntimeStats` → wire `RuntimeStats`（P5-wire 方案 A：快照携带统计）。
///
/// 字段一一对应；`phase` 是 wire `StatsPhase` i32（host `RuntimeStats.phase` 同源，
/// 直接 `as i32`，与 `stats_phase_from_i32` 互为往返）。
#[must_use]
fn runtime_stats_to_wire(stats: RuntimeStats) -> wire::RuntimeStats {
    wire::RuntimeStats {
        rx_bytes: stats.rx_bytes,
        tx_bytes: stats.tx_bytes,
        rx_rate_bps: stats.rx_rate_bps,
        tx_rate_bps: stats.tx_rate_bps,
        latency_ms: stats.latency_ms,
        phase: stats.phase as i32,
        engine_sequence: stats.engine_sequence,
        sample_tick: stats.sample_tick,
    }
}

/// 把最新归一化统计附加到 wire 快照（None → `stats` 留空）。
///
/// 统计是状态事件的伴生数据：`GetSnapshot` 与 `EventBus::publish`（`WatchEvents`
/// 事件）都从 `EventBus::current_stats` 读最新样本后经本函数附加。
#[must_use]
fn snapshot_with_stats(
    mut snapshot: RuntimeSnapshot,
    stats: Option<RuntimeStats>,
) -> RuntimeSnapshot {
    snapshot.stats = stats.map(runtime_stats_to_wire);
    snapshot
}

/// resource `ProxyTunDetection` → wire `ProxyTunDetection`（C5-wire；字段一一对应）。
///
/// `route_policy` 取 [`ProxyTunDetection::route_policy`]（`"exv-before-proxy-tun"` |
/// `"normal"`，对齐 C++ `to_json`）；`kind` 为 `&'static str`（始终 `"proxy_tun"`）。
#[must_use]
fn proxy_tun_detection_to_wire(detection: &ProxyTunDetection) -> wire::ProxyTunDetection {
    wire::ProxyTunDetection {
        detected: detection.detected,
        adapters: detection
            .adapters
            .iter()
            .map(|a| wire::ProxyTunAdapter {
                name: a.name.clone(),
                description: a.description.clone(),
                if_index: a.if_index,
                kind: a.kind.to_string(),
            })
            .collect(),
        route_policy: detection.route_policy().to_string(),
    }
}

/// 把上游 proxy TUN 检测状态附加到 wire 快照（None → `proxy_tun` 留空）。
///
/// C5-wire：检测是状态事件的伴生数据（mirror stats 方案 A）——`GetSnapshot` 与
/// `EventBus::publish`（`WatchEvents` 事件）都从缓存读最新检测后经本函数附加；
/// 仅状态上报，不改任何路由/行为（PRD O1）。
#[must_use]
fn snapshot_with_proxy_tun(
    mut snapshot: RuntimeSnapshot,
    detection: Option<wire::ProxyTunDetection>,
) -> RuntimeSnapshot {
    snapshot.proxy_tun = detection;
    snapshot
}

/// resource `SystemProxySnapshot` → wire `SystemProxyDetection`（EXV_UNFREEZE；
/// 字段一一对应 + 四态拓扑分类）。
///
/// `mode` 取 [`SystemProxyMode`] 的稳定字符串（`"disabled"` | `"manual"` |
/// `"automatic"` | `"mixed"`，对齐 proto 注释）；`endpoint_count` = 规范化端点数；
/// `bypass_merged` 从 [`SystemProxySnapshot::bypass_entries`] 判定（非空 = EXV 豁免已
/// 合并进系统代理设置，设计 §5.5）；`topology` 是 `(proxy_present, tunnel_present)`
/// 的纯函数分类（[`classify`]，设计 §2）——`proxy_present` 取
/// [`SystemProxySnapshot::is_present`]，`tunnel_present` 由调用方（探测链）给出。
#[must_use]
fn system_proxy_snapshot_to_wire(
    snapshot: &SystemProxySnapshot,
    tunnel_present: bool,
) -> wire::SystemProxyDetection {
    wire::SystemProxyDetection {
        mode: match snapshot.mode {
            SystemProxyMode::Disabled => "disabled",
            SystemProxyMode::Manual => "manual",
            SystemProxyMode::Automatic => "automatic",
            SystemProxyMode::Mixed => "mixed",
        }
        .to_string(),
        endpoint_count: snapshot.endpoints.len() as u32,
        bypass_merged: !snapshot.bypass_entries.is_empty(),
        topology: match system_proxy::classify(snapshot.is_present(), tunnel_present) {
            TopologyKind::T0 => "t0",
            TopologyKind::T1 => "t1",
            TopologyKind::T2 => "t2",
            TopologyKind::T3 => "t3",
        }
        .to_string(),
    }
}

/// 把系统代理检测状态附加到 wire 快照（None → `system_proxy` 留空）。
///
/// EXV_UNFREEZE：检测是状态事件的伴生数据（mirror proxy_tun 方案 A）——`GetSnapshot`
/// 与 `EventBus::publish`（`WatchEvents` 事件）都从缓存读最新检测后经本函数附加；
/// 仅状态上报，不改任何路由/行为。
#[must_use]
fn snapshot_with_system_proxy(
    mut snapshot: RuntimeSnapshot,
    detection: Option<wire::SystemProxyDetection>,
) -> RuntimeSnapshot {
    snapshot.system_proxy = detection;
    snapshot
}

/// 组装 wire `ReconnectStatus`（C4 wire unfreeze；纯字段映射，不读配置）。
///
/// `attempts`/`active` 取 host 侧 `reconnect_attempts`/`reconnect_active` 原子（C3a 状态
/// 转发器维护）；`auto_reconnect`/`max_attempts` 由调用方从配置读（0 = unlimited）。
#[must_use]
fn reconnect_status_from_state(
    attempts: u32,
    active: bool,
    auto_reconnect: bool,
    max_attempts: u32,
) -> wire::ReconnectStatus {
    wire::ReconnectStatus {
        auto_reconnect,
        max_attempts,
        current_attempt: attempts,
        active,
    }
}

/// 将当前会话冻结策略与原子重连进展组合为运行时快照；没有会话则保持为空。
fn reconnect_status_from_session(
    attempts: &AtomicU32,
    active: &AtomicBool,
    session_policy: &std::sync::Mutex<Option<ReconnectSessionPolicy>>,
) -> Option<wire::ReconnectStatus> {
    session_policy.lock().ok().and_then(|policy| {
        (*policy).map(|policy| {
            reconnect_status_from_state(
                attempts.load(Ordering::Relaxed),
                active.load(Ordering::Acquire),
                policy.auto_reconnect,
                policy.max_attempts,
            )
        })
    })
}

/// 把自动重连状态附加到 wire 快照（None → `reconnect` 留空）。
///
/// EXV_UNFREEZE：重连状态是状态事件的伴生数据（mirror proxy_tun 方案 A）——
/// `GetSnapshot` 与状态转发器在发布前组装后经本函数附加；仅状态上报，不改任何
/// 重连行为（决策仍在 C3a 重连 worker）。
#[must_use]
fn snapshot_with_reconnect(
    mut snapshot: RuntimeSnapshot,
    reconnect: Option<wire::ReconnectStatus>,
) -> RuntimeSnapshot {
    snapshot.reconnect = reconnect;
    snapshot
}

/// 把 host 自愈（engine 崩溃 respawn）进展附加到 wire 快照（None → `self_heal` 留空）。
///
/// EXV_UNFREEZE 2026-09-05（计划 §4.3）：自愈阶段是状态事件的伴生数据（mirror
/// proxy_tun 方案 A）——`GetSnapshot` 与 `EventBus::publish` 都从缓存读最新阶段后经
/// 本函数附加；仅状态上报，不改任何 respawn 编排/判定行为（respawn 语义留在
/// `crash_recovery`，wire 不携带凭据/栈）。
#[must_use]
pub(crate) fn snapshot_with_self_heal(
    mut snapshot: RuntimeSnapshot,
    status: Option<wire::SelfHealStatus>,
) -> RuntimeSnapshot {
    snapshot.self_heal = status;
    snapshot
}

/// 组装 wire `SelfHealStatus`（EXV_UNFREEZE 2026-09-05；纯字段映射）。
///
/// `stage` 为 [`crate::crash_recovery::SelfHealStage`] 的冻结字符串；`new_pid` 仅
/// succeeded 非零；`error_code` 仅 failed 非空（`respawn_error_code` 码表）。
#[must_use]
pub(crate) fn self_heal_status_wire(
    stage: crate::crash_recovery::SelfHealStage,
    old_pid: u32,
    new_pid: u32,
    error_code: &str,
) -> wire::SelfHealStatus {
    wire::SelfHealStatus {
        stage: stage.as_str().to_string(),
        old_pid,
        new_pid,
        error_code: error_code.to_string(),
    }
}

/// C5（ui-connect-stop-responsiveness）：`auto_reconnect` 配置读缓存的 TTL（冻结值
/// 5s）。覆盖连接期事件突发：一次连接
/// 生命周期内转发器发布路径的磁盘读从每事件一次降为 ≤1 次/5s；外部进程改配置的
/// 最长陈旧窗口 5s（可接受，C5 冻结裁决）。
const RECONNECT_CONFIG_CACHE_TTL: Duration = Duration::from_secs(5);

/// 一次用户连接受理时冻结的重连策略；自动重连尝试复用同一份值。
#[derive(Clone, Copy)]
struct ReconnectSessionPolicy {
    auto_reconnect: bool,
    max_attempts: u32,
    backoff: bool,
    connection_mode: ConnectionMode,
}

impl ReconnectSessionPolicy {
    fn from_config(config: &ExvConfig) -> Self {
        Self {
            auto_reconnect: config.auto_reconnect,
            max_attempts: config.auto_reconnect_max_attempts,
            backoff: config.auto_reconnect_backoff,
            connection_mode: config.connection_mode,
        }
    }

    const fn disabled() -> Self {
        Self {
            auto_reconnect: false,
            max_attempts: 0,
            backoff: false,
            connection_mode: ConnectionMode::Standard,
        }
    }
}

/// C5 缓存条目：上次磁盘读的取值 + 读取时刻。
struct ReconnectConfigEntry {
    auto_reconnect: bool,
    max_attempts: u32,
    /// reconnect-backoff（2026-09-05）：退避开关随缓存携带（drive_reconnect 与
    /// `reconnect_status_cached` 同一磁盘读共用）。
    backoff: bool,
    fetched_at: Instant,
}

/// C5（ui-connect-stop-responsiveness）：`auto_reconnect` 配置读缓存。
///
/// 归 [`KernelControlService`] 所有（`Arc` 共享给状态转发器任务）；三个读点共用：
/// 状态转发器发布、GetSnapshot、`drive_reconnect`。锁为 `std::sync::Mutex` 短临界区
/// （不跨 await 持锁）。失败结果**不缓存**（无负缓存——磁盘恢复后下一次读重试）。
/// 主动失效仅两处，均为配置成功落盘后：`config_set` 的 `save_after_user_submission`
/// 与 `persist_ui_credentials` 的 `save_after_user_submission`（后者不改
/// auto_reconnect 键，但整文件重写，失效廉价且无脑正确）。
struct ReconnectConfigCache {
    entry: std::sync::Mutex<Option<ReconnectConfigEntry>>,
}

impl ReconnectConfigCache {
    fn new() -> Self {
        Self {
            entry: std::sync::Mutex::new(None),
        }
    }

    /// TTL 内命中 → `Some((auto_reconnect, max_attempts, backoff))`；过期/为空 → `None`。
    ///
    /// 只窥视缓存，**绝不读磁盘**——受理快照发布路径（C1 性能约束：零磁盘）用它
    /// 附加缓存版 reconnect 伴生值，未命中一律 `None`。
    fn fresh(&self) -> Option<(bool, u32, bool)> {
        let guard = self.entry.lock().ok()?;
        let entry = guard.as_ref()?;
        (entry.fetched_at.elapsed() < RECONNECT_CONFIG_CACHE_TTL).then_some((
            entry.auto_reconnect,
            entry.max_attempts,
            entry.backoff,
        ))
    }

    /// 回填缓存（磁盘读成功后调用）。
    fn store(&self, auto_reconnect: bool, max_attempts: u32, backoff: bool) {
        if let Ok(mut guard) = self.entry.lock() {
            *guard = Some(ReconnectConfigEntry {
                auto_reconnect,
                max_attempts,
                backoff,
                fetched_at: Instant::now(),
            });
        }
    }

    /// 主动失效（清空缓存条目）：配置成功落盘后调用。
    fn invalidate(&self) {
        if let Ok(mut guard) = self.entry.lock() {
            *guard = None;
        }
    }
}

/// C5 缓存版：从缓存或磁盘组装 wire `ReconnectStatus`（TTL 5s）。
///
/// TTL 内命中缓存 → 零磁盘读；否则读磁盘并回填。读失败 → 保守 `(false, 0)` 且
/// **不缓存失败结果**（下次读重试磁盘，无负缓存）——与既有保守禁用语义一致。
/// 三个读点共用：状态转发器发布（含断线过渡发布）、`get_snapshot`、以及
/// `drive_reconnect` 的重连判定。
///
/// C5 起取代直读版 `reconnect_status_from_config`（后者已删除——转发器/GetSnapshot/
/// drive_reconnect 全部走本函数；受理快照走 `ReconnectConfigCache::fresh` 窥视）。
#[must_use]
fn reconnect_status_cached(
    attempts: &AtomicU32,
    active: &AtomicBool,
    config_dir: &Path,
    cache: &ReconnectConfigCache,
) -> wire::ReconnectStatus {
    let (auto, max, _backoff) = match cache.fresh() {
        Some(hit) => hit,
        None => match ExvConfig::load_from_dir(config_dir) {
            Ok(cfg) => {
                cache.store(
                    cfg.auto_reconnect,
                    cfg.auto_reconnect_max_attempts,
                    cfg.auto_reconnect_backoff,
                );
                (
                    cfg.auto_reconnect,
                    cfg.auto_reconnect_max_attempts,
                    cfg.auto_reconnect_backoff,
                )
            }
            // 配置不可读 → 保守禁用（false/0/关闭退避），不缓存失败结果。
            Err(_) => (false, 0, false),
        },
    };
    reconnect_status_from_state(
        attempts.load(Ordering::Relaxed),
        active.load(Ordering::Acquire),
        auto,
        max,
    )
}

/// host `ServiceStatusSnapshot` → wire `ServiceStatus`（S3/D5 字段一一映射 + R3
/// `health_state` 派生结果）。
///
/// `state` 取 [`ServiceState`] 的稳定字符串（`"stopped"` / `"start_pending"` /
/// `"stop_pending"` / `"running"` / `"other"`）——UI 据此展示服务徽标。`health_state`
/// 取 [`HealthState::as_wire_str`] 的稳定字符串（5 态）。
#[must_use]
fn service_status_to_wire(
    snapshot: &ServiceStatusSnapshot,
    health: HealthState,
) -> wire::ServiceStatus {
    wire::ServiceStatus {
        installed: snapshot.state.is_installed(),
        // `as_wire_str` 覆盖全部状态（含新增 Paused 系）；`NotInstalled` 由 `installed=false`
        // 表达（state 占位 "stopped"，与既有 wire 行为一致）。
        state: snapshot.state.as_wire_str().to_string(),
        binary_path: snapshot.binary_path.clone().unwrap_or_default(),
        health_state: health.as_wire_str().to_string(),
    }
}

/// 把服务上下文（service_status + mode）附加到 wire 快照（S3/D5）。
///
/// `service_status` None → 留空（尚未查询/查询失败）；`mode` 缺省 `"auto"`（与解冻前
/// 展示语义一致——未发生连接路由时展示默认模式）。
#[must_use]
fn snapshot_with_service_context(
    mut snapshot: RuntimeSnapshot,
    service_status: Option<wire::ServiceStatus>,
    mode: Option<String>,
) -> RuntimeSnapshot {
    snapshot.service_status = service_status;
    snapshot.mode = mode.unwrap_or_else(|| ServiceMode::Auto.as_wire_str().to_string());
    snapshot
}

/// C5-wire：探测一次上游 proxy TUN 并把 wire 结果刷新进事件总线缓存。
///
/// 探测失败 → 刷新 `None`（快照 `proxy_tun` 留空，不因探测失败影响状态上报）。
/// `GetSnapshot` 与 engine 事件转发器在发布前调用，保证快照携带最新共存状态。
fn refresh_bus_proxy_tun(events: &EventBus, probe: &ProxyTunProbe) {
    let detection = probe().ok().map(|d| proxy_tun_detection_to_wire(&d));
    events.refresh_proxy_tun(detection);
}

/// EXV_UNFREEZE：探测一次系统代理并把 wire 结果刷新进事件总线缓存。
///
/// 探测失败（SID 不可解析 / 注册表不可读 / 快照自洽性失败）→ 刷新 `None`（快照
/// `system_proxy` 留空，状态上报不因探测失败而失败）。`GetSnapshot` 与 engine 事件
/// 转发器在发布前调用，保证快照携带最新系统代理感知状态（mirror proxy_tun 方案 A）。
fn refresh_bus_system_proxy(events: &EventBus, probe: &SystemProxyProbe) {
    let detection = probe().ok();
    events.refresh_system_proxy(detection);
}

/// 确定性 attempt stand-in（纯 composition 的一次性连接尝试身份：runtime epoch +
/// 固定 attempt id；真实 attempt 由 P3-c 从 engine 事件源化）。
fn attempt_standin(composition: &HostComposition) -> wire::Attempt {
    wire::Attempt {
        runtime_epoch: composition.runtime_epoch_bytes().to_vec(),
        attempt_id: deterministic_attempt_id().to_vec(),
        intent: None,
        prior_error: None,
    }
}

/// 确定性 attempt id 字节（`Uuid::from_u128(2)`，非 nil、与 epoch=1 区分）。
fn deterministic_attempt_id() -> [u8; 16] {
    *Uuid::from_u128(2).as_bytes()
}

/// 确定性 32 字节 digest stand-in（`tag` 首字节 + 零填充；snapshot 的身份 ref）。
fn digest_standin(tag: u8) -> [u8; 32] {
    let mut d = [0u8; 32];
    d[0] = tag;
    d
}

/// Connected 会话的完整组装：attempt + protocol_session/platform_ownership/packet_lease
/// refs + platform_ready/data_running proofs（refs 为确定性 stand-in）。
fn connected_session_standin(composition: &HostComposition) -> wire::ConnectedSession {
    let packet_lease = wire::PacketLeaseRef {
        identity_digest: composition.packet_lease_ref_bytes().to_vec(),
    };
    let protocol_session = wire::ProtocolSessionRef {
        identity_digest: digest_standin(b'P').to_vec(),
    };
    let platform_ownership = wire::PlatformOwnershipRef {
        identity_digest: digest_standin(b'O').to_vec(),
        ownership_version: 1,
        token_digest: digest_standin(b'T').to_vec(),
    };
    wire::ConnectedSession {
        attempt: Some(attempt_standin(composition)),
        protocol_session: Some(protocol_session.clone()),
        platform_ownership: Some(platform_ownership.clone()),
        packet_lease: Some(packet_lease.clone()),
        platform_ready: Some(wire::PlatformReadyProof {
            platform_ownership: Some(platform_ownership.clone()),
            evidence_digest: digest_standin(b'E').to_vec(),
        }),
        data_running: Some(wire::DataRunningProof {
            protocol_session: Some(protocol_session),
            platform_ownership: Some(platform_ownership),
            packet_lease: Some(packet_lease),
            evidence_digest: digest_standin(b'D').to_vec(),
        }),
    }
}

/// Reconciling 的 RecoveryContext stand-in：helper link lost ≈ 包边界丢失
/// （packet_boundary_lost，携带当前 attempt + packet lease）。
fn recovery_context_standin(composition: &HostComposition) -> wire::RecoveryContext {
    wire::RecoveryContext {
        context: Some(wire::recovery_context::Context::PacketBoundaryLost(
            wire::PacketBoundaryLostContext {
                attempt: Some(attempt_standin(composition)),
                packet_lease: Some(wire::PacketLeaseRef {
                    identity_digest: composition.packet_lease_ref_bytes().to_vec(),
                }),
            },
        )),
    }
}

/// Reconciling 的 RecoveryObligation stand-in（阻塞原因未知 → `blocking_error: None`）。
fn recovery_obligation_standin(composition: &HostComposition) -> wire::RecoveryObligation {
    wire::RecoveryObligation {
        owner_runtime_epoch: composition.runtime_epoch_bytes().to_vec(),
        // `bytes` 字段：空 = 无（proto 注释 optional）。
        retirement_operation_id: vec![],
        blocking_error: None,
        platform_ownership: Some(wire::PlatformOwnershipRef {
            identity_digest: digest_standin(b'O').to_vec(),
            ownership_version: 1,
            token_digest: digest_standin(b'T').to_vec(),
        }),
        packet_lease: Some(wire::PacketLeaseRef {
            identity_digest: composition.packet_lease_ref_bytes().to_vec(),
        }),
        canonical_inventory_digest: digest_standin(b'I').to_vec(),
    }
}

// ---------------------------------------------------------------------------
// EventBus：WatchEvents 的 host 侧订阅总线（P3-c1 真实订阅）
// ---------------------------------------------------------------------------

/// engine 事件转发器的初始重连退避。
const ENGINE_EVENT_RECONNECT_BASE: Duration = Duration::from_millis(100);
/// engine 事件转发器的重连退避上限。
const ENGINE_EVENT_RECONNECT_MAX: Duration = Duration::from_secs(5);
/// engine 日志转发器的初始重连退避（R3；聚合-only 消费，断线缺口由 raw 离线对账）。
const ENGINE_LOG_RECONNECT_BASE: Duration = Duration::from_millis(100);
/// engine 日志转发器的重连退避上限。
const ENGINE_LOG_RECONNECT_MAX: Duration = Duration::from_secs(5);
/// `await_status_ready` 的有界等待上限（R3-C1）：引擎状态流永久不可达时，connect/stop
/// 写路径在此上界后放行派发，由 ApplyTunnel/StopTunnel 报真实 gRPC 错误——不永久悬挂。
/// 正常路径下转发器首次挂接在毫秒级完成，此上界充分覆盖。
const STATUS_READY_WAIT: Duration = Duration::from_millis(2000);

// ---------------------------------------------------------------------------
// reconnect-backoff（2026-09-05）：C3a 自动重连指数退避（auto_reconnect_backoff
// 开启时的选项，默认关闭=立即重连现状）。
// ---------------------------------------------------------------------------

/// 数据面自动重连退避 base（计划冻结 2s——首次掉线后 sleep 的延时）。产品路径使用；
/// 测试经 [`KernelControlService::with_reconnect_backoff`] 注入短时长（不改默认）。
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_secs(2);
/// 数据面自动重连退避 cap（计划冻结 30s——用户明确 60s 太长）。同上 seam。
const RECONNECT_BACKOFF_CAP: Duration = Duration::from_secs(30);

/// 第 `attempt` 次自动重连派发前的退避延时（指数退避，几何翻倍封顶）。
///
/// 返回值 = `min(base * 2^(attempt-1), cap)`；`attempt` 从 1 计（既有
/// `reconnect_attempts` 语义：每次实际派发 +1、Connected 成功后清零——清零即让下一次
/// 掉线回到 base，成功自动重置退避序列）。产品默认 base=2s/cap=30s → 序列
/// 2s→4s→8s→16s→30s→30s…；测试经 seam 注入毫秒级 base/cap（生产路径不改默认）。
/// `attempt` 越界（0 / 超大）由 `saturating_sub`/封顶收敛到确定值，不循环、零开销。
#[must_use]
fn reconnect_backoff_delay(base: Duration, cap: Duration, attempt: u32) -> Duration {
    let base_ns = base.as_nanos();
    let cap_ns = cap.as_nanos();
    debug_assert!(base_ns > 0, "reconnect backoff base must be non-zero");
    // k = 从 base 翻倍多少次到达/超过 cap（此后恒 cap）；封顶 63 防 u128 溢出。
    let mut doublings: u32 = 0;
    let mut growth = base_ns;
    while growth < cap_ns && doublings < 63 {
        growth <<= 1;
        doublings += 1;
    }
    let shift = attempt.saturating_sub(1).min(doublings);
    let delay_ns = (base_ns << shift).min(cap_ns);
    Duration::from_nanos(u64::try_from(delay_ns).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------
// 统计转发器诊断（RT-DIAG-03）：kernel.stats.* 冻结码表 + 60s 节流 + 进程内状态机。
// 诊断只进 LogAggregator（source="core"、component="kernel"，经既有 logs.list 契约
// 到 UI 日志页）与 [`EventBus::stats_diagnostic`] 进程内观测口，**不进
// RuntimeSnapshot**（wire frozen；proto 零改动）。fields 只允许冻结白名单键，值只
// 允许稳定枚举字符串与十进制整数；不记录地址/账号/SID/operation id/payload/原始
// 错误字符串/流量内容。
// ---------------------------------------------------------------------------

/// 统计转发器默认首样本期限（码 6 `first_sample_timeout` 触发预算；对齐上游修订节
/// "首样本期限"默认 3000ms；测试经 [`KernelControlService::with_stats_first_sample_timeout`]
/// 注入短时长，生产路径不改默认）。
const STATS_FIRST_SAMPLE_TIMEOUT: Duration = Duration::from_millis(3000);
/// 同码诊断落盘的最小间隔（§4.1 冻结节流窗口：60s）。节流期内被抑制的次数只计入
/// 诊断对象内存计数（`suppressed_since_last`），不落盘、不新增 fields。
const STATS_DIAG_THROTTLE: Duration = Duration::from_secs(60);

/// 统计转发器状态机（§4.1 冻结；host 内部类型，非 wire）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsForwarderState {
    /// 已构造、尚未完成首次订阅。
    Starting,
    /// 订阅成功（含每次重订阅），尚未收到样本。
    Subscribed,
    /// 本订阅生命周期内已收到首条样本。
    Receiving,
    /// 订阅失败/流中断，退避重试中。
    Retrying,
}

/// 统计转发器最近一次错误类别（§4.1 冻结；无 payload——类别细节走日志 fields）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsForwarderError {
    /// `stream_stats` 订阅调用返回 `Err`。
    SubscribeFailed,
    /// 统计流中途收到传输错误项。
    StreamError,
    /// 统计流干净 EOF。
    StreamEnded,
    /// 订阅成功后首样本期限内无任何样本。
    FirstSampleTimeout,
}

/// 统计转发器诊断对象（host 内部，非 wire；`kernel.session.adopted` 不属于转发器
/// 状态机，不进本对象）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsForwarderDiagnostic {
    /// 当前状态机状态。
    pub state: StatsForwarderState,
    /// 订阅尝试计数（语义冻结：per 转发器生命周期从 1 计——同一转发器任务内每次
    /// 订阅尝试 +1，含 EOF/stream_error 后退避重订与 slot swap 立即重订；不跨转发器
    /// 生命周期累计、不随 engine 代重置）。
    pub attempt: u32,
    /// 最近一次错误（无错误 → `None`）。
    pub last_error: Option<StatsForwarderError>,
    /// 最近一样本的 phase（归一化后）。
    pub last_sample_phase: Option<wire::StatsPhase>,
    /// 最近一样本的 engine 序号。
    pub last_sample_sequence: Option<u64>,
    /// 最近一次状态转换时刻（wall-clock epoch ms；供节流与 age 计算）。
    pub last_transition_ms: Option<i64>,
    /// 节流抑制计数（仅内存；自上次同码落盘以来的被抑制次数）。
    pub suppressed_since_last: u32,
}

impl StatsForwarderDiagnostic {
    /// 初始诊断对象（`Starting`、attempt 0、无错误无样本）。
    #[must_use]
    fn starting() -> Self {
        Self {
            state: StatsForwarderState::Starting,
            attempt: 0,
            last_error: None,
            last_sample_phase: None,
            last_sample_sequence: None,
            last_transition_ms: None,
            suppressed_since_last: 0,
        }
    }
}

/// `kernel.stats.*` / `kernel.session.adopted` 冻结诊断码（§4.1 码表 v1；执行时不得
/// 增删码、不得改字段白名单）。本枚举是码字符串/级别/固定文案的唯一落点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatsDiagCode {
    /// 码 1：`stream_stats` 订阅成功（含每次重订阅）。
    Subscribed,
    /// 码 2：一次订阅生命周期内收到第一条样本。
    FirstSample,
    /// 码 3：`stream_stats` 调用返回 `Err`。
    SubscribeFailed,
    /// 码 4：统计流中途收到传输错误项。
    StreamError,
    /// 码 5：统计流干净 EOF。
    StreamEnded,
    /// 码 6：订阅成功后首样本期限内无任何样本。
    FirstSampleTimeout,
    /// 码 7：engine slot 换点触发中断当前流重订阅。
    SlotSwapped,
    /// 码 8：样本 phase 判别值 ∉ 1..=5。
    PhaseUnspecified,
    /// 码 9：样本时间戳回退。
    TimestampRegression,
    /// 码 10：同一订阅流内累计计数回退。
    CounterRegression,
    /// 码 11：engine `Connected` 事件未携带正值会话起点，core 以本地时钟采纳。
    SessionAdopted,
}

impl StatsDiagCode {
    /// 冻结码字符串（logs.list `code` 字段唯一取值来源）。
    fn as_str(self) -> &'static str {
        match self {
            Self::Subscribed => "kernel.stats.subscribed",
            Self::FirstSample => "kernel.stats.first_sample",
            Self::SubscribeFailed => "kernel.stats.subscribe_failed",
            Self::StreamError => "kernel.stats.stream_error",
            Self::StreamEnded => "kernel.stats.stream_ended",
            Self::FirstSampleTimeout => "kernel.stats.first_sample_timeout",
            Self::SlotSwapped => "kernel.stats.slot_swapped",
            Self::PhaseUnspecified => "kernel.stats.phase_unspecified",
            Self::TimestampRegression => "kernel.stats.timestamp_regression",
            Self::CounterRegression => "kernel.stats.counter_regression",
            Self::SessionAdopted => "kernel.session.adopted",
        }
    }

    /// 冻结级别（info/warn）。
    fn level(self) -> &'static str {
        match self {
            Self::Subscribed
            | Self::FirstSample
            | Self::SlotSwapped
            | Self::TimestampRegression
            | Self::SessionAdopted => "info",
            Self::SubscribeFailed
            | Self::StreamError
            | Self::StreamEnded
            | Self::FirstSampleTimeout
            | Self::PhaseUnspecified
            | Self::CounterRegression => "warn",
        }
    }

    /// 冻结中文短句（允许标点差异，不拼接自由文本）。
    fn message(self) -> &'static str {
        match self {
            Self::Subscribed => "统计流已订阅",
            Self::FirstSample => "收到首条统计样本",
            Self::SubscribeFailed => "统计订阅失败，将重试",
            Self::StreamError => "统计流出错，将重试",
            Self::StreamEnded => "统计流结束，将重连",
            Self::FirstSampleTimeout => "首条统计样本超时",
            Self::SlotSwapped => "引擎已切换，重新订阅统计",
            Self::PhaseUnspecified => "统计样本阶段未知",
            Self::TimestampRegression => "统计样本时间戳回退，速率按 0 处理",
            Self::CounterRegression => "统计样本累计计数回退，速率按 0 处理",
            Self::SessionAdopted => "会话起点由本机记录",
        }
    }
}

/// 码 8/9/10 的样本级节流 lane 下标。
const DIAG_LANE_PHASE: usize = 0;
const DIAG_LANE_TIMESTAMP: usize = 1;
const DIAG_LANE_COUNTER: usize = 2;

/// 60s 同码节流器（冻结节流规则的转发器内状态）。
///
/// 码 3/4/5 重试族：连续重复仅在状态变化（Retrying→Subscribed→Receiving，
/// `reset_retry_lane`）、或距上次同码（同类别）落盘 ≥ 60s 时落盘；"退避达上限后进入
/// 60s 周期时的首次"由 ≥60s 判定自然承载。码 8/9/10 按样本级触发、各自独立 60s lane。
/// 时钟用 `tokio::time::Instant`（测试 `start_paused`/`advance` 可注入）。
struct StatsDiagThrottle {
    /// 重试族 lane：`(码, 错误类别)` 为同码同类别键；码 5 无类别（`None`）。
    retry_lane: Option<(
        (StatsDiagCode, Option<StatsErrorKind>),
        tokio::time::Instant,
    )>,
    /// 样本级 lanes（码 8/9/10）。
    sample_lanes: [Option<tokio::time::Instant>; 3],
}

impl StatsDiagThrottle {
    fn new() -> Self {
        Self {
            retry_lane: None,
            sample_lanes: [None; 3],
        }
    }

    /// 状态变化复位（码 3/4/5 连续重复判定的断点）。
    fn reset_retry_lane(&mut self) {
        self.retry_lane = None;
    }

    /// 重试族同码同类别 60s 节流判定（返回 `true` = 本次应落盘）。
    fn allow_retry(&mut self, code: StatsDiagCode, kind: Option<StatsErrorKind>) -> bool {
        let now = tokio::time::Instant::now();
        if let Some(((last_code, last_kind), at)) = self.retry_lane {
            if last_code == code
                && last_kind == kind
                && now.duration_since(at) < STATS_DIAG_THROTTLE
            {
                return false;
            }
        }
        self.retry_lane = Some(((code, kind), now));
        true
    }

    /// 样本级同码 60s 节流判定（返回 `true` = 本次应落盘）。
    fn allow_sample(&mut self, lane: usize) -> bool {
        let now = tokio::time::Instant::now();
        if let Some(at) = self.sample_lanes[lane] {
            if now.duration_since(at) < STATS_DIAG_THROTTLE {
                return false;
            }
        }
        self.sample_lanes[lane] = Some(now);
        true
    }
}

/// 归一化 `StatsPhase` → 冻结枚举字符串（码 2 `phase` fields 唯一取值来源）。
#[must_use]
fn stats_phase_diag_str(phase: wire::StatsPhase) -> &'static str {
    match phase {
        wire::StatsPhase::Unspecified => "unspecified",
        wire::StatsPhase::Idle => "idle",
        wire::StatsPhase::Connecting => "connecting",
        wire::StatsPhase::Connected => "connected",
        wire::StatsPhase::Stopping => "stopping",
        wire::StatsPhase::Failed => "failed",
    }
}

/// 当前 wall-clock epoch 毫秒（诊断 `last_transition_ms` 用；时钟不可得保守 0）。
fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// 落一条冻结诊断码到日志聚合器（source="core"、component="kernel"；白名单 fields
/// 由调用点以固定键值对构造，值只能是稳定枚举字符串或十进制整数）。写失败被忽略
/// （日志是聚合落盘，不回流状态、不阻断转发）。
fn emit_stats_diag(
    logs: &LogAggregator,
    code: StatsDiagCode,
    fields: impl IntoIterator<Item = (&'static str, String)>,
) {
    let fields: BTreeMap<String, String> = fields
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    logs.append_core(
        code.level(),
        "kernel",
        code.as_str(),
        code.message(),
        &fields,
    )
    .ok();
}

/// `WatchEvents` 的 host 侧事件总线（P3-c1 真实订阅；W3-1/P1 S1b wrapper 化）。
///
/// 核心总线事实（严格递增 monotonic tick、最新事件重放、多订阅者 fan-out、统计 lane
/// 的 `publish_stats`/`subscribe_stats` 语义）已上提 common
/// `exv_vpn_host::event_bus::EventBus`（win32 wrapper 与 darwin core 两侧同时消费，
/// 终结 host crate R1 单宿主豁免）。本类型是 win32 wrapper：保留 win32 特有 lane
/// 缓存（proxy_tun/system_proxy/service_status/service_mode/self_heal/stats 诊断）与
/// `publish` 的 lane 装饰链——装饰后的快照再委托核心发布；公共 API 签名不变。
pub struct EventBus {
    /// common 核心总线（W3-1/P1 S1a 上提；持 tick/current/live/stats lane）。
    core: exv_vpn_host::event_bus::EventBus,
    /// C5-wire 最新上游 proxy TUN 检测（`refresh_proxy_tun` 更新；`publish`/`GetSnapshot`
    /// 附加到快照——mirror stats 方案 A，检测是状态事件的伴生数据）。
    proxy_tun_current: std::sync::Mutex<Option<wire::ProxyTunDetection>>,
    /// EXV_UNFREEZE 最新系统代理检测（`refresh_system_proxy` 更新；`publish`/`GetSnapshot`
    /// 附加到快照——mirror proxy_tun 方案 A，检测是状态事件的伴生数据）。
    system_proxy_current: std::sync::Mutex<Option<wire::SystemProxyDetection>>,
    /// S3/D5 最新 SCM 服务状态（`refresh_service_status` 更新；`publish`/`GetSnapshot`
    /// 附加到快照——mirror proxy_tun 方案 A，状态上报不因查询失败而失败）。
    service_status_current: std::sync::Mutex<Option<wire::ServiceStatus>>,
    /// S3/D3 最新展示模式（`refresh_service_mode` 更新；`publish`/`GetSnapshot` 附加；
    /// 缺省 `None` → `"auto"`）。
    service_mode_current: std::sync::Mutex<Option<String>>,
    /// RT-DIAG-03 统计转发器诊断状态（进程内可查；诊断不进 `RuntimeSnapshot`）。
    stats_diagnostic: std::sync::Mutex<StatsForwarderDiagnostic>,
    /// 2026-09-05 host 自愈进展（EXV_UNFREEZE wire `self_heal = 16`；计划 §4.3）：
    /// 最新自愈阶段缓存（`refresh_self_heal` 更新；`publish`/`GetSnapshot` 附加——
    /// mirror proxy_tun 方案 A，自愈窗口内没有自然事件，拉取通道每次组装读当前缓存）。
    self_heal_current: std::sync::Mutex<Option<wire::SelfHealStatus>>,
}

impl EventBus {
    /// 构造空总线（无事件、无统计、无 proxy TUN 检测、tick=0）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: exv_vpn_host::event_bus::EventBus::new(),
            proxy_tun_current: std::sync::Mutex::new(None),
            system_proxy_current: std::sync::Mutex::new(None),
            service_status_current: std::sync::Mutex::new(None),
            service_mode_current: std::sync::Mutex::new(None),
            stats_diagnostic: std::sync::Mutex::new(StatsForwarderDiagnostic::starting()),
            self_heal_current: std::sync::Mutex::new(None),
        }
    }

    /// 铸造下一 monotonic tick（严格递增，从 1 开始；委托核心）。
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "仅单测断言 tick 轴；生产发布一律经 publish/publish_stats 在 gate 内铸造"
        )
    )]
    fn next_tick(&self) -> u64 {
        self.core.next_tick()
    }

    /// 当前已发布的最大 tick（0 = 尚无事件；委托核心）。
    #[must_use]
    pub fn current_tick(&self) -> u64 {
        self.core.current_tick()
    }

    /// 最新发布事件的快照（无事件 → `None`；委托核心）。
    #[must_use]
    pub fn current_snapshot(&self) -> Option<RuntimeSnapshot> {
        self.core.current_snapshot()
    }

    /// 发布一个带快照的事件：先经 win32 lane 装饰链组装快照，再委托核心铸造下一
    /// tick → 更新 `current` → 广播。
    ///
    /// `kind`：`SNAPSHOT` = 完整快照刷新；`TRANSITION` = 状态过渡。返回发布的事件
    /// （含 minted tick），调用方可用于观测。
    ///
    /// P5-wire 方案 A：发布时把 [`Self::current_stats`] 的最新统计附加到快照
    /// （统计是状态事件的伴生数据；无样本 → `stats` 留空）——`WatchEvents` 订阅者
    /// 随每个 snapshot 事件拿到统计。
    ///
    /// C5-wire：同点附加 [`Self::current_proxy_tun`]（上游 proxy TUN 检测，mirror
    /// stats 方案 A；无检测 → `proxy_tun` 留空）——`WatchEvents` 订阅者随每个 snapshot
    /// 事件拿到共存状态（badge 展示）。
    ///
    /// EXV_UNFREEZE：再附加 [`Self::current_system_proxy`]（系统代理感知，mirror
    /// proxy_tun 方案 A；无检测 → `system_proxy` 留空）——订阅者随每个 snapshot 事件
    /// 拿到系统代理感知状态。
    ///
    /// EXV_UNFREEZE 2026-09-05：再附加 [`Self::current_self_heal`]（host 自愈进展，
    /// mirror proxy_tun 方案 A；无上下文 → `self_heal` 留空）——订阅者随每个 snapshot
    /// 事件拿到 engine 崩溃 respawn 进展。
    pub fn publish(&self, kind: wire::RuntimeEventKind, snapshot: RuntimeSnapshot) -> RuntimeEvent {
        // 装饰链在核心 `publish_gate` 临界区内执行（`publish_composing` 的组装闭包），
        // 与上提前的 win32 语义一致：lane 缓存读取与 tick 铸造/`current` 写回串行化，
        // 避免较旧快照在较新 tick 之后写回。
        self.core.publish_composing(kind, |_tick| {
            snapshot_with_self_heal(
                snapshot_with_service_context(
                    snapshot_with_system_proxy(
                        snapshot_with_proxy_tun(
                            snapshot_with_stats(snapshot, self.current_stats()),
                            self.current_proxy_tun(),
                        ),
                        self.current_system_proxy(),
                    ),
                    self.current_service_status(),
                    self.current_service_mode(),
                ),
                self.current_self_heal(),
            )
        })
    }

    /// 订阅事件流（P3-c1 `WatchEvents` 语义；委托核心）。
    ///
    /// `resume_tick == 0` → 先发当前快照 SNAPSHOT 事件（从当前开始）；
    /// `resume_tick > 0` 且已落后（当前 tick > resume）→ 重放当前快照（断线重放
    /// 语义：host 只保留最新快照，真事件日志属 P5）；随后转发 tick > resume 的
    /// 现场事件。
    pub fn subscribe(&self, resume_tick: u64) -> impl Stream<Item = RuntimeEvent> + Send + 'static {
        self.core.subscribe(resume_tick)
    }

    // -----------------------------------------------------------------------
    // P5-b 统计 lane：与 wire 事件 lane 共存于同一总线（互不干扰；tick 对齐关联）。
    // 核心持 wire `RuntimeStats`，本 wrapper 在 host↔wire 边界转换（语义不变）。
    // -----------------------------------------------------------------------

    /// 发布一条归一化统计并更新 `stats_current`。若当前状态为已连接，则为该样本铸造
    /// 新 tick，并把携带该统计的 `SNAPSHOT` 重发到 `WatchEvents`；这使 Tauri 能持续
    /// 收到速率、累计量和会话起点，而不是只停在首次连接快照。其他状态不生成伪刷新，
    /// 样本沿用当前 tick。
    pub fn publish_stats(&self, stats: RuntimeStats) -> RuntimeStats {
        runtime_stats_from_wire(self.core.publish_stats(runtime_stats_to_wire(stats)))
    }

    /// 最新归一化统计（无 → `None`）。
    #[must_use]
    pub fn current_stats(&self) -> Option<RuntimeStats> {
        self.core.current_stats().map(runtime_stats_from_wire)
    }

    // -----------------------------------------------------------------------
    // RT-DIAG-03 统计转发器诊断 lane：进程内观测口（不进 RuntimeSnapshot，不进 wire；
    // 落盘事实走 LogAggregator 的 kernel.stats.* 冻结码，经既有 logs.list 契约可见）。
    // -----------------------------------------------------------------------

    /// 当前统计转发器诊断快照（单测/观测经此断言状态机；进程内可查）。
    #[must_use]
    pub fn stats_diagnostic(&self) -> StatsForwarderDiagnostic {
        self.stats_diagnostic.lock().unwrap().clone()
    }

    /// 统计转发器诊断状态的原地更新（转发器任务专用；短临界区，不跨 await 持锁）。
    pub(crate) fn update_stats_diagnostic(&self, f: impl FnOnce(&mut StatsForwarderDiagnostic)) {
        let mut diagnostic = self.stats_diagnostic.lock().unwrap();
        f(&mut diagnostic);
    }

    // -----------------------------------------------------------------------
    // C5-wire proxy TUN lane：与 wire 事件 lane / 统计 lane 共存（仅缓存，不铸造 tick；
    // `GetSnapshot` 与事件转发器在发布前刷新，`publish`/`GetSnapshot` 附加到快照）。
    // -----------------------------------------------------------------------

    /// 刷新最新上游 proxy TUN 检测缓存（`None` = 探测失败/尚未探测）。
    pub fn refresh_proxy_tun(&self, detection: Option<wire::ProxyTunDetection>) {
        *self.proxy_tun_current.lock().unwrap() = detection;
    }

    /// 最新上游 proxy TUN 检测（无 → `None`）。
    #[must_use]
    pub fn current_proxy_tun(&self) -> Option<wire::ProxyTunDetection> {
        self.proxy_tun_current.lock().unwrap().clone()
    }

    // -----------------------------------------------------------------------
    // EXV_UNFREEZE 系统代理 lane：与 wire 事件 lane / 统计 lane / proxy TUN lane 共存
    // （只缓存，不铸造 tick；`GetSnapshot` 与事件转发器在发布前刷新，`publish`/
    // `GetSnapshot` 附加到快照——mirror proxy_tun 方案 A）。
    // -----------------------------------------------------------------------

    /// 刷新最新系统代理检测缓存（`None` = 探测失败/尚未探测）。
    pub fn refresh_system_proxy(&self, detection: Option<wire::SystemProxyDetection>) {
        *self.system_proxy_current.lock().unwrap() = detection;
    }

    /// 最新系统代理检测（无 → `None`）。
    #[must_use]
    pub fn current_system_proxy(&self) -> Option<wire::SystemProxyDetection> {
        self.system_proxy_current.lock().unwrap().clone()
    }

    // -----------------------------------------------------------------------
    // S3/D5 服务上下文 lane：service_status + mode（mirror proxy_tun 方案 A；只缓存，
    // 不铸造 tick；`GetSnapshot`/connect 路径刷新，`publish`/`GetSnapshot` 附加）。
    // -----------------------------------------------------------------------

    /// 刷新最新 SCM 服务状态缓存（`None` = 尚未查询/查询失败——状态上报不失败）。
    pub fn refresh_service_status(&self, status: Option<wire::ServiceStatus>) {
        *self.service_status_current.lock().unwrap() = status;
    }

    /// 最新 SCM 服务状态（无 → `None`）。
    #[must_use]
    pub fn current_service_status(&self) -> Option<wire::ServiceStatus> {
        self.service_status_current.lock().unwrap().clone()
    }

    /// 刷新最新展示模式缓存（`None` → 快照缺省 `"auto"`）。
    pub fn refresh_service_mode(&self, mode: Option<String>) {
        *self.service_mode_current.lock().unwrap() = mode;
    }

    /// 最新展示模式（无 → `None`，调用方回退 `"auto"`）。
    #[must_use]
    pub fn current_service_mode(&self) -> Option<String> {
        self.service_mode_current.lock().unwrap().clone()
    }

    // -----------------------------------------------------------------------
    // 2026-09-05 host 自愈进展 lane（EXV_UNFREEZE wire `self_heal = 16`；mirror
    // proxy_tun 方案 A：只缓存，不铸造 tick；`publish`/`GetSnapshot` 附加到快照）。
    // 自愈窗口内没有自然事件（engine 已死、respawn 编排中）——阶段变化点由
    // [`Self::publish_self_heal_refresh`] 显式驱动发布，否则 UI 停在上一次快照
    // （计划 §1.3 缺陷的机制根源）。
    // -----------------------------------------------------------------------

    /// 刷新最新自愈阶段缓存（`None` = 无自愈上下文；用户发起连接时清除）。
    pub fn refresh_self_heal(&self, status: Option<wire::SelfHealStatus>) {
        *self.self_heal_current.lock().unwrap() = status;
    }

    /// 最新自愈阶段（无 → `None`）。
    #[must_use]
    pub fn current_self_heal(&self) -> Option<wire::SelfHealStatus> {
        self.self_heal_current.lock().unwrap().clone()
    }

    /// 自愈阶段变化的显式发布点：以调用方组装的相位快照铸造新 tick 发布过渡事件。
    ///
    /// **它不是第二套发布路径**：内部即复用既有 [`Self::publish`]（`TRANSITION`）——
    /// 同一 tick 铸造与 `snapshot_with_*` lane 附加链（stats/proxy_tun/system_proxy/
    /// service_status/mode/self_heal）原样生效，仅触发时机由自愈阶段变化显式驱动。
    /// 相位快照由调用方经 `snapshot_for_phase` 组装（模块私有 → `pub(crate)`）。
    pub fn publish_self_heal_refresh(&self, phase_snapshot: RuntimeSnapshot) -> RuntimeEvent {
        self.publish(wire::RuntimeEventKind::Transition, phase_snapshot)
    }

    /// 订阅归一化统计流（host 内部；P5-c 消费；核心 lane + host 镜像转换）。
    ///
    /// `resume_tick == 0` 或落后 → 先发当前统计（含最新 `sample_tick`），随后转发
    /// 更新的统计。统计不独立铸造 tick，故 live 过滤以发布时刻的 `sample_tick` 判定。
    pub fn subscribe_stats(
        &self,
        resume_tick: u64,
    ) -> impl Stream<Item = RuntimeStats> + Send + 'static {
        self.core
            .subscribe_stats(resume_tick)
            .map(runtime_stats_from_wire)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// wire `RuntimeStats` → host `RuntimeStats`（W3-1/P1 S1b：核心总线持 wire 类型，
/// wrapper 边界往返转换；与 [`runtime_stats_to_wire`] 互逆，`phase` 经
/// `stats_phase_from_i32` 归一）。
#[must_use]
fn runtime_stats_from_wire(stats: wire::RuntimeStats) -> RuntimeStats {
    RuntimeStats {
        rx_bytes: stats.rx_bytes,
        tx_bytes: stats.tx_bytes,
        rx_rate_bps: stats.rx_rate_bps,
        tx_rate_bps: stats.tx_rate_bps,
        latency_ms: stats.latency_ms,
        phase: crate::stats::stats_phase_from_i32(stats.phase),
        engine_sequence: stats.engine_sequence,
        sample_tick: stats.sample_tick,
    }
}

// ---------------------------------------------------------------------------
// 2026-09-05 host 自愈进展：[`crate::crash_recovery::SelfHealReporter`] 真实实现
// （桥接 EventBus self_heal lane + 日志聚合器；计划 §4.3）。相位快照的显式发布
// （`publish_self_heal_refresh`）由调用方（`shutdown.rs` respawn 分支发布点）驱动。
// ---------------------------------------------------------------------------

/// 自愈上报的真实实现：阶段写入 [`EventBus`] self_heal lane + `append_core` 结构化
/// 日志（`kernel.selfheal.detected` / `kernel.selfheal.succeeded`（fields: old_pid/
/// new_pid）/ `kernel.selfheal.failed`（fields: error_code））。无秘密、无栈（proto
/// 日志不变量同源）。测试用 recording fake（`crash_recovery` 测试模块）。
pub struct EventBusSelfHealReporter {
    events: Arc<EventBus>,
    logs: Arc<LogAggregator>,
}

impl EventBusSelfHealReporter {
    /// 构造（`main.rs` 连线：`KernelControlService::events()` + 既有共享日志聚合器）。
    #[must_use]
    pub fn new(events: Arc<EventBus>, logs: Arc<LogAggregator>) -> Self {
        Self { events, logs }
    }
}

impl crate::crash_recovery::SelfHealReporter for EventBusSelfHealReporter {
    fn report(
        &self,
        stage: crate::crash_recovery::SelfHealStage,
        old_pid: u32,
        new_pid: u32,
        error_code: &str,
    ) {
        use crate::crash_recovery::SelfHealStage;
        // 1. lane 刷新（`publish`/`GetSnapshot` 附加链读此缓存）。
        self.events.refresh_self_heal(Some(self_heal_status_wire(
            stage, old_pid, new_pid, error_code,
        )));
        // 2. 结构化日志（进 UI 日志页；无秘密、无栈）。
        let (code, level, message, mut fields) = match stage {
            SelfHealStage::Respawning => (
                "kernel.selfheal.detected",
                "warn",
                format!("engine crash detected; respawn started (old_pid={old_pid})"),
                BTreeMap::new(),
            ),
            SelfHealStage::Succeeded => (
                "kernel.selfheal.succeeded",
                "info",
                format!(
                    "engine respawned (old_pid={old_pid}, new_pid={new_pid}); reconnect required"
                ),
                BTreeMap::new(),
            ),
            SelfHealStage::Failed => (
                "kernel.selfheal.failed",
                "error",
                format!("engine respawn failed (error_code={error_code}); restart required"),
                BTreeMap::new(),
            ),
        };
        match stage {
            SelfHealStage::Respawning => {
                fields.insert("old_pid".to_string(), old_pid.to_string());
            }
            SelfHealStage::Succeeded => {
                fields.insert("old_pid".to_string(), old_pid.to_string());
                fields.insert("new_pid".to_string(), new_pid.to_string());
            }
            SelfHealStage::Failed => {
                fields.insert("error_code".to_string(), error_code.to_string());
            }
        }
        let _ = self
            .logs
            .append_core(level, "kernel", code, &message, &fields);
    }
}

/// C3a：判定 wire 错误是否为「可重试数据面掉线」（自动重连的触发标记）。
///
/// C2 的掉线事件是**唯一** `stage=DataPlane + retry=RetrySameOperation` 来源（code=
/// EffectUnknown 不参与判定——它是平台类失败通用码，C2 未新增错误码避免 wire 契约
/// 变更）；据此把「连接建立后数据面掉线（可重试）」与「连接期失败（DoNotRetry）」
/// 明确区分。
#[must_use]
fn is_retryable_disconnect(error: &wire::VpnError) -> bool {
    error.stage == wire::ErrorStage::DataPlane as i32
        && error.retry == wire::RetryAdvice::RetrySameOperation as i32
}

/// C3a：自动重连的 connect 意图（host 侧从磁盘凭据重新组装的最小意图）。
///
/// 复用既有 connect 流程（`run_connect` → `execute_connect`）需要一份合法
/// `ConnectIntent`：`lookup_key.method=Connect` + 新随机 runtime_epoch/operation_id
/// （新操作——终态事件按 operation_id 关联，陈旧过滤可区分新旧尝试）+ 32 字节
/// request_digest。`principal_digest` 与 UI 同源派生（当前用户 SID → SHA256；
/// engine 侧操作身份以认证 peer 为准，`lookup_key_from_wire` 只用它做 32 字节合法
/// 性校验）；SID 不可用时回落全零（仍合法 32 字节）。凭据/plan 由 `execute_connect`
/// 从磁盘 `config_dir` 重新 load，本意图不携带任何秘密。
#[must_use]
fn reconnect_intent() -> wire::ConnectIntent {
    let principal_digest = current_user_sid()
        .map(|sid| crate::grpc_control::owner_principal_digest(&sid))
        .unwrap_or_else(|| vec![0u8; 32]);
    wire::ConnectIntent {
        lookup_key: Some(wire::OperationLookupKey {
            principal_digest,
            method: wire::OperationMethod::Connect as i32,
            runtime_epoch: Uuid::new_v4().as_bytes().to_vec(),
            operation_id: Uuid::new_v4().as_bytes().to_vec(),
        }),
        request_digest: Sha256::digest(Uuid::new_v4().as_bytes()).to_vec(),
        profile: None,
    }
}

/// 生产调用者持有 composition 直到发布完成；测试包装只观察纯转换结果。
/// 未请求停止却收到当前会话的 Idle，按意外掉线进入同一失败/重连流程。
fn normalize_unexpected_stop(
    event: EngineStatusEvent,
    composition: &HostComposition,
    logs: &LogAggregator,
) -> EngineStatusEvent {
    if composition.phase() == HostPhase::Connected && !stale_terminal_operation(composition, &event) {
        if let EngineStatusEvent::Stopped { operation_id } = event {
            let _ = logs.append_core("warn", "connection", "kernel.connection.unexpected_stop",
                "未请求断开，但引擎报告当前会话已停止",
                &BTreeMap::from([("operation_id".into(), Uuid::from_slice(&operation_id).map(|id| id.to_string()).unwrap_or_default())]));
            return EngineStatusEvent::Failed {
                operation_id,
                error: wire::VpnError {
                    code: wire::ErrorCode::EffectUnknown as i32,
                    stage: wire::ErrorStage::DataPlane as i32,
                    certainty: wire::EffectCertainty::NoEffect as i32,
                    retry: wire::RetryAdvice::RetrySameOperation as i32,
                    ..Default::default()
                },
            };
        }
    }
    event
}

fn runtime_event_from_status_locked(
    ev: &EngineStatusEvent,
    guard: &mut HostComposition,
    logs: &LogAggregator,
) -> Option<(wire::RuntimeEventKind, RuntimeSnapshot)> {
    // 终态事件陈旧过滤：有登记的在途操作且事件 operation_id 不符 → 丢弃（不驱动
    // 状态机、不发布）。
    if matches!(
        ev,
        EngineStatusEvent::Connected { .. }
            | EngineStatusEvent::Failed { .. }
            | EngineStatusEvent::Stopped { .. }
    ) && stale_terminal_operation(&guard, ev)
    {
        return None;
    }
    match ev {
        EngineStatusEvent::Progress { connect_phase, .. } => {
            guard.apply(HostEvent::ConnectPhaseProgress(domain_connect_phase(
                *connect_phase,
            )));
            Some((
                wire::RuntimeEventKind::Transition,
                snapshot_for_phase(guard.phase(), &guard),
            ))
        }
        EngineStatusEvent::Connected {
            session_established_at_ms,
            ..
        } => {
            if guard.phase() == HostPhase::Failed {
                let _ = logs.append_core("debug", "connection", "kernel.connection.late_connected_ignored",
                    "当前操作已经失败，忽略迟到的连接成功事件", &BTreeMap::new());
                return None;
            }
            guard.apply(HostEvent::ProtocolEstablished);
            // RT-SESSION-04：engine 提供正值起点 → 原样采纳（无码）；缺正值
            //（`None` 或 `Some(<=0)`，`connect_status_from_wire` 已把非正归一为
            // `None`）→ core 以本地时钟首次可靠观察时间为 adopted 起点 + 码 11
            //（info，fields source="core_clock"）。同一会话已有确认事实时不重复
            // 采纳（补值只增不减，与 `set_session_established_at_ms` 语义一致；
            // `snapshot_with_confirmed_session_start` 行为不变）。
            if session_established_at_ms
                .filter(|value| *value > 0)
                .is_some()
            {
                guard.set_session_established_at_ms(*session_established_at_ms);
            } else if guard
                .session_established_at_ms()
                .filter(|value| *value > 0)
                .is_none()
            {
                adopt_local_session_start(guard, logs, unix_now_ms());
            }
            let snapshot = snapshot_for_phase(guard.phase(), &guard);
            Some((wire::RuntimeEventKind::Transition, snapshot))
        }
        EngineStatusEvent::Failed { error, .. } => {
            let previous_phase = guard.phase();
            guard.set_last_wire_error(error.clone());
            let domain_error = wire_vpn_error_to_domain(error);
            if previous_phase == HostPhase::Connected {
                guard.apply(HostEvent::ConnectionLost(domain_error));
            } else {
                guard.apply(HostEvent::ConnectFailed(domain_error));
            }
            let _ = logs.append_core(
                "warn", "connection", "kernel.connection.failed",
                "当前连接失败，已更新连接状态；是否重连由会话设置决定",
                &BTreeMap::from([
                    ("operation_id".into(), guard.operation_id().map(|id| Uuid::from_bytes(id).to_string()).unwrap_or_default()),
                    ("previous_phase".into(), format!("{previous_phase:?}")),
                    ("next_phase".into(), format!("{:?}", guard.phase())),
                    ("error_stage".into(), error.stage.to_string()),
                    ("error_code".into(), error.code.to_string()),
                    ("retry_advice".into(), error.retry.to_string()),
                ]),
            );
            Some((
                wire::RuntimeEventKind::Transition,
                snapshot_for_phase(guard.phase(), &guard),
            ))
        }
        EngineStatusEvent::Stopped { .. } => {
            // engine 确认 teardown 完成：数据面（engine）侧加入屏障 → 与 host 侧
            // 齐全后 Stopped（R1 收敛；R2 再补 core 合成 Idle 双保险）。
            //
            // S1.5 (D13) Paused 映射：减负断开后 engine 运行时进入 **Paused**（session
            // 结束 + 路由清 + NIC 保留 adapter），host 侧**复用既有 Stopped 映射**——
            // `connect_status_from_wire` 把 engine Idle 终态归一化为
            // `EngineStatusEvent::Stopped`，此处数据面侧加入屏障 → Stopped → 显式
            // ReopenAdmission → Idle。冻结 portable `HostComposition` 不动：Paused 是
            // engine 运行时状态，host 只以 Stopped + 既有 fine-phase（`connect_phase`
            // 寄存器）映射；网卡保留由退出清理阶段（engine teardown）收敛到 0 残留。
            guard.apply(HostEvent::TeardownSideJoined(TeardownSide::PacketData));
            let snapshot = snapshot_for_phase(guard.phase(), &guard);
            // S1（D14）：Stopped 快照先可观测（快照已组装，Stopped → wire Idle），
            // 随后 controller 显式派发 ReopenAdmission——admission 重开 + phase 回落
            // Idle，同进程二次 Connect 可受理（问题 A 闩锁修复）。**不**在
            // TeardownSideJoined Released 臂内同步回落（Stopped pin 保持）；仅当
            // 屏障确实释放（phase == Stopped）才重开——Stopping/Failed 等其余相
            // 位 no-op，teardown 拒绝语义保留。
            if guard.phase() == HostPhase::Stopped {
                guard.apply(HostEvent::ReopenAdmission);
            }
            Some((wire::RuntimeEventKind::Transition, snapshot))
        }
    }
}

/// RT-SESSION-04：engine `Connected` 事件缺正值 `session_established_at_ms` 时，core
/// 以本地时钟 `now_ms`（首次可靠观察时间）采纳 adopted 起点，并落码 11（info，
/// fields `source="core_clock"`，每次 Connected 采纳事件至多一条）。已有确认事实时
/// 不覆盖、不重复落码（补值只增不减）。`now_ms` 由调用方注入（生产 = 本地时钟；
/// 测试注入确定性值——U9）。
fn adopt_local_session_start(guard: &mut HostComposition, logs: &LogAggregator, now_ms: i64) {
    emit_stats_diag(
        logs,
        StatsDiagCode::SessionAdopted,
        [("source", "core_clock".to_string())],
    );
    guard.set_session_established_at_ms(Some(now_ms));
}

/// R3-C2：判定终态事件是否为"旧操作的迟到终态"——composition 有登记的在途操作且
/// 事件 `operation_id` 与之不符。
#[must_use]
fn stale_terminal_operation(composition: &HostComposition, ev: &EngineStatusEvent) -> bool {
    let Some(current) = composition.operation_id() else {
        return false; // 无登记在途操作：无法判定陈旧，保守透传（不破坏未登记场景）。
    };
    let event_id = match ev {
        EngineStatusEvent::Connected { operation_id, .. }
        | EngineStatusEvent::Failed { operation_id, .. }
        | EngineStatusEvent::Stopped { operation_id } => operation_id,
        EngineStatusEvent::Progress { .. } => return false,
    };
    current.as_slice() != event_id.as_slice()
}

/// wire `ConnectPhase` → domain `ConnectPhase`（8 级一一对应；Unspecified 回落
/// 首个可观测阶段——状态流不产生 Unspecified，防御性回落）。
fn domain_connect_phase(phase: ConnectPhase) -> exv_vpn_domain::model::ConnectPhase {
    use exv_vpn_domain::model::ConnectPhase as P;
    match phase {
        ConnectPhase::ObservingOwnedState => P::ObservingOwnedState,
        ConnectPhase::AcquiringPlatformLease => P::AcquiringPlatformLease,
        ConnectPhase::ConnectingControl => P::ConnectingControl,
        ConnectPhase::AwaitingInteraction => P::AwaitingInteraction,
        ConnectPhase::NegotiatingTunnel => P::NegotiatingTunnel,
        ConnectPhase::ApplyingPlatformTunnel => P::ApplyingPlatformTunnel,
        ConnectPhase::AttachingPacketBoundary => P::AttachingPacketBoundary,
        ConnectPhase::StartingDataPlane => P::StartingDataPlane,
        ConnectPhase::Unspecified => P::ObservingOwnedState,
    }
}

/// R5：路由失败（PromptStart / service engine connect 失败）→ wire `VpnError`。
///
/// 路由失败发生在派发前——engine 未观测到任何失败，故不伪造 engine 观测字段；
/// code/stage/certainty/retry 取服务路由不可用的确定性语义（`SessionBusy` /
/// `Admission` / `NoEffect` / `UseNewOperation`），subject 留空（`wire_vpn_error_to_domain`
/// 回落确定性 runtime stand-in）。该错误只作快照 `FailedDirty.last_error` 的稳定 code
/// 呈现；人读信息由 connect RPC 的 `failed_precondition` 消息承载。
fn route_failure_wire_error(_status: &Status) -> wire::VpnError {
    use wire::{EffectCertainty, ErrorCode, ErrorStage, RetryAdvice};
    wire::VpnError {
        code: ErrorCode::SessionBusy as i32,
        stage: ErrorStage::Admission as i32,
        certainty: EffectCertainty::NoEffect as i32,
        retry: RetryAdvice::UseNewOperation as i32,
        subject: None,
        resource: None,
        native: None,
    }
}

/// wire `VpnError` → domain `VpnError`（R1：失败 err 上状态机）。
///
/// 结构化字段（code/stage/certainty/retry）一一对应，绝不丢失；subject 按分支
/// best-effort 转换（External/Runtime 可解析；缺省/不可解析回落确定性的 runtime
/// stand-in——与 host `unauthorized_error` 同源，红色acted 非秘密）；resource/native
/// 可解析则转，否则 `None`。R1 阶段 engine 失败为占位（真实数据面 R1b 接入），
/// 转换保持诚实（不伪造外部 OperationId——spec §10）。
fn wire_vpn_error_to_domain(error: &wire::VpnError) -> VpnError {
    use exv_vpn_domain::error::{
        EffectCertainty, ErrorCode, ErrorStage, ErrorSubject, RetryAdvice,
    };
    let code = match wire::ErrorCode::try_from(error.code) {
        Ok(wire::ErrorCode::InvalidInput) => ErrorCode::InvalidInput,
        Ok(wire::ErrorCode::IdempotencyConflict) => ErrorCode::IdempotencyConflict,
        Ok(wire::ErrorCode::ConnectInProgress) => ErrorCode::ConnectInProgress,
        Ok(wire::ErrorCode::SessionBusy) => ErrorCode::SessionBusy,
        Ok(wire::ErrorCode::ReconnectAlreadyQueued) => ErrorCode::ReconnectAlreadyQueued,
        Ok(wire::ErrorCode::CancelledBeforeStart) => ErrorCode::CancelledBeforeStart,
        Ok(wire::ErrorCode::AuthorityAlreadyHeld) => ErrorCode::AuthorityAlreadyHeld,
        Ok(wire::ErrorCode::OwnershipAcquisitionPending) => ErrorCode::OwnershipAcquisitionPending,
        Ok(wire::ErrorCode::ObservedConflict) => ErrorCode::ObservedConflict,
        Ok(wire::ErrorCode::JournalCorrupt) => ErrorCode::JournalCorrupt,
        Ok(wire::ErrorCode::DataPlaneBackpressure) => ErrorCode::DataPlaneBackpressure,
        Ok(wire::ErrorCode::PacketLeaseAlreadyAttached) => ErrorCode::PacketLeaseAlreadyAttached,
        Ok(wire::ErrorCode::EffectUnknown) => ErrorCode::EffectUnknown,
        Ok(wire::ErrorCode::ObservationFailed) => ErrorCode::ObservationFailed,
        Ok(wire::ErrorCode::Unauthorized) => ErrorCode::Unauthorized,
        Ok(wire::ErrorCode::DeadlineExceeded) => ErrorCode::DeadlineExceeded,
        Ok(wire::ErrorCode::PlatformDependencyUnavailable) => {
            ErrorCode::PlatformDependencyUnavailable
        }
        Ok(wire::ErrorCode::ActiveAttemptCannotReconcile) => {
            ErrorCode::ActiveAttemptCannotReconcile
        }
        Ok(wire::ErrorCode::ActiveSessionCannotReconcile) => {
            ErrorCode::ActiveSessionCannotReconcile
        }
        _ => ErrorCode::ObservationFailed,
    };
    let stage = match wire::ErrorStage::try_from(error.stage) {
        Ok(wire::ErrorStage::Ingress) => ErrorStage::Ingress,
        Ok(wire::ErrorStage::Admission) => ErrorStage::Admission,
        Ok(wire::ErrorStage::ObservingOwnedState) => ErrorStage::ObservingOwnedState,
        Ok(wire::ErrorStage::AcquiringPlatformLease) => ErrorStage::AcquiringPlatformLease,
        Ok(wire::ErrorStage::ConnectingControl) => ErrorStage::ConnectingControl,
        Ok(wire::ErrorStage::AwaitingInteraction) => ErrorStage::AwaitingInteraction,
        Ok(wire::ErrorStage::NegotiatingTunnel) => ErrorStage::NegotiatingTunnel,
        Ok(wire::ErrorStage::ApplyingPlatformTunnel) => ErrorStage::ApplyingPlatformTunnel,
        Ok(wire::ErrorStage::AttachingPacketBoundary) => ErrorStage::AttachingPacketBoundary,
        Ok(wire::ErrorStage::StartingDataPlane) => ErrorStage::StartingDataPlane,
        Ok(wire::ErrorStage::ProtocolSession) => ErrorStage::ProtocolSession,
        Ok(wire::ErrorStage::DataPlane) => ErrorStage::DataPlane,
        Ok(wire::ErrorStage::Teardown) => ErrorStage::Teardown,
        Ok(wire::ErrorStage::Recovery) => ErrorStage::Recovery,
        Ok(wire::ErrorStage::Journal) => ErrorStage::Journal,
        _ => ErrorStage::ObservingOwnedState,
    };
    let certainty = match wire::EffectCertainty::try_from(error.certainty) {
        Ok(wire::EffectCertainty::NoEffect) => EffectCertainty::NoEffect,
        Ok(wire::EffectCertainty::Applied) => EffectCertainty::Applied,
        Ok(wire::EffectCertainty::Partial) => EffectCertainty::Partial,
        Ok(wire::EffectCertainty::Unknown) => EffectCertainty::Unknown,
        _ => EffectCertainty::NoEffect,
    };
    let retry = match wire::RetryAdvice::try_from(error.retry) {
        Ok(wire::RetryAdvice::DoNotRetry) => RetryAdvice::DoNotRetry,
        Ok(wire::RetryAdvice::RetrySameOperation) => RetryAdvice::RetrySameOperation,
        Ok(wire::RetryAdvice::UseNewOperation) => RetryAdvice::UseNewOperation,
        Ok(wire::RetryAdvice::Reconcile) => RetryAdvice::Reconcile,
        Ok(wire::RetryAdvice::RestartProcess) => RetryAdvice::RestartProcess,
        _ => RetryAdvice::DoNotRetry,
    };
    let subject = error
        .subject
        .as_ref()
        .and_then(wire_subject_to_domain)
        .unwrap_or_else(|| {
            // 缺省/不可解析 → 确定性 runtime stand-in（与 host `unauthorized_error`
            // 同源；红色acted 非秘密；spec §10：不伪造外部 OperationId）。
            ErrorSubject::Runtime(
                exv_vpn_domain::identity::RuntimeEpoch::try_from(uuid::Uuid::from_u128(1))
                    .expect("non-nil runtime epoch"),
            )
        });
    VpnError::try_from((code, stage, certainty, retry, subject, None, None))
        .expect("valid error tuple")
}

/// wire `ErrorSubject` → domain `ErrorSubject`（best-effort：External/Runtime 可解析，
/// 其余分支或 malformed 字节 → `None`，由调用方回落 runtime stand-in）。
fn wire_subject_to_domain(
    subject: &wire::ErrorSubject,
) -> Option<exv_vpn_domain::error::ErrorSubject> {
    use exv_vpn_domain::error::ErrorSubject;
    use exv_vpn_domain::identity::{
        OperationId, OperationLookupKey, OperationMethod, RuntimeEpoch,
    };
    match subject.subject.as_ref()? {
        wire::error_subject::Subject::External(key) => {
            let principal_digest = <[u8; 32]>::try_from(key.principal_digest.as_slice()).ok()?;
            let method = match wire::OperationMethod::try_from(key.method).ok()? {
                wire::OperationMethod::Connect => OperationMethod::Connect,
                wire::OperationMethod::RespondInteraction => OperationMethod::RespondInteraction,
                wire::OperationMethod::Stop => OperationMethod::Stop,
                wire::OperationMethod::Reconcile => OperationMethod::Reconcile,
                wire::OperationMethod::AcquireLease => OperationMethod::AcquireLease,
                wire::OperationMethod::ApplyTunnel => OperationMethod::ApplyTunnel,
                wire::OperationMethod::StopTunnel => OperationMethod::StopTunnel,
                wire::OperationMethod::ReleaseLease => OperationMethod::ReleaseLease,
                _ => return None,
            };
            let runtime_epoch = uuid_from_16(&key.runtime_epoch)?;
            let operation_id = uuid_from_16(&key.operation_id)?;
            let domain_key = OperationLookupKey::try_from((
                exv_vpn_domain::identity::PrincipalDigest::try_from(principal_digest).ok()?,
                method,
                RuntimeEpoch::try_from(runtime_epoch).ok()?,
                OperationId::try_from(operation_id).ok()?,
            ))
            .ok()?;
            Some(ErrorSubject::External(domain_key))
        }
        wire::error_subject::Subject::Runtime(runtime) => {
            let epoch = uuid_from_16(&runtime.runtime_epoch)?;
            Some(ErrorSubject::Runtime(RuntimeEpoch::try_from(epoch).ok()?))
        }
        _ => None,
    }
}

/// 16 字节 wire bytes → `Uuid`（长度不符 → `None`）。
fn uuid_from_16(bytes: &[u8]) -> Option<uuid::Uuid> {
    let arr = <[u8; 16]>::try_from(bytes).ok()?;
    Some(uuid::Uuid::from_bytes(arr))
}

/// 断线表示的快照（engine 断开 → 包边界丢失的恢复上下文，Reconciling 状态）。
///
/// 与 composition 的 `HelperLinkLost` 语义对齐：helper 链路断开 ≈ 包边界丢失。host
/// 不在此处 mutate composition（那属 P3-c2 进程生命周期的 helper-link 处理）；仅向
/// UI 发布恢复中过渡。
fn disconnected_snapshot(composition: &HostComposition) -> RuntimeSnapshot {
    // R2 收敛双保险：已收敛的终态（Idle / Stopped / Failed）不得被 engine 状态流断线
    // 回归成 Reconciling——EOF 只表示 engine 侧掉线，不代表业务已回退。只有非终态
    // （Connecting / Connected / Stopping / Reconciling——连接在途、engine 提前离开）
    // 才以 Reconciling 表示待恢复。`Stopped`/`Idle` 回落 Idle 终态，`Failed` 保持
    // FailedDirty（终态不丢错误上下文）。
    match composition.phase() {
        HostPhase::Idle | HostPhase::Stopped | HostPhase::Failed => {
            return snapshot_for_phase(composition.phase(), composition);
        }
        _ => {}
    }
    wire::RuntimeSnapshot {
        state: Some(wire::runtime_snapshot::State::Reconciling(
            wire::ReconcilingState {
                context: Some(recovery_context_standin(composition)),
                obligation: Some(recovery_obligation_standin(composition)),
            },
        )),
        // stats / proxy_tun 由 `snapshot_with_stats` / `snapshot_with_proxy_tun`
        // 在发布时附加。S3：service_status/mode 由 `snapshot_with_service_context`
        // 在 GetSnapshot/发布时附加（缺省 None/空）。
        stats: None,
        proxy_tun: None,
        // EXV_UNFREEZE 2026-08-24：系统代理感知状态（设计 §5.5），由感知链路附加；占位 None。
        system_proxy: None,
        // EXV_UNFREEZE 2026-08-25：自动重连状态（C3a host 驱动），由状态转发器附加；占位 None。
        reconnect: None,
        // EXV_UNFREEZE 2026-09-05：host 自愈进展，由 EventBus lane 在发布/GetSnapshot
        // 时附加；占位 None。
        self_heal: None,
        operation_id: composition
            .operation_id()
            .map(|id| id.to_vec())
            .unwrap_or_default(),
        service_status: None,
        mode: String::new(),
    }
}

/// 选择快照源（P3-c1 `GetSnapshot` 源化）：engine 快照非 Idle（真实数据）→ engine；
/// engine Idle 占位而 composition 非 Idle（host 有更新知识）→ composition；其余 →
/// engine（Idle 即 Idle）。
fn prefer_snapshot(
    engine: Option<RuntimeSnapshot>,
    composition: RuntimeSnapshot,
) -> RuntimeSnapshot {
    let Some(engine) = engine else {
        return composition;
    };
    // 查询开始后当前操作可能已失败/停止或换代。旧响应不得复活旧会话。
    if !composition.operation_id.is_empty() && !engine.operation_id.is_empty()
        && composition.operation_id != engine.operation_id {
        return composition;
    }
    if matches!(composition.state, Some(wire::runtime_snapshot::State::FailedDirty(_))
        | Some(wire::runtime_snapshot::State::FailedClean(_))
        | Some(wire::runtime_snapshot::State::Stopping(_)))
        && matches!(engine.state, Some(wire::runtime_snapshot::State::Connected(_))
            | Some(wire::runtime_snapshot::State::Connecting(_))) {
        return composition;
    }
    let engine_idle = matches!(engine.state, Some(wire::runtime_snapshot::State::Idle(_)));
    let composition_idle = matches!(
        composition.state,
        Some(wire::runtime_snapshot::State::Idle(_))
    );
    if engine_idle && engine.operation_id.is_empty() && !composition_idle {
        if matches!(composition.state, Some(wire::runtime_snapshot::State::Connected(_))) {
            RuntimeSnapshot { state: Some(wire::runtime_snapshot::State::Reconciling(wire::ReconcilingState {
                context: None, obligation: None,
            })), stats: None, ..composition }
        } else { composition }
    } else {
        engine
    }
}

// ---------------------------------------------------------------------------
// RespondInteraction / Reconcile 校验与重试决策（P3-c1）
// ---------------------------------------------------------------------------

/// 校验 `InteractionResponse`：`interaction_id` 与 `runtime_epoch` 必须 16 字节。
///
/// # Errors
/// 任一字段长度非法 → `Status::invalid_argument`。
fn validate_interaction_response(response: &wire::InteractionResponse) -> Result<(), Status> {
    if response.interaction_id.len() != 16 {
        return Err(Status::invalid_argument(
            "respond_interaction: interaction id must be 16 bytes",
        ));
    }
    if response.runtime_epoch.len() != 16 {
        return Err(Status::invalid_argument(
            "respond_interaction: runtime epoch must be 16 bytes",
        ));
    }
    Ok(())
}

/// `Reconcile` 的重试决策（P3-c1：义务 disposition → 真实重试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconcileRetry {
    /// 义务已终局（成功/失败）→ 无需重试，透传 terminal。
    Terminal,
    /// 义务仍待决（Pending/Unknown/Absent/Rejected）→ 需重试（重新 apply/acquire）。
    Retry,
}

/// 依据观察到的义务 disposition 决策是否重试（P3-c1）。
#[must_use]
fn reconcile_retry_plan(observed: &Option<OperationState>) -> ReconcileRetry {
    match observed.as_ref().and_then(|s| s.state.as_ref()) {
        Some(wire::operation_state::State::Terminal(_)) => ReconcileRetry::Terminal,
        _ => ReconcileRetry::Retry,
    }
}

// ---------------------------------------------------------------------------
// 单元测试：phase → 完整 snapshot、gate 拒绝路径、写路径（fake engine）。
// ---------------------------------------------------------------------------

