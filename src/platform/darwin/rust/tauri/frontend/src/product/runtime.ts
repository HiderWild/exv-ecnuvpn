import { ref, type InjectionKey, type Ref } from "vue";

import {
  kernel,
  onStatus,
  type ConnectCredentials,
  type ConnectIntent,
  type CoreStatus,
  type OperationReply,
  type RuntimeEvent,
  type RuntimeSnapshot,
  type RuntimeState,
  type ServiceControlAction,
  type ServiceControlReply,
} from "../lib/ipc";
import {
  CONNECT_PHASE_ORDER,
  errorAfterEvent,
  errorOf,
  isTerminalState,
  shouldApplyEvent,
  type OpCorrelation,
} from "../lib/status-map";
import { PRODUCT_PHASE_LABELS, present } from "./presenter";
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
  /** T1：请求一次延迟刷新（engine 数据面立即 ping；新延迟经后续快照 stats 到达）。 */
  triggerLatencyRefresh(): Promise<void>;
  /** S3/D5：服务控制（query/install/uninstall/start）。 */
  serviceControl(action: ServiceControlAction): Promise<ServiceControlReply>;
  /** 连接身份与校内地址；真实网关读取 core 配置并探测 EXV 隧道适配器。 */
  connectionInfo?(): Promise<ProductConnectionInfo>;
}

export interface ProductRuntime {
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
  triggerLatencyRefresh: () => kernel.triggerLatencyRefresh(),
  serviceControl: (action) => kernel.serviceControl(action),
  async connectionInfo(): Promise<ProductConnectionInfo> {
    const [config, campusIp] = await Promise.all([
      kernel.configGet(),
      kernel.tunnelAddress().catch(() => null),
    ]);
    const values = new Map(config.items.map((item) => [item.key, item.value]));
    return {
      account: values.get("username")?.trim() || null,
      vpnServer: values.get("server")?.trim() || null,
      campusIp,
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
 * C3（复审 P2-4）：GetSnapshot 含 engine `ObserveOwnedState` gRPC 往返与探测/SCM
 * 成本（composition/engine 锁均为短临界区、`prefer_snapshot` 防假 Idle——此为可接受
 * 的量化论证）；过渡窗口超过 10s 后轮询降频 1s → 3s，长窗口（bootstrap 慢路径）的
 * 轮询成本有界。
 */
const TRANSITION_SNAPSHOT_POLL_SLOW_MS = 3_000;
const TRANSITION_SNAPSHOT_POLL_SLOW_AFTER_MS = 10_000;
/**
 * C4（ui-connect-stop-responsiveness）：`pendingAction` 超时回落。只在事件链与
 * GetSnapshot 轮询双双不可达时触发（正常路径受理快照 ≤1s、轮询兜底 ≤2s 都先到）。
 */
const PENDING_ACTION_TIMEOUT_MS = 4_000;

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

function productStateFor(
  snapshot: RuntimeSnapshot,
  now: () => number,
  displayError: DisplayError,
  connectionInfo: ProductConnectionInfo,
  coreStatus: CoreStatus,
): ProductUiState {
  const productState = present(snapshot, now(), connectionInfo);

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
  if (displayError !== undefined) {
    return { ...productState, coreStatus, description: displayError };
  }

  return { ...productState, coreStatus };
}

function initialDisplayError(snapshot: RuntimeSnapshot): DisplayError {
  return isErrorBearingState(snapshot.runtime) ? errorOf(snapshot.runtime) : undefined;
}

function nextDisplayError(event: RuntimeEvent, previous: DisplayError): DisplayError {
  const next = errorAfterEvent(event, previous);
  if (typeof next === "string") return next;
  if (next === null) {
    return isErrorBearingState(event.snapshot.runtime) ? null : undefined;
  }
  return undefined;
}

function operationIdOf(reply: OperationReply): string | null {
  const operationId = reply.operation_id?.trim();
  return operationId ? operationId : null;
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
function optimisticStages(): ProductStage[] {
  return CONNECT_PHASE_ORDER.map((phase, index) => ({
    phase,
    label: PRODUCT_PHASE_LABELS[phase] ?? phase,
    visual: index === 0 ? "current" : "waiting",
  }));
}

function optimisticProductState(base: ProductUiState, pendingAction: PendingAction): ProductUiState {
  if (pendingAction === "connect") {
    return {
      ...base,
      status: "connecting",
      severity: "attention",
      title: "连接中",
      description: "正在发起连接请求。",
      stages: optimisticStages(),
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
 */
export function createProductRuntime(
  gateway: ProductGateway = tauriProductGateway,
  now: () => number = Date.now,
): ProductRuntime {
  let latestSnapshot = idleSnapshot();
  let displayError: DisplayError = undefined;
  let correlation: OpCorrelation = { activeOpId: null, terminalReached: false, inFlightAction: null };
  let latestConnectionInfo: ProductConnectionInfo = EMPTY_PRODUCT_CONNECTION_INFO;
  let sessionAccount: string | null = null;
  let coreStatus: CoreStatus = "stopped";
  let environmentPending = true;
  let connectionStateConfirmed = true;
  let unlisten: StopListening | null = null;
  let startPromise: Promise<void> | null = null;
  let liveSnapshotTimer: ReturnType<typeof setInterval> | null = null;
  let liveSnapshotInFlight = false;
  let coreStatusTimer: ReturnType<typeof setInterval> | null = null;
  let coreStatusInFlight = false;
  let connectionInfoInFlight: Promise<boolean> | null = null;
  let pendingAction: PendingAction = null;
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
    productStateFor(latestSnapshot, now, displayError, latestConnectionInfo, coreStatus),
  );

  function writeState(): void {
    const active = ["connecting", "awaiting_interaction", "connected"].includes(latestSnapshot.runtime.state);
    const connectionInfo = active && sessionAccount ? { ...latestConnectionInfo, account: sessionAccount } : latestConnectionInfo;
    const visibleSnapshot: RuntimeSnapshot = !connectionStateConfirmed && latestSnapshot.runtime.state === "connected"
      ? { ...latestSnapshot, runtime: { state: "reconciling" }, stats: null }
      : latestSnapshot;
    const base = productStateFor(visibleSnapshot, now, displayError, connectionInfo, coreStatus);
    if (!connectionStateConfirmed && latestSnapshot.runtime.state === "connected") {
      base.title = "连接状态待确认";
      base.description = "与后台的状态通信已中断，正在重新确认连接。";
    }
    if (environmentPending) {
      base.systemProxy = { ...base.systemProxy, status: "loading" };
      base.proxyTun = { ...base.proxyTun, status: "loading" };
    }
    state.value = optimisticProductState(base, pendingAction);
  }

  writeState();

  /**
   * 接纳一份完整快照。后端的周期统计/探测快照偶尔只携带新的伴生数据，可能暂时
   * 缺失本会话已确认的起点；这种不完整更新不能把在线时长清成横线。只有同为
   * Connected 且新值缺失时才保留旧值，断开/新会话/新正值仍按新快照直接替换。
   */
  function adoptSnapshot(snapshot: RuntimeSnapshot, queryBasis?: RuntimeSnapshot): boolean {
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
      ["failed_clean", "failed_dirty", "reconciling", "stopping", "connected"].includes(latestSnapshot.runtime.state) &&
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
      (currentState === "stopping" && nextState === "connected" && snapshot.operation_id === latestSnapshot.operation_id) ||
      // Connected 结束连接操作，但会话仍活动；同态统计和重连投影必须继续更新。
      (correlation.terminalReached && !(currentState === "connected" &&
          (nextState === "connected" || (nextState === "stopping" && snapshot.operation_id === latestSnapshot.operation_id))) &&
        (nextState === "connecting" || nextState === "awaiting_interaction" || nextState === "stopping" || nextState === "connected"))
    ) {
      return false;
    }

    environmentPending = false;
    connectionStateConfirmed = true;
    if (nextState === "reconciling" || nextState === "stopping") correlation.terminalReached = false;

    const previousRuntime = latestSnapshot.runtime;
    const nextRuntime = snapshot.runtime;
    const previousSessionStart = previousRuntime.state === "connected"
      ? previousRuntime.session_established_at_ms
      : undefined;
    const nextSessionStart = nextRuntime.state === "connected"
      ? nextRuntime.session_established_at_ms
      : undefined;
    let next: RuntimeSnapshot;
    if (
      previousRuntime.state === "connected" &&
      nextRuntime.state === "connected" &&
      typeof previousSessionStart === "number" &&
      previousSessionStart > 0 &&
      !(typeof nextSessionStart === "number" && nextSessionStart > 0)
    ) {
      next = {
        ...snapshot,
        runtime: {
          ...nextRuntime,
          session_established_at_ms: previousSessionStart,
        },
      };
    } else {
      next = snapshot;
    }
    // 服务状态防悬空（2026-09-08 用户拍板）：伴生值缺席的中间帧（过渡快照/个别
    // 未回填事件道）保留上一已知值——新值到达即顶替，显示不经历「未知」空窗。
    // 真正的状态变化（含 not_installed）总是携带新值，不受影响；上一已知值只在
    // 本 UI 会话内延续（重启后从首份真实数据起算）。
    if (next.service_status == null && latestSnapshot.service_status != null) {
      next = { ...next, service_status: latestSnapshot.service_status };
    }
    latestSnapshot = next;
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

  function refreshConnectionInfo(): Promise<boolean> {
    if (!gateway.connectionInfo) return Promise.resolve(false);
    if (connectionInfoInFlight !== null) return connectionInfoInFlight;

    connectionInfoInFlight = (async () => {
      try {
        latestConnectionInfo = await gateway.connectionInfo!();
        writeState();
        return true;
      } catch {
        // 配置/本机适配器探测失败不应阻断连接状态流；保留上一次已确认的信息。
        return false;
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
    const refreshed = !disposed && await refreshConnectionInfo();
    if (options?.strict && !refreshed) throw new Error("configuration_refresh_failed");
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
      if (coreStatus === "normal" && ["idle", "failed_clean", "failed_dirty", "reconciling"].includes(latestSnapshot.runtime.state)) {
        // Idle 没有统计事件，也要及时反映用户在其他应用切换代理；只更新环境字段，
        // 不让延迟返回的旧快照覆盖新的连接或断开状态。
        const before = latestSnapshot;
        const snapshot = await gateway.snapshot();
        if (!disposed && before === latestSnapshot) {
          latestSnapshot = { ...latestSnapshot, system_proxy: snapshot.system_proxy, proxy_tun: snapshot.proxy_tun };
          environmentPending = false;
        }
      }
    } catch {
      // 命令层出错等价于控制管道不可用；恢复只在用户随后点击连接时进行。
      coreStatus = "stopped";
      environmentPending = false;
      latestSnapshot = { ...latestSnapshot, system_proxy: null, proxy_tun: null };
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

  /** C4：武装 pendingAction 超时看门狗（事件链与轮询双失联时的最后可操作回落）。 */
  function armPendingActionWatchdog(): void {
    disarmPendingActionWatchdog();
    pendingActionWatchdog = setTimeout(() => {
      pendingActionWatchdog = null;
      if (pendingAction === null) return;
      // 超时行为（C4 冻结）：回到最近快照驱动的可操作态——不制造错误状态、不弹
      // toast、不伪造终态；迟到的 RPC 回复仍按既有路径登记 activeOpId，无状态污染。
      pendingAction = null;
      correlation = { ...correlation, inFlightAction: null };
      writeState();
    }, PENDING_ACTION_TIMEOUT_MS);
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
      // 已确认终态且没有重连工作时停止兜底轮询。
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
    stopLiveSnapshotRefresh();
    stopTransitionSnapshotRefresh();
    // 新操作发起：重置事件静默基准（此前事件属旧操作），乐观窗口首拍即可兜底。
    lastEventAppliedAt = null;
    displayError = null;
    correlation = { activeOpId: null, terminalReached: false, inFlightAction: action };
    pendingAction = action;
    armPendingActionWatchdog();
    // C3：乐观置位时立即启动兜底真相源轮询（事件先到时由 handleStatus/停表条件收口）。
    startTransitionSnapshotRefresh();
    writeState();
  }

  function handleStatus(event: RuntimeEvent): void {
    if (disposed) return;
    const currentSessionUpdate = latestSnapshot.runtime.state === "connected" &&
      ["reconciling", "stopping"].includes(event.snapshot.runtime.state) &&
      (event.snapshot.operation_id ?? event.operation_id) === correlation.activeOpId;
    if (!isNewAutomaticReconnect(event.snapshot) && !currentSessionUpdate &&
      !shouldApplyEvent(event, correlation)) return;

    // C2/P1-2：窗口内放行的事件先采纳；只有相位与窗口方向一致（或已无窗口）才解除
    // pendingAction/看门狗/窗口；不一致（陈旧 pre-click 快照/外来相位）照常采纳显示，
    // 不停表、不解除。
    if (pendingAction !== null && matchesInFlightDirection(event.snapshot.runtime, pendingAction)) {
      releasePendingAction();
    }
    const wasConnected = latestSnapshot.runtime.state === "connected";
    if (!adoptSnapshot(event.snapshot)) return;
    coreStatus = "normal";
    // C3 事件优先：任何被采纳的事件都是主真相，重置静默计时（轮询让位）。
    lastEventAppliedAt = Date.now();
    if (event.snapshot.runtime.state === "connected") {
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
      // 断线清理、状态确认和自动重连期间保持查询兜底，漏事件也能恢复。
      if (needsTransitionPolling(event.snapshot)) {
        startTransitionSnapshotRefresh();
      } else {
        stopTransitionSnapshotRefresh();
      }
    }
    writeState();

    displayError = nextDisplayError(event, displayError);
    writeState();

    if (isTerminalState(event.snapshot.runtime)) {
      correlation.terminalReached = true;
    }
  }

  async function start(): Promise<void> {
    if (startPromise !== null) return startPromise;

    startPromise = (async () => {
      try {
        const snapshot = await gateway.snapshot();
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
      } catch {
        // UI 本身照常打开，明确显示 Core 已停止；用户点击连接才进入受控恢复路径。
        coreStatus = "stopped";
        environmentPending = false;
        writeState();
      }
      await refreshConnectionInfo();

      const stopListening = await gateway.onStatus(handleStatus);
      if (disposed) {
        stopListening();
      } else {
        unlisten = stopListening;
        if (latestSnapshot.runtime.state === "connected") startLiveSnapshotRefresh();
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
    sessionAccount = credentials?.username.trim() || null;
    resetForUserOperation("connect");
    try {
      // 先派发连接请求，身份/校内地址刷新不能阻塞用户看到连接中的即时反馈。
      const reply = await gateway.connect({
        ...NO_PROFILE_CONNECT_INTENT,
        ...(credentials ? { credentials } : {}),
      });
      coreStatus = "normal";
      // C2 窗口生命周期：RPC 回复登记 activeOpId 即关闭窗口（受理快照先到的常态下
      // 窗口已在 handleStatus 解除；此处覆盖回复先到、事件后到的时序，见 F4）。
      correlation.activeOpId = operationIdOf(reply);
      correlation = { ...correlation, inFlightAction: null };
      void refreshConnectionInfo();
      return correlation.activeOpId ?? undefined;
    } catch (error) {
      releasePendingAction();
      writeState();
      throw error;
    }
  }

  async function stop(): Promise<void> {
    resetForUserOperation("stop");
    try {
      const reply = await gateway.stop();
      correlation.activeOpId = operationIdOf(reply);
      correlation = { ...correlation, inFlightAction: null };
    } catch (error) {
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
    const queryBasis = latestSnapshot;
    await gateway.triggerLatencyRefresh();
    const snapshot = await gateway.snapshot();
    // 状态事件已经确认连接后，立即拉取的 IPC 快照仍可能落后一个调度周期。不能让这份
    // 陈旧非连接快照覆盖新会话，否则界面会闪回 idle 并中止实时统计刷新。
    if (latestSnapshot.runtime.state === "connected" && snapshot.runtime.state === "idle" && !snapshot.operation_id) return;
    if (!adoptSnapshot(snapshot, queryBasis)) return;
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

  function dispose(): void {
    disposed = true;
    stopLiveSnapshotRefresh();
    stopCoreStatusRefresh();
    stopTransitionSnapshotRefresh();
    disarmPendingActionWatchdog();
    unlisten?.();
    unlisten = null;
  }

  return {
    source: "real",
    state,
    start,
    connect,
    stop,
    configurationChanged,
    triggerLatencyRefresh,
    serviceControl,
    dispose,
  };
}
