import type { AuthPreparationAction, AuthPreparationEvent, AuthReceiverReply } from "../product/auth-preparation";
// Typed IPC wrappers: Tauri Command (invoke) + Event (listen).
// 契约来源：app/src/kernel/{commands,events,state,logs}.rs（P4-a 骨架）。
// 前端重写（O2）：不复用 C++ webui/ 代码。
//
// Darwin 适配（单一接缝）：不导入宿主 SDK；invoke/listen 语义经壳注入的
// `__EXV_DARWIN_COMMAND_ADAPTER__` 全局 adapter 等价映射（product/command-adapter-global.ts）。

import { darwinCommandAdapter } from "../product/command-adapter-global";

/** 事件退订函数（与宿主 event API 的 `UnlistenFn` 等价的本地形状）。 */
type UnlistenFn = () => void | Promise<void>;

// ---- 运行时状态（镜像 app/src/kernel/state.rs） ----

export type ConnectPhase =
  | "observing_owned_state"
  | "acquiring_platform_lease"
  | "connecting_control"
  | "awaiting_interaction"
  | "negotiating_tunnel"
  | "applying_platform_tunnel"
  | "attaching_packet_boundary"
  | "starting_data_plane";

export interface VpnError {
  code: string;
  message: string;
  stage?: string | null;
  /** 稳定失败原因 id（EXV_UNFREEZE 2026-10-09，tag 8；空 = 缺省）。文案按
   * `lib/copy-tables.ts` 的 REASON_COPY 映射——单一文案来源。 */
  reason?: string | null;
}

export type RuntimeState =
  | { state: "idle"; last_cleanup_at_ms?: number | null }
  | { state: "connecting"; phase: ConnectPhase; attempt_id?: string | null; phase_index: number }
  | { state: "awaiting_interaction"; attempt_id?: string | null; prompt_deadline_ms?: number | null }
  | { state: "connected"; session_established_at_ms?: number | null; summary?: string | null }
  | { state: "stopping"; reason?: string | null }
  | { state: "reconciling"; blocking_error?: VpnError | null }
  | { state: "failed_clean"; error?: VpnError | null }
  | { state: "failed_dirty"; error?: VpnError | null; has_obligation: boolean };

/** 检测到的上游代理 TUN 适配器（C5-wire；镜像 app/src/kernel/state.rs）。 */
export interface ProxyTunAdapter {
  /** 友好名称（如 "Mihomo" / "Meta"）。 */
  name: string;
  /** 接口描述（如 "Wintun Userspace Tunnel"）。 */
  description: string;
  /** IPv4 接口索引。 */
  if_index: number;
  /** 适配器种类（当前恒为 "proxy_tun"）。 */
  kind: string;
}

/** 上游代理 TUN 检测结果（C5-wire；仅状态上报，不改行为）。 */
export interface ProxyTunDetection {
  /** 是否检测到至少一个上游代理 TUN 适配器。 */
  detected: boolean;
  /** 检测到的适配器（detected=false 时为空）。 */
  adapters: ProxyTunAdapter[];
  /** 共存路由策略："exv-before-proxy-tun" | "normal"。 */
  route_policy: string;
}

/** 系统代理检测结果（S2；仅状态上报，不携带端点 URL/秘密——只有计数与短枚举串）。 */
export interface SystemProxyDetection {
  /** 系统代理模式："disabled" | "manual" | "automatic" | "mixed"。 */
  mode: string;
  /** 检测到的系统代理端点数。 */
  endpoint_count: number;
  /** EXV 豁免条目是否已合并进系统代理设置。 */
  bypass_merged: boolean;
  /** 四态拓扑（(proxy_present, tunnel_present) 的纯函数分类）："t0" | "t1" | "t2" | "t3"。 */
  topology: string;
}

/** 自动重连状态（S4；仅状态上报——重连决策在 host 侧 C3a 重连 worker）。 */
export interface ReconnectStatus {
  /** 自动重连开关（config auto_reconnect）。 */
  auto_reconnect: boolean;
  /** 配置的重试预算（config auto_reconnect_max_attempts；0 = 无限）。 */
  max_attempts: number;
  /** 本次连接已执行的重试次数（per-connection；0 = 无）。 */
  current_attempt: number;
  /** 重连尝试是否正在飞行中。 */
  active: boolean;
  /** 重连 reason lane 稳定 id（S5b 暂停事实 reconnect_paused_* 等；空 = 无事实）。 */
  reason?: string;
}

/** host 自愈（engine 崩溃 respawn）状态（EXV_UNFREEZE 2026-09-05；仅状态上报——
 * respawn 决策与编排留在 host，wire 不携带凭据/栈）。 */
export interface SelfHealStatus {
  /** 自愈阶段："respawning" | "succeeded" | "failed"；未知码原样透传。 */
  stage: string;
  /** 崩溃 engine PID；0 = 未知。 */
  old_pid: number;
  /** 新 engine PID；0 = 未知/不适用（stage ≠ "succeeded" 时为 0）。 */
  new_pid: number;
  /** 稳定错误码；仅 stage="failed" 非空。 */
  error_code: string;
}

/** win32 engine SCM 服务状态（S3/D5；仅状态展示，非授权材料——服务存在性不构成
 * capability，peer 身份只来自验证过的传输元数据 + PSK）。 */
export interface ServiceStatus {
  /** 服务是否已在 SCM 注册。 */
  installed: boolean;
  /** SCM 服务状态："stopped" | "start_pending" | "stop_pending" | "running" | "other"。 */
  state: string;
  /** R3 统一服务健康状态："healthy" | "scm_orphan" | "installed_unavailable" |
   * "payload_orphan" | "not_installed"；host 未派生时为空。 */
  health_state?: string | null;
}

export interface StopCompletion {
  stopped: boolean;
  blocks_new_connect: boolean;
  errors: VpnError[];
}

/** 已由 Core 接纳的非秘密会话身份；不来自当前可变配置。 */
export interface SessionIdentity {
  protocol: string;
  username: string;
  server: string;
}

export interface RuntimeSnapshot {
  session_identity?: SessionIdentity | null;
  engine_reconnect?: { attempt: number; next_retry_at_ms: number; reason: string } | null;
  stop_completion?: StopCompletion | null;
  runtime: RuntimeState;
  monotonic_tick: number;
  /** stats-wire 方案 A：快照携带最新归一化统计；null = 尚无样本。 */
  stats?: RuntimeStats | null;
  /** C5-wire：快照携带上游代理 TUN 检测；null = 未探测/探测失败。 */
  proxy_tun?: ProxyTunDetection | null;
  /**
   * R1/R4：本快照所属的 in-flight 连接/停止操作 id（hex16，32 字符）；null/空 = 无
   * 在途操作。前端据此只让「当前用户操作」的事件驱动 UI（旧操作迟到事件不扰动显示）。
   */
  operation_id?: string | null;
  /** S3/D5：快照携带的 win32 engine SCM 服务状态；null = 尚未查询/查询失败。仅展示。 */
  service_status?: ServiceStatus | null;
  /** S3/D3：当前连接模式 "auto" | "service" | "oneshot"；只展示，不做路由输入。 */
  mode?: string | null;
  /** S2：快照携带的系统代理检测；null = 未检测/检测失败。仅状态上报。 */
  system_proxy?: SystemProxyDetection | null;
  /** S4：快照携带的自动重连状态；null = 重连不适用/从未建立连接。仅状态上报。 */
  reconnect?: ReconnectStatus | null;
  /** EXV_UNFREEZE 2026-09-05：快照携带的 host 自愈进展；null = 无自愈上下文。 */
  self_heal?: SelfHealStatus | null;
}

export interface RuntimeEvent {
  monotonic_tick: number;
  kind: "snapshot" | "transition";
  snapshot: RuntimeSnapshot;
  /** R1/R4：本事件所属操作 id（镜像 snapshot.operation_id）；null/空 = 无在途操作。 */
  operation_id?: string | null;
}

export type OperationResult =
  | { result: "pending" }
  | { result: "succeeded"; effect_id?: string | null; authority_epoch?: number | null }
  | { result: "failed"; error?: VpnError | null };

export interface OperationReply {
  stop_completion?: StopCompletion | null;
  result: OperationResult;
  /** R4：本命令生成的操作 id（hex16）；null = 未产生/被拒（无状态事件关联）。 */
  operation_id?: string | null;
}

// ---- 运行期统计（镜像 app/src/kernel/stats.rs + host stats.rs::RuntimeStats） ----

/** 统计采样时刻的粗粒度连接阶段（proto StatsPhase）。 */
export type StatsPhase =
  | "unspecified"
  | "idle"
  | "connecting"
  | "connected"
  | "stopping"
  | "failed";

/** 归一化后的运行期统计（host core 权威；rx/tx_rate_bps 为累计增量 / 时间间隔）。 */
export interface RuntimeStats {
  /** 累计接收字节（engine 权威）。 */
  rx_bytes: number;
  /** 累计发送字节（engine 权威）。 */
  tx_bytes: number;
  /** 归一化接收速率（bytes/sec）。 */
  rx_rate_bps: number;
  /** 归一化发送速率（bytes/sec）。 */
  tx_rate_bps: number;
  /** 往返延迟（ms；0 = 未知）。 */
  latency_ms: number;
  /** 采样时刻的连接阶段。 */
  phase: StatsPhase;
  /** engine 样本序号。 */
  engine_sequence: number;
  /** 发布时的事件总线 tick（与状态事件同一 tick 轴；0 = 尚未发布）。 */
  sample_tick: number;
}

// ---- 日志（镜像 app/src/kernel/logs.rs） ----

export interface LogEvent {
  level: "info" | "warn" | "error" | string;
  component: string;
  code: string;
  message: string;
  fields: Record<string, string>;
  timestamp_ms: number;
}

export interface LogChunk {
  events: LogEvent[];
  next_after_seq: number;
  has_more: boolean;
}

export interface LogsClearReply {
  cleared: boolean;
  removed_entries: number;
}

// ---- Command 参数 ----

/** 仅用于本次连接的短生命周期凭据；组件不接触不透明 secret payload。 */
export interface ConnectCredentials {
  username: string;
  password: string;
  persist: boolean;
}

export interface ConnectIntent {
  profile_ref: string;
  atrust_reauthenticate?: string;
  credentials?: ConnectCredentials | null;
}

// ---- 服务控制（S3/D5：KernelControl.ServiceControl） ----

/**
 * ServiceControl 的 action：query 非提权读；install/uninstall/start 走 runas 提权 seam；
 * rotate_key 轮换（撤销）服务 PSK（同一 runas 提权批量通道，无需重启服务）。
 */
export type ServiceControlAction = "query" | "install" | "uninstall" | "start" | "rotate_key";

/** ServiceControl 回复：post-action 服务状态 + 结果 + 人类可读信息（无秘密/栈）。 */
export interface ServiceVersion {
  expected: number;
  bundled: number | null;
  installed: number | null;
  /** 只比较安装文件；不把文件版本冒充正在运行的进程版本。 */
  matches_bundle: boolean | null;
}

export interface ServiceControlReply {
  service_version?: ServiceVersion | null;
  service_status?: ServiceStatus | null;
  ok: boolean;
  message: string;
}

export interface ConfigItem {
  key: string;
  value: string;
}

/** AUTH-03：浏览器选择状态（B3 起仅状态展示：默认/安装枚举/会话内显式偏好；
 * 选择器等待流已退场——默认浏览器不受支持时按优先序自动选择，零可用静默
 * 回退内嵌，前端不再提供选择界面）。 */
export interface AtrustBrowserStatus {
  /** 系统默认浏览器受支持时为 "chrome" | "safari" | "edge"；否则 null。 */
  default_browser: string | null;
  /** 已安装的受支持浏览器（chrome → safari → edge 固定顺序，即平台优先序）。 */
  installed: string[];
  /** 本会话已记住的显式选择（wire 兼容保留，生产界面无写入方）；null = 未选择。 */
  chosen: string | null;
}

/** 快速入门只提交核心配置；服务安装沿既有独立协调器执行。 */
export interface QuickStartApplyRequest {
  items: ConfigItem[];
}

/** 失败不丢弃前端草稿；service_status 仅保留旧回复兼容。 */
export interface QuickStartApplyReply {
  ok: boolean;
  service_status?: ServiceStatus | null;
  message?: string;
}

export interface ConfigRecovery {
  corrupted: boolean;
  backup_file: string;
}

export interface ConfigReadFailure {
  /** 沿既有 wire 保留；Darwin 为 0，界面不展示 Win32 错误号。 */
  win32_code: number;
  path: string;
}

export interface ConfigPayload {
  items: ConfigItem[];
  /** 本次读取是否因无效本地配置而 bootstrap 默认值；不含原始文件、异常或秘密。 */
  requires_quick_start: boolean;
  config_recovery?: ConfigRecovery | null;
  read_failure?: ConfigReadFailure | null;
}

/** Tauri command 的兼容读模型：旧/测试回复缺少新字段时明确视为 false。 */
interface ConfigPayloadWire {
  items: ConfigItem[];
  requires_quick_start?: boolean;
  config_recovery?: ConfigRecovery | null;
  read_failure?: ConfigReadFailure | null;
}

/** UI 对 Core 控制面的仅有两种结论：正常（已验证管道可通）或已停止。 */
export type CoreStatus = "normal" | "stopped";

// ---- Command 封装（P4-a：后端返回 not_wired 占位错误） ----

/**
 * invoke 的安全封装：壳未注入 adapter（纯浏览器 `vite dev` 预览）时返回友好占位
 * 错误，避免裸 TypeError；语义与「不在 Tauri 外壳内」一致。
 */
async function invokeTauri<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const adapter = darwinCommandAdapter();
  if (!adapter) {
    throw { kind: "not_wired", message: "当前运行在 Tauri 外壳之外（纯浏览器预览）；请在 Tauri 应用中运行以连接 core。" };
  }
  return adapter.call<T>(cmd, args);
}

/** listen 的安全封装：同样经壳注入的 adapter 订阅（回调收到带 `payload` 的事件对象）。 */
function listenTauri<Payload>(
  event: string,
  handler: (event: { payload: Payload }) => void,
): Promise<UnlistenFn> {
  const adapter = darwinCommandAdapter();
  if (!adapter) {
    return Promise.reject({ kind: "not_wired", message: "当前运行在 Tauri 外壳之外（纯浏览器预览）；请在 Tauri 应用中运行以订阅事件。" });
  }
  return adapter.listen<Payload>(event, handler);
}

export const kernel = {
  authPreparationRespond(generation: number, action: AuthPreparationAction): Promise<void> {
    return invokeTauri<void>("auth_preparation_respond", { generation, action });
  },
  authReceiverStatus(): Promise<AuthReceiverReply> { return invokeTauri<AuthReceiverReply>("auth_receiver_status"); },
  restoreOfficialAuthReceiver(): Promise<AuthReceiverReply> { return invokeTauri<AuthReceiverReply>("restore_official_auth_receiver"); },
  connect(intent: ConnectIntent): Promise<OperationReply> {
    return invokeTauri<OperationReply>("connect", { intent });
  },
  stop(): Promise<OperationReply> {
    return invokeTauri<OperationReply>("stop");
  },
  snapshot(): Promise<RuntimeSnapshot> {
    return invokeTauri<RuntimeSnapshot>("snapshot");
  },
  coreStatus(): Promise<CoreStatus> {
    return invokeTauri<CoreStatus>("core_status");
  },
  stats(): Promise<RuntimeStats> {
    return invokeTauri<RuntimeStats>("stats");
  },
  logsList(afterSeq: number, limit: number): Promise<LogChunk> {
    return invokeTauri<LogChunk>("logs_list", { afterSeq, limit });
  },
  logsClear(): Promise<LogsClearReply> {
    return invokeTauri<LogsClearReply>("logs_clear");
  },
  async configGet(): Promise<ConfigPayload> {
    const payload = await invokeTauri<ConfigPayloadWire>("config_get");
    return {
      items: payload.items,
      requires_quick_start: payload.requires_quick_start === true,
      config_recovery: payload.config_recovery ?? null,
      read_failure: payload.read_failure ?? null,
    };
  },

  configSet(items: ConfigItem[]): Promise<boolean> {
    return invokeTauri<boolean>("config_set", { items });
  },
  quickStartApply(request: QuickStartApplyRequest): Promise<QuickStartApplyReply> {
    return invokeTauri<QuickStartApplyReply>("quick_start_apply", { request });
  },
  /** 快速入门「已见」标记（KI-03 对齐 win32 KI-13：`~/.exv/quick-start.json` 单键）。 */
  quickStartSeenGet(): Promise<boolean> {
    return invokeTauri<boolean>("quick_start_seen_get");
  },
  quickStartSeenMark(): Promise<void> {
    return invokeTauri<void>("quick_start_seen_mark");
  },
  /** 读取 EXV 隧道适配器当前的真实 IPv4 地址；未连接/尚未分配时返回 null。 */
  tunnelAddress(): Promise<string | null> {
    return invokeTauri<string | null>("tunnel_address");
  },
  respondInteraction(interactionId: number[], responsePayload: number[]): Promise<OperationReply> {
    return invokeTauri<OperationReply>("respond_interaction", { interactionId, responsePayload });
  },
  /** 触发一次延迟刷新（T1：写本地标记，engine 探测循环立即 ping；延迟经快照 stats 到达）。 */
  triggerLatencyRefresh(operationId: string): Promise<void> {
    return invokeTauri<void>("trigger_latency_refresh", { operationId });
  },
  /** 服务控制（S3/D5）：query 非提权读；install/uninstall/start/rotate_key 经 host runas seam。 */
  serviceControl(action: ServiceControlAction): Promise<ServiceControlReply> {
    return invokeTauri<ServiceControlReply>("service_control", { action });
  },
  /** AUTH-03：浏览器选择状态（默认/安装/会话内偏好的状态展示面）。 */
  atrustBrowserStatus(): Promise<AtrustBrowserStatus> {
    return invokeTauri<AtrustBrowserStatus>("atrust_browser_status");
  },
  /** AUTH-03：会话内显式选择（chrome/safari/edge）；wire 兼容保留（生产界面
   * 无写入方），写入后下次连接优先采用并失效自动选择缓存。 */
  atrustBrowserChoose(browser: string): Promise<boolean> {
    return invokeTauri<boolean>("atrust_browser_choose", { browser });
  },
  /** C3：常驻本地 Agent 状态（原生系统浏览器认证的票据接收面；离线=启动失败事实）。 */
  nativeAgentStatus(): Promise<NativeAgentStatus> {
    return invokeTauri<NativeAgentStatus>("native_agent_status");
  },
};

/** 常驻本地 Agent 状态（`exv://native-agent` 事件同形状）。 */
export interface NativeAgentStatus {
  phase?: "idle" | "active";
  online: boolean;
  /** 离线原因（在线为空串）。 */
  message: string;
}

/** C3：常驻本地 Agent 状态事件（bootstrap 启动/失败时发射；前端启动后以命令兜底查询）。 */
export const EVENT_NATIVE_AGENT = "exv://native-agent";

export function onNativeAgentStatus(cb: (status: NativeAgentStatus) => void): Promise<UnlistenFn> {
  return listenTauri<NativeAgentStatus>(EVENT_NATIVE_AGENT, (event) => cb(event.payload));
}

// ---- Event 订阅（P4-b 接真实 WatchEvents/StreamLogs 后生效） ----

export const EVENT_STATUS = "exv://status";
export const EVENT_LOGS = "exv://logs";
export const EVENT_INTERACTION = "exv://interaction";
export const EVENT_STATS = "exv://stats";

export function onStatus(cb: (ev: RuntimeEvent) => void): Promise<UnlistenFn> {
  return listenTauri<RuntimeEvent>(EVENT_STATUS, (e) => cb(e.payload));
}

export function onLogs(cb: (log: LogEvent) => void): Promise<UnlistenFn> {
  return listenTauri<LogEvent>(EVENT_LOGS, (e) => cb(e.payload));
}

/**
 * 统计推送（stats-wire 方案 A 后为 seam：统计随 `exv://status` 的 `snapshot.stats`
 * 到达，前端直接读 `ev.snapshot.stats`；本事件保留供独立高频推送，暂无发射源）。
 */
export function onStats(cb: (stats: RuntimeStats) => void): Promise<UnlistenFn> {
  return listenTauri<RuntimeStats>(EVENT_STATS, (e) => cb(e.payload));
}

/** 命令错误判定：后端占位错误 kind 为 not_wired。 */
export function isNotWired(e: unknown): boolean {
  return (
    typeof e === "object" &&
    e !== null &&
    (e as { kind?: string }).kind === "not_wired"
  );
}

export type BrowserLoginMilestone = "opening" | "school_login" | "submitting" | "gateway_confirming" | "awaiting_manual" | "recovering" | "ticket_invalid";

/** 仅承载本地认证等待呈现，代次与里程碑均来自连接请求。 */
export interface BrowserLoginEvent {
  /** 当前用户明确选择的临时认证渠道；普通重复首事件保持冻结语义。 */
  channel_override?: boolean;
  active: boolean;
  generation: number;
  mode: "embedded" | "browser" | "headless";
  outcome?: "authenticated" | "cancelled";
  /** 后台认证首事件的冻结呈现上下文；后续阶段沿同一 generation。 */
  recovery?: { mode: "embedded" | "browser" | "headless"; auto_login: boolean };
  milestone?: BrowserLoginMilestone;
  /** 自动恢复事件可选携带；缺省载荷沿用原阶段。 */
  round?: 1 | 2;
}
export function onBrowserLogin(cb: (event: BrowserLoginEvent) => void): Promise<UnlistenFn> {
  return listenTauri<BrowserLoginEvent>("exv://browser-login", (event) => cb(event.payload));
}

export function onAuthPreparation(cb: (event: AuthPreparationEvent) => void): Promise<UnlistenFn> {
  return listenTauri<AuthPreparationEvent>("exv://auth-preparation", event => cb(event.payload));
}
