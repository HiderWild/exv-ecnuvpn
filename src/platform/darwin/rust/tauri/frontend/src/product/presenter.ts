import type { ConnectPhase, RuntimeSnapshot, RuntimeState } from "../lib/ipc";
import {
  CONNECT_PHASE_ORDER,
  STATE_UI,
  errorOf,
  phaseIndexOf,
  phaseLabelOf,
} from "../lib/status-map";
import type { CoarsePhase } from "../lib/status-map";
import {
  formatBytes,
  formatLatency,
  formatOnlineDuration,
  formatRate,
  hasUsableConnectedSample,
} from "./formatters";
import { EMPTY_PRODUCT_CONNECTION_INFO } from "./types";
import type {
  ProductMetricsDisplay,
  ProductNetworkResources,
  ProductProxyTun,
  ProductReconnect,
  ProductSelfHeal,
  ProductService,
  ProductServiceMode,
  ProductServiceState,
  ProductSeverity,
  ProductStage,
  ProductStatus,
  ProductSystemProxy,
  ProductUiState,
  ProductConnectionInfo,
  ServiceScmState,
  SystemProxyMode,
} from "./types";

/**
 * Rust 的工程阶段名不直接进入产品界面。未知阶段仍回退现有 `phaseLabelOf`，避免静默捏造文字。
 */
export const PRODUCT_PHASE_LABELS: Record<ConnectPhase, string> = {
  observing_owned_state: "检查环境",
  acquiring_platform_lease: "准备权限",
  connecting_control: "连接控制",
  awaiting_interaction: "用户认证",
  negotiating_tunnel: "建立通道",
  applying_platform_tunnel: "写入配置",
  attaching_packet_boundary: "启用通道",
  starting_data_plane: "检查网络",
};

interface ProductCopy {
  title: string;
  severity: ProductSeverity;
}

const PRODUCT_COPY: Record<RuntimeState["state"], ProductCopy> = {
  idle: { title: "未连接", severity: "normal" },
  connecting: { title: "连接中", severity: "attention" },
  awaiting_interaction: { title: "认证中", severity: "attention" },
  connected: { title: "已连接", severity: "success" },
  stopping: { title: "断开中", severity: "attention" },
  reconciling: { title: "处理中", severity: "attention" },
  failed_clean: { title: "连接失败", severity: "error" },
  failed_dirty: { title: "需要处理", severity: "blocking" },
};

const PRODUCT_STATUS_BY_COARSE_PHASE: Record<CoarsePhase, ProductStatus> = {
  idle: "idle",
  connecting: "connecting",
  awaiting_interaction: "awaiting",
  connected: "connected",
  stopping: "stopping",
  reconciling: "reconciling",
  failed: "failed",
};

function productStatus(runtime: RuntimeState): ProductStatus {
  return PRODUCT_STATUS_BY_COARSE_PHASE[STATE_UI[runtime.state].coarse];
}

function currentPhase(runtime: RuntimeState): ConnectPhase | null {
  if (runtime.state === "connecting") return runtime.phase;
  if (runtime.state === "awaiting_interaction") return "awaiting_interaction";
  return null;
}

function productPhaseLabel(phase: ConnectPhase): string {
  return PRODUCT_PHASE_LABELS[phase] ?? phaseLabelOf(phase);
}

function stagesFor(runtime: RuntimeState): ProductStage[] {
  const current = currentPhase(runtime);
  if (current === null) return [];

  const currentIndex = phaseIndexOf(current);
  if (currentIndex < 0) return [];

  return CONNECT_PHASE_ORDER.map((phase, index) => ({
    phase,
    label: productPhaseLabel(phase),
    visual: index < currentIndex ? "complete" : index === currentIndex ? "current" : "waiting",
  }));
}

function proxyTunFor(snapshot: RuntimeSnapshot): ProductProxyTun {
  const detection = snapshot.proxy_tun;
  if (detection === null || detection === undefined) {
    return { status: "unknown", adapterNames: [], routePolicy: null };
  }

  const adapterNames = detection.adapters
    .map((adapter) => adapter.name.trim())
    .filter((name) => name.length > 0);

  return {
    status: detection.detected ? "detected" : "not_detected",
    adapterNames,
    routePolicy: detection.route_policy.trim() || null,
  };
}

/** 解析 wire `mode` 字符串为穷尽 typed [`SystemProxyMode`]（未知码 fail-closed → "unknown"）。 */
function parseSystemProxyMode(raw: string): SystemProxyMode {
  switch (raw) {
    case "disabled":
    case "manual":
    case "automatic":
    case "mixed":
      return raw;
    default:
      return "unknown";
  }
}

/** S2：快照 `system_proxy` → 产品系统代理感知（null/未知 → `unknown`，不伪造）。 */
function systemProxyFor(snapshot: RuntimeSnapshot): ProductSystemProxy {
  const wire = snapshot.system_proxy;
  if (wire === null || wire === undefined) {
    return { status: "unknown", endpointCount: 0, bypassMerged: false, topology: null };
  }
  return {
    status: parseSystemProxyMode(wire.mode),
    endpointCount: wire.endpoint_count,
    bypassMerged: wire.bypass_merged,
    topology: wire.topology.trim() || null,
  };
}

/** S4：快照 `reconnect` → 产品自动重连感知（null → 全禁用占位，不伪造在途状态）。 */
function reconnectFor(snapshot: RuntimeSnapshot): ProductReconnect {
  const wire = snapshot.reconnect;
  if (wire === null || wire === undefined) {
    return { enabled: false, active: false, currentAttempt: 0, maxAttempts: 0 };
  }
  return {
    enabled: wire.auto_reconnect,
    active: wire.active,
    currentAttempt: wire.current_attempt,
    maxAttempts: wire.max_attempts,
  };
}

/**
 * 自愈文案表（2026-09-05 host 自愈进展 UI 计划 §4.5，冻结，用户可读）。
 *
 * `failed` 档必须写「重启应用」而非裸「重新连接」（冻结理由）：respawn 失败路径
 * composition 不重建，admission 关闭 + `Reconciling` 相位下用户点连接会被
 * `ConnectRefused` → `failed_precondition` 拒绝，重启应用是唯一恢复路径——裸
 * 「重新连接」承诺了代码做不到的事。
 */
const SELF_HEAL_COPY: Record<"respawning" | "succeeded" | "failed" | "unknown", string> = {
  respawning: "检测到服务组件异常退出，正在自动恢复引擎…",
  succeeded: "引擎已自动恢复；本次连接已断开，请重新连接。",
  failed: "引擎自动恢复失败，请重启应用后重新连接；若重复失败，请查看日志并反馈。",
  unknown: "引擎正在自动恢复（状态未知），请稍候或查看日志。",
};

/** 解析 wire `stage` 字符串为穷举档位（未知码 fail-safe → "unknown"，不伪造完成/失败）。 */
function parseSelfHealStage(raw: string): "respawning" | "succeeded" | "failed" | "unknown" {
  switch (raw) {
    case "respawning":
    case "succeeded":
    case "failed":
      return raw;
    default:
      return "unknown";
  }
}

/**
 * EXV_UNFREEZE 2026-09-05：快照 `self_heal` → 产品自愈进展（计划 §4.5 渲染规则）。
 *
 * 规则 1：null/undefined → `{ active: false }`；规则 3（显示抑制）：`succeeded` 且
 * runtime 为 connecting/awaiting_interaction/connected（用户已重连、host 清除尚未
 * 到达的竞态窗口）→ 按 `{ active: false }` 渲染；规则 4：未知 stage 字符串 →
 * `unknown` 档。`respawning`/`failed` 不抑制（它们本身即真实状态的解释行）。
 */
function selfHealFor(snapshot: RuntimeSnapshot): ProductSelfHeal {
  const wire = snapshot.self_heal;
  if (wire === null || wire === undefined) {
    return { active: false };
  }
  const stage = parseSelfHealStage(wire.stage);
  if (
    stage === "succeeded" &&
    (snapshot.runtime.state === "connecting" ||
      snapshot.runtime.state === "awaiting_interaction" ||
      snapshot.runtime.state === "connected")
  ) {
    return { active: false };
  }
  const description = SELF_HEAL_COPY[stage];
  if (stage === "failed") {
    return { active: true, stage, description, errorCode: wire.error_code };
  }
  return { active: true, stage, description };
}

function metricsFor(snapshot: RuntimeSnapshot, nowMs: number): ProductMetricsDisplay | null {
  if (snapshot.runtime.state !== "connected") {
    return null;
  }

  // 会话起点由 Connected 状态事实提供；不能因为流量采样尚未抵达而把它隐藏。
  const online = formatOnlineDuration(snapshot.runtime.session_established_at_ms, nowMs) ?? "—";

  if (!hasUsableConnectedSample(snapshot.stats)) {
    return { availability: "unavailable", online };
  }

  const stats = snapshot.stats;
  const downloadRate = formatRate(stats.rx_rate_bps);
  const uploadRate = formatRate(stats.tx_rate_bps);
  const downloadTotal = formatBytes(stats.rx_bytes);
  const uploadTotal = formatBytes(stats.tx_bytes);

  // hasUsableConnectedSample 已验证这四个流量值；这层检查保留为防御式边界，绝不回退为零。
  if (downloadRate === null || uploadRate === null || downloadTotal === null || uploadTotal === null) {
    return { availability: "unavailable", online };
  }

  return {
    availability: "available",
    online,
    downloadRate,
    uploadRate,
    downloadTotal,
    uploadTotal,
    // 延迟来自 engine 的独立隧道探测，并不依赖自动重连；不能因「自动重连关闭」
    // 而丢弃一份已经到达的真实 RTT 样本。
    latency: formatLatency(stats.latency_ms),
  };
}

/**
 * 归一化 connectionInfo：trim 后空串归 `null`，不产生零长度显示。
 * 行为冻结为现状（§4.3），本函数只做 trim/空串归 null，不改逻辑、不重试、不回退旧值。
 *
 * 两级占位契约（2026-09-05-product-ui-unimplemented-fields-plan §4.2，冻结）：
 * 三字段全部**有数据口**（config 非秘密项 `username`/`server` + `tunnel_address`
 * 本地适配器探测）→ `null` 一律渲染为「—」（有口值缺档），绝不渲染「未实现」
 * （无数据口档——那三个字段有口，「未实现」会变成谎言）；「未实现」档在产品中的
 * 唯一活跃代表是 networkResources（S1.5 owner，见 `networkResourcesFor`），
 * 该字段永不回落「—」。
 */
function connectionInfoFor(info: ProductConnectionInfo): ProductConnectionInfo {
  const clean = (value: string | null | undefined): string | null => {
    const trimmed = value?.trim();
    return trimmed ? trimmed : null;
  };

  return {
    account: clean(info.account),
    vpnServer: clean(info.vpnServer),
    campusIp: clean(info.campusIp),
  };
}

/** 解析 wire `state` 字符串为穷尽 typed [`ServiceScmState`]（未知码 fail-closed → "other"）。 */
function parseServiceScmState(raw: string): ServiceScmState {
  switch (raw) {
    case "stopped":
    case "start_pending":
    case "running":
    case "stop_pending":
    case "pause_pending":
    case "paused":
    case "continue_pending":
      return raw;
    default:
      return "other";
  }
}

/**
 * S3/D5：快照 `service_status`/`mode` → 产品服务感知。
 *
 * MED [3]：`service_status` 为 null（尚未查询/查询失败）→ `unknown`，UI 不得据此
 * 勾选「先装服务再连接」（auto_install 仅在明确「未装」时生效），连接回退 oneshot。
 */
function serviceFor(snapshot: RuntimeSnapshot): ProductService {
  const wire = snapshot.service_status;
  let status: ProductServiceState;
  if (wire === null || wire === undefined) {
    status = { kind: "unknown" };
  } else if (!wire.installed) {
    status = { kind: "not_installed" };
  } else {
    status = {
      kind: "installed",
      scmState: parseServiceScmState(wire.state),
      healthState: wire.health_state?.trim() || null,
    };
  }
  const mode: ProductServiceMode =
    snapshot.mode === "auto" || snapshot.mode === "service" || snapshot.mode === "oneshot"
      ? snapshot.mode
      : "unknown";
  return { status, mode };
}

/**
 * 网络资源状态（S1.5 三 owner）：快照 wire 未携带 → **无任何数据口**，
 * `available: false` 占位，不伪造 owner 状态。
 * 两级占位契约（§4.2）的「未实现」档：无口的字段固定语义为「未实现」
 * （不可点击、非故障/警告语义），永不回落「—」，直至 S1.5 owner 状态真正接线。
 */
function networkResourcesFor(): ProductNetworkResources {
  return { available: false };
}

function descriptionFor(runtime: RuntimeState): string | null {
  if (runtime.state === "connected") {
    const summary = runtime.summary?.trim();
    return summary || null;
  }

  return errorOf(runtime);
}

function errorCodeFor(runtime: RuntimeState): string | null {
  switch (runtime.state) {
    case "failed_clean":
    case "failed_dirty":
      return runtime.error?.code?.trim() || null;
    case "reconciling":
      return runtime.blocking_error?.code?.trim() || null;
    default:
      return null;
  }
}

/** 将唯一业务事实 `RuntimeSnapshot` 翻译为 UI 不再自行猜测的产品呈现状态。 */
export function present(
  snapshot: RuntimeSnapshot,
  nowMs: number,
  connectionInfo: ProductConnectionInfo = EMPTY_PRODUCT_CONNECTION_INFO,
  coreStatus: ProductUiState["coreStatus"] = "normal",
): ProductUiState {
  const runtime = snapshot.runtime;
  const stateUi = STATE_UI[runtime.state];
  const copy = PRODUCT_COPY[runtime.state];
  const reconnect = reconnectFor(snapshot);

  return {
    coreStatus,
    operationId: snapshot.operation_id?.trim() || null,
    status: productStatus(runtime),
    severity: copy.severity,
    title: copy.title,
    description: descriptionFor(runtime),
    errorCode: errorCodeFor(runtime),
    stages: stagesFor(runtime),
    metrics: metricsFor(snapshot, nowMs),
    connectionInfo: connectionInfoFor(connectionInfo),
    proxyTun: proxyTunFor(snapshot),
    systemProxy: systemProxyFor(snapshot),
    reconnect,
    selfHeal: selfHealFor(snapshot),
    service: serviceFor(snapshot),
    networkResources: networkResourcesFor(),
    connectEnabled: stateUi.connectEnabled,
    stopEnabled: stateUi.stopEnabled,
  };
}
