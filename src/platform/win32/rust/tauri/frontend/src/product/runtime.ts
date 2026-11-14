import { readonly, ref, type InjectionKey, type Ref } from "vue";
import { configuredRoutes } from "../lib/route-rules";
import { pushToast } from "../lib/toast";

import {
  kernel,
  onStatus,
  onBrowserLogin,
  type BrowserLoginEvent,
  type ConnectCredentials,
  type ConnectIntent,
  type CoreStatus,
  type LoginThrottleWarning,
  type OperationReply,
  type RuntimeEvent,
  type RuntimeSnapshot,
  type RuntimeState,
  type ServiceControlAction,
  type ServiceControlReply,
  type SessionIdentity,
} from "../lib/ipc";
import { CONNECT_PHASE_ORDER, disconnectDescriptionOf, errorAfterEvent, errorOf, isTerminalState, normalizeReason, shouldApplyEvent, type OpCorrelation } from "../lib/status-map";
import { connectingFirstStageLabel, PRODUCT_PHASE_LABELS, present, reconnectTransitionOf } from "./presenter";
import {
  EMPTY_PRODUCT_CONNECTION_INFO,
  type ProductConnectionInfo,
  type ProductStage,
  type ProductUiState,
} from "./types";

export type StopListening = () => void;

/** 当前 Rust wire 已定义的无 profile 连接意图；不得由页面补造 profile。 */
export const NO_PROFILE_CONNECT_INTENT: ConnectIntent = Object.freeze({
  profile_ref: "",
});

/** 产品层唯一允许的真实业务入口；刻意不包含 onStats。 */
export interface ProductGateway {
  connect(intent: ConnectIntent): Promise<OperationReply>;
  stop(): Promise<OperationReply>;
  snapshot(): Promise<RuntimeSnapshot>;
  /** Core 存续状态：仅由已认证控制管道的可用性判定。 */
  coreStatus?(): Promise<CoreStatus>;
  onStatus(listener: (event: RuntimeEvent) => void): Promise<StopListening>;
  onBrowserLogin?(listener: (event: boolean | BrowserLoginEvent) => void): Promise<StopListening>;
  /** T1：请求一次延迟刷新（engine 数据面立即 ping；新延迟经后续快照 stats 到达）。 */
  triggerLatencyRefresh(): Promise<void>;
  /** S3/D5：服务控制（query/install/uninstall/start）。 */
  serviceControl(action: ServiceControlAction): Promise<ServiceControlReply>;
  /** 连接身份与校内地址；真实网关读取 core 配置并探测 EXV 隧道适配器。 */
  connectionInfo?(): Promise<ProductConnectionInfo>;
  atrustBrowserFallbackAvailable?(): Promise<boolean>;
  atrustBrowserFallback?(): Promise<void>;
  /** §7.5 手动介入：显示/前置被隐藏的内嵌认证窗口。 */
  atrustShowAuthWindow?(): Promise<void>;
}

export interface ProductRuntime {
  readonly browserLoginActive?: Readonly<Ref<boolean>>;
  readonly browserLoginGeneration?: Readonly<Ref<number | null>>;
  readonly browserLoginRecovery?: Readonly<Ref<BrowserLoginEvent["recovery"] | null>>;
  readonly browserLoginCancelled?: Readonly<Ref<{ generation: number } | null>>;
  readonly browserAuthenticated?: Readonly<Ref<{ attempt: number; generation: number } | null>>;
  /** §7.9：宿主经 `exv://browser-login` 回传的认证阶段；等待结束即清空。 */
  readonly ticketRefreshReason?: Readonly<Ref<"missing" | "expired" | null>>;
  readonly browserLoginStage?: Readonly<Ref<string | null>>;
  /** 验证码风险预告：30 分钟窗口内已多次表单登录时，本次认证等待期间携带的频次
   * 预告数字；认证结束/新连接发起即清空。模态据此渲染「可能触发验证码」提示。 */
  readonly loginThrottleWarning?: Readonly<Ref<LoginThrottleWarning | null>>;
  readonly state: Readonly<Ref<ProductUiState>>;
  readonly source: "real" | "mock";
  start(): Promise<void>;
  connect(credentials?: ConnectCredentials): Promise<string | void>;
  stop(): Promise<void>;
  /** 配置保存后刷新身份；strict 失败返回固定错误，普通探测保持容错。 */
  configurationChanged(options?: { strict?: boolean }): Promise<void>;
  /**
   * T1：触发一次延迟刷新并拉取最新快照。best-effort：engine 周期探测
   * （每 3 分钟）仍会更新延迟，即使显式刷新失败也不视为业务错误。
   */
  triggerLatencyRefresh(): Promise<void>;
  /**
   * S3/D5：执行服务控制并立即拉取最新快照（post-action 服务状态经 GetSnapshot 反映）。
   * 返回 reply（含 post-action 服务状态 + ok + 人类可读信息）。
   */
  serviceControl(action: ServiceControlAction): Promise<ServiceControlReply>;
  atrustBrowserFallbackAvailable?(): Promise<boolean>;
  atrustBrowserFallback?(): Promise<void>;
  /** §7.5 手动介入：显示/前置被隐藏的内嵌认证窗口。 */
  atrustShowAuthWindow?(): Promise<void>;
  dispose(): void;
}

/** 供后续产品壳注入同一运行时；页面不得直接调用 kernel 或 onStatus。 */
export const PRODUCT_RUNTIME_KEY: InjectionKey<ProductRuntime> = Symbol("product-runtime");

/** Tauri 的真实网关：只转发 Task 3 允许的四个现有接口。 */
export const tauriProductGateway: ProductGateway = {
  connect: (intent) => kernel.connect(intent),
  stop: () => kernel.stop(),
  snapshot: () => kernel.snapshot(),
  coreStatus: () => kernel.coreStatus(),
  onStatus: (listener) => onStatus(listener),
  onBrowserLogin: (listener) => onBrowserLogin(listener),
  triggerLatencyRefresh: () => kernel.triggerLatencyRefresh(),
  serviceControl: (action) => kernel.serviceControl(action),
  atrustBrowserFallbackAvailable: () => kernel.atrustBrowserFallbackAvailable(),
  atrustBrowserFallback: () => kernel.atrustBrowserFallback(),
  atrustShowAuthWindow: () => kernel.atrustShowAuthWindow(),
  async connectionInfo(): Promise<ProductConnectionInfo> {
    const [config, campusIp] = await Promise.all([
      kernel.configGet(),
      kernel.tunnelAddress().catch(() => null),
    ]);
    const values = new Map(config.items.map((item) => [item.key, item.value]));
    return {
      account: values.get("protocol") === "atrust" ? null : values.get("username")?.trim() || null,
      accountSource: values.get("protocol") === "atrust" ? "unavailable" : "configured",
      vpnServer: values.get("server")?.trim() || null,
      campusIp,
      routeRuleCount: configuredRoutes(values.get("routes")).length,
    };
  },
};

type DisplayError = string | null | undefined;

/**
 * C3（ui-connect-stop-responsiveness）：connecting/stopping 期间的兜底真相源轮询周期。
 * 事件链（engine → host 转发器 → 壳 WatchEvents → `exv://status`）丢失时，1s 周期的
 * GetSnapshot 在 ≤2s 内把真实相位带回前端。
 */
const TRANSITION_SNAPSHOT_POLL_MS = 1_000;
/**
 * C4（复审 P2-4）：GetSnapshot 含 engine `ObserveOwnedState` gRPC 往返与探测/SCM
 * 成本（composition/engine 锁均为短临界区、`prefer_snapshot` 防假 Idle——此为可接受
 * 的量化论证）；过渡窗口超过 10s 后轮询降频 1s → 3s，长窗口（bootstrap 慢路径）的
 * 轮询成本有界。
 */
const TRANSITION_SNAPSHOT_POLL_SLOW_MS = 3_000;
const TRANSITION_SNAPSHOT_POLL_SLOW_AFTER_MS = 10_000;
/**
 * C4：`pendingAction` 超时回落。只在事件链与 GetSnapshot 轮询双双不可达时触发（正常
 * 路径受理快照 ≤1s、轮询兜底 ≤2s 都先到）。
 *
 * P0-3（计划 v6.1 第二批，2026-09-28 根因 C）：固定 4s 改按路径区分——真死连接由
 * connect RPC 的拒绝路径立即出错误（`connect` 的 catch 解除窗口），乐观窗只在 RPC
 * 悬挂时持续；窗口长度覆盖对应链路的最坏在途时间：
 * - oneshot 30s：提权拉起链（实测 12.8s，UAC 等待含内）+ 预检门 ~1s + 组装；
 * - 服务 90s：最坏 = readiness ensure 15s + 修复批量 30s + 再 ensure 15s = 60s，
 *   加控制面初始化余量。
 * 路径判别用发起时快照的 service_status/mode（复审 F9：不新增 wire 字段；与
 * presenter 的 `connectingFirstStageLabel` 同一信号）。
 *
 * 修复 3b（2026-10-08 UI 重试脱同步）：两常量同时导出供 `ConnectionInteractionHost`
 * 的 `actionBusy` 看门狗复用（同源档位，避免第二份超时来源）。
 */
export const PENDING_ACTION_TIMEOUT_ONESHOT_MS = 30_000;
export const PENDING_ACTION_TIMEOUT_SERVICE_MS = 90_000;
/** 乐观期副文案切换阈值：超过旧 4s 窗口后从「正在发起…」切到「仍在…」。 */
const PENDING_ACTION_SLOW_COPY_AFTER_MS = 4_000;

/** 连接路径（乐观窗口超时与副文案按此区分；stop 不分路径）。 */
type PendingActionPath = "oneshot" | "service";

/** 连接发起时的路径判定：服务已装（或上次路由已是服务形态）→ 服务链。 */
function connectPathFor(snapshot: RuntimeSnapshot): PendingActionPath {
  if (snapshot.mode === "service") return "service";
  return snapshot.service_status?.installed === true ? "service" : "oneshot";
}

function idleSnapshot(): RuntimeSnapshot {
  return {
    runtime: { state: "idle", last_cleanup_at_ms: null },
    monotonic_tick: 0,
    stats: null,
    proxy_tun: null,
    operation_id: null,
    service_status: null,
    mode: "",
  };
}

function isErrorBearingState(runtime: RuntimeState): boolean {
  return (
    runtime.state === "reconciling" ||
    runtime.state === "failed_clean" ||
    runtime.state === "failed_dirty"
  );
}

/**
 * 失败终态的断开语义描述（阶段①「需要处理」分流）：reason lane / stage 可识别时
 * 给出行动指引，否则 null。供错误显示槽（displayError）与 presenter 描述共用同一
 * 文案源，避免状态卡与横幅两套说法。
 */
function failedDisconnectCopyOf(snapshot: RuntimeSnapshot): string | null {
  const runtime = snapshot.runtime;
  if (runtime.state !== "failed_clean" && runtime.state !== "failed_dirty") return null;
  return disconnectDescriptionOf(
    runtime.error ?? null,
    normalizeReason(snapshot.engine_reconnect?.reason),
    normalizeReason(snapshot.reconnect?.reason),
  );
}

function productStateFor(
  snapshot: RuntimeSnapshot,
  now: () => number,
  displayError: DisplayError,
  connectionInfo: ProductConnectionInfo,
  coreStatus: CoreStatus,
  failureEntryPrevState: RuntimeState["state"] | null = null,
): ProductUiState {
  const productState = present(snapshot, now(), connectionInfo, coreStatus, failureEntryPrevState);

  // 2026-09-05 host 自愈进展（计划 §4.5 规则 2 实现层，冻结）：selfHeal active 时
  // 自愈描述优先于 displayError 覆盖（**含 null**）——`reconciling` 属
  // isErrorBearingState，`initialDisplayError` 取 `errorOf(reconciling)`，无
  // `blocking_error` 时是 `null` 而非 `undefined`，覆盖分支必然触发；若不在此合并，
  // presenter 写入的 `respawning`/`failed` 自愈描述会被清成 null，ConnectPage 回落
  // 默认文案。`undefined`/`null` 的既有语义区分保持不变（undefined = 无运行时覆盖、
  // null = 用户新操作已清除旧错误）；active:false（含 succeeded 抑制）时 displayError
  // 链行为与现状完全一致。
  if (productState.selfHeal.active) {
    return {
      ...productState,
      coreStatus,
      description: productState.selfHeal.description,
    };
  }

  // `undefined` 代表没有运行时覆盖：例如已连接时仍可展示 snapshot.summary。
  // `null` 则是用户新操作明确清除了旧错误，避免旧 failed snapshot 再次带回横幅。
  // S3 断开→重连过渡态期间不套错误显示槽：过渡文案优先，错误文案保留给最终
  // 失败终态（与 darwin 同构）。
  if (displayError !== undefined && !reconnectTransitionOf(snapshot)
    && !snapshot.stop_completion?.errors.length) {
    return { ...productState, coreStatus, description: displayError };
  }

  return { ...productState, coreStatus };
}

function initialDisplayError(snapshot: RuntimeSnapshot): DisplayError {
  if (!isErrorBearingState(snapshot.runtime)) return undefined;
  // 阶段①：冷启动即落在失败态时同样按 lane 分流描述（与事件路径同一文案源）。
  return failedDisconnectCopyOf(snapshot) ?? errorOf(snapshot.runtime);
}

/** 活动相位：会话身份投影只在这些相位生效（连接页显示冻结身份）。 */
function isActiveConnectionPhase(state: RuntimeState["state"]): boolean {
  return state === "connecting" || state === "awaiting_interaction" || state === "connected";
}

/**
 * bug3（EXV_UNFREEZE 2026-10-04）：活动会话期间用**会话身份投影**覆盖连接页显示。
 *
 * 后端在连接受理时冻结 protocol/username/server，会话期间设置页保存不再改写连接页。
 * `identity == null`（旧快照/接管会话）时保留既有临时账号覆盖行为；`campusIp` 仍来自
 * 本机适配器实时探测（不受冻结影响）。
 *
 * 两协议账户均来自已接纳会话身份；旧快照缺身份时仅保留本次实际传入凭据，
 * 不以随后读取的设置账户补齐未知网页登录身份。
 */
function sessionConnectionInfo(
  base: ProductConnectionInfo,
  identity: SessionIdentity | null | undefined,
  sessionAccount: string | null,
): ProductConnectionInfo {
  if (identity == null) {
    return { ...base, account: sessionAccount, accountSource: sessionAccount ? "configured" : "unavailable" };
  }
  const protocol: "cisco" | "atrust" = identity.protocol === "atrust" ? "atrust" : "cisco";
  const frozenUsername = identity.username.trim() || null;
  const frozenServer = identity.server.trim() || null;
  return {
    account: frozenUsername,
    accountSource: frozenUsername ? "configured" : "unavailable",
    vpnServer: frozenServer,
    campusIp: base.campusIp,
    routeRuleCount: base.routeRuleCount,
    protocol,
  };
}

function nextDisplayError(event: RuntimeEvent, previous: DisplayError): DisplayError {
  const next = errorAfterEvent(event, previous);
  if (typeof next === "string") {
    // 阶段①（「需要处理」分流）：失败终态若能由 reason lane / stage 识别为断开
    // 语义，错误显示槽直接采用断开行动指引——泛化错误标签（如「操作结果未知」）
    // 会掩埋掉线事实；识别不出时维持既有错误文案（规则 3 语义不变）。
    return failedDisconnectCopyOf(event.snapshot) ?? next;
  }
  if (next === null) {
    return isErrorBearingState(event.snapshot.runtime) ? null : undefined;
  }
  return undefined;
}

function operationIdOf(reply: OperationReply): string | null {
  const operationId = reply.operation_id?.trim();
  return operationId ? operationId : null;
}

function throwIfOperationFailed(reply: OperationReply): void {
  if (reply.result.result === "failed") {
    throw reply.result.error ?? { message: "原操作返回失败，但未提供错误详情。" };
  }
}

type PendingAction = "connect" | "stop" | null;

/**
 * 用户动作一开始就给产品层一个明确的在途状态。
 *
 * 这不是伪造 core 终态：真正的状态仍由带 operation_id 的 status event 接管；这里只
 * 覆盖 IPC 请求尚未返回这一小段时间，让完整模式和极简模式立即进入对应动画并锁住
 * 动作按钮，而不是停留在旧的 idle/connected 画面上。
 */
/**
 * 乐观窗口的 8 级连接阶段：点击连接即显示完整阶段列表（首步「检查环境」进行中、
 * 其余等待），不等第一个真实事件——阶段序号与引擎真实推进一致，只是先呈现占位，
 * 真实 Connecting 事件到达后由快照覆盖。
 */
function optimisticStages(snapshot: RuntimeSnapshot): ProductStage[] {
  return CONNECT_PHASE_ORDER.map((phase, index) => ({
    phase,
    // P0-3a：乐观窗口 = 引擎未挂接窗口——首阶段标签按 service_status/mode 判别
    //（「正在启动引擎/正在准备服务」，与 presenter 的 stagesFor 同一信号/同一文案）。
    label:
      index === 0
        ? connectingFirstStageLabel(snapshot)
        : (PRODUCT_PHASE_LABELS[phase] ?? phase),
    visual: index === 0 ? "current" : "waiting",
  }));
}

function optimisticProductState(
  base: ProductUiState,
  pendingAction: PendingAction,
  path: PendingActionPath,
  elapsedMs: number,
  snapshot: RuntimeSnapshot,
): ProductUiState {
  if (pendingAction === "connect") {
    // P0-3：乐观期副文案——旧 4s 窗口内「正在发起…」，超过后按路径切「仍在连接/
    // 准备服务…」（服务链含 UAC 修复批量，需预告长等待）。
    const description =
      elapsedMs < PENDING_ACTION_SLOW_COPY_AFTER_MS
        ? "正在发起连接请求。"
        : path === "service"
          ? "仍在准备服务，可能需要管理员授权，请稍候…"
          : "仍在连接，请稍候…";
    return {
      ...base,
      status: "connecting",
      severity: "attention",
      title: "连接中",
      description,
      stages: optimisticStages(snapshot),
      errorCode: null,
      metrics: null,
      connectEnabled: false,
      stopEnabled: false,
    };
  }
  if (pendingAction === "stop") {
    return {
      ...base,
      status: "stopping",
      severity: "attention",
      title: "断开中",
      description: "正在安全撤销连接配置。",
      errorCode: null,
      metrics: null,
      connectEnabled: false,
      stopEnabled: false,
    };
  }
  return base;
}

/**
 * 从真实或测试 gateway 构造统一产品运行时。
 *
 * 状态事件的固定顺序：先过滤，再写入快照/产品状态，再计算错误，最后写 terminal 标记。
 *
 * @param gateway 网关（缺省 Tauri 真实网关；测试注入内存实现）。
 * @param now 时钟（测试注入）。
 * @param onAdoptedSnapshot T12：快照采纳钩子——每次成功采纳一份完整快照（事件与
 *   `GetSnapshot` 轮询同一点）后回调；重连通知出口（`reconnect-notify`）经此喂入
 *   R1/R2 reason lane。缺省无操作。
 */
export function createProductRuntime(
  gateway: ProductGateway = tauriProductGateway,
  now: () => number = Date.now,
  onAdoptedSnapshot: (snapshot: RuntimeSnapshot) => void = () => undefined,
): ProductRuntime {
  let latestSnapshot = idleSnapshot();
  let latestStatusSnapshot: { snapshot: RuntimeSnapshot; sequence: number } | null = null;
  let displayError: DisplayError = undefined;
  /**
   * 阶段①（「需要处理」分流）：当前失败 episode 进入前一刻的 runtime 相位
   * （断开语义三判据之一；null = 未知/冷启动直接落在失败态）。同 episode 的
   * failed 态重放不重置（prev 不再是 connected，必须沿用进入时记录的值）；
   * 离开失败态即清空。
   */
  let failureEntryPrevState: RuntimeState["state"] | null = null;
  /** 是否已采纳过至少一份真实快照（区分冷启动首拍即失败与真实相位转移进入失败）。 */
  let sawAdoptedSnapshot = false;
  let correlation: OpCorrelation = { activeOpId: null, terminalReached: false, inFlightAction: null };
  let latestConnectionInfo: ProductConnectionInfo = EMPTY_PRODUCT_CONNECTION_INFO;
  let sessionAccount: string | null = null;
  let coreStatus: CoreStatus = "stopped";
  let connectionStateConfirmed = true;
  let unlisten: StopListening | null = null;
  let unlistenBrowser: StopListening | null = null;
  const browserWaiting = ref(false);
  const browserLoginRecovery = ref<BrowserLoginEvent["recovery"] | null>(null);
  const browserLoginCancelled = ref<{ generation: number } | null>(null);
  // 用户 Stop 结束当前会话；新用户 Connect 才重新授权后续后台恢复呈现。
  let allowRecoveryPresentation = true;
  const browserLoginGeneration = ref<number | null>(null);
  const browserAuthenticated = ref<{ attempt: number; generation: number } | null>(null);
  /** §7.9：宿主回传的自动登录阶段；等待结束（active=false）即清空。 */
  const ticketRefreshReason = ref<"missing" | "expired" | null>(null);
  const browserLoginStage = ref<string | null>(null);
  /** 验证码风险预告（2026-10-07）：本次认证开始事件携带的窗口内登录频次预告；
   * 认证结束（active=false）/新连接发起（retire）即清空。 */
  const loginThrottleWarning = ref<LoginThrottleWarning | null>(null);
  let browserOwnerGeneration: number | null = null;
  let retiredBrowserGeneration = -1;
  /**
   * S3 终局穿透令牌：取消登录等待（Stop 收编浏览器等待）后，已退役代的终局事件
   * 允许穿透过滤进入收口（browserLoginCancelled 标记 + 等待状态归位）；新 Connect
   * 或任一新活动事件清空令牌，旧代终局不得反杀新会话。与 darwin
   * `browser-login-wait` 终局豁免 retire 判定同构。
   */
  let cancelledWaitTerminalPierce: number | null = null;
  function retireBrowserOwner(): void {
    browserWaiting.value = false;
    browserLoginRecovery.value = null;
    browserLoginGeneration.value = null;
    browserLoginStage.value = null;
    ticketRefreshReason.value = null;
    loginThrottleWarning.value = null;
    if (browserOwnerGeneration !== null) retiredBrowserGeneration = Math.max(retiredBrowserGeneration, browserOwnerGeneration);
    browserOwnerGeneration = null;
  }
  let startPromise: Promise<void> | null = null;
  let liveSnapshotTimer: ReturnType<typeof setInterval> | null = null;
  let liveSnapshotInFlight = false;
  let coreStatusTimer: ReturnType<typeof setInterval> | null = null;
  let coreStatusInFlight = false;
  type ConnectionInfoRefreshResult = { ok: true } | { ok: false; error: unknown };
  let connectionInfoInFlight: Promise<ConnectionInfoRefreshResult> | null = null;
  let pendingAction: PendingAction = null;
  /** P0-3：乐观窗口的路径与起点（超时上界/副文案按路径区分；stop 无路径语义，
   *  复用 oneshot 档不产生可见差异——stop 副文案不随 elapsed 切换）。 */
  let pendingActionPath: PendingActionPath = "oneshot";
  let pendingActionSinceMs = 0;
  /** 修复 3a（2026-10-08 UI 重试脱同步）：Tauri 命令在飞时钟——gateway.connect/stop
   *  的 Promise settle 前保持。Rust 侧浏览器认证先于 Core Connect RPC 被完整 await，
   *  期间 core 快照恒为 idle；旧实现把 idle 当「方向一致终态」在 ~1s 内提前解除乐观
   *  窗口，形成「未连接+灰按钮」非法组合（根因 B）。命令在飞是 Rust 侧真实操作的
   *  存在性证明，作为呈现层在飞时钟优先于 pendingAction：C4 看门狗到期只回收
   *  pendingAction（事件关联窗口），不回收本时钟——命令最坏 300s+（无头 10s + 内嵌
   *  300s 预算）期间状态必须持续呈现连接中/等待浏览器登录。 */
  let commandInFlight: PendingAction = null;
  let commandInFlightSinceMs = 0;
  let userOperationSequence = 0;
  /** C3（2026-09-05）：最近一次被采纳状态事件的时间。事件链在流动时轮询让位——
   *  仅当事件静默（饥饿）超过一个周期才拉一次 GetSnapshot 兜底。 */
  let lastEventAppliedAt: number | null = null;
  // C3/C4（ui-connect-stop-responsiveness）：connecting/stopping 兜底真相源轮询 +
  // pendingAction 超时看门狗。
  let transitionSnapshotTimer: ReturnType<typeof setInterval> | null = null;
  let transitionSnapshotInFlight = false;
  let transitionPollStartedAt = 0;
  let transitionPollLastTickAt = 0;
  let pendingActionWatchdog: ReturnType<typeof setTimeout> | null = null;
  let disposed = false;

  const state = ref<ProductUiState>(
    productStateFor(latestSnapshot, now, displayError, latestConnectionInfo, coreStatus, failureEntryPrevState),
  );

  function writeState(): void {
    // bug3：活动相位用会话身份投影（连接受理时冻结），否则读当前配置。设置页保存
    // 不改写活动会话的连接页身份。
    const active = isActiveConnectionPhase(latestSnapshot.runtime.state);
    const connectionInfo = active
      ? sessionConnectionInfo(latestConnectionInfo, latestSnapshot.session_identity, sessionAccount)
      : latestConnectionInfo;
    const visibleSnapshot: RuntimeSnapshot = !connectionStateConfirmed && latestSnapshot.runtime.state === "connected"
      ? { ...latestSnapshot, runtime: { state: "reconciling" }, stats: null }
      : latestSnapshot;
    const base = productStateFor(visibleSnapshot, now, displayError, connectionInfo, coreStatus, failureEntryPrevState);
    if (!connectionStateConfirmed && latestSnapshot.runtime.state === "connected") {
      base.title = "连接状态待确认";
      base.description = "与后台的状态通信已中断，正在重新确认连接。";
    }
    state.value = optimisticProductState(
      base,
      pendingAction,
      pendingActionPath,
      pendingActionSinceMs === 0 ? 0 : Math.max(0, now() - pendingActionSinceMs),
      latestSnapshot,
    );
    // 修复 3a（2026-10-08 UI 重试脱同步）：连接命令在飞期间产品状态不得呈现
    // 「未连接」。兜底轮询把 idle 快照当「方向一致终态」提前解除乐观窗口后，由此
    // 以命令时钟重建「连接中」呈现，且取消（Stop）可点；认证开始事件（active:true，
    // 修复 1 后重试携带新代）到达后由下方等待覆盖接管为「等待浏览器登录」。
    // 「未连接 + 灰按钮」自此为非法组合（状态侧约束；按钮任意相位取消见
    // ConnectionInteractionHost 修复 3b）。
    if (commandInFlight === "connect" && pendingAction !== "stop" && state.value.status === "idle") {
      const held = optimisticProductState(
        state.value,
        "connect",
        pendingActionPath,
        commandInFlightSinceMs === 0 ? 0 : Math.max(0, now() - commandInFlightSinceMs),
        latestSnapshot,
      );
      state.value = { ...held, stopEnabled: true };
    }
    // 修复 2（2026-10-08 UI 重试脱同步）：等待覆盖的相位集与 browser-login 过滤器
    // 的相位门同步加入 failed_clean/failed_dirty——引擎失败后的重试里，浏览器授权
    // 同样先于 Core 受理 Connect，旧终态不是本次连接的结论（与 handleStatus 的
    // idle 丢弃同一推理）；两处集合不一致会让「事件被接受但状态不覆盖」，等待模态
    // 仍被吞。
    if (browserWaiting.value && pendingAction !== "stop" &&
      (browserLoginRecovery.value !== null ||
        ["idle", "connecting", "awaiting_interaction", "failed_clean", "failed_dirty"].includes(latestSnapshot.runtime.state))) {
      state.value = {
        ...state.value, status: "connecting", severity: "attention",
        title: "等待浏览器登录", description: "请在打开的浏览器中完成学校统一认证。",
        connectEnabled: false, stopEnabled: true, metrics: null, errorCode: null,
      };
    }
  }

  /**
   * 接纳一份完整快照。后端的周期统计/探测快照偶尔只携带新的伴生数据，可能暂时
   * 缺失本会话已确认的起点；这种不完整更新不能把在线时长清成横线。只有同为
   * Connected 且新值缺失时才保留旧值，断开/新会话/新正值仍按新快照直接替换。
   */
  function adoptSnapshot(snapshot: RuntimeSnapshot, queryBasis?: RuntimeSnapshot): boolean {
    const adopted = adoptSnapshotInner(snapshot, queryBasis);
    if (adopted) onAdoptedSnapshot(latestSnapshot);
    return adopted;
  }

  function adoptSnapshotInner(snapshot: RuntimeSnapshot, queryBasis?: RuntimeSnapshot): boolean {
    if (pendingAction === "connect" && snapshot.stop_completion) return false;
    if (queryBasis && queryBasis !== latestSnapshot &&
      (queryBasis.operation_id !== latestSnapshot.operation_id || queryBasis.runtime.state !== latestSnapshot.runtime.state)) return false;
    // `GetSnapshot` 是异步兜底，不得把已经由事件确认的较新状态倒灌回页面。服务安装
    // 会拉长一次轮询的返回时间，正是旧 Connecting 快照在 Connected 之后回写的高发点。
    const currentTick = latestSnapshot.monotonic_tick;
    const nextTick = snapshot.monotonic_tick;
    if (nextTick > 0 && currentTick > 0 && nextTick < currentTick) return false;

    const freshReconnectQuery = queryBasis === latestSnapshot && pendingAction === null &&
      snapshot.reconnect?.auto_reconnect === true && Boolean(snapshot.operation_id) &&
      snapshot.operation_id !== correlation.activeOpId &&
      ["failed_clean", "failed_dirty", "reconciling", "connected"].includes(latestSnapshot.runtime.state) &&
      ["connecting", "awaiting_interaction", "connected"].includes(snapshot.runtime.state);
    if (isNewAutomaticReconnect(snapshot) || freshReconnectQuery) {
      correlation = { activeOpId: snapshot.operation_id!, terminalReached: false, inFlightAction: null };
      displayError = null;
    }

    const currentState = latestSnapshot.runtime.state;
    const nextState = snapshot.runtime.state;
    // 部分 engine 拉取快照不带 EventBus tick；对这类快照仍需阻止同一操作内显然倒退的
    // 相位覆盖。新用户动作会先 resetForUserOperation，因此不会挡住新会话。
    if (
      (currentState === "connected" && snapshot.operation_id === latestSnapshot.operation_id &&
        (nextState === "connecting" || nextState === "awaiting_interaction")) ||
      (currentState === "stopping" && nextState === "connected") ||
      // Connected 结束连接操作，但会话仍活动；同态统计和重连投影必须继续更新。
      (correlation.terminalReached && !(currentState === "connected" && nextState === "connected") &&
        (nextState === "connecting" || nextState === "awaiting_interaction" || nextState === "stopping" || nextState === "connected"))
    ) {
      return false;
    }

    connectionStateConfirmed = true;
    if (nextState === "reconciling") correlation.terminalReached = false;

    const previousRuntime = latestSnapshot.runtime;
    const nextRuntime = snapshot.runtime;
    const previousSessionStart = previousRuntime.state === "connected"
      ? previousRuntime.session_established_at_ms
      : undefined;
    const nextSessionStart = nextRuntime.state === "connected"
      ? nextRuntime.session_established_at_ms
      : undefined;
    if (
      previousRuntime.state === "connected" &&
      nextRuntime.state === "connected" &&
      typeof previousSessionStart === "number" &&
      previousSessionStart > 0 &&
      !(typeof nextSessionStart === "number" && nextSessionStart > 0)
    ) {
      latestSnapshot = {
        ...snapshot,
        runtime: {
          ...nextRuntime,
          session_established_at_ms: previousSessionStart,
        },
      };
      sawAdoptedSnapshot = true;
      return true;
    }
    // 阶段①：失败 episode 进入记录——非失败相位首次进入 failed_* 时冻结进入前相位；
    // 同 episode 的 failed 态重放不重置；离开失败态清空。冷启动首拍即失败时相位
    // 未知（合成 idle 不算真实前态），记 null 交由 reason lane 兜底判定。
    if (nextState === "failed_clean" || nextState === "failed_dirty") {
      if (currentState !== "failed_clean" && currentState !== "failed_dirty") {
        failureEntryPrevState = sawAdoptedSnapshot ? currentState : null;
      }
    } else {
      failureEntryPrevState = null;
    }
    sawAdoptedSnapshot = true;
    latestSnapshot = snapshot;
    return true;
  }

  // 新操作必须由较新的、明确标记自动重连的后端快照建立，不能放行任意外来事件。
  function isNewAutomaticReconnect(snapshot: RuntimeSnapshot): boolean {
    return pendingAction === null && Boolean(snapshot.operation_id) &&
      snapshot.operation_id !== correlation.activeOpId &&
      snapshot.monotonic_tick > latestSnapshot.monotonic_tick &&
      snapshot.reconnect?.auto_reconnect === true && snapshot.reconnect.active === true &&
      snapshot.reconnect.current_attempt > 0 &&
      (snapshot.runtime.state === "connecting" || snapshot.runtime.state === "awaiting_interaction");
  }

  function needsTransitionPolling(snapshot: RuntimeSnapshot): boolean {
    return ["connecting", "stopping", "reconciling"].includes(snapshot.runtime.state) ||
      (["failed_clean", "failed_dirty"].includes(snapshot.runtime.state) && snapshot.reconnect?.auto_reconnect === true);
  }

  function refreshConnectionInfo(): Promise<ConnectionInfoRefreshResult> {
    if (!gateway.connectionInfo) return Promise.resolve({ ok: true });
    if (connectionInfoInFlight !== null) return connectionInfoInFlight;

    connectionInfoInFlight = (async (): Promise<ConnectionInfoRefreshResult> => {
      try {
        latestConnectionInfo = await gateway.connectionInfo!();
        writeState();
        return { ok: true };
      } catch (error) {
        // 独立显示读取不改变连接；严格调用者仍接收原错误及其代码。
        return { ok: false, error };
      } finally {
        connectionInfoInFlight = null;
      }
    })();

    return connectionInfoInFlight;
  }

  function refreshConnectionInfoAfterConnected(): void {
    if (!gateway.connectionInfo) return;
    // 连接请求返回后可能仍有一次连接前的配置/适配器探测在途；connected 事件到达时
    // 不能被这次旧探测吞掉，因此在它完成且地址仍为空时补一次 best-effort 探测。
    void refreshConnectionInfo().then(() => {
      if (!disposed && latestSnapshot.runtime.state === "connected" && latestConnectionInfo.campusIp === null) {
        void refreshConnectionInfo();
      }
    });
  }

  // 设置保存只刷新配置身份；重连策略始终由 Host 会话快照经 presenter 投影。
  async function configurationChanged(options?: { strict?: boolean }): Promise<void> {
    // 保存前启动的探测可能还在途；等它结束后再读，不能复用陈旧结果。
    if (connectionInfoInFlight !== null) await connectionInfoInFlight;
    // bug3（EXV_UNFREEZE 2026-10-04）：活动相位下保存设置只为「下一次连接」准备——
    // 不得清空会话身份或临时账号，连接页显示保持冻结（由 writeState 的会话投影决定）。
    // 非活动相位仍按既有语义清空并回落当前配置。
    if (!isActiveConnectionPhase(latestSnapshot.runtime.state)) {
      // 配置身份已变更，旧会话临时账号不能在新来源未确认时回填。
      sessionAccount = null;
      latestConnectionInfo = { ...latestConnectionInfo, account: null, accountSource: undefined };
    }
    writeState();
    if (disposed) return;
    const refreshed = await refreshConnectionInfo();
    if (options?.strict && !refreshed.ok) throw refreshed.error;
  }

  function stopLiveSnapshotRefresh(): void {
    if (liveSnapshotTimer === null) return;
    clearInterval(liveSnapshotTimer);
    liveSnapshotTimer = null;
  }

  function stopCoreStatusRefresh(): void {
    if (coreStatusTimer === null) return;
    clearInterval(coreStatusTimer);
    coreStatusTimer = null;
  }

  /** 常规存续观察只查控制管道；它不会扫描进程，也不会拉起 Core。 */
  async function refreshCoreStatus(): Promise<void> {
    if (disposed || coreStatusInFlight || !gateway.coreStatus) return;
    coreStatusInFlight = true;
    try {
      coreStatus = await gateway.coreStatus();
    } catch {
      // 命令层出错等价于控制管道不可用；恢复只在用户随后点击连接时进行。
      coreStatus = "stopped";
    } finally {
      coreStatusInFlight = false;
      if (!disposed) writeState();
    }
  }

  function startCoreStatusRefresh(): void {
    if (coreStatusTimer !== null || disposed || !gateway.coreStatus) return;
    coreStatusTimer = setInterval(() => {
      void refreshCoreStatus();
    }, 3_000);
  }

  async function refreshLiveSnapshot(): Promise<void> {
    if (
      disposed ||
      liveSnapshotInFlight ||
      latestSnapshot.runtime.state !== "connected"
    ) {
      return;
    }

    liveSnapshotInFlight = true;
    const queryBasis = latestSnapshot;
    try {
      const snapshot = await gateway.snapshot();
      coreStatus = "normal";
      if (disposed) return;

      // 用户已经断开/进入过渡态时，丢弃晚到的旧快照，避免旧连接数据回写页面。
      if (latestSnapshot.runtime.state !== "connected") return;

      if (!adoptSnapshot(snapshot, queryBasis)) return;
      if (snapshot.runtime.state !== "connected") {
        stopLiveSnapshotRefresh();
        if (needsTransitionPolling(snapshot)) startTransitionSnapshotRefresh();
      } else if (latestConnectionInfo.campusIp === null) {
        // 隧道地址可能比 connected 事件晚一个调度周期；在实时刷新期间继续
        // best-effort 探测，直到前端真正拿到校内地址。
        void refreshConnectionInfo();
      }
      writeState();
    } catch {
      // 通信失败无法判断隧道是否已断；暂停已连接展示与计时，继续查询确认真实状态。
      connectionStateConfirmed = false;
      coreStatus = "stopped";
      writeState();
    } finally {
      liveSnapshotInFlight = false;
    }
  }

  function startLiveSnapshotRefresh(): void {
    if (liveSnapshotTimer !== null || disposed) return;
    liveSnapshotTimer = setInterval(() => {
      void refreshLiveSnapshot();
    }, 1000);
  }

  /**
   * C2 窗口方向裁决（复审 P1-2）：connect → connecting 家族（connecting /
   * awaiting_interaction；后者是 GetSnapshot 经 engine 源可见的认证阶段）或终态；
   * stop → stopping 或终态。方向不一致的快照照常采纳显示，但**不停表、不解除**
   * 乐观窗口——防陈旧 pre-click 快照提前关窗。
   */
  function matchesInFlightDirection(runtime: RuntimeState, action: PendingAction): boolean {
    if (action === "connect") {
      return (
        runtime.state === "connecting" ||
        runtime.state === "awaiting_interaction" ||
        isTerminalState(runtime)
      );
    }
    return runtime.state === "stopping" || isTerminalState(runtime);
  }

  /** C4：解除乐观窗口（pendingAction/inFlightAction/看门狗三者同步清空）。 */
  function releasePendingAction(): void {
    disarmPendingActionWatchdog();
    pendingAction = null;
    correlation = { ...correlation, inFlightAction: null };
  }

  /** C4：解除 pendingAction 超时看门狗。 */
  function disarmPendingActionWatchdog(): void {
    if (pendingActionWatchdog === null) return;
    clearTimeout(pendingActionWatchdog);
    pendingActionWatchdog = null;
  }

  /** C4：武装 pendingAction 超时看门狗（事件链与轮询双失联时的最后可操作回落）。
   *  P0-3：超时上界按路径（oneshot 30s / 服务 90s，见常量注记）。 */
  function armPendingActionWatchdog(path: PendingActionPath): void {
    disarmPendingActionWatchdog();
    const timeoutMs =
      path === "service"
        ? PENDING_ACTION_TIMEOUT_SERVICE_MS
        : PENDING_ACTION_TIMEOUT_ONESHOT_MS;
    pendingActionWatchdog = setTimeout(() => {
      pendingActionWatchdog = null;
      if (pendingAction === null) return;
      // 超时行为（C4 冻结）：回到最近快照驱动的可操作态——不制造错误状态、不弹
      // toast、不伪造终态；迟到的 RPC 回复仍按既有路径登记 activeOpId，无状态污染。
      pendingAction = null;
      correlation = { ...correlation, inFlightAction: null };
      writeState();
    }, timeoutMs);
  }

  function stopTransitionSnapshotRefresh(): void {
    if (transitionSnapshotTimer === null) return;
    clearInterval(transitionSnapshotTimer);
    transitionSnapshotTimer = null;
  }

  /** C3：connecting/stopping 兜底真相源轮询（1s；>10s 窗口降频 3s）。幂等启动。 */
  function startTransitionSnapshotRefresh(): void {
    if (transitionSnapshotTimer !== null || disposed) return;
    transitionPollStartedAt = Date.now();
    transitionPollLastTickAt = 0;
    transitionSnapshotTimer = setInterval(() => {
      void refreshTransitionSnapshot();
    }, TRANSITION_SNAPSHOT_POLL_MS);
  }

  /** C3 轮询体：GetSnapshot → 采纳 → 方向一致才解除乐观窗口；错误不清窗口。 */
  async function refreshTransitionSnapshot(): Promise<void> {
    if (disposed || transitionSnapshotInFlight) return;
    const nowTick = Date.now();
    // C3 事件饥饿门（2026-09-05）：事件链在流动（1s 内有被采纳事件）→ 不拉
    // GetSnapshot，让事件驱动 UI；只有事件静默超过一个周期才兜底拉一次。
    if (lastEventAppliedAt !== null && nowTick - lastEventAppliedAt < TRANSITION_SNAPSHOT_POLL_MS) {
      return;
    }
    if (
      transitionPollLastTickAt !== 0 &&
      nowTick - transitionPollStartedAt > TRANSITION_SNAPSHOT_POLL_SLOW_AFTER_MS &&
      nowTick - transitionPollLastTickAt < TRANSITION_SNAPSHOT_POLL_SLOW_MS
    ) {
      return;
    }
    transitionPollLastTickAt = nowTick;
    transitionSnapshotInFlight = true;
    const queryBasis = latestSnapshot;
    try {
      const snapshot = await gateway.snapshot();
      if (disposed) return;
      coreStatus = "normal";
      if (!adoptSnapshot(snapshot, queryBasis)) return;
      // 轮询成功采纳即真相到达——但只有相位与窗口方向一致（或已无窗口）才解除
      // pendingAction/看门狗/窗口（复审 P1-2）；不一致照常采纳显示，不停表不解除。
      if (pendingAction !== null && matchesInFlightDirection(snapshot.runtime, pendingAction)) {
        releasePendingAction();
      }
      const runtimeState = snapshot.runtime.state;
      // 停止条件（C3）：采纳的快照离开 connecting/stopping 状态集（含终态）。
      if (!needsTransitionPolling(snapshot)) {
        stopTransitionSnapshotRefresh();
      }
      if (runtimeState === "connected") startLiveSnapshotRefresh();
      writeState();
    } catch {
      // C3 错误语义（冻结）：管道失败不是连接结论——coreStatus 置 stopped 镜像
      // refreshCoreStatus，但不清 pendingAction、不伪造终态；可操作回落只由
      // C4 看门狗负责。
      coreStatus = "stopped";
      writeState();
    } finally {
      transitionSnapshotInFlight = false;
    }
  }

  function resetForUserOperation(action: Exclude<PendingAction, null>): void {
    userOperationSequence += 1;
    stopLiveSnapshotRefresh();
    stopTransitionSnapshotRefresh();
    // 新操作发起：重置事件静默基准（此前事件属旧操作），乐观窗口首拍即可兜底。
    lastEventAppliedAt = null;
    displayError = null;
    // 重试 Stop 尚未取得新结果，原真实清理义务仍有效；只有新 Connect 清理旧摘要。
    if (action === "connect") latestSnapshot = { ...latestSnapshot, stop_completion: null, session_identity: null };
    correlation = { activeOpId: null, terminalReached: false, inFlightAction: action };
    pendingAction = action;
    // P0-3：记录路径与起点——超时上界/副文案按路径区分（连接路径用发起时快照判定）。
    pendingActionPath = action === "connect" ? connectPathFor(latestSnapshot) : "oneshot";
    pendingActionSinceMs = now();
    armPendingActionWatchdog(pendingActionPath);
    // C3：乐观置位时立即启动兜底真相源轮询（事件先到时由 handleStatus/停表条件收口）。
    startTransitionSnapshotRefresh();
    writeState();
  }

  function handleStatus(event: RuntimeEvent): void {
    if (disposed) return;
    const snapshot = event.snapshot.operation_id == null && event.operation_id
      ? { ...event.snapshot, operation_id: event.operation_id }
      : event.snapshot;
    // 浏览器授权在 Core 受理 Connect 之前完成；此时旧 Idle 不是本次连接终态。
    // Stop 仍消费真实清理事件，不能让取消流程被浏览器等待遮住。
    if (browserWaiting.value && pendingAction !== "stop" && snapshot.runtime.state === "idle") return;
    const currentSessionUncertain = Boolean(correlation.activeOpId) && latestSnapshot.runtime.state === "connected" &&
      snapshot.runtime.state === "reconciling" &&
      (snapshot.operation_id ?? event.operation_id) === correlation.activeOpId;
    if (!isNewAutomaticReconnect(snapshot) && !currentSessionUncertain &&
      !shouldApplyEvent(event, correlation)) return;

    // C2/P1-2：窗口内放行的事件先采纳；只有相位与窗口方向一致（或已无窗口）才解除
    // pendingAction/看门狗/窗口；不一致（陈旧 pre-click 快照/外来相位）照常采纳显示，
    // 不停表、不解除。
    const completesPendingDirection = pendingAction !== null && matchesInFlightDirection(snapshot.runtime, pendingAction);
    const wasConnected = latestSnapshot.runtime.state === "connected";
    if (!adoptSnapshot(snapshot)) return;
    latestStatusSnapshot = { snapshot: latestSnapshot, sequence: userOperationSequence };
    if (completesPendingDirection) releasePendingAction();
    coreStatus = "normal";
    // C3 事件优先：任何被采纳的事件都是主真相，重置静默计时（轮询让位）。
    lastEventAppliedAt = Date.now();
    if (snapshot.runtime.state === "connected") {
      startLiveSnapshotRefresh();
      stopTransitionSnapshotRefresh();
      if (!wasConnected) {
        refreshConnectionInfoAfterConnected();
        // engine 的周期探测是低频的；每次新会话额外请求一次真实探测，避免用户进入
        // 已连接界面后还要等待其周期。失败仍由后端周期探测兜底，不能产生未处理拒绝。
        void triggerLatencyRefresh().catch(() => undefined);
      }
    } else {
      stopLiveSnapshotRefresh();
      // C3 启停：采纳 connecting/stopping 即确保轮询在跑（幂等）；离开状态集（含
      // 终态/reconciling）停表。
      if (needsTransitionPolling(snapshot)) {
        startTransitionSnapshotRefresh();
      } else {
        stopTransitionSnapshotRefresh();
      }
    }
    writeState();

    displayError = nextDisplayError(event, displayError);
    writeState();

    if (isTerminalState(snapshot.runtime)) {
      correlation.terminalReached = true;
    }
  }

  async function start(): Promise<void> {
    if (startPromise !== null) return startPromise;

    startPromise = (async () => {
      const initialBasis = latestSnapshot;
      const initialSequence = userOperationSequence;
      const stopListening = await gateway.onStatus(handleStatus);
      if (disposed) {
        stopListening();
        return;
      }
      unlisten = stopListening;
      try {
        const snapshot = await gateway.snapshot();
        if (!disposed && initialBasis === latestSnapshot && initialSequence === userOperationSequence) {
          coreStatus = "normal";
          adoptSnapshot(snapshot);
          displayError = initialDisplayError(snapshot);
          // 初始快照即视为已发生的一次采纳：其后的真实事件按事件优先推进。
          lastEventAppliedAt = Date.now();
          // C3 启停：UI 打开时 core 已处于过渡相位 → 兜底真相源轮询立即在跑。
          if (needsTransitionPolling(snapshot)) {
            startTransitionSnapshotRefresh();
          }
          writeState();
        }
      } catch {
        if (!disposed && initialBasis === latestSnapshot && initialSequence === userOperationSequence) {
          // UI 本身照常打开，明确显示 Core 已停止；用户点击连接才进入受控恢复路径。
          coreStatus = "stopped";
          writeState();
        }
      }
      await refreshConnectionInfo();

      if (disposed) return;
      if (latestSnapshot.runtime.state === "connected") startLiveSnapshotRefresh();
      if (gateway.onBrowserLogin) {
        const stopBrowser = await gateway.onBrowserLogin((event) => {
          if (disposed) return;
          const active = typeof event === "boolean" ? event : event.active;
          const generation = typeof event === "boolean" ? null : event.generation;
          // §7.9：可选 stage 只在对象载荷上；布尔旧载荷与无 stage 事件均视为无阶段。
          const stage = typeof event === "boolean" ? null : event.stage ?? null;
          // 验证码风险预告：随认证开始事件（active=true）一次性下发；其余事件视为无预告。
          const throttleWarning = typeof event === "boolean" ? null : event.throttle_warning ?? null;
          if (active) {
            const recovery = typeof event === "boolean" ? undefined : event.recovery;
            const boundRecovery = browserLoginRecovery.value !== null && generation === browserOwnerGeneration;
            const channelOverride = typeof event !== "boolean" && event.channel_override === true
              && recovery !== undefined && generation !== null && generation === browserOwnerGeneration
              && browserWaiting.value;
            const startsRecovery = recovery !== undefined && generation !== null
              && allowRecoveryPresentation && pendingAction === null && commandInFlight === null;
            const ordinaryConnect = recovery === undefined && browserLoginRecovery.value === null
              && pendingAction === "connect"
              && ["idle", "connecting", "awaiting_interaction", "failed_clean", "failed_dirty"].includes(latestSnapshot.runtime.state);
            // 后台首事件由原 Rust owner 授权，可先于 Core 状态事件；后续阶段只沿已绑定代。
            if ((!boundRecovery && !startsRecovery && !ordinaryConnect && !channelOverride) ||
              (generation !== null && (generation <= retiredBrowserGeneration ||
                (browserOwnerGeneration !== null && generation < browserOwnerGeneration)))) return;
            if ((startsRecovery && !boundRecovery) || channelOverride) browserLoginRecovery.value = { ...recovery! };
            browserOwnerGeneration = generation;
            cancelledWaitTerminalPierce = null;
          } else if (generation !== null && browserOwnerGeneration !== generation) {
            // S3 终局事件穿透：等待已被本地取消收编（owner 清空）且持有穿透令牌时，
            // 恰为已退役代的终局事件仍放行收口；更旧终局与新连接在飞期间一律拒收。
            const pierce = browserOwnerGeneration === null && cancelledWaitTerminalPierce !== null
              && generation === cancelledWaitTerminalPierce
              && pendingAction !== "connect" && commandInFlight !== "connect";
            if (!pierce) return;
          }
          if (!active && typeof event !== "boolean" && event.outcome === "authenticated"
            && generation !== null && generation > retiredBrowserGeneration && pendingAction === "connect") {
            browserAuthenticated.value = { attempt: userOperationSequence, generation };
          }
          if (!active && typeof event !== "boolean" && event.outcome === "cancelled"
            && generation !== null
            && (generation > retiredBrowserGeneration || generation === cancelledWaitTerminalPierce)) {
            browserLoginCancelled.value = { generation };
          }
          if (!active || generation !== browserLoginGeneration.value || stage === "awaiting_manual"
            || (typeof event !== "boolean" && event.channel_override)) ticketRefreshReason.value = null;
          if (active && stage !== "awaiting_manual" && typeof event !== "boolean" && event.ticket_refresh_reason) ticketRefreshReason.value = event.ticket_refresh_reason;
          // 同一认证请求恢复中只保留一行提示，直到真实人工介入或本次等待结束。
          const keepRecovery = generation === browserLoginGeneration.value
            && browserLoginStage.value === "recovering" && stage !== "awaiting_manual";
          browserLoginGeneration.value = active ? generation : null;
          browserWaiting.value = active;
          browserLoginStage.value = active ? (keepRecovery ? "recovering" : stage) : null;
          // 验证码风险预告只在事件**携带**该字段时写入（宿主仅随认证开始事件下发一次，
          // 后续阶段事件不带此字段、不得覆盖已存预告）；清除统一走等待结束/新连接的
          // retireBrowserOwner。
          if (active && throttleWarning) loginThrottleWarning.value = throttleWarning;
          if (!active) {
            retireBrowserOwner();
            // 终局已落地，穿透机会随之消费；后续重复终局按普通旧事件拒收。
            cancelledWaitTerminalPierce = null;
          }
          writeState();
        });
        if (disposed) stopBrowser(); else unlistenBrowser = stopBrowser;
      }
      startCoreStatusRefresh();
      void refreshCoreStatus();
    })();

    try {
      await startPromise;
    } catch (error) {
      startPromise = null;
      throw error;
    }
  }

  async function connect(credentials?: ConnectCredentials): Promise<string | void> {
    allowRecoveryPresentation = true;
    browserLoginCancelled.value = null;
    browserAuthenticated.value = null;
    retireBrowserOwner();
    // S3：新连接清空终局穿透令牌，旧等待的终局不得反杀新会话。
    cancelledWaitTerminalPierce = null;
    sessionAccount = credentials?.username.trim() || null;
    resetForUserOperation("connect");
    // 修复 3a：命令在飞时钟起点（呈现层以此为准，见 writeState）。
    commandInFlight = "connect";
    commandInFlightSinceMs = now();
    const sequence = userOperationSequence;
    try {
      // 先派发连接请求，身份/校内地址刷新不能阻塞用户看到连接中的即时反馈。
      const reply = await gateway.connect({
        ...NO_PROFILE_CONNECT_INTENT,
        ...(credentials ? { credentials } : {}),
      });
      // Stop 或新 Connect 已开始时，迟到回复不再接管当前操作的事件关联，
      // 也不清命令时钟（新操作已覆盖）。
      if (disposed || sequence !== userOperationSequence) return;
      commandInFlight = null;
      coreStatus = "normal";
      // C2 窗口生命周期：RPC 回复登记 activeOpId 即关闭窗口（受理快照先到的常态下
      // 窗口已在 handleStatus 解除；此处覆盖回复先到、事件后到的时序，见 F4）。
      correlation.activeOpId = operationIdOf(reply);
      correlation = { ...correlation, inFlightAction: correlation.activeOpId ? null : pendingAction };
      throwIfOperationFailed(reply);
      writeState();
      void refreshConnectionInfo();
      return correlation.activeOpId ?? undefined;
    } catch (error) {
      if (sequence !== userOperationSequence) throw error;
      commandInFlight = null;
      retireBrowserOwner();
      // 被取消的旧 connect 不得清掉随后发起、尚未结束的 stop。
      if (pendingAction !== "stop") releasePendingAction();
      writeState();
      throw error;
    }
  }

  function applyStopCompletion(reply: OperationReply): void {
    let completion = reply.stop_completion;
    if (!completion) return;
    if (reply.result.result === "failed" && reply.result.error?.code === "STOP_DEADLINE_EXCEEDED" &&
        latestSnapshot.stop_completion) {
      const original = latestSnapshot.stop_completion.errors;
      completion = { ...completion,
        blocks_new_connect: latestSnapshot.stop_completion.blocks_new_connect || completion.blocks_new_connect,
        errors: [...original, ...completion.errors.filter(error => !original.some(previous =>
          previous.code === error.code && previous.message === error.message))] };
    }
    const failed = reply.result.result === "failed";
    const failure = reply.result.result === "failed" ? reply.result.error ?? completion.errors[0] : undefined;
    // 同 ID 的真实终态先到时，不让较早的回复摘要倒写；其实际错误独立呈现。
    if (latestStatusSnapshot?.sequence === userOperationSequence && operationIdOf(reply) &&
        operationIdOf(reply) === latestStatusSnapshot.snapshot.operation_id &&
        operationIdOf(reply) === latestSnapshot.operation_id && isTerminalState(latestStatusSnapshot.snapshot.runtime)) {
      if (completion.errors.length) {
        const details = completion.errors.map(error => [error.message, error.code].filter(Boolean).join(" ")).join("; ");
        console.error("较早停止回执报告错误", completion);
        pushToast("较早停止回执报告错误：" + details, "warning");
      }
      return;
    }
    // 摘要是本次停止尝试的原返回；Pending 不妨碍呈现它，也不等于隧道已停止。
    latestSnapshot = {
      ...latestSnapshot,
      operation_id: operationIdOf(reply),
      runtime: completion.stopped
        ? { state: "idle", last_cleanup_at_ms: null }
        : failed ? { state: "failed_dirty", error: failure, has_obligation: completion.blocks_new_connect }
        : { state: "stopping", reason: null },
      stats: completion.stopped ? null : latestSnapshot.stats,
      stop_completion: completion,
    };
    releasePendingAction();
    if (completion.stopped || failed) {
      stopLiveSnapshotRefresh();
      stopTransitionSnapshotRefresh();
      correlation.terminalReached = true;
    }
    writeState();
  }

  async function stop(): Promise<void> {
    allowRecoveryPresentation = false;
    browserLoginCancelled.value = null;
    const cancellingBrowserCandidate = browserWaiting.value;
    const previousCorrelation = { ...correlation };
    browserAuthenticated.value = null;
    const retiredBefore = retiredBrowserGeneration;
    retireBrowserOwner();
    // S3 终局穿透令牌：本次 Stop 正在取消浏览器登录等待时，该等待代次的终局
    // 事件允许穿透 retire 过滤进入收口（语义=中断本轮登录+解除重连武装呈现）。
    // 仅在 retire 确实推进到刚被收编的等待代次时登记，避免旧水位误开穿透。
    if (cancellingBrowserCandidate && retiredBrowserGeneration > retiredBefore) {
      cancelledWaitTerminalPierce = retiredBrowserGeneration;
    }
    resetForUserOperation("stop");
    // 修复 3a：stop 同样登记命令时钟（对称收口；断开呈现仍由 pendingAction 驱动，
    // 此时钟只为防止旧 connect 的迟到 settle 覆盖新操作的呈现）。
    commandInFlight = "stop";
    commandInFlightSinceMs = now();
    const sequence = userOperationSequence;
    try {
      const reply = await gateway.stop();
      if (disposed || sequence !== userOperationSequence) return;
      commandInFlight = null;
      if (cancellingBrowserCandidate && reply.result.result === "succeeded" &&
          !operationIdOf(reply) && !reply.stop_completion) {
        // 此回复只结束已存在的浏览器候选，不产生 Stop ID 或隧道终态。
        correlation = previousCorrelation;
        releasePendingAction();
        writeState();
        return;
      }
      correlation.activeOpId = operationIdOf(reply);
      correlation = { ...correlation, inFlightAction: correlation.activeOpId ? null : pendingAction };
      applyStopCompletion(reply);
      throwIfOperationFailed(reply);
    } catch (error) {
      if (sequence !== userOperationSequence) throw error;
      commandInFlight = null;
      releasePendingAction();
      writeState();
      throw error;
    }
  }

  /**
   * T1：请求一次延迟刷新并立即拉取最新快照（新延迟经 `stats.latency_ms` 到达）。
   * 不触碰错误/关联状态——这是周期探测，不是用户业务操作。
   */
  async function triggerLatencyRefresh(): Promise<void> {
    await gateway.triggerLatencyRefresh();
    const snapshot = await gateway.snapshot();
    // 状态事件已经确认连接后，立即拉取的 IPC 快照仍可能落后一个调度周期。不能让这份
    // 陈旧非连接快照覆盖新会话，否则界面会闪回 idle 并中止实时统计刷新。
    if (latestSnapshot.runtime.state === "connected" && snapshot.runtime.state === "idle" && !snapshot.operation_id) return;
    if (!adoptSnapshot(snapshot)) return;
    writeState();
  }

  /**
   * S3/D5：执行服务控制并直接消费 reply 中的 post-action 服务状态。
   * ServiceControlReply 已经携带同一事务结束时的服务状态；这里不再追加 GetSnapshot，
   * 避免 UI 在 SCM 操作已经完成后继续等待第二次 IPC。reply 没有状态时保留现有快照。
   * 不触碰错误/关联状态——服务控制是独立于连接状态机的生命周期操作。
   */
  async function serviceControl(action: ServiceControlAction): Promise<ServiceControlReply> {
    const reply = await gateway.serviceControl(action);
    if (reply.service_status !== undefined && reply.service_status !== null) {
      latestSnapshot = {
        ...latestSnapshot,
        service_status: reply.service_status,
      };
      writeState();
    }
    return reply;
  }

  async function atrustBrowserFallbackAvailable(): Promise<boolean> {
    return await gateway.atrustBrowserFallbackAvailable?.() ?? false;
  }

  async function atrustBrowserFallback(): Promise<void> {
    if (!gateway.atrustBrowserFallback) throw new Error("系统浏览器 fallback 当前不可用。");
    await gateway.atrustBrowserFallback();
  }

  async function atrustShowAuthWindow(): Promise<void> {
    if (!gateway.atrustShowAuthWindow) throw new Error("手动介入认证窗口当前不可用。");
    await gateway.atrustShowAuthWindow();
  }

  function dispose(): void {
    disposed = true;
    retireBrowserOwner();
    cancelledWaitTerminalPierce = null;
    stopLiveSnapshotRefresh();
    stopCoreStatusRefresh();
    stopTransitionSnapshotRefresh();
    disarmPendingActionWatchdog();
    unlisten?.();
    unlisten = null;
    unlistenBrowser?.();
    unlistenBrowser = null;
  }

  return {
    source: "real",
    browserLoginActive: readonly(browserWaiting),
    browserLoginRecovery: readonly(browserLoginRecovery),
    browserLoginCancelled: readonly(browserLoginCancelled),
    browserLoginGeneration: readonly(browserLoginGeneration),
    browserAuthenticated: readonly(browserAuthenticated),
    browserLoginStage: readonly(browserLoginStage),
    ticketRefreshReason: readonly(ticketRefreshReason),
    loginThrottleWarning: readonly(loginThrottleWarning),
    state,
    start,
    connect,
    stop,
    configurationChanged,
    triggerLatencyRefresh,
    serviceControl,
    atrustBrowserFallbackAvailable,
    atrustBrowserFallback,
    atrustShowAuthWindow,
    dispose,
  };
}
