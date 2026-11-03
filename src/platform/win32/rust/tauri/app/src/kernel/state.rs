//! 运行时状态镜像（proto/exv/v1/common.proto: RuntimeSnapshot / RuntimeEvent /
//! ConnectPhase / OperationReply）。字段名与 proto 一致，serde 用 snake_case，
//! 供 Command 返回与 Event payload 复用。P4-b 与 core 的 gRPC 消息机械映射。

use serde::{Deserialize, Serialize};

use super::stats::RuntimeStats;

/// 连接阶段（proto ConnectPhase）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectPhase {
    ObservingOwnedState,
    AcquiringPlatformLease,
    ConnectingControl,
    AwaitingInteraction,
    NegotiatingTunnel,
    ApplyingPlatformTunnel,
    AttachingPacketBoundary,
    StartingDataPlane,
}

/// 结构化错误（proto VpnError 的精简 UI 视图）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpnError {
    /// 稳定诊断代码，空表示无。
    pub code: String,
    /// 面向用户的简短描述（不含诊断栈）。
    pub message: String,
    /// wire 已有阶段；前端仅以此说明失败发生处，不猜具体根因。
    pub stage: Option<String>,
    /// 稳定失败原因 id（EXV_UNFREEZE 2026-10-09，tag 8；`<域>/<细分>` 小写，空 =
    /// 缺省）。前端 `lib/copy-tables.ts` 按 id 映射文案——单一文案来源，与壳侧
    /// `wire.rs` 的 reason 文案表互指 + 一致性测试钉住。
    pub reason: String,
}

/// 停止终态事实；None 表示旧端没有提供，不能解释成 stopped=false。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopCompletion {
    pub stopped: bool,
    pub blocks_new_connect: bool,
    pub errors: Vec<VpnError>,
}

/// RuntimeSnapshot 的 state oneof 各分支。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RuntimeState {
    Idle {
        last_cleanup_at_ms: Option<i64>,
    },
    Connecting {
        phase: ConnectPhase,
        attempt_id: Option<String>,
        /// 已完成/进行中的连接阶段序号（0..=7），供前端进度显示。
        phase_index: u8,
    },
    AwaitingInteraction {
        attempt_id: Option<String>,
        prompt_deadline_ms: Option<u64>,
    },
    Connected {
        session_established_at_ms: Option<i64>,
        /// 简要连接信息（redacted：不含凭据/证书）。
        summary: Option<String>,
    },
    Stopping {
        reason: Option<String>,
    },
    Reconciling {
        blocking_error: Option<VpnError>,
    },
    FailedClean {
        error: Option<VpnError>,
    },
    FailedDirty {
        error: Option<VpnError>,
        /// 是否存在待恢复义务（recovery obligation）。
        has_obligation: bool,
    },
}

/// 检测到的上游代理 TUN 适配器（proto `ProxyTunAdapter`；Phase 7 C5-wire）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyTunAdapter {
    /// 友好名称（如 "Mihomo" / "Meta"）。
    pub name: String,
    /// 接口描述（如 "Wintun Userspace Tunnel"）。
    pub description: String,
    /// IPv4 接口索引。
    pub if_index: u32,
    /// 适配器种类（当前恒为 "proxy_tun"）。
    pub kind: String,
}

/// 上游代理 TUN 检测结果（proto `ProxyTunDetection`；Phase 7 C5-wire，仅状态上报）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyTunDetection {
    /// 是否检测到至少一个上游代理 TUN 适配器。
    pub detected: bool,
    /// 检测到的适配器（`detected == false` 时为空）。
    pub adapters: Vec<ProxyTunAdapter>,
    /// 共存路由策略："exv-before-proxy-tun" | "normal"。
    pub route_policy: String,
}

/// 系统代理检测结果（proto `SystemProxyDetection`；S2 系统代理，仅状态上报——不携带
/// 端点 URL / 注册表值 / 秘密，只有计数与短枚举串，与 RuntimeStats/LogEvent 同不变式）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemProxyDetection {
    /// 系统代理模式："disabled" | "manual" | "automatic" | "mixed"。
    pub mode: String,
    /// 检测到的系统代理端点数（endpoints 规范化后计数）。
    pub endpoint_count: u32,
    /// EXV 豁免条目是否已合并进系统代理设置（bypass 非空 = 已合并）。
    pub bypass_merged: bool,
    /// 四态拓扑（`(proxy_present, tunnel_present)` 的纯函数分类）："t0" | "t1" | "t2" | "t3"。
    pub topology: String,
}

/// 自动重连状态（proto `ReconnectStatus`；S4 重连，仅状态上报——重连决策仍在 host 侧
/// C3a 重连 worker）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectStatus {
    /// 自动重连开关（config `auto_reconnect`）。
    pub auto_reconnect: bool,
    /// 配置的重试预算（config `auto_reconnect_max_attempts`；0 = 无限）。
    pub max_attempts: u32,
    /// 本次连接已执行的重试次数（per-connection，Connected 时清零；0 = 无）。
    pub current_attempt: u32,
    /// 重连尝试是否正在飞行中。
    pub active: bool,
    /// R2（T6b，第九次窄解冻 tag 5）恢复事件的稳定 reason id（空 = 无 R2 事件）。
    /// UI 文案按 id 路由（T12）；不是授权材料。
    pub reason: String,
}

/// engine 侧 R1 自愈遥测（proto `ReconnectState`，common.proto 第十次窄解冻随
/// `RuntimeSnapshot.engine_reconnect` tag 19 到 UI；T8 采纳快照分量）。仅状态上报。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineReconnectState {
    /// 本次事件描述的第几次尝试（触发事件即第 1 次）。
    pub attempt: u32,
    /// 下次重试的墙钟时刻（UTC epoch ms；0 = 尝试在途/无下次）。
    pub next_retry_at_ms: i64,
    /// 当前重连步骤的稳定 reason id（如 `tunnel_lost` / `healed` /
    /// `attempts_exhausted`；空 = 不适用）。UI 文案按 id 路由（T12）。
    pub reason: String,
}

/// host 自愈（engine 崩溃 respawn）状态（proto `SelfHealStatus`；EXV_UNFREEZE
/// 2026-09-05，仅状态上报——respawn 决策与编排留在 host，wire 不携带凭据/栈）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfHealStatus {
    /// 自愈阶段："respawning" | "succeeded" | "failed"；未知码原样透传（前端
    /// fail-safe 渲染为 unknown 档）。
    pub stage: String,
    /// 崩溃 engine PID；0 = 不可得。
    pub old_pid: u32,
    /// 新 engine PID；`stage != "succeeded"` 时为 0。
    pub new_pid: u32,
    /// 稳定错误码；仅 `stage == "failed"` 非空。
    pub error_code: String,
}

/// 连接受理时冻结的会话身份投影（proto `SessionIdentity`；EXV_UNFREEZE 2026-10-04，
/// bug3：会话活动期间设置页改协议/账号/服务器不得改写连接页显示）。仅显示，非授权
/// 材料；`None` = 无活动会话身份（UI 回落当前配置）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIdentity {
    /// 活动会话使用的协议："cisco" | "atrust"。
    pub protocol: String,
    /// 受理时冻结的账号；空 = 无（aTrust 经系统认证、无可读账号）。
    pub username: String,
    /// 受理时冻结的服务器地址；空 = 无。
    pub server: String,
}

/// win32 engine SCM 服务状态（proto `ServiceStatus`；S3/D5，仅状态展示，非授权材料——
/// 服务存在性不构成 capability，peer 身份只来自验证过的传输元数据 + PSK）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// 服务是否已在 SCM 注册。
    pub installed: bool,
    /// SCM 服务状态："stopped" | "start_pending" | "stop_pending" | "running" | "other"。
    pub state: String,
    /// SCM 二进制路径（含启动参数）；未安装时为空。
    pub binary_path: Option<String>,
    /// R3 统一服务健康状态："healthy" | "scm_orphan" | "installed_unavailable" |
    /// "payload_orphan" | "not_installed"；host 未派生时为空。
    pub health_state: Option<String>,
}

/// `KernelControl.ServiceControl` 的 UI 视图（proto `ServiceControlReply`；S3/D5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceControlReply {
    /// action 执行后（或 query 时）的当前服务状态；`None` = 查询失败未获取。
    pub service_status: Option<ServiceStatus>,
    /// action 是否成功（query 恒 true；变更 action 反映提权子命令结果）。
    pub ok: bool,
    /// 人类可读结果/详情（无秘密、无栈）。
    pub message: String,
}

/// `KernelControl.ServiceControl` 的 action（S3/D5：query 非提权读；变更走 runas 提权
/// seam；RotateKey 轮换（撤销）服务 PSK——同一提权批量通道，无需重启服务）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceControlAction {
    Query,
    Install,
    Uninstall,
    Start,
    RotateKey,
}

/// RuntimeSnapshot 的 UI 视图（mirror 自 proto RuntimeSnapshot）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    #[serde(default)]
    pub stop_completion: Option<StopCompletion>,
    pub runtime: RuntimeState,
    /// 单调 tick（WatchEvents resume 游标），0 表示未知。
    pub monotonic_tick: u64,
    /// 快照携带的最新归一化统计（stats-wire 方案 A：`GetSnapshot`/`WatchEvents`
    /// 随状态附带；`None` = 尚无样本，前端显示占位）。
    pub stats: Option<RuntimeStats>,
    /// 快照携带的上游代理 TUN 检测（C5-wire：`GetSnapshot`/`WatchEvents` 随状态附带；
    /// `None` = 未探测/探测失败，`detected` 区分「探测到无」与「未探测」）。
    pub proxy_tun: Option<ProxyTunDetection>,
    /// 本快照所属的 in-flight 连接/停止操作 id（R1 契约，mirror proto
    /// `RuntimeSnapshot.operation_id`；`Some(hex16)` = 有在途操作，`None` = 无）。
    /// 前端据此只让「当前用户操作」的事件驱动 UI（旧操作迟到事件不扰动显示）。
    pub operation_id: Option<String>,
    /// 快照携带的 win32 engine SCM 服务状态（S3/D5：`GetSnapshot`/`WatchEvents` 随状态
    /// 附带；`None` = 尚未查询/查询失败）。仅展示，非授权材料。
    pub service_status: Option<ServiceStatus>,
    /// 快照携带的当前连接模式（S3/D3：`"auto" | "service" | "oneshot"`）。只展示，
    /// 不做路由输入——路由决策在 core 侧（M3 决策表），无 `ConnectIntent.mode` 字段。
    pub mode: String,
    /// 快照携带的系统代理检测（S2：`GetSnapshot`/`WatchEvents` 随状态附带；
    /// `None` = 未检测/检测失败）。
    pub system_proxy: Option<SystemProxyDetection>,
    /// 快照携带的自动重连状态（S4：`GetSnapshot`/`WatchEvents` 随状态附带；
    /// `None` = 重连不适用/从未建立连接）。
    pub reconnect: Option<ReconnectStatus>,
    /// 快照携带的 engine 侧 R1 自愈遥测（T8 tag 19：engine `ObserveOwnedState`
    /// 快照携带最近一次状态事件的重连分量，core `GetSnapshot` 源化偏好透传；
    /// `None` = 无 R1 语义）。T12 通知出口按 `reason` id 路由气泡文案。
    pub engine_reconnect: Option<EngineReconnectState>,
    /// 快照携带的 host 自愈进展（EXV_UNFREEZE 2026-09-05：`GetSnapshot`/`WatchEvents`
    /// 随状态附带；`None` = 无自愈上下文）。
    pub self_heal: Option<SelfHealStatus>,
    /// 快照携带的会话身份投影（EXV_UNFREEZE 2026-10-04，bug3：`GetSnapshot`/
    /// `WatchEvents` 随状态附带；`None` = 无活动会话身份，UI 回落当前配置）。
    pub session_identity: Option<SessionIdentity>,
}

/// 运行时事件类型（proto RuntimeEventKind）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    Snapshot,
    Transition,
}

/// WatchEvents 的增量事件（proto RuntimeEvent）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub monotonic_tick: u64,
    pub kind: RuntimeEventKind,
    pub snapshot: RuntimeSnapshot,
    /// 本事件所属操作 id（R1 契约，mirror proto `RuntimeEvent.operation_id`；
    /// `Some(hex16)` = 有在途操作，`None` = 无）。
    pub operation_id: Option<String>,
}

/// 一条操作回复（proto OperationReply 的 UI 视图）。
///
/// R1w 异步契约：core 的 Connect/Stop 先回 `pending`（终态走 status 通道），故
/// [`OperationResult`] 增加 `Pending` 分支——前端收到 pending = 「已受理，等待
/// `exv://status` 事件」，**不是失败**（修复盲清缺陷 B-F1 的配套）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum OperationResult {
    /// 操作已受理（异步）：终态/阶段经 `exv://status` 事件驱动。
    Pending,
    Succeeded {
        effect_id: Option<String>,
        authority_epoch: Option<u64>,
    },
    Failed {
        error: Option<VpnError>,
    },
}

/// Command（connect/stop/...）的统一回复形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationReply {
    #[serde(default)]
    pub stop_completion: Option<StopCompletion>,
    pub result: OperationResult,
    /// 本命令所属操作 id（UI 侧组装 intent 时生成，随回复回传前端做事件关联；
    /// `Some(hex16)` = 已受理（host 以该 id 关联状态事件），`None` = 未产生/被拒）。
    pub operation_id: Option<String>,
}

impl ConnectPhase {
    /// proto 序号，供前端进度条。
    pub fn index(self) -> u8 {
        match self {
            Self::ObservingOwnedState => 0,
            Self::AcquiringPlatformLease => 1,
            Self::ConnectingControl => 2,
            Self::AwaitingInteraction => 3,
            Self::NegotiatingTunnel => 4,
            Self::ApplyingPlatformTunnel => 5,
            Self::AttachingPacketBoundary => 6,
            Self::StartingDataPlane => 7,
        }
    }
}
