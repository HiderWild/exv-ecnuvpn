//! 固定 root Engine 的 E4 bootstrap 与唯一 Common 控制服务。
//!
//! `MAC-CSTP-05` 起：`apply_tunnel` 消费一次性封包后执行真实的
//! WebVPN 登录 + CSTP 协商（见 [`crate::protocol`]），经状态流发布真实阶段
//! 与 typed 失败；Cisco 与 aTrust 各自保有协议任务，共享真实停止与宿主资源账本。

// 中文文档中的技术术语不逐个加反引号；apply 处理器承载完整受理语义。
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]

use std::{
    env, fmt,
    io::{self, Write},
    net::Ipv4Addr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use exv_vpn_darwin_ipc::{
    bootstrap::{EngineBootstrapRecord, EngineTicketV1, TicketError, consume_engine_ticket},
    connect_envelope::{DarwinAtrustConnect, DarwinEngineConnectV1},
    listener::{PreauthListener, RootListenerConfig},
    path::{RuntimeDir, RuntimeDirError},
    peer::ExpectedPeer,
    tonic_bridge::AuthenticatedIncoming,
};

use exv_vpn_cstp::codec::{CSTP_PACKET_TYPE_KEEPALIVE, Codec};
use exv_vpn_cstp::session::{
    CSTP_PACKET_TYPE_DPD_REQUEST, CSTP_PACKET_TYPE_DPD_RESPONSE, CstpControlEvent,
};

use crate::log_sink::LogSink;
use crate::packet::probe::{LatencyProbeConfig, LatencyTap, ProbeState};
use crate::packet::pump;
use crate::platform::route::{self as platform_route, AppliedRoute, ControlRouteUpdate};
use crate::platform::utun::Utun;
use crate::stats::StatsPublisher;
use exv_vpn_wire::generated::{
    self as wire,
    helper_control_server::{HelperControl, HelperControlServer},
};
use tokio_stream::{Empty, StreamExt, wrappers::BroadcastStream};
use tonic::{Request, Response, Status, transport::Server};
use zeroize::Zeroize;

use crate::atrust_auth::{self, AtrustConnectError, reason as atrust_reason};
use crate::atrust_prepare::{self, PreparedAtrust};
use crate::connect_intent::{ConnectIntent, decode_connect_intent};
use crate::core_exit_policy::CoreExitPolicy;
use crate::protocol::EstablishedSession;
use crate::tunnel_hold::{AtrustHold, HoldObservation, TunnelHold};
use crate::underlay::DarwinPhysicalUnderlay;

/// 数据面存活标记：Some = 管线处于保持态，None = 已 teardown/无连接。
struct HeldTunnel;

/// T2/M6：采纳身份与 R1 材料的协议判别维度（设计 §7 keep/采纳行——协议必须入
/// 身份，否则 aTrust 非秘密占位用户名 `-atrust` 使比较面退化为 server 相等，
/// 跨协议接管窗口重开——Windows 真踩过）。判别子**不得**用 SID/会话秘密或其
/// 哈希（`PipelineIdentity` 用于采纳判定，秘密衍生值禁入）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtocolKind {
    Cisco,
    Atrust,
}

impl ProtocolKind {
    /// 采纳诊断使用的协议名称（"cisco" | "atrust"）。
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Cisco => "cisco",
            Self::Atrust => "atrust",
        }
    }
}

/// T4（D9）+M6：引擎侧 R1 重拨材料——内存驻留、协议枚举两臂、Zeroizing 持有、
/// 随善后清位。Cisco 臂字段与 pre-M6 结构逐字一致（行为零变化）；aTrust 臂 =
/// V2 aTrust 载荷的非秘密身份值 + 缓存 BrowserSession wire（apply 受理时捕获）。
/// 不派生 Debug（秘密臂禁打印）。
#[derive(Clone)]
pub(crate) enum ReconnectMaterials {
    /// Cisco：服务器/一次性密码/UA/校园路由/MTU（既有字段原样移入本臂）。
    Cisco {
        server: String,
        username: String,
        password: zeroize::Zeroizing<String>,
        user_agent: String,
        routes: Vec<String>,
        resolver_domains: Vec<String>,
        #[allow(
            dead_code,
            reason = "mtu_override 为材料完整性保留：管线参数走 plan_mtu，信封线程化（挂账）后由材料供值"
        )]
        mtu_override: Option<u16>,
    },
    /// aTrust（M6 材料臂）：R1 重拨从本臂取 BrowserSession 重跑 browser_handoff
    /// （isLogin 复核——失效即 R3 短路），身份值/资源字节与首连同源。
    Atrust(AtrustMaterials),
}

/// aTrust R1 材料臂的负载（与 V2 aTrust 载荷的非秘密身份值+会话 wire 同集）。
/// `browser_session` 是秘密（Zeroizing；绝不入日志/trace/Debug——本类型不派生
/// Debug）。
#[derive(Clone)]
pub(crate) struct AtrustMaterials {
    pub(crate) gateway: String,
    pub(crate) browser_session: zeroize::Zeroizing<Vec<u8>>,
    pub(crate) platform: String,
    pub(crate) language: String,
    pub(crate) resource_request: Vec<u8>,
    pub(crate) resolver_domains: Vec<String>,
    pub(crate) atrust_dns_enabled: bool,
}

impl AtrustMaterials {
    /// 从解码后的 V2 aTrust 载荷捕获材料（apply 受理时调用；副本自载荷取得，
    /// 原载荷随后照常消费）。
    pub(crate) fn from_payload(payload: &DarwinAtrustConnect) -> Self {
        Self {
            gateway: payload.gateway().to_owned(),
            browser_session: zeroize::Zeroizing::new(payload.browser_session_wire().to_vec()),
            platform: payload.platform().to_owned(),
            language: payload.language().to_owned(),
            resource_request: payload.resource_request().to_vec(),
            resolver_domains: payload.resolver_domains().to_vec(),
            atrust_dns_enabled: payload.atrust_dns_enabled(),
        }
    }
}

/// T2 采纳判定的管线身份：与凭据无关的非秘密比较面（同服务器+同用户名+**同协议**
/// 与同 resolver 域列表 = 同一连接配置的接管；密码/会话永不参与比较——新认证材料属"新认证"语境，走
/// `fresh_authentication` 分叉由 core 显式 Stop 后重建）。M6 起协议入身份
/// （`protocol` 判别维度；设计 §7）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PipelineIdentity {
    pub(crate) server: String,
    pub(crate) username: String,
    pub(crate) protocol: ProtocolKind,
    pub(crate) resolver_domains: Vec<String>,
    pub(crate) atrust_dns_enabled: bool,
}

/// T2/M6 采纳裁决（纯函数面——裁决表三维：身份匹配×协议×fresh 的唯一权威，
/// [`EngineControlService::apply_tunnel`] 的持有分叉与密闭裁决表测试共用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdoptionDecision {
    /// 全匹配 + fresh=false：采纳（不重登录/协商，切换事件归属）。
    Adopt,
    /// 身份或协议不匹配：typed 拒绝（core 显式 Stop 后重建/换配置）。
    Mismatch,
    /// 全匹配 + fresh=true：新认证替换——引擎不隐式拆旧建新，core 编排
    /// Stop→重 Apply。
    NewAuth,
}

#[must_use]
pub(crate) fn adoption_decision(
    held: &PipelineIdentity,
    intent: &PipelineIdentity,
    fresh_authentication: bool,
) -> AdoptionDecision {
    if held != intent {
        AdoptionDecision::Mismatch
    } else if fresh_authentication {
        AdoptionDecision::NewAuth
    } else {
        AdoptionDecision::Adopt
    }
}

/// Engine 唯一持有的跨会话平台资源账本。
///
/// `retained_device` 只在可重连的数据面掉线窗口中存在；业务路由和地址绝不放入此处。
/// 删除失败的路由保留精确原值，后续 Stop/退出会重试，不用“已执行清理”冒充已移除。
#[derive(Default)]
struct ResourceLedger {
    proxy_policy: Option<crate::system_proxy::Owner>,
    control_route: Option<AppliedRoute>,
    bypass_routes: Vec<AppliedRoute>,
    resolvers: Vec<crate::campus_resolver::InstalledResolver>,
    retained_device: Option<Utun>,
    pending_routes: Vec<AppliedRoute>,
}

/// 旁路账本按目的地 upsert（同目的地重复 reconcile 不产生重复行——Cisco 单槽
/// 语义的多行推广）。
fn upsert_bypass_ledger(ledger: &mut ResourceLedger, route: AppliedRoute) {
    if let Some(existing) = ledger
        .bypass_routes
        .iter_mut()
        .find(|existing| existing.destination == route.destination)
    {
        *existing = route;
    } else {
        ledger.bypass_routes.push(route);
    }
}

/// SIGTERM/SIGINT 请求的优雅关停标志（信号处理器唯一写入，async-signal-safe）。
pub(crate) static TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

/// 信号请求后的管线 teardown 完成标志：退出路径必须等它为真，防止进程
/// 退出与逆序 teardown 竞速造成路由残留。
///
/// W1 统一退出收敛：SIGTERM/SIGINT、Core EOF 与 W2 心跳超时三类
/// 触发共用同一收敛等待（见 [`converge_and_await_teardown`]）。语义是「最近一次
/// 声明的 attempt 的 teardown 已完成」：`apply_tunnel` 声明新 attempt 时会先复位
/// 本标志，防止上一代管线的完成位让收敛等待误读提前通过。
static TERMINATION_TEARDOWN_DONE: AtomicBool = AtomicBool::new(false);

/// 统一退出收敛等待管线 teardown 完成的上界（沿用既有 SIGTERM 路径的 10s）。
const TERMINATION_TEARDOWN_BOUND: Duration = Duration::from_secs(10);

/// 收敛等待的轮询间隔（沿用既有 SIGTERM 路径的 50ms）。
const TERMINATION_TEARDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// SIGTERM/SIGINT 标志轮询间隔（信号处理器只做原子写入，主循环负责轮询）。
const TERMINATION_SIGNAL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// W2 心跳：engine 侧看门狗的超时上界（15s，硬时间界）。core 侧 ticker 每 10s 发
/// 一条 KeepAlive（core 侧常量）；认证握手完成起 15s 未收到即判定失联，触发
/// [`EngineRuntime::request_heartbeat_timeout`]（W1 顶层 select 第三臂接管统一
/// 收敛退出）。对齐 win32 `HEARTBEAT_TIMEOUT_MS` 与 proto KeepAlive 注释。
pub(crate) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15);

/// W2 心跳：看门狗的检查周期（500ms；对齐 win32 `heartbeat_timeout_watcher` 的
/// 默认检查节拍）。
pub(crate) const HEARTBEAT_WATCHDOG_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// 信号处理器：只做原子写入，不触碰锁与分配器。
extern "C" fn termination_request_handler(_signal: libc::c_int) {
    TERMINATION_REQUESTED.store(true, Ordering::SeqCst);
}

/// 安装 SIGTERM/SIGINT 的优雅关停处理器。
///
/// Windows 对齐：服务停止请求（macOS 上 launchd/用户的 SIGTERM）必须走与 Core
/// 失联相同的逆序 teardown 路径，而不是默认终止造成路由/地址残留。主循环的
/// 1 秒 tick 读取 [`TERMINATION_REQUESTED`] 收敛到 teardown。
pub fn install_signal_shutdown() {
    // SAFETY: 处理器只做原子 store；注册调用本身无复杂不变量。
    unsafe {
        libc::signal(
            libc::SIGTERM,
            termination_request_handler as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            termination_request_handler as *const () as libc::sighandler_t,
        );
    }
}

/// Engine 的本地连接协调状态：单活动尝试、权威 offer 持有者与状态广播。
pub(crate) struct EngineRuntime {
    reconnect_fault: Mutex<Option<crate::reconnect_fault::ReconnectFault>>,
    surface: std::sync::RwLock<Arc<dyn crate::platform::surface::PlatformSurface>>,
    attempt: Mutex<Option<ActiveAttempt>>,
    /// 只仲裁本Engine的装配接纳与Stop；不覆盖协议/浏览器状态。
    lifecycle_gate: tokio::sync::Mutex<()>,
    execution_changed: tokio::sync::Notify,
    execution_errors: Mutex<Vec<wire::VpnError>>,
    stop_operations: super::stop_operations::StopOperations,
    live: Mutex<Option<HeldTunnel>>,
    resources: Mutex<ResourceLedger>,
    status_tx: tokio::sync::broadcast::Sender<wire::ConnectStatusEvent>,
    /// 数据面统计注册表与 `StreamStats` 推送端点（W-MVP-13）。
    stats: Arc<StatsPublisher>,
    /// 结构化日志 sink 与 `StreamLogs` 推送端点（MAC-OBS-13 S1）。
    logs: Arc<LogSink>,
    /// 统一退出收敛的 dead flag：三类终止触发（SIGTERM/SIGINT、Core EOF、W2 心跳
    /// 超时）收敛时置位；保持循环的既有消费点据此 break 进逆序 teardown。
    shutdown: Arc<AtomicBool>,
    /// W2 心跳超时的触发槽：唯一触发方是 [`spawn_heartbeat_watchdog`] 的常驻
    /// 看门狗（心跳监视超 15s）；触发后与信号/EOF 同入 W1 统一收敛。
    /// T1 复审整改：Arc 包裹可整体轮换——`Notify` permit 无法清空，跨会话携带
    /// 的 runtime 必须在会话入口换新槽，否则上一会话空闲期看门狗落存的 permit
    /// 会让下一会话 select 瞬时假触发。
    heartbeat_timeout: std::sync::RwLock<Arc<TriggerSlot>>,
    /// W2.5 service 形态的显式停止触发槽（`Shutdown` RPC 唯一生产触发方；
    /// service 会话循环消费：拆隧道 → exit(0)，`KeepAlive={SuccessfulExit:false}`
    /// 下不复活）。会话形态为 None——Shutdown 对其恒 NOT_APPLICABLE。
    /// T1 复审整改：同 heartbeat_timeout 的轮换语义（EOF/Shutdown 竞态残留）。
    explicit_stop: Option<std::sync::RwLock<Arc<TriggerSlot>>>,
    /// W2 心跳监视：最近一次 KeepAlive 到达时刻；零点=认证握手完成时刻
    /// （[`HeartbeatWatch::arm`] 武装），未武装不参与看门狗判定。
    heartbeat: Mutex<HeartbeatWatch>,
    /// T1：core 会话终结策略位（生产默认 [`CoreExitPolicy::Teardown`]；
    /// 连接成功时由 record_pipeline_connected 注入 KeepTunnel）。
    core_exit_policy: std::sync::RwLock<CoreExitPolicy>,
    /// T2：管线身份（Connected 时刻记录；采纳判定与收敛清除）。
    pipeline_identity: Mutex<Option<PipelineIdentity>>,
    /// T2：当前数据面 ifindex 与 Connected 纪元毫秒（采纳事件重放 Connected
    /// 时须沿用真实值，不为采纳伪造新会话起点）。
    live_if_index: std::sync::atomic::AtomicU32,
    connected_at_ms: std::sync::atomic::AtomicI64,
    /// T2：管线状态事件的当前归属 operation_id（采纳时切换——管线后续事件
    /// （如 SessionLost）必须能被新 core 的投影按其 operation_id 接收，
    /// 否则采纳方对后续失联失明）。
    current_operation_id: Mutex<Vec<u8>>,
    /// 仅当前 Cisco 数据面登记；容量 1 通知不携带连接/持久化状态。
    latency_owner: Mutex<Option<LatencyRefreshOwner>>,
    latency_generation: std::sync::atomic::AtomicU64,
    /// 发起用户 uid；无策略信封时用于读取该用户的重连配置。
    owner_uid: u32,
    /// T4（D9）：R1 重拨材料（管线存活期驻留；善后清位。Debug 永不打印）。
    reconnect_materials: Mutex<Option<ReconnectMaterials>>,
}

impl EngineRuntime {
    fn surface(&self) -> Arc<dyn crate::platform::surface::PlatformSurface> {
        match self.surface.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => Arc::new(crate::platform::surface::RealSurface),
        }
    }
    pub(crate) fn new() -> Self {
        Self::with_explicit_stop(None)
    }

    /// oneshot 沿已验证的发起用户创建运行时；提权身份仅用于宿主操作。
    fn new_oneshot_for_owner(owner_uid: u32) -> Self {
        Self::with_owner_and_explicit_stop(owner_uid, None)
    }

    /// W2.5 service 形态构造：显式停止槽在位（`Shutdown` RPC 实装）。
    pub(crate) fn new_service() -> Self {
        Self::with_explicit_stop(Some(TriggerSlot::default()))
    }

    /// Service 会话沿用发起用户 uid，供无策略信封时读取用户重连配置。
    pub(crate) fn new_for_owner(owner_uid: u32) -> Self {
        Self::with_owner_and_explicit_stop(owner_uid, Some(TriggerSlot::default()))
    }

    fn with_explicit_stop(explicit_stop: Option<TriggerSlot>) -> Self {
        // SAFETY: geteuid 只读取当前进程凭证。
        Self::with_owner_and_explicit_stop(unsafe { libc::geteuid() }, explicit_stop)
    }

    fn with_owner_and_explicit_stop(
        owner_uid: u32,
        explicit_stop: Option<TriggerSlot>,
    ) -> Self {
        let (status_tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            surface: std::sync::RwLock::new(Arc::new(crate::platform::surface::RealSurface)),
            reconnect_fault: Mutex::new(None),
            attempt: Mutex::new(None),
            lifecycle_gate: tokio::sync::Mutex::new(()),
            execution_changed: tokio::sync::Notify::new(),
            execution_errors: Mutex::new(Vec::new()),
            stop_operations: super::stop_operations::StopOperations::default(),
            live: Mutex::new(None),
            resources: Mutex::new(ResourceLedger::default()),
            status_tx,
            stats: Arc::new(StatsPublisher::new()),
            logs: Arc::new(LogSink::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            heartbeat_timeout: std::sync::RwLock::new(Arc::new(TriggerSlot::default())),
            explicit_stop: explicit_stop.map(|slot| std::sync::RwLock::new(Arc::new(slot))),
            heartbeat: Mutex::new(HeartbeatWatch::default()),
            core_exit_policy: std::sync::RwLock::new(CoreExitPolicy::default()),
            pipeline_identity: Mutex::new(None),
            live_if_index: std::sync::atomic::AtomicU32::new(0),
            connected_at_ms: std::sync::atomic::AtomicI64::new(0),
            current_operation_id: Mutex::new(Vec::new()),
            latency_owner: Mutex::new(None),
            latency_generation: std::sync::atomic::AtomicU64::new(0),
            reconnect_materials: Mutex::new(None),
            owner_uid,
        }
    }

    /// W2 心跳超时的触发入口（唯一生产调用方是常驻看门狗）。
    ///
    /// 触发后与 SIGTERM/SIGINT、Core EOF 同入统一退出收敛，不新增第二条
    /// teardown 路径。
    fn request_heartbeat_timeout(&self) {
        if let Ok(slot) = self.heartbeat_timeout.read() {
            slot.request();
        }
    }

    /// 等待 W2 心跳超时触发槽（先触发后等待也能完成：notify_one 留有 permit）。
    /// 等待方克隆 Arc 后 await，不持有读锁跨 await。
    pub(crate) async fn wait_heartbeat_timeout(&self) {
        let slot = match self.heartbeat_timeout.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => return,
        };
        slot.wait().await;
    }

    /// T1 复审整改：会话入口轮换本会话生命周期触发槽（心跳超时/显式停止）。
    /// `Notify` 的 permit 无法清空，`requested` 电平无复位路径——唯一可靠的
    /// 跨会话零残留手段是整体换新槽（旧槽随旧等待方消亡）。
    pub(crate) fn rotate_session_trigger_slots(&self) {
        if let Ok(mut slot) = self.heartbeat_timeout.write() {
            *slot = Arc::new(TriggerSlot::default());
        }
        if let Some(explicit_stop) = &self.explicit_stop
            && let Ok(mut slot) = explicit_stop.write()
        {
            *slot = Arc::new(TriggerSlot::default());
        }
    }

    /// W2.5：武装本会话心跳（uid 门准入完成=心跳零点；service 会话循环入口调用）。
    pub(crate) fn arm_session_heartbeat(&self) {
        self.heartbeat.lock().expect("heartbeat watch lock").arm();
    }

    /// W2.5：结构化日志 sink 只读句柄（service 会话循环的收敛诊断发布点）。
    pub(crate) fn logs(&self) -> &Arc<LogSink> {
        &self.logs
    }

    /// T1：当前 core 会话终结策略（生产默认 Teardown）。
    pub(crate) fn core_exit_policy(&self) -> CoreExitPolicy {
        self.core_exit_policy
            .read()
            .map_or(CoreExitPolicy::Teardown, |guard| *guard)
    }

    /// T1：注入 core 会话终结策略（生产调用方由后续任务接线；测试注入 KeepTunnel
    /// 驱动 keep 分支）。
    pub(crate) fn set_core_exit_policy(&self, policy: CoreExitPolicy) {
        if let Ok(mut guard) = self.core_exit_policy.write() {
            *guard = policy;
        }
    }

    /// 无策略信封时，重连配置沿用发起用户的数据域。
    pub(crate) fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    /// T4（D9）：驻留 R1 材料（apply 受理时捕获；管线存活期可取）。
    pub(crate) fn store_reconnect_materials(&self, materials: ReconnectMaterials) {
        if let Ok(mut slot) = self.reconnect_materials.lock() {
            *slot = Some(materials);
        }
    }

    /// T4（D9）：取 R1 材料克隆（无驻留=None——不重拨）。
    pub(crate) fn reconnect_materials(&self) -> Option<ReconnectMaterials> {
        self.reconnect_materials
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// T2：管线是否处于持有态（live 标记在场）。
    pub(crate) fn pipeline_is_held(&self) -> bool {
        self.live.lock().map_or(false, |slot| slot.is_some())
    }

    /// T2：读取管线身份克隆（未持有时 None）。
    pub(crate) fn pipeline_identity(&self) -> Option<PipelineIdentity> {
        self.pipeline_identity
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// T2：Connected 时刻的管线事实记录（身份/ifindex/纪元/op 归属/keep 策略）。
    /// 由 run_tunnel_pipeline 在数据面真实 Connected 后调用——生产侧 keep 策略
    /// 的唯一注入点（连接成功即 keep；用户级设置项留 T8 接线）。
    pub(crate) fn record_pipeline_connected(
        &self,
        identity: PipelineIdentity,
        if_index: u32,
        connected_at_ms: i64,
        operation_id: Vec<u8>,
    ) {
        if let Ok(mut slot) = self.pipeline_identity.lock() {
            *slot = Some(identity);
        }
        self.live_if_index
            .store(if_index, std::sync::atomic::Ordering::Relaxed);
        self.connected_at_ms
            .store(connected_at_ms, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut slot) = self.current_operation_id.lock() {
            *slot = operation_id;
        }
        // T2（复审必改 3）：keep 策略只对 service 形态注入（explicit_stop 槽在位
        // =service 形态的既有判别面）。oneshot 形态无采纳端点，EOF 必须维持
        // "收敛+退出"红测行为；oneshot keep 属端点统一（T7）后的灰度项。
        if self.explicit_stop.is_some() {
            self.set_core_exit_policy(CoreExitPolicy::KeepTunnel);
        }
    }

    /// T2：采纳时切换管线事件归属（身份/管线事实不变）。
    pub(crate) fn adopt_operation(&self, operation_id: Vec<u8>) {
        if let Ok(mut slot) = self.current_operation_id.lock() {
            *slot = operation_id;
        }
    }

    /// T2：当前事件归属的 operation_id 克隆（空 = 无归属）。
    pub(crate) fn current_operation_id(&self) -> Vec<u8> {
        self.current_operation_id
            .lock()
            .map_or_else(|_| Vec::new(), |slot| slot.clone())
    }

    /// 每次派发读取当前采纳归属；尚未归属时才使用任务最初id。
    fn operation_for_event(&self, initial: &[u8]) -> Vec<u8> {
        let current = self.current_operation_id();
        if current.is_empty() {
            initial.to_vec()
        } else {
            current
        }
    }

    fn consume_reconnect_fault(&self) -> bool {
        let operation = self.current_operation_id();
        let consumed = self.reconnect_fault.lock().ok()
            .and_then(|mut fault| fault.as_mut().map(|fault| fault.poll(&operation)))
            .unwrap_or(false);
        if consumed {
            self.logs.publish("warn", "packet", "RECONNECT_FAULT_CONSUMED",
                "外部验收标记已消费，当前连接的传输任务将退出",
                &crate::session_diagnostics::identity(&operation));
        }
        consumed
    }

    /// T2：采纳事件重放所需的管线事实（ifindex/纪元）。
    pub(crate) fn connected_facts(&self) -> (u32, i64) {
        (
            self.live_if_index
                .load(std::sync::atomic::Ordering::Relaxed),
            self.connected_at_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// T2：管线收敛后的身份清位、事件归属清空与策略复位（teardown 后的会话
    /// 回到默认策略；收敛空场景调用无害）。归属必须一并清空——复用 runtime 的
    /// 下一代管线在 record 前发布事件时，读到上一代归属会让新 core 过滤掉
    /// 全部阶段事件（复审必改 4）。
    pub(crate) fn clear_pipeline_facts(&self) {
        self.remove_latency_owner(None);
        if let Ok(mut slot) = self.pipeline_identity.lock() {
            *slot = None;
        }
        self.live_if_index
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.connected_at_ms
            .store(0, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut slot) = self.current_operation_id.lock() {
            *slot = Vec::new();
        }
        if let Ok(mut slot) = self.reconnect_materials.lock() {
            *slot = None;
        }
        self.set_core_exit_policy(CoreExitPolicy::Teardown);
    }

    /// W2.5 显式停止请求（`Shutdown` RPC，service 形态实装；会话形态无槽不动作）。
    pub(crate) fn request_explicit_stop(&self) {
        if let Some(explicit_stop) = &self.explicit_stop
            && let Ok(slot) = explicit_stop.read()
        {
            slot.request();
        }
    }

    /// 显式停止是否已被请求（诊断/测试读取）。
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "电平读取当前仅测试断言使用；生产触发后即收敛退出")
    )]
    pub(crate) fn explicit_stop_requested(&self) -> bool {
        self.explicit_stop
            .as_ref()
            .and_then(|slot| slot.read().ok())
            .is_some_and(|slot| slot.is_requested())
    }

    /// 等待显式停止触发槽；会话形态（无槽）恒不完成。
    pub(crate) async fn wait_explicit_stop(&self) {
        let Some(explicit_stop) = &self.explicit_stop else {
            std::future::pending::<()>().await;
            return;
        };
        let slot = match explicit_stop.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(_) => return,
        };
        slot.wait().await;
    }

    /// 登录前读取本次物理出口事实后，确保 VPN 服务端的精确 `/32` 专用路由走当前网关。
    ///
    /// 已由本 Engine 创建/迁移的路由才记入本地所有权账本；外部已有且完全一致的条目
    /// 可以复用，但 Stop 不会删除它。
    fn ensure_control_route(
        &self,
        egress: crate::egress::physical_route::PhysicalEgress,
        server: Ipv4Addr,
    ) -> Result<ControlRouteUpdate, crate::platform::PlatformError> {
        let surface = self.surface();
        let desired = AppliedRoute {
            destination: server,
            prefix: 32,
            gateway: egress.gateway,
            ifindex: egress.ifindex,
        };
        let mut resources = self.resources.lock().expect("resource ledger lock");
        if let Some(previous) = resources.control_route
            && previous.destination != desired.destination
        {
            surface.route_delete(&previous)?;
            resources.control_route = None;
        }
        let observed = surface.route_read_exact(desired.destination, desired.prefix)?;
        let update = match observed {
            Some(actual) if actual == desired => ControlRouteUpdate::Reused,
            Some(_) => {
                surface.route_change(&desired)?;
                // 原路由操作成功即登记本次所有权，供 Stop/退出清理。
                resources.control_route = Some(desired);
                ControlRouteUpdate::Changed
            }
            None => match surface.route_add(&desired) {
                Ok(()) => {
                    resources.control_route = Some(desired);
                    ControlRouteUpdate::Added
                }
                Err(crate::platform::PlatformError::Route("ACK", error))
                    if error.raw_os_error() == Some(libc::EEXIST) =>
                {
                    match surface.route_read_exact(desired.destination, desired.prefix)? {
                        Some(actual) if actual == desired => ControlRouteUpdate::Reused,
                        Some(_) => {
                            surface.route_change(&desired)?;
                            resources.control_route = Some(desired);
                            ControlRouteUpdate::Changed
                        }
                        None => return Err(crate::platform::PlatformError::Route("RACE", error)),
                    }
                }
                Err(error) => return Err(error),
            },
        };
        Ok(update)
    }

    fn reconcile_bypass_routes(
        &self,
        egress: crate::egress::physical_route::PhysicalEgress,
        destinations: &[Ipv4Addr],
    ) -> Result<Vec<ControlRouteUpdate>, crate::platform::PlatformError> {
        let surface = self.surface();
        // 去重保序（防御性：组装层已去重）。
        let mut desired: Vec<Ipv4Addr> = Vec::new();
        for destination in destinations {
            if !desired.contains(destination) {
                desired.push(*destination);
            }
        }
        let mut resources = self.resources.lock().expect("resource ledger lock");
        // 期望集之外的旧账目行先删除（单网关形态=既有「目标变化即迁移」语义）。
        // 复审 A-1 整改：删除失败不丢账——失败行与未处理行保留在账本（供 Stop/
        // complete_resource_cleanup 重试删除），对齐 pre-M4 单槽「删除失败保留
        // 槽位」语义；不在期望集但删除失败的行留账不视为已迁移。
        let mut pending = std::mem::take(&mut resources.bypass_routes);
        let mut ledger: Vec<crate::platform::route::AppliedRoute> = Vec::new();
        while let Some(route) = pending.pop() {
            if desired.contains(&route.destination) {
                ledger.push(route);
            } else if let Err(error) = surface.route_delete(&route) {
                // 失败行回账 + 未处理行（pending 余量）全部回账后返回错误。
                ledger.push(route);
                ledger.extend(pending);
                resources.bypass_routes = ledger;
                return Err(error);
            }
        }
        resources.bypass_routes = ledger;
        let mut updates = Vec::new();
        for destination in &desired {
            let wanted = AppliedRoute {
                destination: *destination,
                prefix: 32,
                gateway: egress.gateway,
                ifindex: egress.ifindex,
            };
            let observed = surface.route_read_exact(wanted.destination, wanted.prefix)?;
            let update = match observed {
                Some(actual) if actual == wanted => ControlRouteUpdate::Reused,
                Some(_) => {
                    surface.route_change(&wanted)?;
                    // 原路由操作成功即登记本次所有权，供 Stop/退出清理。
                    upsert_bypass_ledger(&mut resources, wanted);
                    ControlRouteUpdate::Changed
                }
                None => match surface.route_add(&wanted) {
                    Ok(()) => {
                        upsert_bypass_ledger(&mut resources, wanted);
                        ControlRouteUpdate::Added
                    }
                    Err(crate::platform::PlatformError::Route("ACK", error))
                        if error.raw_os_error() == Some(libc::EEXIST) =>
                    {
                        match surface.route_read_exact(wanted.destination, wanted.prefix)? {
                            Some(actual) if actual == wanted => ControlRouteUpdate::Reused,
                            Some(_) => {
                                surface.route_change(&wanted)?;
                                upsert_bypass_ledger(&mut resources, wanted);
                                ControlRouteUpdate::Changed
                            }
                            None => {
                                return Err(crate::platform::PlatformError::Route("RACE", error));
                            }
                        }
                    }
                    Err(error) => return Err(error),
                },
            };
            updates.push(update);
        }
        Ok(updates)
    }
    fn take_or_acquire_device(&self) -> Result<Utun, crate::platform::PlatformError> {
        if let Some(device) = self
            .resources
            .lock()
            .expect("resource ledger lock")
            .retained_device
            .take()
        {
            return Ok(device);
        }
        Utun::acquire()
    }

    fn retain_device(&self, device: Utun) {
        let mut resources = self.resources.lock().expect("resource ledger lock");
        debug_assert!(
            resources.retained_device.is_none(),
            "only one retained utun"
        );
        resources.retained_device = Some(device);
    }

    fn remember_route_cleanup_failure(&self, route: AppliedRoute) {
        let mut resources = self.resources.lock().expect("resource ledger lock");
        if !resources.pending_routes.contains(&route) {
            resources.pending_routes.push(route);
        }
    }

    /// 完整清理所有仍由本 Engine 持有的跨会话资源。
    ///
    /// Cisco DNS 来自本次真实 offer；安装条目立即交现有账本持有。
    fn install_cisco_resolvers(&self, domains: &[String], servers: &[Ipv4Addr]) -> Result<(), crate::campus_resolver::ResolverError> {
        let nameservers = servers.iter().map(ToString::to_string).collect::<Vec<_>>();
        self.logs.publish("info", "cstp", "CISCO_DNS_PARAMETERS", "Cisco 隧道分域 DNS 参数", &[
            ("domains", domains.join(",")), ("dns_servers", nameservers.join(",")),
        ]);
        if domains.is_empty() { return Ok(()); }
        let (installed, error) = self.surface().install_resolvers(domains, &nameservers, &self.logs);
        self.resources.lock().expect("resource ledger lock").resolvers.extend(installed);
        if let Some(error) = error {
            self.logs.publish("error", "platform", "RESOLVER_STEP_FAILED",
                format!("campus resolver step failed: {error}"), &[("reason", error.reason().to_owned())]);
            return Err(error);
        }
        Ok(())
    }

    /// 既有 resolver 清理在正常 Cisco Stop 与完整账本清理中共用。
    fn cleanup_resolvers(&self) -> bool {
        let mut resources = self.resources.lock().expect("resource ledger lock");
        let mut retry = Vec::new();
        for entry in resources.resolvers.drain(..) {
            if self.surface().remove_resolvers(std::slice::from_ref(&entry), &self.logs) > 0 {
                retry.push(entry);
            }
        }
        resources.resolvers = retry;
        resources.resolvers.is_empty()
    }

    /// 每一项独立尝试；失败条目保持在账本中，utun FD 始终释放，使下一次 Stop/退出可
    /// 重试路由而不会为了一个失败无限保留设备。
    fn complete_resource_cleanup(&self) -> Result<(), crate::platform::PlatformError> {
        let proxy_clean = self.cleanup_proxy_policy();
        let resolvers_clean = self.cleanup_resolvers();
        let mut resources = self.resources.lock().expect("resource ledger lock");
        let mut first_error = if proxy_clean { None } else { Some(crate::platform::PlatformError::Readback("PROXY_CLEANUP")) };
        if !resolvers_clean && first_error.is_none() {
            first_error = Some(crate::platform::PlatformError::Readback("RESOLVER_CLEANUP"));
        }
        let mut retry = Vec::new();
        for route in resources.bypass_routes.drain(..).rev() {
            if let Err(error) = self.surface().route_delete(&route) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                retry.push(route);
            }
        }
        for route in resources.pending_routes.drain(..) {
            if let Err(error) = self.surface().route_delete(&route) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                retry.push(route);
            }
        }
        resources.pending_routes = retry;
        if let Some(route) = resources.control_route {
            match platform_route::delete(&route) {
                Ok(()) => resources.control_route = None,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        // Drop even if route removal failed: device ownership is independent and must not leak.
        drop(resources.retained_device.take());
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn has_cleanup_obligations(&self) -> bool {
        let resources = self.resources.lock().expect("resource ledger lock");
        !self.pipeline_is_held()
            && (!resources.pending_routes.is_empty()
                || !resources.bypass_routes.is_empty()
                || !resources.resolvers.is_empty()
                || resources.proxy_policy.is_some()
                || resources.control_route.is_some()
                || resources.retained_device.is_some())
    }

    fn pipeline_activity_cleared(&self) -> bool {
        let attempt_active = self.attempt.lock().map_or(true, |slot| slot.is_some());
        let live = self.live.lock().map_or(true, |slot| slot.is_some());
        !attempt_active && !live
    }

    fn apply_cisco_proxy_policy(&self, routes: &[AppliedRoute], identity: &PipelineIdentity) {
        self.apply_proxy_policy(routes, &identity.resolver_domains);
    }

    fn apply_proxy_policy(&self, routes: &[AppliedRoute], domains: &[String]) {
        if !self.cleanup_proxy_policy() { return; }
        let routes: Vec<String> = routes.iter().map(|route| format!("{}/{}", route.destination, route.prefix)).collect();
        let (owner, result) = self.surface().apply_proxy_policy(&routes, domains);
        self.resources.lock().expect("resource ledger lock").proxy_policy = owner;
        match result {
            Ok(status) => self.logs.publish("info", "platform", "PROXY_POLICY_RESULT", format!("系统代理校园例外：{status:?}"), &[]),
            Err(error) => self.logs.publish("warn", "platform", "PROXY_POLICY_FAILED", format!("校园 IP 隧道仍可用，系统代理例外应用失败：{error}"), &[]),
        }
    }

    fn cleanup_proxy_policy(&self) -> bool {
        let mut resources = self.resources.lock().expect("resource ledger lock");
        let Some(owner) = resources.proxy_policy.as_mut() else { return true; };
        match self.surface().cleanup_proxy_policy(owner) {
            Ok(status) => {
                self.logs.publish("info", "platform", "PROXY_POLICY_CLEANUP", format!("系统代理例外清理：{status:?}"), &[]);
                resources.proxy_policy = None;
                true
            }
            Err(error) => {
                self.logs.publish("warn", "platform", "PROXY_POLICY_CLEANUP_FAILED", format!("系统代理例外清理失败，保留清理记录：{error}"), &[]);
                false
            }
        }
    }

    fn observed_proxy_policy(&self) -> Option<wire::SystemProxyDetection> {
        let mut resources = self.resources.lock().ok()?;
        let owner = resources.proxy_policy.as_mut()?;
        let mode = self.surface().proxy_policy_effective(owner).ok()??;
        Some(wire::SystemProxyDetection {
            mode: mode.into(), endpoint_count: 0, bypass_merged: true, topology: "t3".into(),
        })
    }

    fn publish_r1_wait(&self, attempt: u32, delay: Duration) {
        let _ = self.status_tx.send(wire::ConnectStatusEvent {
            operation_id: self.current_operation_id(),
            connect_phase: wire::ConnectPhase::ConnectingControl as i32,
            coarse_phase: wire::StatsPhase::Connecting as i32,
            reconnect_state: Some(wire::ReconnectState {
                attempt,
                next_retry_at_ms: now_epoch_ms()
                    .saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX)),
                reason: "tunnel_lost".into(),
            }),
            ..Default::default()
        });
    }

    async fn reconnect_delay(&self, cancel: &AtomicBool, delay: Duration) {
        let cancelled = async {
            loop {
                let changed = self.execution_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                changed.await;
            }
        };
        tokio::select! { () = tokio::time::sleep(delay) => {}, () = cancelled => {} }
    }

    fn request_pipeline_cancel(&self) {
        if let Ok(slot) = self.attempt.lock()
            && let Some(attempt) = slot.as_ref()
        {
            attempt.cancel.store(true, Ordering::SeqCst);
        }
        self.remove_latency_owner(None);
        self.execution_changed.notify_waiters();
    }

    /// StopTunnel 的完成边界：先等待本地 pipeline 停止并释放它临时拥有的资源，再清
    /// ledger 中的空闲 utun/控制路由/失败重试条目。
    async fn stop_and_wait_for_cleanup(&self) -> Result<(), crate::platform::PlatformError> {
        self.request_pipeline_cancel();
        let wait = async {
            loop {
                let changed = self.execution_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.pipeline_activity_cleared() {
                    break;
                }
                changed.await;
            }
        };
        if tokio::time::timeout(TERMINATION_TEARDOWN_BOUND, wait)
            .await
            .is_err()
        {
            return Err(crate::platform::PlatformError::Route(
                "CLEANUP_WAIT",
                io::Error::from_raw_os_error(libc::ETIMEDOUT),
            ));
        }
        let outcome = self.complete_resource_cleanup();
        // T2（复审必改 5）：显式 Stop 的成功路径同样清管线事实与策略——否则
        // KeepTunnel 残留让 Stop 后的 EOF 继续走 keep 分支携带空 runtime。
        if outcome.is_ok() {
            // S2：显式停止=善后已验证（清理链走完）→ 行使痕迹删除权；未验证
            // 路径不删——残留痕即下次连接的预检触发信号（双亡格兜底）。
            crate::connect_gate::process_gate().finish_verified_teardown(self.owner_uid);
            self.clear_pipeline_facts();
        }
        outcome
    }
}

/// 当前实际数据面探测归属；cancel 沿用完整执行 owner，不重造连接生命周期。
struct LatencyRefreshOwner {
    generation: u64,
    cancel: Arc<AtomicBool>,
    probe: Arc<Mutex<ProbeState>>,
    sender: tokio::sync::mpsc::Sender<u64>,
}

/// 退出、早退或 panic 时摘除本代通知与 tap；旧 guard 不摘除后来的 owner。
struct LatencyRefreshRegistration<'a> {
    runtime: &'a EngineRuntime,
    generation: u64,
    receiver: tokio::sync::mpsc::Receiver<u64>,
}

impl Drop for LatencyRefreshRegistration<'_> {
    fn drop(&mut self) {
        self.runtime.remove_latency_owner(Some(self.generation));
    }
}

impl EngineRuntime {
    fn install_latency_owner<'a>(
        &'a self,
        probe: Arc<Mutex<ProbeState>>,
        cancel: Arc<AtomicBool>,
    ) -> LatencyRefreshRegistration<'a> {
        let generation = self.latency_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut slot = self.latency_owner.lock().expect("latency owner lock");
        if let Some(previous) = slot.take() {
            previous
                .probe
                .lock()
                .expect("latency probe lock")
                .deactivate();
        }
        if cancel.load(Ordering::SeqCst) {
            probe.lock().expect("latency probe lock").deactivate();
        } else {
            *slot = Some(LatencyRefreshOwner {
                generation,
                cancel,
                probe,
                sender,
            });
        }
        LatencyRefreshRegistration {
            runtime: self,
            generation,
            receiver,
        }
    }

    fn remove_latency_owner(&self, generation: Option<u64>) {
        let mut slot = self.latency_owner.lock().expect("latency owner lock");
        if slot
            .as_ref()
            .is_some_and(|owner| generation.is_none_or(|value| value == owner.generation))
        {
            let owner = slot.take().expect("latency owner present");
            owner.probe.lock().expect("latency probe lock").deactivate();
        }
    }

    fn accept_latency_refresh(&self, operation_id: &[u8]) -> Result<(), Status> {
        let reject = || Status::failed_precondition("DARWIN_LATENCY_OWNER_NOT_ACTIVE");
        if operation_id.len() != 16
            || operation_id != self.current_operation_id()
            || !self.pipeline_is_held()
        {
            return Err(reject());
        }
        if self
            .pipeline_identity()
            .is_some_and(|identity| identity.protocol == ProtocolKind::Atrust)
        {
            return Err(Status::unimplemented("当前 aTrust 协议不支持时延刷新"));
        }
        let attempt = self.attempt.lock().expect("attempt slot lock");
        let slot = self.latency_owner.lock().expect("latency owner lock");
        let owner = slot.as_ref().ok_or_else(reject)?;
        if owner.cancel.load(Ordering::SeqCst)
            || self.shutdown.load(Ordering::SeqCst)
            || !attempt
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(&attempt.cancel, &owner.cancel))
        {
            return Err(reject());
        }
        match owner.sender.try_send(owner.generation) {
            // 已有一个本代通知时合并；不是声称已发包或收到 RTT。
            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                if owner
                    .probe
                    .lock()
                    .expect("latency probe lock")
                    .queue_refresh()
                {
                    Ok(())
                } else {
                    Err(reject())
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(reject()),
        }
    }

    /// 通知消费与 Stop 摘除共享短锁；旧代通知不能作用于新 probe。
    fn process_latency_refresh(
        &self,
        generation: u64,
        now: std::time::Instant,
        send: &mut dyn FnMut(Vec<u8>),
    ) {
        let slot = self.latency_owner.lock().expect("latency owner lock");
        if let Some(owner) = slot.as_ref()
            && owner.generation == generation
            && !owner.cancel.load(Ordering::SeqCst)
            && !self.shutdown.load(Ordering::SeqCst)
        {
            owner
                .probe
                .lock()
                .expect("latency probe lock")
                .tick(now, send);
        }
    }
}
/// W2 心跳监视：`last` = 最近一次心跳到达时刻（或武装零点）；`None` = 未武装。
///
/// 与 win32 `HeartbeatWatch` 的关键差异（权威规范 §二 oneshot.2）：**零点=认证握手
/// 完成时刻**，不是进程启动——握手完成（对端已验证）时 [`Self::arm`] 武装并置
/// 零点；未武装（无已认证活跃会话）看门狗不判定。时间源用 tokio 时钟
/// （`start_paused` 单测注入虚拟时钟）。
#[derive(Debug, Default)]
struct HeartbeatWatch {
    /// 最近一次 KeepAlive 到达时刻；未收到过心跳时为武装零点。
    last: Option<tokio::time::Instant>,
}

impl HeartbeatWatch {
    /// 武装（认证握手完成）：零点=此刻。单连接 accept 模型下握手先于任何 RPC，
    /// arm 与 handler 首条 KeepAlive 之间的微小竞态无害（touch 同样落在握手后）。
    fn arm(&mut self) {
        self.last = Some(tokio::time::Instant::now());
    }

    /// 刷新最近心跳时刻（`KeepAlive` handler 语义：touch）。
    fn touch(&mut self) {
        self.last = Some(tokio::time::Instant::now());
    }

    /// 距最近一次心跳（未收到过心跳时=武装零点）的时长；未武装返回 `None`
    /// （看门狗对无会话状态不判定）。
    fn elapsed(&self) -> Option<Duration> {
        Some(tokio::time::Instant::now() - self.last?)
    }
}

/// W2 心跳超时的触发槽（可 await 信号 + 电平标志）。
///
/// 唯一生产触发方是 [`spawn_heartbeat_watchdog`] 的常驻看门狗；触发后令顶层
/// select 与信号/EOF 走同一收敛序列。W2.5 起 service 形态的显式停止
/// （`Shutdown` RPC）复用同一机制（[`EngineRuntime::explicit_stop`]）。
#[derive(Debug, Default)]
struct TriggerSlot {
    requested: AtomicBool,
    notify: tokio::sync::Notify,
}

impl TriggerSlot {
    /// 触发（幂等；电平置位 + 唤醒一个等待者）。
    fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// 是否已触发（电平语义，供看门狗/诊断读取）。
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "电平读取当前仅测试断言使用；生产触发后即收敛")
    )]
    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// 等待触发；notify_one 在等待前后注册都能完成，不丢触发。
    async fn wait(&self) {
        self.notify.notified().await;
    }
}

/// 统一退出收敛的触发源（仅诊断标注；三类触发共用同一收敛序列）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminationTrigger {
    /// SIGTERM/SIGINT（launchd/用户停止；由信号处理器置标志，主循环轮询）。
    PosixSignals,
    /// Core 关闭已认证连接（EOF/失联，经 handoff_with_close_signal）。
    CoreClosed,
    /// W2 心跳超时（常驻看门狗触发：认证握手完成起 15s 未收到 KeepAlive）。
    HeartbeatTimeout,
}

/// T1：该终止触发是否进入"保持持有"分支（不收敛 teardown）。
///
/// 只有 core 会话终结类触发（EOF/心跳超时）受策略管辖；显式信号始终收敛
/// （显式停止是用户/launchd 意图，不受 keep 策略豁免）。
pub(crate) fn core_exit_keeps_tunnel(trigger: TerminationTrigger, policy: CoreExitPolicy) -> bool {
    matches!(
        trigger,
        TerminationTrigger::CoreClosed | TerminationTrigger::HeartbeatTimeout
    ) && policy == CoreExitPolicy::KeepTunnel
}

const OBSERVE_ONLY_MESSAGE: &str = "DARWIN_ENGINE_OBSERVE_ONLY";
/// T2 采纳裁决的稳定码：身份不匹配（新认证材料）→ 由 core 显式 Stop 后重建。
const ADOPT_MISMATCH_MESSAGE: &str = "DARWIN_ENGINE_ADOPT_MISMATCH";
/// T4：引擎侧 R1 预算（v6 退避 2s→30s 封顶的 darwin 固定档；信封线程化挂账）。
const R1_MAX_ATTEMPTS: u32 = 5;

fn r1_backoff_delay(attempt: u32) -> Duration {
    let base_secs: u64 = [2, 4, 8, 16, 30]
        .into_iter()
        .nth(attempt.saturating_sub(1).min(4) as usize)
        .unwrap_or(30);
    Duration::from_secs(base_secs)
}

/// Cisco R1 重拨的实际建立与退避接缝；预算属于原执行 owner，认证终态原样返回。
async fn cisco_r1_establish<T, F, Fut>(
    runtime: &EngineRuntime,
    cancel: &AtomicBool,
    attempt: &mut u32,
    mut establish: F,
) -> Result<T, crate::protocol::ConnectError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, crate::protocol::ConnectError>>,
{
    loop {
        if cancel.load(Ordering::Acquire) { return Err(crate::protocol::ConnectError::Cancelled); }
        match establish().await {
            Ok(session) => return Ok(session),
            Err(error) => {
                if cancel.load(Ordering::Acquire) || matches!(error, crate::protocol::ConnectError::Cancelled) {
                    return Err(crate::protocol::ConnectError::Cancelled);
                }
                runtime.logs.publish("error", "protocol", "R1_RECONNECT_FAILED",
                    format!("engine-side reconnect establish failed: {error}"), &[("attempt", attempt.to_string())]);
                if error.requires_new_authentication() || *attempt >= R1_MAX_ATTEMPTS { return Err(error); }
                *attempt += 1;
                let delay = r1_backoff_delay(*attempt);
                runtime.publish_r1_wait(*attempt, delay);
                runtime.reconnect_delay(cancel, delay).await;
            }
        }
    }
}

/// Cisco R1 的实际协议终态发布；保持当前采纳归属、typed阶段和原始错误。
fn finish_cisco_protocol_failure(runtime: &EngineRuntime, initial_operation: &[u8], error: &crate::protocol::ConnectError) {
    if let Err(cleanup) = runtime.complete_resource_cleanup() {
        runtime.logs.publish("error", "engine", "R1_TERMINAL_CLEANUP_INCOMPLETE", format!("R1 terminal cleanup incomplete: {cleanup}"), &[]);
    }
    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
        operation_id: runtime.operation_for_event(initial_operation),
        connect_phase: error.failure_phase() as i32,
        coarse_phase: wire::StatsPhase::Failed as i32,
        error: Some(error.to_vpn_error()),
        ..Default::default()
    });
}

/// 仅兼容未携带策略信封的旧 Core：受理时读取一次，之后不读用户配置。
fn owner_auto_reconnect_enabled(owner_uid: u32) -> bool {
    // SAFETY: getpwuid 只读取系统用户数据库；返回结构在拷贝字段后立即释放。
    let home = unsafe {
        let entry = libc::getpwuid(owner_uid);
        if entry.is_null() {
            return false;
        }
        let dir = (*entry).pw_dir;
        if dir.is_null() {
            return false;
        }
        std::ffi::CStr::from_ptr(dir).to_string_lossy().into_owned()
    };
    let Ok(payload) = std::fs::read_to_string(std::path::Path::new(&home).join(".exv/config.json"))
    else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&payload)
        .ok()
        .and_then(|config| {
            config
                .get("auto_reconnect")
                .and_then(|value| value.as_bool())
        })
        .unwrap_or(false)
}
/// T2：管线持有但身份记录缺失（收敛竞态窗口）→ 保守拒绝。
const ADOPT_NEW_AUTH_MESSAGE: &str = "DARWIN_ENGINE_ADOPT_NEW_AUTH";
const ADOPT_IDENTITY_MISSING_MESSAGE: &str = "DARWIN_ENGINE_ADOPT_IDENTITY_MISSING";
const APPLY_INVALID_MESSAGE: &str = "DARWIN_ENGINE_APPLY_INVALID";
/// 当前 Unix epoch 毫秒（诊断与会话起点字段；取时失败返回 0）。
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(0)
        })
}

/// 距 `started_ms` 的流逝毫秒（会话时长的诊断字段）。
fn elapsed_since_ms(started_ms: i64) -> i64 {
    now_epoch_ms().saturating_sub(started_ms)
}

pub(crate) const ATTEMPT_ACTIVE_MESSAGE: &str = "DARWIN_ENGINE_ATTEMPT_ACTIVE";
const STATUS_LAGGED_MESSAGE: &str = "DARWIN_ENGINE_STATUS_LAGGED";

#[derive(Debug)]
pub enum EngineBootstrapError {
    Arguments,
    Credentials,
    RuntimeDir(RuntimeDirError),
    Ticket(TicketError),
    Listener,
    Pipe,
    Server,
}

impl fmt::Display for EngineBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Arguments => "Darwin Engine bootstrap arguments are invalid",
            Self::Credentials => "Darwin Engine bootstrap credentials are invalid",
            Self::RuntimeDir(_) => "Darwin Engine runtime directory is invalid",
            Self::Ticket(_) => "Darwin Engine ticket is invalid",
            Self::Listener => "Darwin Engine listener bootstrap failed",
            Self::Pipe => "Darwin Engine bootstrap pipe failed",
            Self::Server => "Darwin Engine Observe server failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for EngineBootstrapError {}

#[derive(Debug)]
struct EngineArgs {
    runtime_dir: PathBuf,
    owner_uid: u32,
    core_pid: u32,
}

impl EngineArgs {
    fn from_process() -> Result<Self, EngineBootstrapError> {
        let values: Vec<_> = env::args_os().skip(1).collect();
        if values.len() != 6
            || values[0] != "--runtime-dir"
            || values[2] != "--owner-uid"
            || values[4] != "--core-pid"
        {
            return Err(EngineBootstrapError::Arguments);
        }
        let owner_uid = values[3]
            .to_str()
            .and_then(|value| value.parse().ok())
            .filter(|value: &u32| *value != 0)
            .ok_or(EngineBootstrapError::Arguments)?;
        let core_pid = values[5]
            .to_str()
            .and_then(|value| value.parse().ok())
            .filter(|value: &u32| *value > 0)
            .ok_or(EngineBootstrapError::Arguments)?;
        Ok(Self {
            runtime_dir: PathBuf::from(&values[1]),
            owner_uid,
            core_pid,
        })
    }
}

/// 从固定 argv 与 Authorization communications pipe 运行 root bootstrap。
///
/// W2.5 起 argv 形态分派：首参 `--service` 进入常驻 service 形态（见
/// [`crate::service_runtime`]）；否则为既有 per-core-session 会话形态。
///
/// # Errors
///
/// argv、实际 uid/euid、ticket、root listener、pipe record 或控制服务任一
/// 阶段失败时返回稳定的 [`EngineBootstrapError`]，且不创建 utun 或平台网络资源。
pub fn run_from_process() -> Result<(), EngineBootstrapError> {
    // 诊断：Engine stderr 由服务代理置空，panic 消息落盘供开发期读取。
    std::panic::set_hook(Box::new(|info| {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/private/tmp/exv-engine-panic.log")
        {
            use std::io::Write as _;
            let _ = writeln!(file, "{info}");
        }
    }));
    let values: Vec<_> = env::args_os().skip(1).collect();
    if values
        .first()
        .is_some_and(|value| value == std::ffi::OsStr::new("--service"))
    {
        return crate::service_runtime::run_service_from_process(&values);
    }
    let args = EngineArgs::from_process()?;
    // SAFETY: these calls only read this process credentials.
    let (uid, euid) = unsafe { (libc::getuid(), libc::geteuid()) };
    if uid != args.owner_uid || euid != 0 {
        return Err(EngineBootstrapError::Credentials);
    }
    let runtime_dir = RuntimeDir::open_existing_for_uid(&args.runtime_dir, args.owner_uid)
        .map_err(EngineBootstrapError::RuntimeDir)?;
    let (runtime_dir, ticket) =
        open_and_validate_engine_ticket(runtime_dir, args.owner_uid, args.core_pid)?;

    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| EngineBootstrapError::Server)?;
    tokio_runtime.block_on(run_observe_bootstrap(
        runtime_dir,
        args,
        ticket.into_auth_key(),
    ))
}

/// 打开 runtime 目录后的 ticket 消费与 owner/core_pid 校验。
///
/// 本函数是 L2 修复的落点：消费成功但校验失败（或消费本身失败）时，**失败返回前**
/// 必须做 guarded ticket 清理 + `cleanup_empty()`——提前失败不等于可泄漏。清理失败
/// 只写 stderr 并保留原始 typed 错误，不得用 `remove_dir_all` 扩大删除面。
///
/// # Errors
///
/// 消费或校验失败时返回 [`EngineBootstrapError::Ticket`]。
fn open_and_validate_engine_ticket(
    runtime_dir: RuntimeDir,
    owner_uid: u32,
    core_pid: u32,
) -> Result<(RuntimeDir, EngineTicketV1), EngineBootstrapError> {
    let ticket = match consume_engine_ticket(&runtime_dir) {
        Ok(ticket) => ticket,
        Err(error) => {
            return Err(cleanup_failed_bootstrap_runtime(
                runtime_dir,
                EngineBootstrapError::Ticket(error),
            ));
        }
    };
    if ticket.owner_uid() != owner_uid || ticket.core_pid() != core_pid {
        return Err(cleanup_failed_bootstrap_runtime(
            runtime_dir,
            EngineBootstrapError::Ticket(TicketError::Malformed),
        ));
    }
    Ok((runtime_dir, ticket))
}

async fn run_observe_bootstrap(
    mut runtime_dir: RuntimeDir,
    args: EngineArgs,
    auth_key: exv_vpn_darwin_ipc::auth::AuthKey,
) -> Result<(), EngineBootstrapError> {
    runtime_dir
        .seal_for_root_publisher()
        .map_err(EngineBootstrapError::RuntimeDir)?;
    let listener_config = RootListenerConfig::new(
        runtime_dir,
        ExpectedPeer::new(args.owner_uid, args.core_pid),
        auth_key,
    )
    .map_err(|_| EngineBootstrapError::Listener)?;
    // SAFETY: this Engine bootstrap has not started worker threads before this root-only bind;
    // the listener implementation restores the process umask before returning.
    let listener = unsafe { PreauthListener::bind_root_publisher_single_threaded(listener_config) }
        .map_err(|_| EngineBootstrapError::Listener)?;
    let pid = current_pid()?;
    let (sender, incoming) = AuthenticatedIncoming::channel(1);
    // 顶层保留 EngineRuntime 句柄：统一退出收敛要置 shutdown/cancel 并观察
    // attempt/live 收尾，不再让 service 独占 runtime。
    let engine_runtime = Arc::new(EngineRuntime::new_oneshot_for_owner(args.owner_uid));
    // 沿本轮 per-core runtime 目录启用；service 形态保持原启动与目录规则。
    if std::fs::read(args.runtime_dir.join("exv-native-acceptance.opt-in")).ok().as_deref() == Some(b"connected-transport\n") {
        *engine_runtime.reconnect_fault.lock().expect("reconnect fault lock") =
            Some(crate::reconnect_fault::ReconnectFault::new(args.runtime_dir.clone()));
    }
    let service = EngineControlService::from_runtime(Arc::clone(&engine_runtime));
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(HelperControlServer::new(service))
            .serve_with_incoming(incoming)
            .await
    });

    write_record(EngineBootstrapRecord::Ready { pid })?;
    let operation = async {
        let connection = listener
            .accept_until_authenticated()
            .await
            .map_err(|_| EngineBootstrapError::Listener)?;
        let close_signal = sender
            .handoff_with_close_signal(connection)
            .await
            .map_err(|_| EngineBootstrapError::Listener)?;
        // W2 心跳：认证握手完成=心跳零点（权威规范：零点是握手完成时刻，不是进程
        // 启动），同时拉起 engine 层常驻看门狗（不挂在隧道保持循环上——保持循环
        // 只在持隧道期存在；未武装前看门狗恒不判定）。
        engine_runtime
            .heartbeat
            .lock()
            .expect("heartbeat watch lock")
            .arm();
        spawn_heartbeat_watchdog(Arc::clone(&engine_runtime));
        // W1 统一退出收敛：SIGTERM/SIGINT、Core EOF（close_signal）与 W2 心跳超时
        // 三类触发进入同一收敛序列——置 shutdown（接通
        // dead flag）→ 管线经既有消费点逆序 teardown → 有界等待
        // TERMINATION_TEARDOWN_DONE（10s 上界）。收敛完成前不 drop(sender)/关
        // server：否则会取消在途 handler，进程退出与 teardown 竞速会把已应用的
        // utun 路由与挂物理出口的 bypass /32 静态路由残留在内核。
        let trigger = tokio::select! {
            signal = close_signal => {
                // bridge 内部错误的 Err 与 EOF 同样进入收敛（close 结果本就不再
                // 影响退出 record 语义）。
                let _ = signal;
                TerminationTrigger::CoreClosed
            }
            () = poll_termination_signals() => TerminationTrigger::PosixSignals,
            () = engine_runtime.wait_heartbeat_timeout() => TerminationTrigger::HeartbeatTimeout,
        };
        // T1 keep 分支：core 会话终结（EOF/心跳超时）且策略为 KeepTunnel 时，先不
        // 收敛 teardown——检测保留（日志记录 EOF/超时事实），管线持有等待；本形态
        // 无第二会话端点（采纳接线在端点统一任务），显式信号是唯一既有终结出口，
        // 到来后与既有路径共享收敛与收尾（sender/server 的 drop 顺序不变）。
        if core_exit_keeps_tunnel(trigger, engine_runtime.core_exit_policy()) {
            engine_runtime.logs.publish(
                "info",
                "lifecycle",
                "CORE_EXIT_KEEP",
                "core session ended; tunnel held per on_core_exit policy",
                &[("trigger", format!("{trigger:?}"))],
            );
            poll_termination_signals().await;
        }
        let converged = converge_and_await_teardown(&engine_runtime).await;
        if !converged {
            // 有界超时：仍走既有退出路径（cleanup + 退出 record），但落诊断。
            engine_runtime.logs.publish(
                "error",
                "lifecycle",
                "TEARDOWN_TIMEOUT",
                format!(
                    "termination convergence hit the teardown bound, exiting with teardown \
                     unconfirmed (trigger={trigger:?})"
                ),
                &[("trigger", format!("{trigger:?}"))],
            );
        }
        drop(sender);
        server
            .await
            .map_err(|_| EngineBootstrapError::Server)?
            .map_err(|_| EngineBootstrapError::Server)?;
        Ok(())
    }
    .await;
    let cleanup = listener
        .cleanup()
        .map_err(|_| EngineBootstrapError::Listener);
    let outcome = operation.and(cleanup);
    match outcome {
        Ok(()) => {
            write_record(EngineBootstrapRecord::ExitOk { pid })?;
            Ok(())
        }
        Err(error) => {
            let _ = write_record(EngineBootstrapRecord::ExitError { pid, code: 1 });
            Err(error)
        }
    }
}

fn current_pid() -> Result<u32, EngineBootstrapError> {
    // SAFETY: this call only reads the current process pid.
    u32::try_from(unsafe { libc::getpid() }).map_err(|_| EngineBootstrapError::Credentials)
}

/// 提前失败路径（尚未发布 listener）的 runtime 目录回收。
///
/// 语义与正常退出收敛一致：先 guarded 清理固定 `engine.ticket`（held dirfd +
/// `unlinkat`，路径字符串不参与删除），再 `cleanup_empty()`（只 `rmdir` 空目录，非空即
/// 保留现场）。任何清理失败都只写一行 stderr 并返回原 typed 错误——清理失败不得
/// 覆盖真正的 bootstrap 失败原因，也不得用 `remove_dir_all` 扩大删除面。
fn cleanup_failed_bootstrap_runtime(
    runtime_dir: RuntimeDir,
    primary: EngineBootstrapError,
) -> EngineBootstrapError {
    if let Err(error) =
        exv_vpn_darwin_ipc::bootstrap::remove_engine_ticket_for_failed_bootstrap(&runtime_dir)
    {
        eprintln!("exv.engine.bootstrap: failed-bootstrap ticket cleanup skipped: {error}");
    }
    if let Err(error) = runtime_dir.cleanup_empty() {
        eprintln!("exv.engine.bootstrap: failed-bootstrap runtime cleanup skipped: {error}");
    }
    primary
}

fn write_record(record: EngineBootstrapRecord) -> Result<(), EngineBootstrapError> {
    let stdout = io::stdout();
    let mut pipe = stdout.lock();
    record
        .write_to(&mut pipe)
        .map_err(|_| EngineBootstrapError::Pipe)?;
    pipe.flush().map_err(|_| EngineBootstrapError::Pipe)
}

/// 轮询 SIGTERM/SIGINT 请求标志。
///
/// 信号处理器只做原子写入（async-signal-safe，不可 await），主循环以固定间隔
/// 轮询收敛；置位后返回，由统一收敛序列接管。
pub(crate) async fn poll_termination_signals() {
    poll_signal_flag(&TERMINATION_REQUESTED).await;
}

/// [`poll_termination_signals`] 的可注入变体：W2.5 service 会话循环经它传入独立
/// 标志（单测隔离进程级静态，避免并行测试间竞态）。
pub(crate) async fn poll_signal_flag(flag: &AtomicBool) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(TERMINATION_SIGNAL_POLL_INTERVAL).await;
    }
}

/// W2 常驻心跳看门狗：按 [`HEARTBEAT_WATCHDOG_CHECK_INTERVAL`] 周期检查
/// [`HeartbeatWatch`]，已武装（存在已认证活跃会话）且超 [`HEARTBEAT_TIMEOUT`]
/// 未收到 KeepAlive → `request_heartbeat_timeout()`（W1 顶层 select 第三臂接管
/// 统一收敛退出）。
///
/// engine 层常驻任务（顶层 spawn，不挂在隧道保持循环上——保持循环只在持隧道期
/// 存在；本波只有会话形态，超时动作=统一收敛退出）。未武装（握手未完成）恒不
/// 判定；触发一次后结束（进程随收敛退出）。锁中毒按超时处理（fail closed：
/// 宁可误收敛也不留无主隧道）。
pub(crate) fn spawn_heartbeat_watchdog(runtime: Arc<EngineRuntime>) -> tokio::task::JoinHandle<()> {
    spawn_heartbeat_watchdog_with(
        runtime,
        HEARTBEAT_WATCHDOG_CHECK_INTERVAL,
        HEARTBEAT_TIMEOUT,
    )
}

/// [`spawn_heartbeat_watchdog`] 的可注入节奏变体（单测用小间隔加速；executor
/// 与生产同一 [`heartbeat_watchdog_loop`]，无第二条看门狗路径）。
pub(crate) fn spawn_heartbeat_watchdog_with(
    runtime: Arc<EngineRuntime>,
    check_interval: Duration,
    timeout: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(heartbeat_watchdog_loop(runtime, check_interval, timeout))
}

/// 看门狗主循环：检查节拍与超时上界由参数注入（测试用小值加速），生产经
/// [`spawn_heartbeat_watchdog`] 传入默认常量。
async fn heartbeat_watchdog_loop(
    runtime: Arc<EngineRuntime>,
    check_interval: Duration,
    timeout: Duration,
) {
    loop {
        tokio::time::sleep(check_interval).await;
        let timed_out = runtime.heartbeat.lock().map_or(true, |watch| {
            watch.elapsed().is_some_and(|elapsed| elapsed > timeout)
        });
        if timed_out {
            runtime.logs.publish(
                "error",
                "lifecycle",
                "HEARTBEAT_TIMEOUT",
                "no KeepAlive within the timeout bound; converging to teardown",
                &[("timeout_secs", timeout.as_secs().to_string())],
            );
            runtime.request_heartbeat_timeout();
            return;
        }
    }
}

/// 统一退出收敛执行体：三类终止触发（SIGTERM/SIGINT、Core EOF、W2 心跳超时）
/// 在触发后调用同一本函数，不区分触发源。
///
/// 顺序：
/// 1. 置 `runtime.shutdown`（接通既有 dead flag，保持循环的消费点不变）；
/// 2. 对进行中的 attempt 置既有 cancel（`establish` 的段边界消费点，隧道组装中
///    也能尽早自清理；持有态由 1 的消费点 break，两条都是既有机制，无第二套）；
/// 3. 有界等待管线 teardown 完成（[`TERMINATION_TEARDOWN_DONE`] 或
///    attempt/live 已清空），上界 [`TERMINATION_TEARDOWN_BOUND`]。
///
/// 返回 true = teardown 确认完成（或本就无管线任务）；false = 有界超时（调用方
/// 仍继续既有退出路径，但应记录诊断）。
pub(crate) async fn converge_and_await_teardown(runtime: &EngineRuntime) -> bool {
    let converged = converge_and_await_teardown_with(runtime, &TERMINATION_TEARDOWN_DONE).await;
    // S2：善后已验证才行使痕迹删除权（v6 删除权）；未验证保留，作为下次连接
    // 的预检触发信号（双亡格）。
    if converged {
        crate::connect_gate::process_gate().finish_verified_teardown(runtime.owner_uid());
    }
    // T2：收敛即管线事实清位（身份/采纳面随管线消亡；teardown 后的会话回默认
    // 策略）。收敛超时同样清——身份残留会让下一 Apply 误入采纳分叉。
    runtime.clear_pipeline_facts();
    converged
}

/// [`converge_and_await_teardown`] 的可注入变体：teardown 完成标志由调用方提供，
/// 单测用它隔离进程级静态（勿在单测间共享真静态）。
async fn converge_and_await_teardown_with(
    runtime: &EngineRuntime,
    _teardown_done: &AtomicBool,
) -> bool {
    runtime.shutdown.store(true, Ordering::SeqCst);
    if let Ok(slot) = runtime.attempt.lock()
        && let Some(attempt) = slot.as_ref()
    {
        attempt.cancel.store(true, Ordering::SeqCst);
    }
    runtime.execution_changed.notify_waiters();
    // 边界一（idle）：无管线任务（attempt 与 live 均空）时立即完成，不等一个
    // 永远不会置位的 teardown 完成位。
    if tunnel_activity_cleared(runtime) {
        return runtime.complete_resource_cleanup().is_ok();
    }
    // 边界二（组装中/持有中）：等管线经既有消费点自清理。完成条件是
    // 子管线释放live及资源后，由完整执行owner清attempt；不得仅凭子管线结束提前完成。
    let deadline = tokio::time::Instant::now() + TERMINATION_TEARDOWN_BOUND;
    loop {
        if tunnel_activity_cleared(runtime) {
            return runtime.complete_resource_cleanup().is_ok();
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(TERMINATION_TEARDOWN_POLL_INTERVAL).await;
    }
}

/// attempt 与 live 均为空 = 无进行中的组装/持有（管线的每条收尾路径都会清两者）。
///
/// 锁中毒按「仍有活动」处理：收敛宁可多等（受上界约束）也不误判 idle 提前退出。
fn tunnel_activity_cleared(runtime: &EngineRuntime) -> bool {
    let attempt_active = match runtime.attempt.lock() {
        Ok(slot) => slot.is_some(),
        Err(_) => true,
    };
    let held = match runtime.live.lock() {
        Ok(slot) => slot.is_some(),
        Err(_) => true,
    };
    !attempt_active && !held
}

/// 一个活动连接尝试的关联信息。
struct ActiveAttempt {
    cancel: Arc<AtomicBool>,
}

/// 完整装配/R1任务的完成回执；子管线不再清除此owner。
struct ExecutionCompletion {
    runtime: Arc<EngineRuntime>,
    cancel: Arc<AtomicBool>,
}
impl Drop for ExecutionCompletion {
    fn drop(&mut self) {
        let mut slot = self.runtime.attempt.lock().expect("attempt slot lock");
        if slot
            .as_ref()
            .is_some_and(|attempt| Arc::ptr_eq(&attempt.cancel, &self.cancel))
        {
            // 完整执行 owner 结束即释放秘密材料；无需等 Core 后续 Stop 才清零。
            *self.runtime.reconnect_materials.lock().expect("reconnect materials lock") = None;
            slot.take();
        }
        drop(slot);
        self.runtime.execution_changed.notify_waiters();
    }
}

/// 唯一 Common 控制服务实现。
pub(crate) struct EngineControlService {
    runtime: Arc<EngineRuntime>,
}

impl EngineControlService {
    // 生产路径改由 run_observe_bootstrap 持 runtime 句柄构造（from_runtime）；
    // new 仅供单测独立建服务。
    /// 从外部构造的 runtime 建服务：顶层 run_observe_bootstrap / W2.5 service
    /// 会话循环均需保留同一句柄执行统一收敛。
    pub(crate) fn from_runtime(runtime: Arc<EngineRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl HelperControl for EngineControlService {
    type MaintainOwnerLeaseStream = Empty<Result<wire::HelperLeaseMessage, Status>>;
    type StreamLogsStream =
        std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<wire::LogEvent, Status>> + Send>>;
    type StreamStatsStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<wire::StatsEvent, Status>> + Send>,
    >;
    type StreamConnectStatusStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<wire::ConnectStatusEvent, Status>> + Send>,
    >;

    async fn maintain_owner_lease(
        &self,
        _request: Request<tonic::Streaming<wire::HostLeaseMessage>>,
    ) -> Result<Response<Self::MaintainOwnerLeaseStream>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn observe_owned_state(
        &self,
        request: Request<wire::ObserveOwnedStateRequest>,
    ) -> Result<Response<wire::ObserveOwnedStateReply>, Status> {
        if request.get_ref().lookup_key.is_some() {
            return Err(Status::invalid_argument(OBSERVE_ONLY_MESSAGE));
        }
        Ok(Response::new(wire::ObserveOwnedStateReply {
            snapshot: Some(wire::RuntimeSnapshot {
                system_proxy: self.runtime.observed_proxy_policy(),
                operation_id: self.runtime.current_operation_id(),
                reconnect: None,
                self_heal: None,
                state: Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
                    last_cleanup: None,
                })),
                ..Default::default()
            }),
            authority_fence: Some(wire::AuthorityFence::default()),
        }))
    }

    async fn acquire_lease(
        &self,
        _request: Request<wire::AcquireLeaseRequest>,
    ) -> Result<Response<wire::AcquireLeaseReply>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn apply_tunnel(
        &self,
        request: Request<wire::ApplyTunnelRequest>,
    ) -> Result<Response<wire::ApplyTunnelReply>, Status> {
        // MAC-CSTP-05：消费一次性封包后执行真实 WebVPN 登录 + CSTP 协商；
        // 仍不创建 utun、不应用路由/DNS、不启动数据面。
        let request = request.into_inner();
        let operation_id = request
            .lookup_key
            .as_ref()
            .map(|key| key.operation_id.clone())
            .filter(|value| value.len() == 16)
            .ok_or_else(|| Status::invalid_argument(APPLY_INVALID_MESSAGE))?;
        let plan = request
            .plan
            .ok_or_else(|| Status::invalid_argument(APPLY_INVALID_MESSAGE))?;
        let intent = decode_connect_intent(request.secret_payload)
            .map_err(|_| Status::invalid_argument(APPLY_INVALID_MESSAGE))?;

        let _admission = self.runtime.lifecycle_gate.lock().await;
        if self.runtime.pipeline_activity_cleared() && self.runtime.has_cleanup_obligations() {
            return Err(Status::failed_precondition(
                "DARWIN_ENGINE_CLEANUP_INCOMPLETE",
            ));
        }
        // T2 采纳分叉：管线持有中（T1 keep 携带或同会话重建）时的 ApplyTunnel
        // 不再是"新尝试"而是"接管裁决"——同服务器+同用户名（跨平台指南 §6.2：
        // 无新认证材料的明确接管保留旧隧道）→ 采纳：不重新登录/协商，切换事件
        // 归属并按本次 operation_id 重放 Connected；新认证材料（服务器或用户名
        // 不同）→ typed 拒绝，由 core 显式 Stop 后重建（引擎不做隐式拆旧建新）。
        let (intent_identity, fresh_authentication) = match &intent {
            ConnectIntent::Cisco(envelope) => (
                PipelineIdentity {
                    server: envelope.server().to_owned(),
                    username: envelope.username().to_owned(),
                    protocol: ProtocolKind::Cisco,
                    atrust_dns_enabled: true,
                    resolver_domains: envelope.resolver_domains().to_vec(),
                },
                false,
            ),
            ConnectIntent::Atrust(atrust) => (
                PipelineIdentity {
                    server: atrust.gateway().to_owned(),
                    username: atrust_auth::ATRUST_IDENTITY_USERNAME_PLACEHOLDER.to_owned(),
                    protocol: ProtocolKind::Atrust,
            atrust_dns_enabled: atrust.atrust_dns_enabled(),
                    resolver_domains: atrust.resolver_domains().to_vec(),
                },
                atrust.fresh_authentication(),
            ),
        };
        if self.runtime.pipeline_is_held() {
            let identity = self
                .runtime
                .pipeline_identity()
                .ok_or_else(|| Status::failed_precondition(ADOPT_IDENTITY_MISSING_MESSAGE))?;
            match adoption_decision(&identity, &intent_identity, fresh_authentication) {
                AdoptionDecision::Mismatch => {
                    self.runtime.logs.publish(
                        "info",
                        "engine",
                        "ADOPT_MISMATCH",
                        "held pipeline identity differs from the new apply; caller must Stop then re-apply",
                        &[
                            ("held_protocol", identity.protocol.as_str().to_owned()),
                            ("apply_protocol", intent_identity.protocol.as_str().to_owned()),
                        ],
                    );
                    return Err(Status::failed_precondition(ADOPT_MISMATCH_MESSAGE));
                }
                AdoptionDecision::NewAuth => {
                    // 复审 B4 收敛（设计 §7）：同身份+同协议的新认证不走采纳——引擎不
                    // 隐式拆旧建新，typed 拒绝后由 core 编排 Stop→重建。缓存会话过期
                    // 失效走复核失败链（R3），不进此门。
                    self.runtime.logs.publish(
                        "info",
                        "engine",
                        "ADOPT_NEW_AUTH",
                        "held pipeline matches the identity but the apply carries fresh authentication; caller must Stop then re-apply",
                        &[("protocol", identity.protocol.as_str().to_owned())],
                    );
                    return Err(Status::failed_precondition(ADOPT_NEW_AUTH_MESSAGE));
                }
                AdoptionDecision::Adopt => {}
            }
            self.runtime.adopt_operation(operation_id.clone());
            let (if_index, connected_at_ms) = self.runtime.connected_facts();
            self.runtime.logs.publish(
                "info",
                "engine",
                "ADOPTED",
                "new core session adopted the held pipeline",
                &[],
            );
            let _ = self.runtime.status_tx.send(wire::ConnectStatusEvent {
                aftermath_verified: false,
                reconnect_state: None,
                stop_completion: None,
                own_tunnel_if_index: if_index,
                session_established_at_ms: connected_at_ms,
                operation_id,
                connect_phase: wire::ConnectPhase::StartingDataPlane as i32,
                coarse_phase: wire::StatsPhase::Connected as i32,
                error: None,
            });
            return Ok(Response::new(wire::ApplyTunnelReply {
                result: Some(wire::apply_tunnel_reply::Result::Pending(
                    wire::ApplyAccepted {
                        operation_id: self.runtime.current_operation_id(),
                        authority_fence: Some(wire::AuthorityFence::default()),
                    },
                )),
            }));
        }

        // S2 连接门预检（2026-10-26 验收修复计划）：任何系统变更之前的有界小
        // 预检（痕在/版本变更/进程首连三触发源）。持有中管线（上方采纳分叉）与
        // 在途 attempt 不进本门——活隧道的 resolver/路由不是残留；在途组装的
        // Apply 将在下方 attempt 门被拒，此处先查可在预检前快速返回。
        if self.runtime.attempt.lock().map_or(true, |slot| slot.is_some()) {
            return Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE));
        }
        let gate_mode = if self.runtime.explicit_stop.is_some() {
            "service"
        } else {
            "oneshot"
        };
        if let Err(rejection) = crate::connect_gate::process_gate()
            .admit_connect(
                self.runtime.owner_uid(),
                &intent_identity.server,
                &intent_identity.username,
                gate_mode,
                self.runtime.logs(),
            )
            .await
        {
            // 阻断码经 core 的 failed_precondition 透传通道直达前端
            //（`DARWIN_CORE_PREFLIGHT_BLOCKED|<reason_ids>|<摘要>`）。
            return Err(Status::failed_precondition(rejection.message()));
        }

        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut attempt_slot = self
                .runtime
                .attempt
                .lock()
                .map_err(|_| Status::internal(ATTEMPT_ACTIVE_MESSAGE))?;
            if attempt_slot.is_some() {
                return Err(Status::failed_precondition(ATTEMPT_ACTIVE_MESSAGE));
            }
            self.runtime
                .execution_errors
                .lock()
                .expect("execution errors lock")
                .clear();
            // 新一代 attempt 声明即复位上一代的 teardown 完成位：完成位语义是
            // 「最近一次声明的 attempt 的 teardown 已完成」，不复位会让重连后的
            // 第二代管线在收敛等待中被上一代的完成位误判为已清理。
            TERMINATION_TEARDOWN_DONE.store(false, Ordering::SeqCst);
            *attempt_slot = Some(ActiveAttempt {
                cancel: Arc::clone(&cancel),
            });
        }
        // 只有全新受理才冻结策略；拒绝/采纳不触碰原执行闭包的策略和材料。
        let r1_enabled = request.reconnect_policy.as_ref().map_or_else(
            || owner_auto_reconnect_enabled(self.runtime.owner_uid()),
            |policy| policy.enabled,
        );
        // 全新受理明确设定初始归属；后续每轮只读当前归属，采纳后不回退到任务旧id。
        self.runtime.adopt_operation(operation_id.clone());

        if let ConnectIntent::Atrust(atrust) = intent {
            // aTrust 计划字段（M3 定稿）：mtu 与用户校园路由经 wire 计划注入
            // （Cisco 走信封 routes；aTrust 信封无路由字段——计划是等价通道）。
            // 畸形条目（字节数≠4）静默丢弃：计划是 core 的受信输入，管线侧
            // parse_cidr 会再做最终校验（双防线，不因畸形计划拒绝连接）。
            let plan_mtu = plan.mtu;
            let campus_routes = plan
                .ipv4_routes
                .iter()
                .filter_map(|route| {
                    let octets: [u8; 4] = route.network.clone().try_into().ok()?;
                    Some(format!("{}/{}", Ipv4Addr::from(octets), route.prefix_len))
                })
                .collect();
            // M6 材料臂：apply 受理即捕获（副本自解码载荷取得；管线存活期供引擎侧
            // R1 重拨，善后清位时随 Zeroizing 清零——与 Cisco 臂同一驻留位）。
            self.runtime
                .store_reconnect_materials(ReconnectMaterials::Atrust(
                    AtrustMaterials::from_payload(&atrust),
                ));
            spawn_atrust_connect_attempt(
                self.runtime.clone(),
                atrust,
                plan_mtu,
                campus_routes,
                operation_id.clone(),
                Arc::clone(&cancel),
                r1_enabled,
            );
            return Ok(Response::new(wire::ApplyTunnelReply {
                result: Some(wire::apply_tunnel_reply::Result::Pending(
                    wire::ApplyAccepted {
                        operation_id,
                        authority_fence: Some(wire::AuthorityFence::default()),
                    },
                )),
            }));
        }
        let ConnectIntent::Cisco(envelope) = intent else {
            return Err(Status::invalid_argument(APPLY_INVALID_MESSAGE));
        };

        let runtime = Arc::clone(&self.runtime);
        let task_operation_id = operation_id.clone();
        let server = envelope.server().to_owned();
        let user_agent = envelope.user_agent().to_owned();
        let mut campus_routes = envelope.routes().to_vec();
        // T2（复审必改 1）：身份必须在 take_credentials 之后、zeroize **之前**
        // 捕获——zeroize 会把 String 清零并置空，之后的 clone 恒为空串，采纳
        // 匹配将永久失败。身份只含非秘密面（服务器+用户名）。
        let mut pipeline_identity = PipelineIdentity {
            server: server.clone(),
            username: envelope.username().to_owned(),
            protocol: ProtocolKind::Cisco,
            atrust_dns_enabled: true,
            resolver_domains: envelope.resolver_domains().to_vec(),
        };
        let (username, password) = envelope.take_credentials();
        // T4（D9）：R1 材料驻留——副本在原始值清零前捕获；管线存活期供引擎侧
        // 重拨，善后清位时随 Zeroizing 清零（不跨连接生命周期留存）。
        if r1_enabled {
            self.runtime.store_reconnect_materials(ReconnectMaterials::Cisco {
                server: server.clone(),
                username: username.clone(),
                password: zeroize::Zeroizing::new(password.clone()),
                user_agent: user_agent.clone(),
                routes: campus_routes.clone(),
                resolver_domains: pipeline_identity.resolver_domains.clone(),
                mtu_override: u16::try_from(plan.mtu).ok().filter(|mtu| *mtu > 0),
            });
        }

        let plan_mtu = plan.mtu;
        tokio::spawn(async move {
            let _execution = ExecutionCompletion {
                runtime: Arc::clone(&runtime),
                cancel: Arc::clone(&cancel),
            };
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let is_cancelled = || cancel.load(std::sync::atomic::Ordering::SeqCst);
            let mut username = username;
            let mut password = password;
            let result = crate::protocol::establish_observed(
                &server,
                username.as_bytes(),
                password.as_bytes(),
                &user_agent,
                is_cancelled,
                &|phase| {
                    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                        aftermath_verified: false,
                        reconnect_state: None,
                        stop_completion: None,
                        own_tunnel_if_index: 0,
                        session_established_at_ms: 0,
                        operation_id: runtime.operation_for_event(&task_operation_id),
                        connect_phase: phase as i32,
                        coarse_phase: wire::StatsPhase::Connecting as i32,
                        error: None,
                    });
                },
                Some((&runtime.logs, &task_operation_id)),
                |egress, server| runtime.ensure_control_route(egress, server).map(|_| ()),
            )
            .await;
            username.zeroize();
            password.zeroize();
            match result {
                Ok(mut established) if is_cancelled() => {
                    close_established(&runtime, &mut established).await;
                    if let Err(error) = runtime.complete_resource_cleanup() {
                        runtime.logs.publish(
                            "error",
                            "engine",
                            "CANCELLED_CONNECT_CLEANUP_INCOMPLETE",
                            format!("cancelled connection cleanup incomplete: {error}"),
                            &[],
                        );
                    }
                    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                        aftermath_verified: false,
                        reconnect_state: None,
                        stop_completion: None,
                        own_tunnel_if_index: 0,
                        session_established_at_ms: 0,
                        operation_id: runtime.operation_for_event(&task_operation_id),
                        connect_phase: wire::ConnectPhase::NegotiatingTunnel as i32,
                        coarse_phase: wire::StatsPhase::Failed as i32,
                        error: Some(ConnectCancelled::vpn_error()),
                    });
                }
                Ok(initial_established) => {
                    // MAC-UTUN-06 / MAC-NET-07 / MAC-DATA-08：协商成功后建立完整平台
                    // 数据面；失败沿逆序 teardown，成功后保持并按需 teardown。
                    // T4（R1=engine，v6 §2.7）：SessionLost 不再直接终态——按驻留
                    // 材料（D9）退避重拨（有界次数），同一 operation_id 保持 core
                    // 投影连续；预算内不可恢复或材料缺失才终态。
                    let mut r1_attempt = 0_u32;
                    let mut established_slot = Some(initial_established);
                    let mut needs_establish = false;
                    let outcome = loop {
                        if needs_establish {
                            let Some(ReconnectMaterials::Cisco {
                                server,
                                username,
                                password,
                                user_agent,
                                routes,
                                resolver_domains,
                                ..
                            }) = runtime.reconnect_materials()
                            else {
                                break TunnelOutcome::SessionLost;
                            };
                            let observe_reconnect_phase = |phase| {
                                    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                                        aftermath_verified: false,
                                        reconnect_state: None,
                                        stop_completion: None,
                                        own_tunnel_if_index: 0,
                                        session_established_at_ms: 0,
                                        operation_id: runtime
                                            .operation_for_event(&task_operation_id),
                                        connect_phase: phase as i32,
                                        coarse_phase: wire::StatsPhase::Connecting as i32,
                                        error: None,
                                    });
                                };
                            match cisco_r1_establish(&runtime, &cancel, &mut r1_attempt, || crate::protocol::establish_observed(
                                &server,
                                username.as_bytes(),
                                password.as_bytes(),
                                &user_agent,
                                is_cancelled,
                                &observe_reconnect_phase,
                                Some((&runtime.logs, &task_operation_id)),
                                |egress, server| {
                                    runtime.ensure_control_route(egress, server).map(|_| ())
                                },
                            ))
                            .await
                            {
                                Ok(next) if !is_cancelled() => {
                                    pipeline_identity = PipelineIdentity {
                                        server: server.clone(),
                                        username: username.clone(),
                                        protocol: ProtocolKind::Cisco,
                                        atrust_dns_enabled: true,
                                        resolver_domains,
                                    };
                                    campus_routes = routes.clone();
                                    established_slot = Some(next);
                                }
                                Ok(mut established) => {
                                    close_established(&runtime, &mut established).await;
                                    break TunnelOutcome::Cancelled;
                                }
                                Err(crate::protocol::ConnectError::Cancelled) => break TunnelOutcome::Cancelled,
                                Err(error) => break TunnelOutcome::ProtocolFailed(error),
                            }
                        }
                        let runtime_for_pipeline = Arc::clone(&runtime);
                        let operation_for_pipeline =
                            runtime.operation_for_event(&task_operation_id);
                        let identity_for_pipeline = pipeline_identity.clone();
                        let routes_for_pipeline = campus_routes.clone();
                        let cancel_for_pipeline = Arc::clone(&cancel);
                        let Some(established) = established_slot.take() else {
                            // 上一轮管线已消费会话且未重建（理论不可达——所有
                            // 到达路径都先经 establish 重填）。
                            break TunnelOutcome::Failed("R1", 0, 0);
                        };
                        let attempt = tokio::spawn(async move {
                            run_tunnel_pipeline(
                                &runtime_for_pipeline,
                                established,
                                operation_for_pipeline,
                                plan_mtu,
                                routes_for_pipeline,
                                &cancel_for_pipeline,
                                identity_for_pipeline,
                            )
                            .await
                        })
                        .await
                        .unwrap_or_else(|panic| {
                            runtime.logs.publish(
                                "error",
                                "pipeline",
                                "PIPELINE_PANIC",
                                format!("pipeline panicked: {panic}"),
                                &[],
                            );
                            TunnelOutcome::Failed("PANIC", 0, 0)
                        });
                        if !matches!(attempt, TunnelOutcome::SessionLost) {
                            break attempt;
                        }
                        // 本次受理冻结的 R1 开关不随保存设置改变。
                        if !r1_enabled {
                            runtime.logs.publish(
                                "info",
                                "pipeline",
                                "R1_DISABLED_BY_OWNER_SWITCH",
                                "data plane lost; owner auto_reconnect switch is off — terminal (baseline behavior)",
                                &[],
                            );
                            break attempt;
                        }
                        // T4（复审必改 D1）：预算按"重试调度次数"计——SessionLost
                        // 与 establish 失败都消耗（旧行为只在 SessionLost 递增，
                        // establish 持续失败时恒 1<5 无限循环）。
                        r1_attempt += 1;
                        if r1_attempt > R1_MAX_ATTEMPTS {
                            runtime.logs.publish(
                                "error",
                                "pipeline",
                                "R1_EXHAUSTED",
                                "engine-side reconnect budget exhausted",
                                &[("attempts", r1_attempt.to_string())],
                            );
                            break attempt;
                        }
                        // 外层执行owner跨越R1退避和重拨，Stop不会观察到虚假空槽。
                        let delay = r1_backoff_delay(r1_attempt);
                        runtime.logs.publish(
                            "warn",
                            "pipeline",
                            "PUMP_LOST_R1_RETRY",
                            "data plane lost; engine-side reconnect retry scheduled",
                            &[
                                ("attempt", r1_attempt.to_string()),
                                ("delay_ms", delay.as_millis().to_string()),
                            ],
                        );
                        runtime.publish_r1_wait(r1_attempt, delay);
                        runtime.reconnect_delay(&cancel, delay).await;
                        if is_cancelled() {
                            break TunnelOutcome::Cancelled;
                        }
                        needs_establish = true;
                    };
                    match outcome {
                        TunnelOutcome::Cancelled => {}
                        TunnelOutcome::SessionLost => {
                            runtime.logs.publish(
                                "error",
                                "pipeline",
                                "PUMP_LOST",
                                "data plane pump finished unexpectedly",
                                &[],
                            );
                            // T2（复审必改 2）：终态事件归属读 runtime 当前值——
                            // 采纳后归属已切换，旧 task_operation_id 会被采纳方
                            // 投影过滤，造成采纳方对失联失明。
                            let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                                aftermath_verified: false,
                                reconnect_state: None,
                                stop_completion: None,
                                own_tunnel_if_index: 0,
                                session_established_at_ms: 0,
                                operation_id: runtime.current_operation_id(),
                                connect_phase: wire::ConnectPhase::NegotiatingTunnel as i32,
                                coarse_phase: wire::StatsPhase::Failed as i32,
                                error: Some(ConnectCancelled::session_lost_vpn_error()),
                            });
                        }
                        TunnelOutcome::ProtocolFailed(error) => {
                            finish_cisco_protocol_failure(&runtime, &task_operation_id, &error);
                        }
                        TunnelOutcome::Failed(step, errno, marker) => {
                            runtime.logs.publish(
                                "error",
                                "pipeline",
                                "PLATFORM_STEP_FAILED",
                                format!("platform failed at {step} errno={errno}"),
                                &[
                                    ("step", step.to_owned()),
                                    ("errno", errno.to_string()),
                                    ("marker", marker.to_string()),
                                ],
                            );
                            let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                                aftermath_verified: false,
                                reconnect_state: None,
                                stop_completion: None,
                                own_tunnel_if_index: 0,
                                session_established_at_ms: 0,
                                operation_id: runtime.current_operation_id(),
                                connect_phase: wire::ConnectPhase::ApplyingPlatformTunnel as i32,
                                coarse_phase: wire::StatsPhase::Failed as i32,
                                error: Some(ConnectCancelled::platform_vpn_error(
                                    step, errno, marker,
                                )),
                            });
                        }
                    }
                }
                Err(error) => {
                    // 登录前控制路由可能已经成功写入、但随后登录/回读失败。无后续重试
                    // 事件时此处必须释放它；若此前存在自动重连窗口的空闲 utun，同样收口。
                    if let Err(cleanup_error) = runtime.complete_resource_cleanup() {
                        runtime.logs.publish(
                            "error",
                            "engine",
                            "CONNECT_FAILURE_CLEANUP_INCOMPLETE",
                            format!("failed connection cleanup incomplete: {cleanup_error}"),
                            &[],
                        );
                    }
                    let phase = error.failure_phase();
                    let vpn_error = error.to_vpn_error();
                    // MAC-OBS-13 S1：协议协商 typed 失败路径同步进 sink。`ConnectError`
                    // 的 Display 只含 typed 诊断码（凭据与主机名永不入错误文本）。
                    runtime.logs.publish(
                        "error",
                        "protocol",
                        "ESTABLISH_FAILED",
                        format!("CSTP establish failed: {error}"),
                        &[
                            ("code", vpn_error.code.to_string()),
                            ("stage", vpn_error.stage.to_string()),
                        ],
                    );
                    let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                        aftermath_verified: false,
                        reconnect_state: None,
                        stop_completion: None,
                        own_tunnel_if_index: 0,
                        session_established_at_ms: 0,
                        operation_id: runtime.operation_for_event(&task_operation_id),
                        connect_phase: phase as i32,
                        coarse_phase: wire::StatsPhase::Failed as i32,
                        error: Some(vpn_error),
                    });
                }
            }
        });

        Ok(Response::new(wire::ApplyTunnelReply {
            result: Some(wire::apply_tunnel_reply::Result::Pending(
                wire::ApplyAccepted {
                    operation_id,
                    authority_fence: Some(wire::AuthorityFence::default()),
                },
            )),
        }))
    }

    async fn stop_tunnel(
        &self,
        request: Request<wire::StopTunnelRequest>,
    ) -> Result<Response<wire::StopTunnelReply>, Status> {
        let request = request.into_inner();
        if request
            .lookup_key
            .as_ref()
            .is_none_or(|key| key.operation_id.len() != 16)
        {
            return Err(Status::invalid_argument(APPLY_INVALID_MESSAGE));
        }
        let key = request.lookup_key.expect("validated lookup key");
        let runtime = Arc::clone(&self.runtime);
        let reply = self
            .runtime
            .stop_operations
            .run(key, request.request_digest, move || async move {
                let _transition = runtime.lifecycle_gate.lock().await;
                let result = runtime.stop_and_wait_for_cleanup().await;
                let stopped = runtime.pipeline_activity_cleared();
                let blocks = !stopped || runtime.has_cleanup_obligations();
                if let Err(error) = &result {
                    runtime.logs.publish(
                        "error",
                        "engine",
                        "STOP_CLEANUP_FAILED",
                        error.to_string(),
                        &[],
                    );
                }
                let mut errors = if stopped {
                    std::mem::take(
                        &mut *runtime
                            .execution_errors
                            .lock()
                            .expect("execution errors lock"),
                    )
                } else {
                    runtime
                        .execution_errors
                        .lock()
                        .expect("execution errors lock")
                        .clone()
                };
                if let Err(error) = result {
                    let (reason, errno) = match error {
                        crate::platform::PlatformError::Route("CLEANUP_WAIT", error) => {
                            ("darwin/stop_wait", error.raw_os_error())
                        }
                        crate::platform::PlatformError::Route(_, error) => {
                            ("darwin/route_cleanup", error.raw_os_error())
                        }
                        _ => ("darwin/stop_incomplete", None),
                    };
                    errors.push(super::stop_operations::stop_error(reason, errno));
                }
                super::stop_operations::stop_reply(stopped, blocks, errors)
            })
            .await?;
        Ok(Response::new(reply))
    }

    async fn get_operation(
        &self,
        request: Request<wire::GetOperationRequest>,
    ) -> Result<Response<wire::GetOperationReply>, Status> {
        let key = request
            .into_inner()
            .lookup_key
            .ok_or_else(|| Status::invalid_argument(APPLY_INVALID_MESSAGE))?;
        Ok(Response::new(self.runtime.stop_operations.get(&key).await))
    }

    async fn release_lease(
        &self,
        _request: Request<wire::ReleaseLeaseRequest>,
    ) -> Result<Response<wire::ReleaseLeaseReply>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn stream_logs(
        &self,
        _request: Request<wire::StreamLogsRequest>,
    ) -> Result<Response<Self::StreamLogsStream>, Status> {
        // MAC-OBS-13 S1：同一已认证控制通道上的既有 `StreamLogs` 接缝（不新建通道）。
        // `resume_tick` 契约为 0 = 从当前流位置开始；Engine 无离线缓冲补拉。
        let receiver = self.runtime.logs.open_stream();
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
        )))
    }

    async fn stream_stats(
        &self,
        request: Request<wire::StreamStatsRequest>,
    ) -> Result<Response<Self::StreamStatsStream>, Status> {
        // W-MVP-13：同一已认证控制通道上的既有统计推送接缝（不新建通道）。
        let interval = request.into_inner().sample_interval_ms;
        let receiver = self.runtime.stats.open_stream(interval);
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
        )))
    }

    async fn stream_connect_status(
        &self,
        _request: Request<wire::StreamConnectStatusRequest>,
    ) -> Result<Response<Self::StreamConnectStatusStream>, Status> {
        // attach-before-apply：Core 先挂本流，再 ApplyTunnel；事件经广播分发。
        let stream = BroadcastStream::new(self.runtime.status_tx.subscribe())
            .map(|item| item.map_err(|_| Status::internal(STATUS_LAGGED_MESSAGE)));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn keep_alive(
        &self,
        request: Request<wire::KeepAliveRequest>,
    ) -> Result<Response<wire::KeepAliveReply>, Status> {
        // W2 心跳（E7 两层切分的活性层，本波为会话形态的退出权威）：单连接 accept
        // 模型下只有唯一已认证连接上的 RPC 才能到达本 handler（对端已验证，对齐
        // win32「transport 已 gate、handler 无需二次校验」的语义）。语义 = touch
        // 心跳时间戳 + 回显 `monotonic_tick`（照 win32：tick 仅诊断序号，不校验
        // 递增）。高频心跳不打日志（win32 同款取舍）。
        let tick = request.get_ref().monotonic_tick;
        if let Ok(mut watch) = self.runtime.heartbeat.lock() {
            watch.touch();
        }
        Ok(Response::new(wire::KeepAliveReply {
            monotonic_tick: tick,
        }))
    }

    async fn trigger_latency_refresh(
        &self,
        request: Request<wire::HelperLatencyRefreshRequest>,
    ) -> Result<Response<wire::HelperLatencyRefreshReply>, Status> {
        // 与既有 Apply/Stop 共用已认证控制服务和本 Engine 的接纳锁。
        let _admission = self.runtime.lifecycle_gate.lock().await;
        self.runtime
            .accept_latency_refresh(&request.into_inner().operation_id)?;
        Ok(Response::new(wire::HelperLatencyRefreshReply {
            accepted: true,
        }))
    }

    async fn auth_receiver_control(
        &self, request: Request<wire::AuthReceiverControlRequest>,
    ) -> Result<Response<wire::AuthReceiverControlReply>, Status> {
        // Bootstrap 与 service 生产形态均复用此已认证会话和 runtime owner。
        crate::official_auth_receiver::control(self.runtime.owner_uid(), request.into_inner())
            .await
            .map(Response::new)
    }

    async fn service_manage(
        &self,
        _request: Request<wire::ServiceManageRequest>,
    ) -> Result<Response<wire::ServiceManageReply>, Status> {
        Err(Status::unimplemented(OBSERVE_ONLY_MESSAGE))
    }

    async fn shutdown(
        &self,
        _request: Request<wire::ShutdownRequest>,
    ) -> Result<Response<wire::ShutdownReply>, Status> {
        // 2026-09-05 方案 B（win32 Shutdown RPC）的 darwin 侧实装随 W2.5 分叉：
        // - service 形态（显式停止槽在位）：请求显式停止 → 会话循环消费本请求，
        //   执行「拆隧道 → exit(0)」——launchd `KeepAlive={SuccessfulExit:false}`
        //   下干净退出不复活（权威规范 §二.2.4：显式停止是 service 形态的退出权威）。
        //   回 ACCEPTED 与 win32 语义对齐（armed 同一退出序列；本 handler 不直接
        //   exit——先让 RPC 应答落盘，退出由会话循环在收敛后执行）。
        // - 会话/oneshot 形态（无槽）：生命周期随 core 会话，显式停机「不适用」
        //   ——维持 NOT_APPLICABLE（解冻记录 T7 责任项的 darwin 部分至此为真：
        //   service 形态已存在，本 handler 不再是无条件 stub）。
        if self.runtime.explicit_stop.is_some() {
            self.runtime.request_explicit_stop();
            return Ok(Response::new(wire::ShutdownReply {
                outcome: wire::ShutdownOutcome::Accepted as i32,
            }));
        }
        Ok(Response::new(wire::ShutdownReply {
            outcome: wire::ShutdownOutcome::NotApplicable as i32,
        }))
    }
}

/// 平台管线的结果分类。
enum TunnelOutcome {
    /// 用户取消/Stop：已按序 teardown。
    Cancelled,
    /// CSTP 会话在持有期死亡。
    SessionLost,
    /// 重拨的真实协议终态；保留认证分类、阶段及最后错误。
    ProtocolFailed(crate::protocol::ConnectError),
    /// 平台步骤失败：步骤名、OS errno、DNS 诊断标记。
    Failed(&'static str, i32, i32),
}

/// 一次连接的完整平台管线：utun → 地址/MTU → 路由 → DNS → 双泵 → Connected →
/// 保持（DPD 应答 + keepalive + 泵监视）→ 逆序 teardown。
///
/// 失败时只逆序恢复已真实获取的资源；取消/会话死亡/平台失败都走同一条 teardown。
#[allow(clippy::too_many_lines)]
async fn run_tunnel_pipeline(
    runtime: &Arc<EngineRuntime>,
    mut established: EstablishedSession,
    operation_id: Vec<u8>,
    plan_mtu: u32,
    campus_routes: Vec<String>,
    cancel: &Arc<AtomicBool>,
    pipeline_identity: PipelineIdentity,
) -> TunnelOutcome {
    // CSTP owner在子管线外，早退或panic也必须收取其真实完成回执。
    let tasks = Arc::new(tokio::sync::Mutex::new(established.tasks.take()));
    let inner_tasks = Arc::clone(&tasks);
    let inner_runtime = Arc::clone(runtime);
    let cancel = Arc::clone(cancel);
    let outcome = tokio::spawn(async move {
        run_cisco_data_plane(
            &inner_runtime,
            established,
            operation_id,
            plan_mtu,
            campus_routes,
            &cancel,
            pipeline_identity,
            &inner_tasks,
        )
        .await
    })
    .await;
    finish_cisco_data_plane(runtime, outcome, &tasks).await
}

async fn finish_cisco_data_plane(
    runtime: &EngineRuntime,
    outcome: Result<TunnelOutcome, tokio::task::JoinError>,
    tasks: &tokio::sync::Mutex<Option<exv_vpn_cstp::session::CstpTasks>>,
) -> TunnelOutcome {
    close_cstp_tasks(runtime, tasks).await;
    match outcome {
        Ok(outcome) => outcome,
        Err(_) => {
            // 子任务已join；泵的Drop也取得双线程回执。live仅是标记，不是假资源。
            *runtime.live.lock().expect("live lock") = None;
            runtime.stats.registry().detach_pump();
            runtime
                .execution_errors
                .lock()
                .expect("execution errors lock")
                .push(super::stop_operations::stop_error(
                    "darwin/pipeline_task",
                    None,
                ));
            TunnelOutcome::Failed("PANIC", 0, 0)
        }
    }
}

async fn close_established(runtime: &EngineRuntime, established: &mut EstablishedSession) {
    if let Some(tasks) = established.tasks.as_mut() {
        if let Err(error) = tasks.close().await {
            runtime
                .execution_errors
                .lock()
                .expect("execution errors lock")
                .push(super::stop_operations::stop_error(
                    "darwin/cstp_close",
                    None,
                ));
            runtime.logs.publish(
                "warn",
                "cstp",
                "CSTP_CLOSE_COMPLETED_WITH_ERROR",
                error,
                &[],
            );
        }
        established.tasks.take();
    }
}

async fn close_cstp_tasks(
    runtime: &EngineRuntime,
    tasks: &tokio::sync::Mutex<Option<exv_vpn_cstp::session::CstpTasks>>,
) {
    let mut owner = tasks.lock().await;
    if let Some(tasks) = owner.as_mut() {
        if let Err(error) = tasks.close().await {
            runtime
                .execution_errors
                .lock()
                .expect("execution errors lock")
                .push(super::stop_operations::stop_error(
                    "darwin/cstp_close",
                    None,
                ));
            runtime.logs.publish(
                "warn",
                "cstp",
                "CSTP_CLOSE_COMPLETED_WITH_ERROR",
                error,
                &[],
            );
        }
        owner.take();
    }
}

fn queue_latency_probe(writer: &tokio::sync::mpsc::UnboundedSender<Vec<u8>>, frame: Vec<u8>) {
    let _ = writer.send(frame);
}
async fn run_cisco_data_plane(
    runtime: &EngineRuntime,
    established: EstablishedSession,
    operation_id: Vec<u8>,
    plan_mtu: u32,
    campus_routes: Vec<String>,
    cancel: &Arc<AtomicBool>,
    pipeline_identity: PipelineIdentity,
    cstp_tasks: &tokio::sync::Mutex<Option<exv_vpn_cstp::session::CstpTasks>>,
) -> TunnelOutcome {
    // T2（复审必改 4）：管线入口即切换事件归属到本代 operation——上一代若经
    // SessionLost 自拆（无收敛清位）残留旧归属，本代早期的阶段事件会被新 core
    // 按 operation_id 过滤丢弃。Connected 时刻的 record 会再次确认同一值。
    let operation_id = runtime.operation_for_event(&operation_id);
    runtime.adopt_operation(operation_id.clone());
    // Connected 时刻的 epoch ms：status 事件与会话时长诊断共用的单一事实源；
    // 0 = 尚未到达 Connected。Atomic 是因为闭包随管线 future 跨线程（Cell 非 Send）。
    let session_started_ms = std::sync::atomic::AtomicI64::new(0);
    let own_tunnel_if_index = std::sync::atomic::AtomicU32::new(0);
    let publish = |phase: wire::ConnectPhase, coarse: wire::StatsPhase| {
        // MAC-OBS-13 S1：连接状态推进同步进 sink（每阶段一条 info；字段只含
        // 白名单阶段名，无凭据/证书材料）。
        runtime.logs.publish(
            "info",
            "pipeline",
            "PHASE",
            format!("connect phase -> {phase:?} coarse {coarse:?}"),
            &[
                ("phase", format!("{phase:?}")),
                ("coarse", format!("{coarse:?}")),
            ],
        );
        let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
            aftermath_verified: false,
            reconnect_state: None,
            stop_completion: None,
            own_tunnel_if_index: own_tunnel_if_index.load(Ordering::Relaxed),
            session_established_at_ms: session_started_ms.load(Ordering::Relaxed),
            // T2：事件归属读 runtime 当前值（采纳时切换）；本地捕获值仅作
            // runtime 尚未记录时的兜底（管线 Connected 前无采纳，二者一致）。
            operation_id: {
                let current = runtime.current_operation_id();
                if current.is_empty() {
                    operation_id.clone()
                } else {
                    current
                }
            },
            connect_phase: phase as i32,
            coarse_phase: coarse as i32,
            error: None,
        });
    };
    let is_cancelled = || cancel.load(std::sync::atomic::Ordering::SeqCst);
    let errno_of = |error: &crate::platform::PlatformError| match error {
        crate::platform::PlatformError::Utun(_, e)
        | crate::platform::PlatformError::Interface(_, e)
        | crate::platform::PlatformError::Route(_, e) => e.raw_os_error().unwrap_or(0),
        _ => 0,
    };

    let EstablishedSession {
        offer,
        egress: _,
        write_channel,
        read_channel,
        mut control_rx,
        tasks: _,
    } = established;

    // C2 延迟探测：据真实 offer 构建探测配置（目标 = 客户端隧道子网网络基地址
    // `network_base`；真实学校网关应答沿 win32 S5 口径待真机校准）。节拍由本保持
    // 循环的 1s tick 驱动，回包匹配挂下行泵（LatencyTap），RTT 写共享统计注册表；
    // 手动刷新 marker 不做（见 `packet/probe.rs` 模块注释的取舍）。
    let latency_probe = Arc::new(Mutex::new(ProbeState::new(LatencyProbeConfig {
        target: crate::packet::probe::network_base(offer.ipv4_address, offer.prefix),
        source: offer.ipv4_address,
        dpd_enabled: crate::packet::probe::DPD_PROBE_ENABLED,
        ping_interval: std::time::Duration::from_secs(
            crate::packet::probe::LATENCY_PING_INTERVAL_SECS,
        ),
    })));

    // ---- 1. utun（AttachingPacketBoundary 真实入口）。----
    publish(
        wire::ConnectPhase::ApplyingPlatformTunnel,
        wire::StatsPhase::Connecting,
    );
    let device = match runtime.take_or_acquire_device() {
        Ok(device) => device,
        Err(error) => {
            // 登录前已可能写入控制 `/32`；即使 utun 创建失败也必须在非重试终态前
            // 尝试收口，不能让失败分支绕开 ledger。
            let _ = runtime.complete_resource_cleanup();
            return TunnelOutcome::Failed("UTUN", errno_of(&error), 0);
        }
    };
    own_tunnel_if_index.store(device.ifindex(), Ordering::Relaxed);
    publish(
        wire::ConnectPhase::AttachingPacketBoundary,
        wire::StatsPhase::Connecting,
    );

    // ---- 2. 地址/MTU/路由/DNS（ApplyingPlatformTunnel；失败逆序回滚已应用项）。----
    let mut applied_routes: Vec<AppliedRoute> = Vec::new();
    let effective_mtu = if plan_mtu > 0 && plan_mtu < u32::from(offer.mtu) {
        u16::try_from(plan_mtu).unwrap_or(offer.mtu)
    } else {
        offer.mtu
    };
    let mut negotiated = crate::session_diagnostics::identity(&operation_id);
    negotiated.extend(crate::session_diagnostics::build_fields());
    negotiated.extend([
        ("offer_mtu", offer.mtu.to_string()),
        ("requested_mtu", plan_mtu.to_string()),
        ("effective_mtu", effective_mtu.to_string()),
        ("tunnel_ifindex", device.ifindex().to_string()),
    ]);
    runtime.logs.publish(
        "info",
        "cstp",
        "TUNNEL_EFFECTIVE_PARAMETERS",
        "隧道待应用参数",
        &negotiated,
    );

    if let Err(error) = crate::platform::interface::apply_ipv4(
        &device,
        offer.ipv4_address,
        offer.prefix,
        effective_mtu,
    ) {
        let errno = errno_of(&error);
        let substep = match &error {
            crate::platform::PlatformError::Interface(name, _)
            | crate::platform::PlatformError::Readback(name) => name,
            _ => "ADDRESS",
        };
        runtime.logs.publish(
            "error",
            "platform",
            "ADDRESS_FAILED",
            format!("address failed at {substep}: {error}"),
            &[
                ("substep", substep.to_owned()),
                ("errno", errno.to_string()),
            ],
        );
        drop(device);
        // 地址阶段失败时业务路由尚未开始，但控制 `/32` 已在 establish 前创建；统一走
        // ledger，保证后续非重试 Failed 之前没有被遗漏的专用路由或旧 idle utun。
        let _ = runtime.complete_resource_cleanup();
        return TunnelOutcome::Failed(substep, errno, 0);
    }

    let mut wanted: Vec<(Ipv4Addr, u8)> = Vec::new();
    for text in offer.routes.iter().chain(campus_routes.iter()) {
        if let Some(cidr) = parse_cidr(text)
            && !wanted.contains(&cidr)
        {
            wanted.push(cidr);
        }
    }
    for server in &offer.dns_servers {
        let cidr = (*server, 32);
        if !wanted.contains(&cidr) {
            wanted.push(cidr);
        }
    }
    let mut route_failure: Option<(&'static str, i32)> = None;
    for (destination, prefix) in wanted {
        // 网关 = 本隧道地址（INET 网关形态，Clash 同款）：AF_LINK 接口路由形态
        // 在 p2p utun 上建路由时 rt_ifa 解析不到接口地址，未绑定源的 sendto 报
        // EADDRNOTAVAIL；经本隧道地址转发时内核按网关命中 dstaddr=自身，源选择
        // 取本接口地址。
        let applied = AppliedRoute {
            destination,
            prefix,
            gateway: offer.ipv4_address,
            ifindex: device.ifindex(),
        };
        if let Err(error) = platform_route::add(&applied) {
            let errno = match &error {
                crate::platform::PlatformError::Route(_, e) => e.raw_os_error().unwrap_or(0),
                _ => 0,
            };
            route_failure = Some(("ROUTE", errno));
            break;
        }
        runtime.remember_route_cleanup_failure(applied);
        applied_routes.push(applied);
    }

    if let Some((step, errno)) = route_failure {
        // 逆序恢复：路由 → utun（系统 DNS 未被修改，无需恢复）。
        for route in applied_routes.drain(..).rev() {
            if platform_route::delete(&route).is_ok() {
                runtime
                    .resources
                    .lock()
                    .expect("resource ledger lock")
                    .pending_routes
                    .retain(|owned| owned != &route);
            }
        }
        drop(device);
        let _ = runtime.complete_resource_cleanup();
        return TunnelOutcome::Failed(step, errno, 0);
    }

    // 路由已就绪，才安装用户显式选择的域；不修改物理网卡或全局 DNS。
    if let Err(error) = runtime.install_cisco_resolvers(&pipeline_identity.resolver_domains, &offer.dns_servers) {
        let _ = runtime.complete_resource_cleanup();
        drop(device);
        return TunnelOutcome::Failed(error.reason(), 0, 0);
    }

    runtime.apply_cisco_proxy_policy(&applied_routes, &pipeline_identity);
    // ---- 3. 双泵（StartingDataPlane 真实入口）+ Connected。----
    publish(
        wire::ConnectPhase::StartingDataPlane,
        wire::StatsPhase::Connecting,
    );
    let pumps = pump::start(
        device.fd(),
        write_channel.clone(),
        read_channel,
        &runtime.logs,
        Some(LatencyTap::for_execution(
            Arc::clone(&latency_probe),
            Arc::clone(runtime.stats.registry()),
            Arc::clone(cancel),
        )),
    );
    let mut latency_registration = runtime.install_latency_owner(
        Arc::clone(&latency_probe), Arc::clone(cancel),
    );
    // W-MVP-13：挂接本次双泵计数器并把粗阶段置为 Connected（统计样本的事实源）。
    runtime
        .stats
        .registry()
        .attach_pump(Arc::clone(&pumps.rx_bytes), Arc::clone(&pumps.tx_bytes));
    runtime
        .stats
        .registry()
        .set_phase(wire::StatsPhase::Connected);
    // 数据面真实运行即会话起点——Connected 事件带上真实时间戳（前端在线时长的
    // 事实源；此前恒 0 导致 UI 永远显示「—」）。
    session_started_ms.store(now_epoch_ms(), Ordering::Relaxed);
    publish(
        wire::ConnectPhase::StartingDataPlane,
        wire::StatsPhase::Connected,
    );
    // T2：Connected 事实记录——身份（采纳判定）、ifindex/纪元（采纳事件重放）、
    // 事件归属（采纳切换）与 keep 策略（连接成功即 keep 的生产注入点）。
    runtime.record_pipeline_connected(
        pipeline_identity,
        own_tunnel_if_index.load(Ordering::Relaxed),
        session_started_ms.load(Ordering::Relaxed),
        operation_id.clone(),
    );

    // ---- 4. 保持：DPD 应答 + 周期 keepalive + 泵监视。----
    let keepalive = Codec::encode_raw(CSTP_PACKET_TYPE_KEEPALIVE, &[]).expect("keepalive frame");
    let dpd_response =
        Codec::encode_raw(CSTP_PACKET_TYPE_DPD_RESPONSE, &[]).expect("dpd response frame");
    if let Ok(mut live) = runtime.live.lock() {
        *live = Some(HeldTunnel);
    }
    let mut diagnostics = crate::session_diagnostics::SessionDiagnostics::new();
    let mut session_lost = false;
    let mut ticks: u32 = 0;
    let mut probe_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1), Duration::from_secs(1),
    );
    probe_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if is_cancelled()
            || runtime.shutdown.load(Ordering::SeqCst)
            || TERMINATION_REQUESTED.load(Ordering::SeqCst)
        {
            break;
        }
        if runtime.consume_reconnect_fault() {
            // 终止本次真实 CSTP 任务；原控制通道/泵观察和 SessionLost 出口接续。
            close_cstp_tasks(runtime, cstp_tasks).await;
        }
        if pumps.either_finished() {
            // 任一泵死亡 = 数据面失败，不维持 Connected。
            runtime.logs.publish(
                "error",
                "packet",
                "PUMP_EXITED",
                "a pump thread exited while tunnel held",
                &[],
            );
            session_lost = true;
            break;
        }
        tokio::select! {
            biased;
            control = control_rx.recv() => {
                if let Some(event) = &control { diagnostics.observe(event, &runtime.logs, &operation_id); }
                match control {
                Some(CstpControlEvent::Control { kind: CSTP_PACKET_TYPE_DPD_REQUEST, .. }) => {
                    if write_channel.send(dpd_response.clone()).is_err() {
                        diagnostics.queue_failures += 1; session_lost = true;
                    } else { diagnostics.dpd_reply_queued += 1; }
                }
                Some(CstpControlEvent::DpdResponse) => {
                    // C2 主动 DPD 探测（默认关，`DPD_PROBE_ENABLED`）的应答计时；
                    // 与上一分支网关 DPD request 的被动应答（会话维持）是不同路径。
                    if let Some(rtt) = latency_probe
                        .lock()
                        .expect("latency probe lock")
                        .on_dpd_response(std::time::Instant::now())
                    {
                        runtime.stats.registry().record_latency(rtt);
                    }
                }
                Some(CstpControlEvent::Control { kind: CSTP_PACKET_TYPE_KEEPALIVE, .. }) => {}
                Some(CstpControlEvent::Control { kind, .. }) => {
                    // 保留非平凡控制帧的实际类型；完整结束原因由会话事件另行携带。
                    runtime.logs.publish(
                        "warn",
                        "cstp",
                        "CSTP_CONTROL_FRAME",
                        format!("server sent control frame kind={kind:#04x}"),
                        &[
                            ("kind", format!("{kind:#04x}")),
                            (
                                "connected_for_ms",
                                elapsed_since_ms(session_started_ms.load(Ordering::Relaxed)).to_string(),
                            ),
                        ],
                    );
                }
                Some(CstpControlEvent::WriteFailed(_) | CstpControlEvent::SessionEnded(_)) => {
                    session_lost = true;
                }
                Some(CstpControlEvent::IoDiagnostics(_)) => {}
                None => {
                    runtime.logs.publish(
                        "warn", "cstp", "CSTP_CONTROL_CHANNEL_CLOSED",
                        "CSTP 控制事件通道已结束，按现有会话丢失流程处理",
                        &crate::session_diagnostics::identity(&runtime.operation_for_event(&operation_id)),
                    );
                    session_lost = true;
                }
            }},
            _ = probe_tick.tick() => {
                ticks += 1;
                if ticks.is_multiple_of(15) {
                    if write_channel.send(keepalive.clone()).is_err() {
                        diagnostics.queue_failures += 1; session_lost = true;
                    } else { diagnostics.keepalive_queued += 1; }
                }
                // C2 延迟探测节拍：周期 ping / 超时回退。探测包经 write_channel 注入
                // 上行（CSTP Data 帧）；发送失败 = TLS 写任务已退出 → 尽力而为忽略
                // （对齐 win32），掉线由泵/控制面观察点上报。
                latency_probe
                    .lock()
                    .expect("latency probe lock")
                    .tick(std::time::Instant::now(), &mut |frame: Vec<u8>| {
                        queue_latency_probe(&write_channel, frame);
                    });
            }
            Some(generation) = latency_registration.receiver.recv() => {
                runtime.process_latency_refresh(generation, std::time::Instant::now(), &mut |frame| {
                    queue_latency_probe(&write_channel, frame);
                });
            }
        }
        if session_lost {
            break;
        }
    }

    // 先停止本代探测与 tap，再沿既有真实资源链逆序 teardown。
    drop(latency_registration);
    // ---- 5. 逆序 teardown：泵 → CSTP → resolver → 路由 → utun。----
    // 有界收取已经到达的最终样本，不等待网络任务、不改变清理顺序。
    for _ in 0..256 {
        match control_rx.try_recv() {
            Ok(event) => diagnostics.observe(&event, &runtime.logs, &operation_id),
            Err(_) => break,
        }
    }
    diagnostics.emit(
        &runtime.logs,
        &operation_id,
        if session_lost {
            "session_lost"
        } else {
            "stop_requested"
        },
    );
    let teardown_started = std::time::Instant::now();
    runtime.stats.registry().set_phase(wire::StatsPhase::Idle);
    runtime.stats.registry().detach_pump();
    if !pumps.shutdown() {
        runtime
            .execution_errors
            .lock()
            .expect("execution errors lock")
            .push(super::stop_operations::stop_error("darwin/pump_join", None));
    }
    drop(write_channel);
    drop(control_rx);
    // 真实回执在路由/设备拆除之前收取；已结束的协议错误不冒充仍存活的资源。
    close_cstp_tasks(runtime, cstp_tasks).await;
    let mut teardown_complete = runtime.cleanup_proxy_policy();
    teardown_complete &= runtime.cleanup_resolvers();
    for route in applied_routes.drain(..) {
        if let Err(error) = platform_route::delete(&route) {
            let (step, errno) = match &error {
                crate::platform::PlatformError::Route(step, source) => {
                    (*step, source.raw_os_error())
                }
                _ => ("DELETE", None),
            };
            let mut fields =
                crate::session_diagnostics::identity(&runtime.operation_for_event(&operation_id));
            fields.extend([
                ("destination", route.destination.to_string()),
                ("prefix", route.prefix.to_string()),
                ("ifindex", route.ifindex.to_string()),
                ("step", step.to_owned()),
                (
                    "errno",
                    errno.map_or_else(|| "none".to_owned(), |value| value.to_string()),
                ),
            ]);
            runtime.logs.publish(
                "warn",
                "platform",
                "ROUTE_DELETE_FAILED",
                format!("route deletion failed before ledger retry: {error}"),
                &fields,
            );
            runtime.remember_route_cleanup_failure(route);
            teardown_complete = false;
        } else {
            runtime
                .resources
                .lock()
                .expect("resource ledger lock")
                .pending_routes
                .retain(|owned| owned != &route);
        }
    }
    // 只有业务路由均已删除时才允许把 utun 留给重连。若先前删除失败，先走完整
    // ledger 清理并放弃复用；不得让“空闲 utun”与残留 VPN 专用路由同时存在。
    let preserve_for_reconnect = session_lost && !is_cancelled() && teardown_complete;
    if preserve_for_reconnect {
        if let Err(error) = crate::platform::interface::clear_ipv4(&device, offer.ipv4_address) {
            runtime.logs.publish(
                "error",
                "platform",
                "IDLE_UTUN_ADDRESS_CLEAR_FAILED",
                format!("failed to remove address from retained utun: {error}"),
                &[],
            );
            // 保留带旧地址的 utun 会继续拥有源地址，违反空闲设备边界；失败时宁可
            // 释放设备，并由下一次重连重新创建。
            teardown_complete = false;
            drop(device);
            if let Err(cleanup_error) = runtime.complete_resource_cleanup() {
                runtime.logs.publish(
                    "error",
                    "engine",
                    "TEARDOWN_RESOURCE_REMAINS",
                    format!("resource cleanup incomplete: {cleanup_error}"),
                    &[],
                );
            } else {
                // 地址移除失败后不复用本设备；ledger 已成功收口时允许后续以新设备重连。
                teardown_complete = true;
            }
        } else {
            runtime.retain_device(device);
        }
    } else {
        drop(device);
        if let Err(error) = runtime.complete_resource_cleanup() {
            teardown_complete = false;
            runtime.logs.publish(
                "error",
                "engine",
                "TEARDOWN_RESOURCE_REMAINS",
                format!("resource cleanup incomplete: {error}"),
                &[],
            );
        } else {
            // 先前一次逐条删除可以暂态失败；完整账本重试若成功，资源已如实清空，
            // 仅放弃本轮 utun 复用，下一次连接重新读取出口并创建设备即可。
            teardown_complete = true;
        }
    }
    let mut teardown_fields = crate::session_diagnostics::identity(&operation_id);
    teardown_fields.push((
        "elapsed_ms",
        teardown_started.elapsed().as_millis().to_string(),
    ));
    runtime.logs.publish(
        if teardown_complete { "info" } else { "error" },
        "engine",
        if teardown_complete {
            "TEARDOWN_COMPLETED"
        } else {
            "TEARDOWN_INCOMPLETE"
        },
        if teardown_complete {
            "隧道资源已完成清理"
        } else {
            "隧道资源清理未完成，失败条目保留待重试"
        },
        &teardown_fields,
    );
    TERMINATION_TEARDOWN_DONE.store(true, Ordering::SeqCst);
    if let Ok(mut live) = runtime.live.lock() {
        *live = None;
    }

    if session_lost && !is_cancelled() {
        TunnelOutcome::SessionLost
    } else {
        TunnelOutcome::Cancelled
    }
}

#[derive(Debug)]
enum AtrustOutcome {
    Cancelled,
    SessionLost(
        &'static str,
        Option<crate::atrust_tunnel::AtrustCloseObservation>,
    ),
    Failed(&'static str, i32, i32),
}
/// aTrust 连接任务的取消轮询间隔（认证/数据面组装 future 的 drop 语义由
/// select 分支保证：cancel 触发即放弃当前阶段 future——Common P3 纪律，本地
/// TLS/TCP 连接随 future 释放，不宣称服务端注销）。
const ATRUST_CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// M6：aTrust 建立阶段（egress→Underlay→认证→数据面组装）的分类结局。初始
/// 连接与 R1 重拨共用同一建立路径；调用方按轮次决定失败处置——初始失败=终态
/// 投影，重拨失败=预算内退避重试（对齐 Cisco R1 的 `R1_RECONNECT_FAILED`）。
enum AtrustEstablishOutcome {
    /// 建立完成：物理出口事实 + 数据面组装产物（节点 TLS+L3 握手完成）。
    Ready {
        egress: crate::egress::physical_route::PhysicalEgress,
        prepared: PreparedAtrust,
    },
    /// 取消（Stop/退出收敛置位；终态由 Stop 流回复承担）。
    Cancelled,
    /// Source 复核 isLogin=0：驻留/缓存会话已失效（跨仓契约判别——M4 A4）。
    /// R3 短路：不耗重拨预算、不循环，直转终态链。
    SessionInvalidated,
    /// 其余失败（终态投影三元组：稳定 reason/VpnError/相位——与 M3 初始连接
    /// 的既有投影逐字同源）。
    Failed {
        reason: &'static str,
        error: wire::VpnError,
        phase: wire::ConnectPhase,
    },
}

/// M6 建立阶段的注入面（「establish 注入接缝」——基建轮挂账上调项，本轮兑现）：
/// 生产=真实 egress/认证/组装（[`atrust_establish_phase`]）；密闭测试注入假
/// 建立以驱动 R1 环全链（重拨/失效短路/旁路重推导不依赖真实网络）。
type AtrustEstablish = std::sync::Arc<
    dyn Fn(
            AtrustMaterials,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = AtrustEstablishOutcome> + Send>>
        + Send
        + Sync,
>;

/// 真实建立阶段（生产唯一实现）：物理出口 Underlay → Common 六请求认证 →
/// 数据面组装（节点选择→节点 TLS→L3 握手）。阶段事件（ConnectingControl/
/// NegotiatingTunnel）与非秘密计数日志（AUTH_COMPLETED/DATA_PLANE_READY）与
/// M3 的既有投影同源——初始连接与 R1 重拨每轮都发布（UI 沿同一 operation_id
/// 看到"重连中"的相位回退）。
async fn atrust_establish_phase(
    runtime: &EngineRuntime,
    materials: &AtrustMaterials,
    cancel: &Arc<AtomicBool>,
    operation_id: &[u8],
    plan_mtu: u32,
) -> AtrustEstablishOutcome {
    let is_cancelled = || cancel.load(Ordering::SeqCst);
    let cancel_watch = || async {
        loop {
            if is_cancelled() {
                return;
            }
            tokio::time::sleep(ATRUST_CANCEL_POLL_INTERVAL).await;
        }
    };
    let publish_phase = |phase: wire::ConnectPhase| {
        let operation = {
            let current = runtime.current_operation_id();
            if current.is_empty() {
                operation_id.to_vec()
            } else {
                current
            }
        };
        let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
            aftermath_verified: false,
            reconnect_state: None,
            stop_completion: None,
            own_tunnel_if_index: 0,
            session_established_at_ms: 0,
            operation_id: operation,
            connect_phase: phase as i32,
            coarse_phase: wire::StatsPhase::Connecting as i32,
            error: None,
        });
    };

    // 阶段 ConnectingControl：认证编排（对齐 establish_observed 在登录前发布
    // ConnectingControl 的相位）。
    publish_phase(wire::ConnectPhase::ConnectingControl);
    // 物理出口 Underlay：接口选择（生产=当前默认物理接口，解析与拨号同接口）。
    // egress 事实（网关+ifindex）一并捕获——M4 旁路账本的 /32 行走同一物理网关
    // （discover_default 的等价展开：find_physical_egress + 同口绑定）。R1 重拨
    // 每轮重探（物理出口可能已切换）。
    let egress = match crate::egress::physical_route::find_physical_egress() {
        Ok(egress) => egress,
        Err(error) => {
            runtime.logs.publish(
                "error",
                "engine",
                atrust_reason::EGRESS_FAILED,
                format!("aTrust 物理出口构造失败: {error}"),
                &[],
            );
            return AtrustEstablishOutcome::Failed {
                reason: atrust_reason::EGRESS_FAILED,
                error: wire::VpnError {
                    code: wire::ErrorCode::ObservationFailed as i32,
                    stage: wire::ErrorStage::ConnectingControl as i32,
                    ..Default::default()
                },
                phase: wire::ConnectPhase::ConnectingControl,
            };
        }
    };
    let underlay = match DarwinPhysicalUnderlay::bound_to_interface(egress.ifindex) {
        Ok(underlay) => Arc::new(underlay),
        Err(error) => {
            runtime.logs.publish(
                "error",
                "engine",
                atrust_reason::EGRESS_FAILED,
                format!("aTrust 物理出口构造失败: {error}"),
                &[],
            );
            return AtrustEstablishOutcome::Failed {
                reason: atrust_reason::EGRESS_FAILED,
                error: wire::VpnError {
                    code: wire::ErrorCode::ObservationFailed as i32,
                    stage: wire::ErrorStage::ConnectingControl as i32,
                    ..Default::default()
                },
                phase: wire::ConnectPhase::ConnectingControl,
            };
        }
    };

    // 认证编排 vs 取消：select 放弃即认证 future drop（本地连接释放）。
    let authentication = atrust_auth::authenticate_with_materials(
        &materials.gateway,
        &materials.browser_session,
        &materials.platform,
        &materials.language,
        &materials.resource_request,
        underlay.clone(),
    );
    tokio::pin!(authentication);
    let outcome = tokio::select! {
        biased;
        () = cancel_watch() => None,
        result = &mut authentication => Some(result),
    };
    let authenticated = match outcome {
        None => return AtrustEstablishOutcome::Cancelled,
        Some(Err(error)) => {
            // R3 判别先行（M4 A4 跨仓契约）：Source 复核 isLogin=0 = 会话失效，
            // 与传输错误分流（失效短路不耗预算）。
            if let AtrustConnectError::Auth(auth) = &error
                && atrust_auth::source_session_invalidated(auth)
            {
                runtime.logs.publish(
                    "error",
                    "engine",
                    atrust_reason::SESSION_INVALIDATED,
                    "aTrust 源会话已失效（Source 复核 isLogin=0）——R3 终态，不重拨",
                    &[],
                );
                return AtrustEstablishOutcome::SessionInvalidated;
            }
            match &error {
                AtrustConnectError::Auth(auth) => {
                    runtime.logs.publish(
                        "error",
                        "engine",
                        error.reason(),
                        format!("aTrust 认证失败（阶段 {:?}）: {auth}", auth.stage),
                        &[],
                    );
                }
                other => {
                    runtime.logs.publish(
                        "error",
                        "engine",
                        other.reason(),
                        format!("aTrust 认证编排失败: {other}"),
                        &[],
                    );
                }
            }
            return AtrustEstablishOutcome::Failed {
                reason: error.reason(),
                error: error.vpn_error(),
                phase: wire::ConnectPhase::ConnectingControl,
            };
        }
        Some(Ok(authenticated)) => authenticated,
    };

    // 认证完成（非秘密计数日志；不再发布中间态终态——B4 消解）。
    let catalog = authenticated.catalog();
    runtime.logs.publish(
        "info",
        "engine",
        atrust_reason::AUTH_COMPLETED,
        "aTrust 认证编排完成（接数据面）",
        &[
            ("resource_bytes", authenticated.resource_bytes().to_string()),
            ("apps", catalog.apps().len().to_string()),
            ("node_groups", catalog.node_groups().len().to_string()),
        ],
    );

    // 阶段 NegotiatingTunnel：节点选择 + 节点 TLS + L3 握手（对齐 Cisco 在 CSTP
    // CONNECT 前发布 NegotiatingTunnel 的相位）。借用作用域收束：pin 住的组装
    // future 持有认证产物引用，块结束即释放，产物随后可移动销毁。
    publish_phase(wire::ConnectPhase::NegotiatingTunnel);
    let prepared = {
        let preparation = atrust_prepare::prepare_data_plane(
            &authenticated,
            &materials.gateway,
            &materials.platform,
            underlay.as_ref(),
            plan_mtu,
        );
        tokio::pin!(preparation);
        tokio::select! {
            biased;
            () = cancel_watch() => None,
            result = &mut preparation => Some(result),
        }
    };
    let prepared = match prepared {
        None => return AtrustEstablishOutcome::Cancelled,
        Some(Err(error)) => {
            runtime.logs.publish(
                "error",
                "engine",
                error.reason(),
                format!("aTrust 数据面组装失败: {error}"),
                &[],
            );
            return AtrustEstablishOutcome::Failed {
                reason: error.reason(),
                error: error.vpn_error(),
                phase: wire::ConnectPhase::NegotiatingTunnel,
            };
        }
        Some(Ok(prepared)) => prepared,
    };
    drop(authenticated);
    runtime.logs.publish(
        "info",
        "engine",
        atrust_reason::DATA_PLANE_READY,
        "aTrust 数据面就绪（L3 握手完成，双泵接管 utun）",
        &[
            ("vip", prepared.offer.ipv4_address.to_string()),
            ("mtu", prepared.offer.mtu.to_string()),
            (
                "campus_dns_servers",
                prepared.campus_dns_servers.len().to_string(),
            ),
            ("bypasses", prepared.bypasses.len().to_string()),
        ],
    );
    AtrustEstablishOutcome::Ready { egress, prepared }
}

/// 生产的建立注入（[`atrust_establish_phase`] 的 Arc 包装）。
fn production_atrust_establish(
    runtime: Arc<EngineRuntime>,
    operation_id: Vec<u8>,
    plan_mtu: u32,
) -> AtrustEstablish {
    Arc::new(move |materials| {
        let runtime = Arc::clone(&runtime);
        let operation_id = operation_id.clone();
        Box::pin(async move {
            // cancel 句柄在每轮建立时从 attempt 槽取当前值（与连接任务同一来源；
            // R1 重拨重新声明的 attempt 槽携带同一 cancel Arc）。
            let cancel = runtime
                .attempt
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(|attempt| Arc::clone(&attempt.cancel)))
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
            atrust_establish_phase(&runtime, &materials, &cancel, &operation_id, plan_mtu).await
        })
    })
}

/// M3+M6（darwin aTrust）：V2 aTrust 载荷的完整连接任务——建立（egress→认证→
/// 数据面组装）→ M4 旁路账本 → 平台管线（utun/路由/双泵/保持环）→ **R1 重拨环**
/// （SessionLost→按驻留材料重拨，预算/退避/attempt 槽/owner 开关门与 Cisco R1
/// 同一套）。
///
/// ## 成功/失败边界（M3 钉死、M6 补 R1）
///
/// - **成功** = 认证完成后**继续接数据面**：L3 握手成功（VIP 已分配、
///   `run_packets` worker 运行、双泵接管 utun）即由管线发布**真实 Connected**
///   （StartingDataPlane 相位事件，对齐 Cisco 时序）。认证完成只发非秘密计数
///   日志，不发终态事件。
/// - **初始建立失败**（认证/组装/旁路，首轮）：M3 语义不变——稳定 reason 码 +
///   typed `VpnError` 终态事件。
/// - **R1 重拨**（SessionLost 后，owner `auto_reconnect` 开启时）：
///   - 材料臂取 BrowserSession→重跑 browser_handoff（**isLogin 复核**）；
///   - **认证失效短路（R3）**：Source 复核 isLogin=0 → 不耗预算直转终态链
///     （稳定 reason `DARWIN_ATRUST_SESSION_INVALIDATED`）；
///   - **节点旁路重推导**（二轮 A5，M4 接缝兑现）：establish 成功后、管线前按
///     新目录 `PreparedAtrust.bypasses` 调 `reconcile_bypass_routes`（先删不再需要的旧行，再加新行）；
///   - 其余建立失败 = 预算内退避重试（`R1_RECONNECT_FAILED`）；预算耗尽 =
///     `R1_EXHAUSTED` + `Failed("R1")` 终态（对齐 Cisco）；
///   - **边界（二轮 D4）**：R1 只用引擎内存材料，不咨询 core 磁盘缓存——用户在
///     退避窗口完成新登录时分叉（旧材料→失效终态），恢复走手动重连。
/// - 取消（Stop/退出收敛）：select 放弃当前阶段 future（本地连接释放）；建立
///   阶段取消发布既有 Cancelled 形态终态；管线阶段取消由管线 teardown 收口。
#[allow(clippy::too_many_lines)]
fn spawn_atrust_connect_attempt(
    runtime: Arc<EngineRuntime>,
    atrust: DarwinAtrustConnect,
    plan_mtu: u32,
    campus_routes: Vec<String>,
    operation_id: Vec<u8>,
    cancel: Arc<AtomicBool>,
    r1_enabled: bool,
) {
    let establish =
        production_atrust_establish(Arc::clone(&runtime), operation_id.clone(), plan_mtu);
    tokio::spawn(async move {
        let _execution = ExecutionCompletion {
            runtime: Arc::clone(&runtime),
            cancel: Arc::clone(&cancel),
        };
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let identity = PipelineIdentity {
            server: atrust.gateway().to_owned(),
            username: atrust_auth::ATRUST_IDENTITY_USERNAME_PLACEHOLDER.to_owned(),
            protocol: ProtocolKind::Atrust,
            atrust_dns_enabled: atrust.atrust_dns_enabled(),
            resolver_domains: atrust.resolver_domains().to_vec(),
        };
        run_atrust_connection(
            &runtime,
            identity,
            plan_mtu,
            campus_routes,
            operation_id,
            cancel,
            establish,
            &|| r1_enabled,
        )
        .await;
    });
}

/// M6：aTrust 连接任务的 R1 环主体（建立→旁路账本→平台管线→SessionLost 重拨）。
/// 生产入口 [`spawn_atrust_connect_attempt`]；密闭测试直接调用并注入假建立与
/// owner 开关（「establish 注入接缝」——不依赖真实网络即可驱动重拨全链）。
///
/// 环结构对齐 Cisco R1（`apply_tunnel` 的 Cisco 分支）：
/// - SessionLost → owner `auto_reconnect` 门（D3：关=基线终态，与 core 侧 R2
///   同一开关同一语义）→ 预算（D1：SessionLost 与建立失败都计数）→ 退避
///   （`r1_backoff_delay`）→ 重拨（完整owner持续持有attempt供Stop取消）；
/// - 重拨轮次从**材料臂**（apply 受理时捕获的 BrowserSession/身份值/资源字节）
///   重跑 browser_handoff（isLogin 复核——二轮 D4：只用引擎内存材料，不咨询
///   core 磁盘缓存）；
/// - **认证失效短路（R3）**：Source 复核 isLogin=0 → 不耗预算直转终态链；
/// - **节点旁路重推导**（二轮 A5，M4 接缝兑现）：每轮 establish 成功后、管线
///   前按新目录 `PreparedAtrust.bypasses` reconcile 旁路账本（先删旧集之外的行，再增加新集）；初始轮失败=PLATFORM_STEP_FAILED 终态
///   （M4 语义），重拨轮失败=预算内重试。
#[allow(clippy::too_many_lines)]
#[allow(
    clippy::too_many_arguments,
    reason = "注入面参数与 Cisco R1 环的任务上下文同集——建立/身份/计划/操作/取消/owner 门逐项注入是密闭测试的接缝要求"
)]
async fn run_atrust_connection(
    runtime: &Arc<EngineRuntime>,
    identity: PipelineIdentity,
    plan_mtu: u32,
    campus_routes: Vec<String>,
    operation_id: Vec<u8>,
    cancel: Arc<AtomicBool>,
    establish: AtrustEstablish,
    owner_reconnect: &(dyn Fn() -> bool + Sync),
) {
    // 接缝也只在环入口读取一次；生产值来自 Apply 真正受理时的冻结快照。
    let r1_enabled = owner_reconnect();
    let is_cancelled = || cancel.load(Ordering::SeqCst);
    // M6 复审 B-4（M7 兑现）：终态事件的归属读 runtime 当前值（采纳时切换）；
    // 本地捕获值仅作 runtime 尚未记录时的兜底——对齐 Cisco T2 必改 2 与管线
    // `publish` 闭包的同一纪律。否则采纳窗口下（新 core 采纳同一管线后）旧
    // R1 环的失效终态会带着旧 operation_id 发布，被新 core 按 id 过滤丢弃，
    // 终态失明只剩心跳 15s 兜底。
    let event_operation_id = || {
        let current = runtime.current_operation_id();
        if current.is_empty() {
            operation_id.clone()
        } else {
            current
        }
    };
    let terminal = |reason: &'static str,
                    error: wire::VpnError,
                    log_level: &'static str,
                    phase: wire::ConnectPhase| {
        let mut error = error;
        error.reason = reason.to_owned();
        let operation_id = event_operation_id();
        runtime.logs.publish(
            log_level,
            "engine",
            reason,
            "aTrust 连接任务终态（M6：认证+数据面+R1）",
            &[("operation_len", operation_id.len().to_string())],
        );
        let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
            aftermath_verified: false,
            reconnect_state: None,
            stop_completion: None,
            own_tunnel_if_index: 0,
            session_established_at_ms: 0,
            operation_id,
            connect_phase: phase as i32,
            coarse_phase: wire::StatsPhase::Failed as i32,
            error: Some(error),
        });
    };
    let cancelled_event = || {
        let operation_id = event_operation_id();
        let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
            aftermath_verified: false,
            reconnect_state: None,
            stop_completion: None,
            own_tunnel_if_index: 0,
            session_established_at_ms: 0,
            operation_id,
            connect_phase: wire::ConnectPhase::ConnectingControl as i32,
            coarse_phase: wire::StatsPhase::Failed as i32,
            error: Some(ConnectCancelled::vpn_error()),
        });
    };
    // 事件归属：环入口切换到本代 operation（与 M3 行为一致；重拨轮沿用当前归属
    // ——采纳后归属已切换，事件经 runtime.current_operation_id 投影）。
    let operation_id = runtime.operation_for_event(&operation_id);
    runtime.adopt_operation(operation_id.clone());

    let mut r1_attempt = 0_u32;
    // 首轮失败=终态投影（M3 语义）；首轮成功后进入重拨语境（失败=预算内重试）。
    let mut initial = true;
    let outcome = loop {
        // ---- 建立阶段（egress→Underlay→认证→数据面组装；每轮全量重跑）----
        // 材料取自驻留槽（apply 受理时捕获；与 Cisco R1 同一来源）：善后清位
        // （Stop/收敛）后材料缺席→不重拨（基线终态），不带着孤儿材料重拨；
        // 臂不匹配（Cisco 臂驻留）等同材料缺席——防御性不跨协议误用。
        let Some(ReconnectMaterials::Atrust(resident)) = runtime.reconnect_materials() else {
            break AtrustOutcome::SessionLost("session_lost", None);
        };
        let resolver_domains = resident.resolver_domains.clone();
        let atrust_dns_enabled = resident.atrust_dns_enabled;
        let (egress, mut prepared) = match establish(resident).await {
            AtrustEstablishOutcome::Ready { egress, prepared } => (egress, prepared),
            AtrustEstablishOutcome::Cancelled => {
                cancelled_event();
                break AtrustOutcome::Cancelled;
            }
            AtrustEstablishOutcome::SessionInvalidated => {
                // R3 短路：终态已由 terminal 投影——以 Cancelled 出环（环外投影
                // 为 no-op；不耗重拨预算、不循环）。
                terminal(
                    atrust_reason::SESSION_INVALIDATED,
                    atrust_auth::session_invalidated_vpn_error(),
                    "error",
                    wire::ConnectPhase::ConnectingControl,
                );
                break AtrustOutcome::Cancelled;
            }
            AtrustEstablishOutcome::Failed {
                reason,
                error,
                phase,
            } => {
                if initial {
                    terminal(reason, error, "error", phase);
                    break AtrustOutcome::Cancelled;
                }
                // R1 重拨的建立失败：预算内退避重试（日志已由建立阶段发布；此处
                // 记 R1 事实，对齐 Cisco R1_RECONNECT_FAILED）。
                runtime.logs.publish(
                    "error",
                    "protocol",
                    "R1_RECONNECT_FAILED",
                    "engine-side aTrust reconnect establish failed",
                    &[
                        ("attempt", r1_attempt.to_string()),
                        ("reason", reason.to_owned()),
                    ],
                );
                if r1_attempt >= R1_MAX_ATTEMPTS {
                    runtime.logs.publish(
                        "error",
                        "pipeline",
                        "R1_EXHAUSTED",
                        "engine-side reconnect budget exhausted",
                        &[("attempts", r1_attempt.to_string())],
                    );
                    if let Err(cleanup_error) = runtime.complete_resource_cleanup() {
                        runtime.logs.publish(
                            "error",
                            "engine",
                            "TEARDOWN_RESOURCE_REMAINS",
                            format!("resource cleanup incomplete: {cleanup_error}"),
                            &[],
                        );
                    }
                    break AtrustOutcome::Failed("R1", 0, 0);
                }
                r1_attempt += 1;
                if r1_attempt > R1_MAX_ATTEMPTS {
                    runtime.logs.publish(
                        "error",
                        "pipeline",
                        "R1_EXHAUSTED",
                        "engine-side reconnect budget exhausted",
                        &[("attempts", r1_attempt.to_string())],
                    );
                    if let Err(cleanup_error) = runtime.complete_resource_cleanup() {
                        runtime.logs.publish(
                            "error",
                            "engine",
                            "TEARDOWN_RESOURCE_REMAINS",
                            format!("resource cleanup incomplete: {cleanup_error}"),
                            &[],
                        );
                    }
                    break AtrustOutcome::Failed("R1", 0, 0);
                }
                // 重拨期间保持 attempt 槽占用（D2：Stop 取消通道）。
                let delay = r1_backoff_delay(r1_attempt);
                runtime.publish_r1_wait(r1_attempt, delay);
                runtime.reconnect_delay(&cancel, delay).await;
                if is_cancelled() {
                    break AtrustOutcome::Cancelled;
                }
                continue;
            }
        };

        // ---- M4 多行旁路账本（初始与 R1 重拨同一 reconciler；重拨以新目录
        // node_endpoint_ips 为期望集——先删旧集之外的行，再增加新集）----
        if is_cancelled() {
            close_atrust_prepared(runtime, &mut prepared).await;
            break AtrustOutcome::Cancelled;
        }
        if let Err(error) = runtime.reconcile_bypass_routes(egress, &prepared.bypasses) {
            close_atrust_prepared(runtime, &mut prepared).await;
            runtime.logs.publish(
                "error",
                "platform",
                "PLATFORM_STEP_FAILED",
                format!("aTrust bypass route reconciliation failed: {error}"),
                &[("step", "BYPASS".to_owned())],
            );
            if initial {
                let _ = runtime.complete_resource_cleanup();
                let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                    aftermath_verified: false,
                    reconnect_state: None,
                    stop_completion: None,
                    own_tunnel_if_index: 0,
                    session_established_at_ms: 0,
                    operation_id: runtime.current_operation_id(),
                    connect_phase: wire::ConnectPhase::ApplyingPlatformTunnel as i32,
                    coarse_phase: wire::StatsPhase::Failed as i32,
                    error: Some(ConnectCancelled::platform_vpn_error(
                        "BYPASS",
                        errno_of_platform(&error),
                        0,
                    )),
                });
                break AtrustOutcome::Cancelled;
            }
            // 重拨轮的旁路失败与建立失败同处置（预算内重试）。
            runtime.logs.publish(
                "error",
                "protocol",
                "R1_RECONNECT_FAILED",
                "engine-side aTrust reconnect bypass reconcile failed",
                &[("attempt", r1_attempt.to_string())],
            );
            r1_attempt += 1;
            if r1_attempt > R1_MAX_ATTEMPTS {
                runtime.logs.publish(
                    "error",
                    "pipeline",
                    "R1_EXHAUSTED",
                    "engine-side reconnect budget exhausted",
                    &[("attempts", r1_attempt.to_string())],
                );
                let _ = runtime.complete_resource_cleanup();
                break AtrustOutcome::Failed("R1", 0, 0);
            }
            let delay = r1_backoff_delay(r1_attempt);
            runtime.publish_r1_wait(r1_attempt, delay);
            runtime.reconnect_delay(&cancel, delay).await;
            if is_cancelled() {
                break AtrustOutcome::Cancelled;
            }
            continue;
        }
        // aTrust 首次建立需要材料；关闭 R1 时建立后不为重拨继续保留秘密。
        if !r1_enabled {
            *runtime.reconnect_materials.lock().expect("reconnect materials lock") = None;
        }
        initial = false;

        // ---- 平台管线（utun→路由→resolver→双泵→Connected→保持环→逆序
        // teardown；aTrust 失联不保留空闲 utun——retain=false）----
        let attempt = run_atrust_tunnel_pipeline(
            runtime,
            prepared,
            runtime.operation_for_event(&operation_id),
            plan_mtu,
            campus_routes.clone(),
            &cancel,
            PipelineIdentity { resolver_domains, atrust_dns_enabled, ..identity.clone() },
        )
        .await;
        if !matches!(attempt, AtrustOutcome::SessionLost(_, _)) {
            break attempt;
        }
        // 本次受理冻结的 R1 开关不随保存设置改变。
        if !r1_enabled {
            runtime.logs.publish(
                "info",
                "pipeline",
                "R1_DISABLED_BY_OWNER_SWITCH",
                "data plane lost; owner auto_reconnect switch is off — terminal (baseline behavior)",
                &[],
            );
            break attempt;
        }
        // T4（复审必改 D1）：预算按"重试调度次数"计——SessionLost 与建立失败
        // 都消耗。
        r1_attempt += 1;
        if r1_attempt > R1_MAX_ATTEMPTS {
            runtime.logs.publish(
                "error",
                "pipeline",
                "R1_EXHAUSTED",
                "engine-side reconnect budget exhausted",
                &[("attempts", r1_attempt.to_string())],
            );
            break attempt;
        }
        // 完整执行owner跨越R1退避和重拨，子管线只释放自己的资源，不清attempt。
        let delay = r1_backoff_delay(r1_attempt);
        runtime.logs.publish(
            "warn",
            "pipeline",
            "PUMP_LOST_R1_RETRY",
            "data plane lost; engine-side reconnect retry scheduled",
            &[
                ("attempt", r1_attempt.to_string()),
                ("delay_ms", delay.as_millis().to_string()),
            ],
        );
        runtime.publish_r1_wait(r1_attempt, delay);
        runtime.reconnect_delay(&cancel, delay).await;
        if is_cancelled() {
            break AtrustOutcome::Cancelled;
        }
        // 环回到建立阶段：从材料臂重跑 browser_handoff（isLogin 复核）。
    };
    match outcome {
        AtrustOutcome::Cancelled => {}
        AtrustOutcome::SessionLost(reason, close) => {
            runtime.logs.publish(
                "error",
                "pipeline",
                "PUMP_LOST",
                "aTrust data plane session lost",
                &[("reason", reason.to_owned())],
            );
            let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                aftermath_verified: false,
                reconnect_state: None,
                stop_completion: None,
                own_tunnel_if_index: 0,
                session_established_at_ms: 0,
                operation_id: runtime.current_operation_id(),
                connect_phase: wire::ConnectPhase::NegotiatingTunnel as i32,
                coarse_phase: wire::StatsPhase::Failed as i32,
                error: Some(ConnectCancelled::atrust_session_lost_vpn_error(
                    close.as_ref(),
                )),
            });
        }
        AtrustOutcome::Failed(step, errno, marker) => {
            runtime.logs.publish(
                "error",
                "pipeline",
                "PLATFORM_STEP_FAILED",
                format!("platform failed at {step} errno={errno}"),
                &[
                    ("step", step.to_owned()),
                    ("errno", errno.to_string()),
                    ("marker", marker.to_string()),
                ],
            );
            let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
                aftermath_verified: false,
                reconnect_state: None,
                stop_completion: None,
                own_tunnel_if_index: 0,
                session_established_at_ms: 0,
                operation_id: runtime.current_operation_id(),
                connect_phase: wire::ConnectPhase::ApplyingPlatformTunnel as i32,
                coarse_phase: wire::StatsPhase::Failed as i32,
                error: Some(ConnectCancelled::platform_vpn_error(step, errno, marker)),
            });
        }
    }
}
/// 一次连接的完整平台管线：utun → 地址/MTU → 路由 → DNS → 双泵 → Connected →
/// 保持（按会话形态分派）→ 逆序 teardown。
///
/// 失败时只逆序恢复已真实获取的资源；取消/会话死亡/平台失败都走同一条 teardown。
/// 保持环经 [`TunnelHold`] 按会话分派（M3：CSTP=keepalive/DPD/控制通道；
/// aTrust=run_packets worker 存活观察，心跳由 Common L3Connection 内部独占）。
#[allow(clippy::too_many_lines)]
async fn run_atrust_data_plane(
    runtime: &EngineRuntime,
    prepared: PreparedAtrust,
    operation_id: Vec<u8>,
    plan_mtu: u32,
    campus_routes: Vec<String>,
    cancel: &Arc<AtomicBool>,
    pipeline_identity: PipelineIdentity,
    protocol_owner: &Arc<tokio::sync::Mutex<Option<crate::atrust_tunnel::AtrustPacketTunnel>>>,
) -> AtrustOutcome {
    // T2（复审必改 4）：管线入口即切换事件归属到本代 operation——上一代若经
    // SessionLost 自拆（无收敛清位）残留旧归属，本代早期的阶段事件会被新 core
    // 按 operation_id 过滤丢弃。Connected 时刻的 record 会再次确认同一值。
    let operation_id = runtime.operation_for_event(&operation_id);
    runtime.adopt_operation(operation_id.clone());
    // Connected 时刻的 epoch ms：status 事件与会话时长诊断共用的单一事实源；
    // 0 = 尚未到达 Connected。Atomic 是因为闭包随管线 future 跨线程（Cell 非 Send）。
    let session_started_ms = std::sync::atomic::AtomicI64::new(0);
    let own_tunnel_if_index = std::sync::atomic::AtomicU32::new(0);
    let publish = |phase: wire::ConnectPhase, coarse: wire::StatsPhase| {
        // MAC-OBS-13 S1：连接状态推进同步进 sink（每阶段一条 info；字段只含
        // 白名单阶段名，无凭据/证书材料）。
        runtime.logs.publish(
            "info",
            "pipeline",
            "PHASE",
            format!("connect phase -> {phase:?} coarse {coarse:?}"),
            &[
                ("phase", format!("{phase:?}")),
                ("coarse", format!("{coarse:?}")),
            ],
        );
        let _ = runtime.status_tx.send(wire::ConnectStatusEvent {
            aftermath_verified: false,
            reconnect_state: None,
            stop_completion: None,
            own_tunnel_if_index: own_tunnel_if_index.load(Ordering::Relaxed),
            session_established_at_ms: session_started_ms.load(Ordering::Relaxed),
            // T2：事件归属读 runtime 当前值（采纳时切换）；本地捕获值仅作
            // runtime 尚未记录时的兜底（管线 Connected 前无采纳，二者一致）。
            operation_id: {
                let current = runtime.current_operation_id();
                if current.is_empty() {
                    operation_id.clone()
                } else {
                    current
                }
            },
            connect_phase: phase as i32,
            coarse_phase: coarse as i32,
            error: None,
        });
    };
    let is_cancelled = || cancel.load(std::sync::atomic::Ordering::SeqCst);
    let errno_of = |error: &crate::platform::PlatformError| match error {
        crate::platform::PlatformError::Utun(_, e)
        | crate::platform::PlatformError::Interface(_, e)
        | crate::platform::PlatformError::Route(_, e) => e.raw_os_error().unwrap_or(0),
        _ => 0,
    };

    // 会话形态拆解：本机接口参数（offer）+ 泵/保持环原材料。aTrust 的 offer
    // 由数据面组装给出（VIP=节点分配；prefix 恒 32；无服务端路由/DNS——校园
    // 范围由用户配置经计划 routes 注入，与 Cisco envelope routes 同源）。
    let offer = prepared.offer.clone();
    let retain_on_session_loss = false;

    // ---- 1. utun（AttachingPacketBoundary 真实入口）。----
    publish(
        wire::ConnectPhase::ApplyingPlatformTunnel,
        wire::StatsPhase::Connecting,
    );
    let device = match runtime.surface().acquire_device() {
        Ok(device) => device,
        Err(error) => {
            // 登录前已可能写入控制 `/32`；即使 utun 创建失败也必须在非重试终态前
            // 尝试收口，不能让失败分支绕开 ledger。
            let _ = runtime.complete_resource_cleanup();
            return AtrustOutcome::Failed("UTUN", errno_of(&error), 0);
        }
    };
    own_tunnel_if_index.store(device.ifindex(), Ordering::Relaxed);
    publish(
        wire::ConnectPhase::AttachingPacketBoundary,
        wire::StatsPhase::Connecting,
    );

    // ---- 2. 地址/MTU/路由/DNS（ApplyingPlatformTunnel；失败逆序回滚已应用项）。----
    let mut applied_routes: Vec<AppliedRoute> = Vec::new();
    let effective_mtu = if plan_mtu > 0 && plan_mtu < u32::from(offer.mtu) {
        u16::try_from(plan_mtu).unwrap_or(offer.mtu)
    } else {
        offer.mtu
    };
    let mut negotiated = crate::session_diagnostics::identity(&operation_id);
    negotiated.extend(crate::session_diagnostics::build_fields());
    negotiated.extend([
        ("offer_mtu", offer.mtu.to_string()),
        ("requested_mtu", plan_mtu.to_string()),
        ("effective_mtu", effective_mtu.to_string()),
        ("tunnel_ifindex", device.ifindex().to_string()),
    ]);
    runtime.logs.publish(
        "info",
        "cstp",
        "TUNNEL_EFFECTIVE_PARAMETERS",
        "隧道待应用参数",
        &negotiated,
    );

    if let Err(error) =
        runtime
            .surface()
            .apply_ipv4(&device, offer.ipv4_address, offer.prefix, effective_mtu)
    {
        let errno = errno_of(&error);
        let substep = match &error {
            crate::platform::PlatformError::Interface(name, _)
            | crate::platform::PlatformError::Readback(name) => name,
            _ => "ADDRESS",
        };
        runtime.logs.publish(
            "error",
            "platform",
            "ADDRESS_FAILED",
            format!("address failed at {substep}: {error}"),
            &[
                ("substep", substep.to_owned()),
                ("errno", errno.to_string()),
            ],
        );
        drop(device);
        // 地址阶段失败时业务路由尚未开始，但控制 `/32` 已在 establish 前创建；统一走
        // ledger，保证后续非重试 Failed 之前没有被遗漏的专用路由或旧 idle utun。
        let _ = runtime.complete_resource_cleanup();
        return AtrustOutcome::Failed(substep, errno, 0);
    }

    let mut wanted: Vec<(Ipv4Addr, u8)> = Vec::new();
    for text in offer.routes.iter().chain(campus_routes.iter()) {
        if let Some(cidr) = parse_cidr(text)
            && !wanted.contains(&cidr)
        {
            wanted.push(cidr);
        }
    }
    for server in &offer.dns_servers {
        let cidr = (*server, 32);
        if !wanted.contains(&cidr) {
            wanted.push(cidr);
        }
    }
    // M4（aTrust）：隧道 DNS 服务器（`campus_dns_servers`——目录下发，非接口 DNS）
    // 经 /32 路由走隧道（设计 §5.3），入校园路由批次；畸形条目跳过（目录数据
    // 不因单条畸形拒绝连接）。
    {
        let atrust = &prepared;
        for server in &atrust.campus_dns_servers {
            match server.parse::<Ipv4Addr>() {
                Ok(address) => {
                    let cidr = (address, 32);
                    if !wanted.contains(&cidr) {
                        wanted.push(cidr);
                    }
                }
                Err(_) => runtime.logs.publish(
                    "warn",
                    "platform",
                    "CAMPUS_DNS_UNPARSEABLE",
                    "campus dns server entry is not an IPv4 literal; /32 route skipped",
                    &[],
                ),
            }
        }
    }
    let mut route_failure: Option<(&'static str, i32)> = None;
    for (destination, prefix) in wanted {
        // 网关 = 本隧道地址（INET 网关形态，Clash 同款）：AF_LINK 接口路由形态
        // 在 p2p utun 上建路由时 rt_ifa 解析不到接口地址，未绑定源的 sendto 报
        // EADDRNOTAVAIL；经本隧道地址转发时内核按网关命中 dstaddr=自身，源选择
        // 取本接口地址。
        let applied = AppliedRoute {
            destination,
            prefix,
            gateway: offer.ipv4_address,
            ifindex: device.ifindex(),
        };
        if let Err(error) = runtime.surface().route_add(&applied) {
            let errno = match &error {
                crate::platform::PlatformError::Route(_, e) => e.raw_os_error().unwrap_or(0),
                _ => 0,
            };
            route_failure = Some(("ROUTE", errno));
            break;
        }
        runtime.remember_route_cleanup_failure(applied);
        applied_routes.push(applied);
    }

    // DNS：不改系统 DNS（Windows 对齐，见 platform/mod.rs 说明）；
    // offer DNS 服务器的 /32 路由已加入，隧道侧可直接查询。

    if let Some((step, errno)) = route_failure {
        // 逆序恢复：路由 → utun（系统 DNS 未被修改，无需恢复）。resolver 尚未
        // 安装（安装序在校园路由之后），无该族回滚项。
        for route in applied_routes.drain(..).rev() {
            if runtime.surface().route_delete(&route).is_err() {
                runtime.remember_route_cleanup_failure(route);
            }
        }
        drop(device);
        let _ = runtime.complete_resource_cleanup();
        return AtrustOutcome::Failed(step, errno, 0);
    }

    // 校园域分域解析：最终格式、可读权限、通知和异步装载等待均沿既有实现。
    // 安装序在校园路由
    // 之后（门轨 B 的解析探针需要隧道 DNS /32 已入路由表）。失败=typed 平台
    // 步骤失败（外来同名冲突/写入/HUP/门未装载），按逆序回滚本管线已应用项。
    let mut installed_resolvers: Vec<crate::campus_resolver::InstalledResolver> = Vec::new();
    if pipeline_identity.atrust_dns_enabled && !pipeline_identity.resolver_domains.is_empty() {
        let atrust = &prepared;
        let (installed, error) = runtime.surface().install_resolvers(
            &pipeline_identity.resolver_domains,
            &atrust.campus_dns_servers,
            &runtime.logs,
        );
        runtime
            .resources
            .lock()
            .expect("resource ledger lock")
            .resolvers
            .extend(installed.iter().cloned());
        installed_resolvers = installed;
        if let Some(error) = error {
            runtime.logs.publish(
                "error",
                "platform",
                "RESOLVER_STEP_FAILED",
                format!("campus resolver step failed: {error}"),
                &[("reason", error.reason().to_owned())],
            );
            // install_all已尝试回滚，失败条目仍在runtime账本中等待真实重试。

            for route in applied_routes.drain(..).rev() {
                if runtime.surface().route_delete(&route).is_err() {
                    runtime.remember_route_cleanup_failure(route);
                }
            }
            drop(device);
            let _ = runtime.complete_resource_cleanup();
            return AtrustOutcome::Failed(error.reason(), 0, 0);
        }
    }

    // ---- 3. 双泵（StartingDataPlane 真实入口）+ Connected。----
    publish(
        wire::ConnectPhase::StartingDataPlane,
        wire::StatsPhase::Connecting,
    );
    runtime.apply_proxy_policy(&applied_routes, &pipeline_identity.resolver_domains);
    // 数据泵按会话形态接线（设计 §6.1）：CSTP=帧泵（上行包编码 CSTP Data 帧，
    // 下行收 Data payload；无界通道）；aTrust=原始包泵（上行剥 AF 头的原始
    // IPv4 字节入有界 mpsc 喂 Common `run_packets`，`PacketEvent::Packet` 下行
    // 加 AF 头写 utun；`PacketEngine::submit` 是 run_packets 内部调用，宿主
    // 不直触）。
    let PreparedAtrust { channels, .. } = prepared;
    let pumps = pump::start_raw(
        device.fd(),
        channels.outgoing,
        channels.incoming,
        &runtime.logs,
    );
    // W-MVP-13：挂接本次双泵计数器并把粗阶段置为 Connected（统计样本的事实源）。
    runtime
        .stats
        .registry()
        .attach_pump(Arc::clone(&pumps.rx_bytes), Arc::clone(&pumps.tx_bytes));
    runtime
        .stats
        .registry()
        .set_phase(wire::StatsPhase::Connected);
    // 数据面真实运行即会话起点——Connected 事件带上真实时间戳（前端在线时长的
    // 事实源；此前恒 0 导致 UI 永远显示「—」）。
    session_started_ms.store(now_epoch_ms(), Ordering::Relaxed);
    publish(
        wire::ConnectPhase::StartingDataPlane,
        wire::StatsPhase::Connected,
    );
    // T2：Connected 事实记录——身份（采纳判定）、ifindex/纪元（采纳事件重放）、
    // 事件归属（采纳切换）与 keep 策略（连接成功即 keep 的生产注入点）。
    runtime.record_pipeline_connected(
        pipeline_identity,
        own_tunnel_if_index.load(Ordering::Relaxed),
        session_started_ms.load(Ordering::Relaxed),
        operation_id.clone(),
    );

    // ---- 4. 保持：按会话形态分派（设计 §6.1）+ 泵监视。----
    // 保持环协议无关化（M3）：CSTP=DPD 应答+周期 keepalive+延迟探测节拍+控制
    // 通道 EOF 判定（CstpHold，行为从原保持环逐字节平移）；aTrust=run_packets
    // worker 存活观察（AtrustHold——心跳由 Common L3Connection 内部 worker
    // 独占，宿主不发帧）。
    let mut hold = AtrustHold::new_shared(
        Arc::clone(protocol_owner),
        Arc::clone(&runtime.logs),
        operation_id.clone(),
    );
    if let Ok(mut live) = runtime.live.lock() {
        *live = Some(HeldTunnel);
    }
    let mut session_lost = false;
    loop {
        if is_cancelled()
            || runtime.shutdown.load(Ordering::SeqCst)
            || TERMINATION_REQUESTED.load(Ordering::SeqCst)
        {
            break;
        }
        if runtime.consume_reconnect_fault() {
            if let Some(owner) = protocol_owner.lock().await.as_ref() {
                owner.fail_transport_for_acceptance();
            }
        }
        if pumps.either_finished() {
            // 任一泵死亡 = 数据面失败，不维持 Connected。
            runtime.logs.publish(
                "error",
                "packet",
                "PUMP_EXITED",
                "a pump thread exited while tunnel held",
                &[],
            );
            session_lost = true;
            break;
        }
        if hold.handle_control().await == HoldObservation::Lost {
            session_lost = true;
            break;
        }
    }

    // ---- 5. 逆序 teardown：泵 → 会话 → 路由 → utun。----
    // 会话收尾在泵停止前（CSTP：控制通道有界收取+会话诊断汇总；aTrust：标记
    // 宿主停机意图——泵线程退出不被误报为协议掉线）。
    hold.settle(session_lost).await;
    let teardown_started = std::time::Instant::now();
    runtime.stats.registry().set_phase(wire::StatsPhase::Idle);
    runtime.stats.registry().detach_pump();
    pumps.shutdown();
    // 会话释放（aTrust：关闭 run_packets owner 并取回终止类别）；CSTP 的
    // write_channel/control_rx 随 hold Drop 关闭（对齐原显式 drop 顺序）。
    hold.release().await;
    let lost_reason = if session_lost {
        hold.session_lost_reason()
    } else {
        ""
    };
    let close_observation = hold.take_close_observation();
    drop(hold);
    let mut teardown_complete = runtime.cleanup_proxy_policy();
    // resolver 文件先于业务路由清理；删除或通知失败的真实条目留在runtime账本，
    // 本次完整清理和后续新Stop均可重试，不以预检代替停止回执。
    if !installed_resolvers.is_empty() {
        let resolver_failures = runtime
            .surface()
            .remove_resolvers(&installed_resolvers, &runtime.logs);
        if resolver_failures > 0 {
            teardown_complete = false;
        } else {
            runtime
                .resources
                .lock()
                .expect("resource ledger lock")
                .resolvers
                .retain(|owned| !installed_resolvers.contains(owned));
        }
    }
    for route in applied_routes.drain(..) {
        if let Err(error) = runtime.surface().route_delete(&route) {
            let (step, errno) = match &error {
                crate::platform::PlatformError::Route(step, source) => {
                    (*step, source.raw_os_error())
                }
                _ => ("DELETE", None),
            };
            let mut fields =
                crate::session_diagnostics::identity(&runtime.operation_for_event(&operation_id));
            fields.extend([
                ("destination", route.destination.to_string()),
                ("prefix", route.prefix.to_string()),
                ("ifindex", route.ifindex.to_string()),
                ("step", step.to_owned()),
                (
                    "errno",
                    errno.map_or_else(|| "none".to_owned(), |value| value.to_string()),
                ),
            ]);
            runtime.logs.publish(
                "warn",
                "platform",
                "ROUTE_DELETE_FAILED",
                format!("route deletion failed before ledger retry: {error}"),
                &fields,
            );
            runtime.remember_route_cleanup_failure(route);
            teardown_complete = false;
        } else {
            runtime
                .resources
                .lock()
                .expect("resource ledger lock")
                .pending_routes
                .retain(|owned| owned != &route);
        }
    }
    // 只有业务路由均已删除时才允许把 utun 留给重连。若先前删除失败，先走完整
    // ledger 清理并放弃复用；不得让“空闲 utun”与残留 VPN 专用路由同时存在。
    // aTrust R1 每轮重新创建utun，不保留空闲设备；Cisco保留既有复用语义。
    let preserve_for_reconnect =
        retain_on_session_loss && session_lost && !is_cancelled() && teardown_complete;
    if preserve_for_reconnect {
        if let Err(error) = runtime.surface().clear_ipv4(&device, offer.ipv4_address) {
            runtime.logs.publish(
                "error",
                "platform",
                "IDLE_UTUN_ADDRESS_CLEAR_FAILED",
                format!("failed to remove address from retained utun: {error}"),
                &[],
            );
            // 保留带旧地址的 utun 会继续拥有源地址，违反空闲设备边界；失败时宁可
            // 释放设备，并由下一次重连重新创建。
            teardown_complete = false;
            drop(device);
            if let Err(cleanup_error) = runtime.complete_resource_cleanup() {
                runtime.logs.publish(
                    "error",
                    "engine",
                    "TEARDOWN_RESOURCE_REMAINS",
                    format!("resource cleanup incomplete: {cleanup_error}"),
                    &[],
                );
            } else {
                // 地址移除失败后不复用本设备；ledger 已成功收口时允许后续以新设备重连。
                teardown_complete = true;
            }
        } else {
            runtime.retain_device(device);
        }
    } else {
        drop(device);
        if let Err(error) = runtime.complete_resource_cleanup() {
            teardown_complete = false;
            runtime.logs.publish(
                "error",
                "engine",
                "TEARDOWN_RESOURCE_REMAINS",
                format!("resource cleanup incomplete: {error}"),
                &[],
            );
        } else {
            // 先前一次逐条删除可以暂态失败；完整账本重试若成功，资源已如实清空，
            // 仅放弃本轮 utun 复用，下一次连接重新读取出口并创建设备即可。
            teardown_complete = true;
        }
    }
    let mut teardown_fields = crate::session_diagnostics::identity(&operation_id);
    teardown_fields.push((
        "elapsed_ms",
        teardown_started.elapsed().as_millis().to_string(),
    ));
    runtime.logs.publish(
        if teardown_complete { "info" } else { "error" },
        "engine",
        if teardown_complete {
            "TEARDOWN_COMPLETED"
        } else {
            "TEARDOWN_INCOMPLETE"
        },
        if teardown_complete {
            "隧道资源已完成清理"
        } else {
            "隧道资源清理未完成，失败条目保留待重试"
        },
        &teardown_fields,
    );
    TERMINATION_TEARDOWN_DONE.store(true, Ordering::SeqCst);
    if let Ok(mut live) = runtime.live.lock() {
        *live = None;
    }

    if session_lost && !is_cancelled() {
        AtrustOutcome::SessionLost(lost_reason, close_observation)
    } else {
        AtrustOutcome::Cancelled
    }
}

async fn close_atrust_prepared(runtime: &EngineRuntime, prepared: &mut PreparedAtrust) {
    if let Some(mut owner) = prepared.owner.take() {
        if matches!(owner.close().await, Err("worker_stopped")) {
            runtime
                .execution_errors
                .lock()
                .expect("execution errors lock")
                .push(super::stop_operations::stop_error(
                    "darwin/atrust_close",
                    None,
                ));
        }
    }
}

async fn run_atrust_tunnel_pipeline(
    runtime: &Arc<EngineRuntime>,
    mut prepared: PreparedAtrust,
    operation_id: Vec<u8>,
    plan_mtu: u32,
    campus_routes: Vec<String>,
    cancel: &Arc<AtomicBool>,
    identity: PipelineIdentity,
) -> AtrustOutcome {
    let owner = prepared.owner.take();
    let owner = Arc::new(tokio::sync::Mutex::new(owner));
    let child_owner = Arc::clone(&owner);
    let child_runtime = Arc::clone(runtime);
    let cancel = Arc::clone(cancel);
    let result = tokio::spawn(async move {
        run_atrust_data_plane(
            &child_runtime,
            prepared,
            operation_id,
            plan_mtu,
            campus_routes,
            &cancel,
            identity,
            &child_owner,
        )
        .await
    })
    .await;
    if let Some(mut owner) = owner.lock().await.take() {
        if matches!(owner.close().await, Err("worker_stopped")) {
            runtime
                .execution_errors
                .lock()
                .expect("execution errors lock")
                .push(super::stop_operations::stop_error(
                    "darwin/atrust_close",
                    None,
                ));
        }
    }
    match result {
        Ok(AtrustOutcome::SessionLost(_, _)) if runtime.has_cleanup_obligations() => {
            AtrustOutcome::Failed("RESOURCE_CLEANUP", 0, 0)
        }
        Ok(outcome) => outcome,
        Err(_) => {
            *runtime.live.lock().expect("live lock") = None;
            runtime.stats.registry().detach_pump();
            runtime
                .execution_errors
                .lock()
                .expect("execution errors lock")
                .push(super::stop_operations::stop_error(
                    "darwin/pipeline_task",
                    None,
                ));
            AtrustOutcome::Failed("PANIC", 0, 0)
        }
    }
}

fn errno_of_platform(error: &crate::platform::PlatformError) -> i32 {
    match error {
        crate::platform::PlatformError::Utun(_, e)
        | crate::platform::PlatformError::Interface(_, e)
        | crate::platform::PlatformError::Route(_, e) => e.raw_os_error().unwrap_or(0),
        crate::platform::PlatformError::Readback(_) => 0,
    }
}
/// 解析 canonical IPv4 CIDR（Engine 侧对用户意图的再验证；拒绝 0.0.0.0/0）。
fn parse_cidr(text: &str) -> Option<(Ipv4Addr, u8)> {
    let (address_text, prefix_text) = text.split_once('/')?;
    let prefix = prefix_text.parse::<u8>().ok()?;
    if !(1..=32).contains(&prefix) || prefix.to_string() != prefix_text {
        return None;
    }
    let address: Ipv4Addr = address_text.parse().ok()?;
    if address.to_string() != address_text || address.is_unspecified() {
        return None;
    }
    let mask = u32::MAX << (32 - u32::from(prefix));
    (u32::from(address) & mask == u32::from(address)).then_some((address, prefix))
}

/// 取消场景的 typed VpnError 组装辅助（保持 match 分支可读）。
struct ConnectCancelled;

impl ConnectCancelled {
    fn vpn_error() -> wire::VpnError {
        wire::VpnError {
            code: wire::ErrorCode::CancelledBeforeStart as i32,
            stage: wire::ErrorStage::ConnectingControl as i32,
            ..Default::default()
        }
    }

    /// 仅投影本次 aTrust SessionLost 已携带的原码。
    fn atrust_session_lost_vpn_error(
        close: Option<&crate::atrust_tunnel::AtrustCloseObservation>,
    ) -> wire::VpnError {
        let mut error = Self::session_lost_vpn_error();
        error.native = close
            .and_then(|observed| observed.raw_os_error())
            .map(|code| wire::RedactedNativeError {
                category: wire::NativeErrorCategory::Resource as i32,
                namespace: wire::NativeErrorNamespace::PosixErrno as i32,
                code: i64::from(code),
            });
        error
    }

    /// 持有期间会话丢失（网关关闭/TLS 断链/泵死亡）的 typed 失败。
    ///
    /// W3-2/P3（主轨升级）：此错误只发生在 `establish` 成功、数据面曾真实运行之后
    /// （泵意外退出 / DPD 失败 / 控制通道关闭），故 `stage` 升级为 `DataPlane` 且
    /// `retry=RetrySameOperation`——Core 侧以「`stage=DataPlane + retry=
    /// RetrySameOperation`」为**可重试掉线**的唯一标记（镜像 win32 C2 语义）触发数据面
    /// 自动重连；与 Core 自身的 engine 失联终态（`ProtocolSession` +
    /// `UseNewOperation`，流 EOF 路径）靠 retry 字段明确区分。零 proto 改动
    /// （`ErrorStage`/`RetryAdvice` 枚举既有），`code=EffectUnknown` 维持平台类通用码
    /// 不参与判定。
    fn session_lost_vpn_error() -> wire::VpnError {
        wire::VpnError {
            code: wire::ErrorCode::EffectUnknown as i32,
            stage: wire::ErrorStage::DataPlane as i32,
            retry: wire::RetryAdvice::RetrySameOperation as i32,
            ..Default::default()
        }
    }

    /// 平台步骤失败的 typed 失败；code 区分步骤，errno/retry 为诊断值。
    fn platform_vpn_error(step: &'static str, errno: i32, marker: i32) -> wire::VpnError {
        use wire::ErrorCode;
        let (code, stage) = match step {
            "UTUN" => (
                ErrorCode::EffectUnknown,
                wire::ErrorStage::AttachingPacketBoundary,
            ),
            "PUMP" => (
                ErrorCode::DataPlaneBackpressure,
                wire::ErrorStage::AttachingPacketBoundary,
            ),
            "ADDRESS" | "SIOCSIFADDR" => (
                ErrorCode::ObservationFailed,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCSIFDSTADDR" => (
                ErrorCode::IdempotencyConflict,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCGIFFLAGS" => (
                ErrorCode::ConnectInProgress,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCSIFFLAGS" => (
                ErrorCode::SessionBusy,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "SIOCSIFMTU" => (
                ErrorCode::ReconnectAlreadyQueued,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            // 只有外来同名文件是冲突；DNS 写入、通知和装载失败保留各自类别。
            "DARWIN_ATRUST_RESOLVER_WRITE_FAILED" | "DARWIN_ATRUST_RESOLVER_SIGNAL_FAILED" => (
                ErrorCode::EffectUnknown,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "DARWIN_ATRUST_RESOLVER_GATE_NOT_LOADED" => (
                ErrorCode::ObservationFailed,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            "ROUTE" | "BYPASS" | "RESOLVER" | "DARWIN_ATRUST_RESOLVER_CONFLICT" => (
                ErrorCode::ObservedConflict,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
            _ => (
                ErrorCode::JournalCorrupt,
                wire::ErrorStage::ApplyingPlatformTunnel,
            ),
        };
        let is_resolver = step == "RESOLVER" || step.starts_with("DARWIN_ATRUST_RESOLVER_");
        let resource = is_resolver.then(|| {
            use sha2::{Digest, Sha256};
            wire::OpaqueResourceRef {
                kind: wire::ResourceKind::Dns as i32,
                // 此错误关联本机 resolver 目录，不派生认证或会话材料。
                identity_digest: Sha256::digest(crate::campus_resolver::RESOLVER_DIR.as_bytes())
                    .to_vec(),
            }
        });
        wire::VpnError {
            reason: if is_resolver {
                step.to_owned()
            } else {
                String::new()
            },
            code: code as i32,
            stage: stage as i32,
            certainty: wire::EffectCertainty::Applied as i32,
            retry: if is_resolver {
                wire::RetryAdvice::UseNewOperation as i32
            } else {
                marker
            },
            subject: None,
            resource,
            native: Some(wire::RedactedNativeError {
                category: wire::NativeErrorCategory::Resource as i32,
                namespace: wire::NativeErrorNamespace::PosixErrno as i32,
                code: i64::from(errno),
            }),
            operation_detail: None,
        }
    }
}

