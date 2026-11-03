import type { AuthPreparationAction, AuthPreparationEvent, AuthReceiverReply } from "../product/auth-preparation";
// Typed IPC wrappers: Tauri Command (invoke) + Event (listen).
// 契约来源：app/src/kernel/{commands,events,state,logs}.rs（P4-a 骨架）。
// 前端重写（O2）：不复用 C++ webui/ 代码。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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
  /**
   * R2（T6b）恢复事件的稳定 reason id（engine_lost_service / engine_lost_oneshot /
   * engine_lost_oneshot_gated_off；空 = 无 R2 事件）。通知文案按 id 路由（T12）。
   */
  reason?: string;
}

/**
 * engine 侧 R1 自愈遥测（T6a 产出、T8 tag 19 随快照携带；经 core `GetSnapshot`
 * 源化偏好到达）。仅状态上报；`reason` 为稳定 id（tunnel_lost / healed /
 * heal_assembly_failed / heal_timeout / auth_rejected / attempts_exhausted）。
 */
export interface EngineReconnectState {
  /** 本次事件描述的第几次尝试（触发事件即第 1 次）。 */
  attempt: number;
  /** 下次重试的墙钟时刻（UTC epoch ms；0 = 尝试在途/无下次）。 */
  next_retry_at_ms: number;
  /** 当前重连步骤的稳定 reason id；空 = 不适用。 */
  reason: string;
}

/** 连接受理时冻结的会话身份投影（EXV_UNFREEZE 2026-10-04，bug3；仅显示，非授权
 * 材料）。会话活动期间设置页改协议/账号/服务器不得改写连接页显示。 */
export interface SessionIdentity {
  /** 活动会话使用的协议："cisco" | "atrust"（未知码由前端按 Cisco 保守处理）。 */
  protocol: string;
  /** 受理时冻结的账号；空 = 无（aTrust 经系统认证、无可读账号）。 */
  username: string;
  /** 受理时冻结的服务器地址；空 = 无。 */
  server: string;
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

export interface RuntimeSnapshot {
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
  /**
   * T8 tag 19：快照携带的 engine 侧 R1 自愈遥测；null = 无 R1 语义（core 快照
   * 组装路径不带、engine `ObserveOwnedState` 透传时携带）。T12 通知按 reason 路由。
   */
  engine_reconnect?: EngineReconnectState | null;
  /** EXV_UNFREEZE 2026-09-05：快照携带的 host 自愈进展；null = 无自愈上下文。 */
  self_heal?: SelfHealStatus | null;
  /** EXV_UNFREEZE 2026-10-04（bug3）：快照携带的会话身份投影；null = 无活动会话
   * 身份（连接页回落当前配置）。 */
  session_identity?: SessionIdentity | null;
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
  credentials?: ConnectCredentials | null;
}

// ---- 服务控制（S3/D5：KernelControl.ServiceControl） ----

/**
 * ServiceControl 的 action：query 非提权读；install/uninstall/start 走 runas 提权 seam；
 * rotate_key 轮换（撤销）服务 PSK（同一 runas 提权批量通道，无需重启服务）。
 */
export type ServiceControlAction = "query" | "install" | "uninstall" | "start" | "rotate_key";

/** ServiceControl 回复：post-action 服务状态 + 结果 + 人类可读信息（无秘密/栈）。 */
export interface ServiceControlReply {
  service_status?: ServiceStatus | null;
  ok: boolean;
  message: string;
}

export interface ConfigItem {
  key: string;
  value: string;
}

/** 快速入门的一次性提交：只保存核心配置（T4 后服务安装由前端应用级协调器独立执行）。 */
export interface QuickStartApplyRequest {
  items: ConfigItem[];
}

/** 失败不丢弃前端草稿；成功时可附带安装后的服务状态。 */
export interface QuickStartApplyReply {
  ok: boolean;
  service_status?: ServiceStatus | null;
  message?: string;
}

/** R-D3-C：当次启动损坏修复的一次性报告（core 损坏分支产出，不持久化）。 */
export interface ConfigRecovery {
  corrupted: boolean;
  /** 覆盖前的整字节备份文件名（配置目录内，仅文件名）。 */
  backup_file: string;
}

/** S4：config.json 读失败（core 三档自愈后仍不可读）的结构化报告。 */
export interface ConfigReadFailure {
  /** Win32 原始错误码（5=拒绝访问、32=占用…；0=未知）。 */
  win32_code: number;
  /** 读取失败的 config.json 全路径（本地诊断，无秘密）。 */
  path: string;
}

export interface ConfigPayload {
  items: ConfigItem[];
  /** 本次读取是否因无效本地配置而 bootstrap 默认值；不含原始文件、异常或秘密。 */
  requires_quick_start: boolean;
  /** R-D3-C：损坏修复报告（kernel.configGet 恒返回，null=无损坏）；测试注入可省略。 */
  config_recovery?: ConfigRecovery | null;
  /** S4：读失败报告（kernel.configGet 恒返回，null=读取成功含自愈成功）；测试注入可省略。 */
  read_failure?: ConfigReadFailure | null;
}

/** Tauri command 的兼容读模型：旧/测试回复缺少新字段时明确视为 false。 */
interface ConfigPayloadWire {
  items: ConfigItem[];
  requires_quick_start?: boolean;
  config_recovery?: { corrupted?: boolean; backup_file?: string } | null;
  read_failure?: { win32_code?: number; path?: string } | null;
}

/** UI 对 Core 控制面的仅有两种结论：正常（已验证管道可通）或已停止。 */
export type CoreStatus = "normal" | "stopped";

/** T9（§2.9）core 生命周期事件：壳侧保守死亡判定后的弱提示广播。 */
export type CoreLifecycleState = "crashed" | "recovered";

export interface CoreLifecycleEvent {
  state: CoreLifecycleState;
  /** 涉及的 core 进程 pid；0 = UI 从未托管子进程。 */
  core_pid: number;
}

// ---- Command 封装（P4-a：后端返回 not_wired 占位错误） ----

/** 是否运行在 Tauri 外壳内（纯浏览器 `vite dev` 预览时无 IPC bridge）。 */
function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * invoke 的安全封装：在纯浏览器预览（非 Tauri）下返回友好占位错误，
 * 避免裸 TypeError（Cannot read properties of undefined (reading 'invoke')）。
 */
async function invokeTauri<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw { kind: "not_wired", message: "当前运行在 Tauri 外壳之外（纯浏览器预览）；请在 Tauri 应用中运行以连接 core。" };
  }
  return invoke<T>(cmd, args);
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
  atrustBrowserFallback(): Promise<void> {
    return invokeTauri<void>("atrust_browser_fallback");
  },
  atrustBrowserFallbackAvailable(): Promise<boolean> {
    return invokeTauri<boolean>("atrust_browser_fallback_available");
  },
  /** §7.5 手动介入：显示/前置被隐藏的内嵌认证窗口。 */
  atrustShowAuthWindow(): Promise<void> {
    return invokeTauri<void>("atrust_show_auth_window");
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
    const recovery = payload.config_recovery;
    const readFailure = payload.read_failure;
    return {
      items: payload.items,
      requires_quick_start: payload.requires_quick_start === true,
      // 与 requires_quick_start 同一兼容口径：仅 corrupted 明确为 true 才透出，
      // 旧壳/测试回复缺少该字段时静默视为无损坏。
      config_recovery:
        recovery?.corrupted === true
          ? { corrupted: true, backup_file: recovery.backup_file ?? "" }
          : null,
      // S4：read_failure 对象在场即读失败（缺 win32_code 视为未知码 0）；旧壳/
      // 测试回复缺少该字段时视为读取成功。
      read_failure: readFailure
        ? { win32_code: readFailure.win32_code ?? 0, path: readFailure.path ?? "" }
        : null,
    };
  },
  configSet(items: ConfigItem[]): Promise<boolean> {
    return invokeTauri<boolean>("config_set", { items });
  },
  /** S4：「以管理员身份修复权限」——壳 runas 重置 config.json ACL（只动权限不动
   * 内容）后由前端重试 config_get。用户取消 UAC 等失败以拒绝值返回。 */
  configRepairPermissions(): Promise<void> {
    return invokeTauri<void>("config_repair_permissions");
  },
  quickStartApply(request: QuickStartApplyRequest): Promise<QuickStartApplyReply> {
    return invokeTauri<QuickStartApplyReply>("quick_start_apply", { request });
  },
  /** 读取 EXV 隧道适配器当前的真实 IPv4 地址；未连接/尚未分配时返回 null。 */
  tunnelAddress(): Promise<string | null> {
    return invokeTauri<string | null>("tunnel_address");
  },
  respondInteraction(interactionId: number[], responsePayload: number[]): Promise<OperationReply> {
    return invokeTauri<OperationReply>("respond_interaction", { interactionId, responsePayload });
  },
  /** 触发一次延迟刷新（T1：写本地标记，engine 探测循环立即 ping；延迟经快照 stats 到达）。 */
  triggerLatencyRefresh(): Promise<void> {
    return invokeTauri<void>("trigger_latency_refresh");
  },
  /** 服务控制（S3/D5）：query 非提权读；install/uninstall/start/rotate_key 经 host runas seam。 */
  serviceControl(action: ServiceControlAction): Promise<ServiceControlReply> {
    return invokeTauri<ServiceControlReply>("service_control", { action });
  },
  /** T9（§2.9）弱提示「重启 core」按钮：确认死亡后重拉；core 活着则幂等成功。 */
  coreRestart(): Promise<void> {
    return invokeTauri<void>("core_restart");
  },
  /** T9 唤醒失败模态「尝试重启程序」：relaunch 整个 app（O3 停机后新 UI 重新拉起 core）。 */
  appRestart(): Promise<void> {
    return invokeTauri<void>("app_restart");
  },
  /** T9 唤醒失败模态「仅退出」：退出整个 app（后台不残留 EXV 进程）。 */
  appExit(): Promise<void> {
    return invokeTauri<void>("app_exit");
  },
};

// ---- Event 订阅（P4-b 接真实 WatchEvents/StreamLogs 后生效） ----

export const EVENT_STATUS = "exv://status";
export const EVENT_LOGS = "exv://logs";
export const EVENT_INTERACTION = "exv://interaction";
export const EVENT_STATS = "exv://stats";
/** T9（§2.9）：core 崩溃/恢复广播（壳侧保守死亡判定驱动；前端渲染弱提示横幅）。 */
export const EVENT_CORE_LIFECYCLE = "exv://core-lifecycle";

export function onStatus(cb: (ev: RuntimeEvent) => void): Promise<UnlistenFn> {
  return listen<RuntimeEvent>(EVENT_STATUS, (e) => cb(e.payload));
}

/** 验证码风险预告载荷（`exv://browser-login` 首个 active=true 事件可选携带）：
 * 30 分钟滚动窗口内已记录的学校表单登录次数达到阈值时下发；数字与 Rust 侧
 * `login_attempt_tracker.rs` 常量同源，警示句模板只在 BrowserAuthWaitingModal 渲染。 */
export interface LoginThrottleWarning {
  attempts: number;
  window_minutes: number;
  threshold: number;
}

/** 桌面端浏览器认证等待，与 Core 的 VPN 连接状态分别报告。 */
export interface BrowserLoginEvent {
  /** 当前用户明确选择的临时认证渠道；普通重复首事件保持冻结语义。 */
  channel_override?: boolean;
  active: boolean;
  generation: number;
  outcome?: "authenticated" | "cancelled";
  /** 后台认证首事件的冻结呈现上下文；后续阶段沿同一 generation。 */
  recovery?: { mode: "embedded" | "browser" | "headless"; auto_login: boolean };
  stage?: string | null;
  /** 自动恢复事件可选携带；旧载荷不要求轮次。 */
  round?: 1 | 2;
  throttle_warning?: LoginThrottleWarning | null;
}

export function onBrowserLogin(
  cb: (event: BrowserLoginEvent) => void,
): Promise<UnlistenFn> {
  return listen<BrowserLoginEvent>("exv://browser-login", (event) => cb(event.payload));
}

export function onLogs(cb: (log: LogEvent) => void): Promise<UnlistenFn> {
  return listen<LogEvent>(EVENT_LOGS, (e) => cb(e.payload));
}

/**
 * 统计推送（stats-wire 方案 A 后为 seam：统计随 `exv://status` 的 `snapshot.stats`
 * 到达，前端直接读 `ev.snapshot.stats`；本事件保留供独立高频推送，暂无发射源）。
 */
export function onStats(cb: (stats: RuntimeStats) => void): Promise<UnlistenFn> {
  return listen<RuntimeStats>(EVENT_STATS, (e) => cb(e.payload));
}

/** T9（§2.9）：core 崩溃/恢复事件订阅（弱提示横幅的唯一驱动源）。 */
export function onCoreLifecycle(cb: (event: CoreLifecycleEvent) => void): Promise<UnlistenFn> {
  return listen<CoreLifecycleEvent>(EVENT_CORE_LIFECYCLE, (e) => cb(e.payload));
}

/** 命令错误判定：后端占位错误 kind 为 not_wired。 */
export function isNotWired(e: unknown): boolean {
  return (
    typeof e === "object" &&
    e !== null &&
    (e as { kind?: string }).kind === "not_wired"
  );
}

export function onAuthPreparation(cb: (event: AuthPreparationEvent) => void): Promise<UnlistenFn> {
  return listen<AuthPreparationEvent>("exv://auth-preparation", event => cb(event.payload));
}
