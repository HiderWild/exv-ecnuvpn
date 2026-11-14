import { configuredRoutes } from "../lib/route-rules";
import { pushToast } from "../lib/toast";
import { assertConfigReadable } from "../lib/config-read-failure";
import { commandErrorMessage } from "../lib/command-error";
import { serviceInternalDiagnostic } from "../lib/service-error";
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
  type SessionIdentity,
  type ServiceControlAction,
  type ServiceControlReply,
} from "../lib/ipc";
import {
  CONNECT_PHASE_ORDER,
  disconnectDescriptionOf,
  errorAfterEvent,
  errorOf,
  isTerminalState,
  shouldApplyEvent,
  type OpCorrelation,
} from "../lib/status-map";
import { PRODUCT_PHASE_LABELS, present, reconnectTransitionOf } from "./presenter";
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
  triggerLatencyRefresh(operationId: string): Promise<void>;
  /** S3/D5：服务控制（query/install/uninstall/start）。 */
  serviceControl(action: ServiceControlAction): Promise<ServiceControlReply>;
  /** 连接身份与校内地址；真实网关读取 core 配置并探测 EXV 隧道适配器。 */
  connectionInfo?(): Promise<ProductConnectionInfo>;
}

export interface ProductRuntime {
  readonly state: Readonly<Ref<ProductUiState>>;
  /** 独立服务读取失败；服务命令的原成功结果继续保留。 */
  readonly serviceStatusReadError?: Readonly<Ref<string>>;
  readonly source: "real" | "mock";
  start(): Promise<void>;
  connect(credentials?: ConnectCredentials, atrustReauthenticate?: string): Promise<string | void>;
  stop(): Promise<void>;
  /** 配置保存后刷新身份；strict 失败返回固定错误，普通探测保持容错。 */
  configurationChanged(options?: { strict?: boolean }): Promise<void>;
  /**
   * T1：以当前已接纳 operation 请求刷新；成功仅表示 owner 受理，RTT 等后续 Stats。
   */
  triggerLatencyRefresh(): Promise<void>;
  /**
   * 执行服务控制；成功动作缺少状态时独立查询，保留原命令ok和message。
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
  triggerLatencyRefresh: (operationId) => kernel.triggerLatencyRefresh(operationId),
  serviceControl: (action) => kernel.serviceControl(action),
  async connectionInfo(): Promise<ProductConnectionInfo> {
    const [config, campusIp] = await Promise.all([
      kernel.configGet(),
      kernel.tunnelAddress().catch(() => null),
    ]);
    assertConfigReadable(config);
    const values = new Map(config.items.map((item) => [item.key, item.value]));
    return {
      protocol: values.get("protocol") === "atrust" ? "atrust" : "cisco",
      account: values.get("protocol") === "atrust" ? null : values.get("username")?.trim() || null,
      accountSource: values.get("protocol") === "atrust" ? "unavailable" : "configured",
      vpnServer: values.get(values.get("protocol") === "atrust" ? "atrust_server" : "server")?.trim() || null,
      campusIp,
      routeRuleCount: values.has("routes") ? configuredRoutes(values.get("routes")!).length : null,
    };
  },
};

/** 活动呈现只消费已接纳快照身份，配置只提供实时地址和已保存规则数量。 */
function sessionConnectionInfo(base: ProductConnectionInfo, identity: SessionIdentity | null | undefined): ProductConnectionInfo {
  const protocol = identity?.protocol === "atrust" ? "atrust" : identity?.protocol === "cisco" ? "cisco" : undefined;
  return {
    account: identity?.username.trim() || null,
    ...(protocol ? { accountSource: identity?.username.trim() ? "configured" as const : "unavailable" as const } : {}),
    vpnServer: identity?.server.trim() || null,
    campusIp: base.campusIp,
    ...(protocol ? { protocol } : {}),
    ...(base.routeRuleCount !== undefined ? { routeRuleCount: base.routeRuleCount } : {}),
  };
}

type DisplayError = string | null | undefined;

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
 * 失败终态的断开语义描述（阶段①「需要处理」分流，win32 树同构镜像）：stage 可识别
 * 时给出行动指引，否则 null。供错误显示槽（displayError）与 presenter 描述共用同一
 * 文案源，避免状态卡与横幅两套说法。darwin wire 暂无重连 reason lane——reason
 * 判据恒为 null，待 wire 解冻后自动生效。
 */
function failedDisconnectCopyOf(snapshot: RuntimeSnapshot): string | null {
  const runtime = snapshot.runtime;
  if (runtime.state !== "failed_clean" && runtime.state !== "failed_dirty") return null;
  return disconnectDescriptionOf(runtime.error ?? null, null, null);
}

function productStateFor(
  snapshot: RuntimeSnapshot,
  now: () => number,
  displayError: DisplayError,
  connectionInfo: ProductConnectionInfo,
  coreStatus: ProductUiState["coreStatus"],
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
  // S3 断开→重连过渡态期间不套错误显示槽：过渡文案优先，错误文案保留给最终失败终态。
  if (displayError !== undefined && !reconnectTransitionOf(snapshot)
    && !snapshot.stop_completion?.errors.length && !snapshot.stop_completion?.blocks_new_connect) {
    return { ...productState, coreStatus, description: displayError };
  }

  return { ...productState, coreStatus };
}

function initialDisplayError(snapshot: RuntimeSnapshot): DisplayError {
  if (!isErrorBearingState(snapshot.runtime)) return undefined;
  // 阶段①：冷启动即落在失败态时同样按 stage 分流描述（与事件路径同一文案源）。
  return failedDisconnectCopyOf(snapshot) ?? errorOf(snapshot.runtime);
}

function nextDisplayError(event: RuntimeEvent, previous: DisplayError): DisplayError {
  const next = errorAfterEvent(event, previous);
  if (typeof next === "string") {
    // 阶段①（「需要处理」分流）：失败终态若能由 stage 识别为断开语义，错误显示槽
    // 直接采用断开行动指引——泛化错误标签会掩埋掉线事实；识别不出时维持既有错误
    // 文案（规则 3 语义不变）。
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
  onAdoptedSnapshot: (snapshot: RuntimeSnapshot) => void = () => undefined,
): ProductRuntime {
  let latestSnapshot = idleSnapshot();
  // 服务读取只保护在途旧响应；后续有效快照继续更新服务事实。
  let serviceStatusVersion = 0;
  const serviceStatusReadError = ref("");
  let latestStatusSnapshot: { snapshot: RuntimeSnapshot; sequence: number } | null = null;
  let displayError: DisplayError = undefined;
  /**
   * 阶段①（「需要处理」分流，win32 树同构镜像）：当前失败 episode 进入前一刻的
   * runtime 相位（断开语义三判据之一；null = 未知/冷启动直接落在失败态）。同
   * episode 的 failed 态重放不重置（prev 不再是 connected，必须沿用进入时记录的
   * 值）；离开失败态即清空。
   */
  let failureEntryPrevState: RuntimeState["state"] | null = null;
  /** 是否已采纳过至少一份真实快照（区分冷启动首拍即失败与真实相位转移进入失败）。 */
  let sawAdoptedSnapshot = false;
  let correlation: OpCorrelation = { activeOpId: null, terminalReached: false, inFlightAction: null };
  let latestConnectionInfo: ProductConnectionInfo = EMPTY_PRODUCT_CONNECTION_INFO;
  let coreStatus: ProductUiState["coreStatus"] = null;
  let environmentPending = true;
  let unlisten: StopListening | null = null;
  let startPromise: Promise<void> | null = null;
  let liveSnapshotTimer: ReturnType<typeof setInterval> | null = null;
  let liveSnapshotInFlight = false;
  let environmentTimer: ReturnType<typeof setInterval> | null = null;
  let environmentInFlight = false;
  type ConnectionInfoRefreshResult = { ok: true } | { ok: false; error: unknown };
  let connectionInfoInFlight: Promise<ConnectionInfoRefreshResult> | null = null;
  let pendingAction: PendingAction = null;
  // 用户命令各自持有本地代次，旧 RPC 的完成不能改写后续命令的关联或忙碌窗口。
  let commandGeneration = 0;
  // 只有原业务输入推进此版本，显示合并不能使旧读取重新获得业务采纳资格。
  let businessVersion = 0;
  let liveDisplayScene = "";
  let liveDisplayTick = 0;
  let liveReadError: { key: string; error: unknown } | null = null;
  let environmentDisplayScene = "";
  let environmentDisplayTick = 0;
  let environmentReadError: { key: string; error: unknown } | null = null;
  let disposed = false;

  const state = ref<ProductUiState>(
    productStateFor(latestSnapshot, now, displayError, latestConnectionInfo, coreStatus, failureEntryPrevState),
  );

  function writeState(): void {
    const active = !["idle", "failed_clean", "failed_dirty"].includes(latestSnapshot.runtime.state);
    const connectionInfo = active
      ? sessionConnectionInfo(latestConnectionInfo, latestSnapshot.session_identity) : latestConnectionInfo;
    const base = productStateFor(latestSnapshot, now, displayError, connectionInfo, coreStatus, failureEntryPrevState);
    if (environmentPending) {
      base.systemProxy = { ...base.systemProxy, status: "loading" };
      base.proxyTun = { ...base.proxyTun, status: "loading" };
    }
    // 原停止中摘要必须可见；保持 pending 不等于覆盖原错误/blocks 的展示。
    state.value = pendingAction === "stop" && latestSnapshot.runtime.state === "stopping"
      ? base : optimisticProductState(base, pendingAction);
  }

  writeState();

  /**
   * 接纳一份完整快照。后端的周期统计/探测快照偶尔只携带新的伴生数据，可能暂时
   * 缺失本会话已确认的起点；这种不完整更新不能把在线时长清成横线。只有同为
   * Connected 且新值缺失时才保留旧值，断开/新会话/新正值仍按新快照直接替换。
   */
  function isCurrentEngineReconnect(snapshot: RuntimeSnapshot): boolean {
    return Boolean(snapshot.engine_reconnect) && snapshot.runtime.state === "connecting" &&
      Boolean(snapshot.operation_id) && snapshot.operation_id === correlation.activeOpId &&
      ["connected", "connecting"].includes(latestSnapshot.runtime.state);
  }

  function serviceStatusFromSnapshot(snapshot: RuntimeSnapshot, readVersion: number): RuntimeSnapshot["service_status"] {
    if (readVersion !== serviceStatusVersion || snapshot.service_status == null) return latestSnapshot.service_status;
    serviceStatusVersion += 1;
    serviceStatusReadError.value = "";
    return snapshot.service_status;
  }

  function adoptSnapshot(snapshot: RuntimeSnapshot, serviceReadVersion = serviceStatusVersion): boolean {
    if (pendingAction === "connect" && snapshot.stop_completion) return false;
    const currentTick = latestSnapshot.monotonic_tick;
    const nextTick = snapshot.monotonic_tick;
    if (nextTick > 0 && currentTick > 0 && nextTick < currentTick) return false;

    if (isNewAutomaticReconnect(snapshot)) {
      correlation = { activeOpId: snapshot.operation_id!, terminalReached: false, inFlightAction: null };
      displayError = null;
    }

    const currentState = latestSnapshot.runtime.state;
    const nextState = snapshot.runtime.state;
    const engineReconnect = isCurrentEngineReconnect(snapshot);
    if (engineReconnect) correlation = { ...correlation, terminalReached: false };
    // 部分 engine 拉取快照不带 EventBus tick；对这类快照仍需阻止同一操作内显然倒退的
    // 相位覆盖。新用户动作会先 resetForUserOperation，因此不会挡住新会话。
    if (
      (!engineReconnect && currentState === "connected" && snapshot.operation_id === latestSnapshot.operation_id &&
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
    // 缺席字段沿用已知值；旧在途读取保留较新的服务Query结果。
    next = { ...next, service_status: serviceStatusFromSnapshot(snapshot, serviceReadVersion) };
    // 阶段①：失败 episode 进入记录——非失败相位首次进入 failed_* 时冻结进入前相位；
    // 同 episode 的 failed 态重放不重置；离开失败态清空。冷启动首拍即失败时相位
    // 未知（合成 idle 不算真实前态），记 null 交由 stage/reason lane 判定。
    if (nextState === "failed_clean" || nextState === "failed_dirty") {
      if (currentState !== "failed_clean" && currentState !== "failed_dirty") {
        failureEntryPrevState = sawAdoptedSnapshot ? currentState : null;
      }
    } else {
      failureEntryPrevState = null;
    }
    sawAdoptedSnapshot = true;
    latestSnapshot = next;
    businessVersion += 1;
    // 阶段④：快照采纳钩子（win32 树同构）——failed-notify 等通知出口经此喂入
    //（仅原事件与启动基线）。
    onAdoptedSnapshot(latestSnapshot);
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
    // 配置身份已变更，空闲来源待刷新；活动身份仍只取已接纳快照。
    latestConnectionInfo = { ...latestConnectionInfo, account: null, accountSource: undefined };
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

  function displayReadBasis() {
    const runtime = latestSnapshot.runtime;
    const scene = JSON.stringify([commandGeneration, latestSnapshot.operation_id ?? null,
      runtime.state, runtime.state === "connected" ? runtime.session_established_at_ms ?? null : null]);
    return { version: businessVersion, scene };
  }

  function isCurrentDisplayRead(basis: ReturnType<typeof displayReadBasis>): boolean {
    return !disposed && basis.version === businessVersion && basis.scene === displayReadBasis().scene;
  }

  function matchesDisplaySource(snapshot: RuntimeSnapshot, connected: boolean): boolean {
    if (snapshot.operation_id && snapshot.operation_id !== latestSnapshot.operation_id) return false;
    if (connected) {
      if (snapshot.runtime.state !== "connected" || latestSnapshot.runtime.state !== "connected") return false;
      const start = snapshot.runtime.session_established_at_ms;
      const currentStart = latestSnapshot.runtime.session_established_at_ms;
      if (typeof start === "number" && start > 0 && typeof currentStart === "number" && currentStart > 0 && start !== currentStart) return false;
    }
    const tick = snapshot.monotonic_tick;
    return !(tick > 0 && latestSnapshot.monotonic_tick > 0 && tick < latestSnapshot.monotonic_tick);
  }

  function reportDisplayReadError(error: unknown, previous: { key: string; error: unknown } | null) {
    const detail = error != null && typeof error === "object" ? error as Record<string, unknown> : null;
    const native = detail?.native != null && typeof detail.native === "object"
      ? detail.native as Record<string, unknown> : null;
    const fields = [detail?.message ?? String(error), detail?.kind, detail?.code,
      native?.category, native?.namespace, native?.code].filter(value => value != null && value !== "");
    const key = fields.map(String).join(" | ");
    if (previous?.key !== key) {
      console.error("显示数据读取失败", error);
      pushToast("显示数据读取失败：" + key, "warning");
    }
    return { key, error };
  }

  function stopEnvironmentRefresh(): void {
    if (environmentTimer === null) return;
    clearInterval(environmentTimer);
    environmentTimer = null;
  }

  function needsEnvironmentDisplay(): boolean {
    return ["idle", "failed_clean", "failed_dirty", "reconciling"].includes(latestSnapshot.runtime.state);
  }

  async function refreshEnvironment(): Promise<void> {
    if (disposed || environmentInFlight || pendingAction !== null || !needsEnvironmentDisplay()) return;
    environmentInFlight = true;
    const basis = displayReadBasis();
    const serviceReadVersion = serviceStatusVersion;
    if (environmentDisplayScene !== basis.scene) {
      environmentDisplayScene = basis.scene;
      environmentDisplayTick = 0;
      environmentReadError = null;
    }
    try {
      const snapshot = await gateway.snapshot();
      if (!isCurrentDisplayRead(basis)) return;
      environmentReadError = null;
      if (!matchesDisplaySource(snapshot, false) ||
          (snapshot.monotonic_tick > 0 && snapshot.monotonic_tick < environmentDisplayTick)) return;
      if (snapshot.monotonic_tick > 0) environmentDisplayTick = snapshot.monotonic_tick;
      latestSnapshot = { ...latestSnapshot,
        system_proxy: snapshot.system_proxy ?? latestSnapshot.system_proxy,
        proxy_tun: snapshot.proxy_tun ?? latestSnapshot.proxy_tun,
        service_status: serviceStatusFromSnapshot(snapshot, serviceReadVersion) };
    } catch (error) {
      if (isCurrentDisplayRead(basis)) environmentReadError = reportDisplayReadError(error, environmentReadError);
    } finally {
      environmentInFlight = false;
      if (isCurrentDisplayRead(basis)) {
        environmentPending = false;
        writeState();
      }
    }
  }

  function startEnvironmentRefresh(): void {
    if (environmentTimer !== null || disposed) return;
    environmentTimer = setInterval(() => { void refreshEnvironment(); }, 3_000);
  }

  async function refreshLiveSnapshot(): Promise<void> {
    if (disposed || liveSnapshotInFlight || pendingAction !== null || latestSnapshot.runtime.state !== "connected") return;
    liveSnapshotInFlight = true;
    const basis = displayReadBasis();
    const serviceReadVersion = serviceStatusVersion;
    if (liveDisplayScene !== basis.scene) {
      liveDisplayScene = basis.scene;
      liveDisplayTick = 0;
      liveReadError = null;
    }
    try {
      const snapshot = await gateway.snapshot();
      if (!isCurrentDisplayRead(basis)) return;
      liveReadError = null;
      if (!matchesDisplaySource(snapshot, true) ||
          (snapshot.monotonic_tick > 0 && snapshot.monotonic_tick < liveDisplayTick)) return;
      if (snapshot.monotonic_tick > 0) liveDisplayTick = snapshot.monotonic_tick;
      latestSnapshot = { ...latestSnapshot,
        stats: snapshot.stats ?? latestSnapshot.stats,
        system_proxy: snapshot.system_proxy ?? latestSnapshot.system_proxy,
        proxy_tun: snapshot.proxy_tun ?? latestSnapshot.proxy_tun,
        service_status: serviceStatusFromSnapshot(snapshot, serviceReadVersion) };
      if (latestConnectionInfo.campusIp === null) void refreshConnectionInfo();
    } catch (error) {
      if (isCurrentDisplayRead(basis)) liveReadError = reportDisplayReadError(error, liveReadError);
    } finally {
      liveSnapshotInFlight = false;
      // 读失败或无新样本仍按原会话起点推进在线时长。
      if (isCurrentDisplayRead(basis)) writeState();
    }
  }

  function startLiveSnapshotRefresh(): void {
    if (liveSnapshotTimer !== null || disposed) return;
    liveSnapshotTimer = setInterval(() => {
      // 统计读取可以在飞，在线计时仍只依赖原会话起点。
      if (!disposed && latestSnapshot.runtime.state === "connected") writeState();
      void refreshLiveSnapshot();
    }, 1000);
  }

  // Connect 的真实进度撤去点击覆盖；Stop 中间相位不能结束原等待。
  function matchesInFlightDirection(snapshot: RuntimeSnapshot, action: PendingAction): boolean {
    if (action === "connect") {
      return snapshot.runtime.state === "connecting" || snapshot.runtime.state === "awaiting_interaction" || isTerminalState(snapshot.runtime);
    }
    return snapshot.stop_completion?.stopped === true || ["idle", "failed_clean", "failed_dirty"].includes(snapshot.runtime.state);
  }

  function releasePendingAction(keepAssociation = false): void {
    pendingAction = null;
    // 关联只过滤旧事件，不作为按钮门；receipt 可结束本次局部等待。
    if (!keepAssociation) correlation = { ...correlation, inFlightAction: null };
    if (latestSnapshot.runtime.state === "connected") startLiveSnapshotRefresh();
  }

  function resetForUserOperation(action: Exclude<PendingAction, null>): number {
    const generation = ++commandGeneration;
    businessVersion += 1;
    stopLiveSnapshotRefresh();
    displayError = null;
    if (action === "connect") latestSnapshot = { ...latestSnapshot, stop_completion: null, session_identity: null };
    correlation = { activeOpId: null, terminalReached: false, inFlightAction: action };
    pendingAction = action;
    writeState();
    return generation;
  }

  function handleStatus(event: RuntimeEvent): void {
    if (disposed) return;
    const snapshot = event.snapshot.operation_id == null && event.operation_id
      ? { ...event.snapshot, operation_id: event.operation_id }
      : event.snapshot;
    const currentSessionUpdate = Boolean(correlation.activeOpId) && latestSnapshot.runtime.state === "connected" &&
      (["reconciling", "stopping"].includes(snapshot.runtime.state) || isCurrentEngineReconnect(snapshot)) &&
      (snapshot.operation_id ?? event.operation_id) === correlation.activeOpId;
    if (!isNewAutomaticReconnect(snapshot) && !currentSessionUpdate &&
      !shouldApplyEvent(event, correlation)) return;

    // 只由原业务输入撤点击覆盖；停止中摘要不清仍在等待的 Stop。
    const awaitingAction = pendingAction ?? correlation.inFlightAction;
    // 无 ID 成功后的关联不允许相反方向终态接管；旧无 ID 仍由原过滤器拒绝。
    if (pendingAction === null && correlation.activeOpId === null && awaitingAction !== null &&
        isTerminalState(snapshot.runtime) && !matchesInFlightDirection(snapshot, awaitingAction)) return;
    const completesPendingDirection = awaitingAction !== null && matchesInFlightDirection(snapshot, awaitingAction);
    const wasConnected = latestSnapshot.runtime.state === "connected";
    if (!adoptSnapshot(snapshot)) return;
    latestStatusSnapshot = { snapshot: latestSnapshot, sequence: commandGeneration };
    if (completesPendingDirection) releasePendingAction();
    coreStatus = "normal";
    if (snapshot.runtime.state === "connected") {
      startLiveSnapshotRefresh();
      if (!wasConnected) {
        refreshConnectionInfoAfterConnected();
        // engine 的周期探测是低频的；每次新会话额外请求一次真实探测，避免用户进入
        // 已连接界面后还要等待其周期。失败仍由后端周期探测兜底，不能产生未处理拒绝。
        void triggerLatencyRefresh().catch(() => undefined);
      }
    } else {
      stopLiveSnapshotRefresh();
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
      const initialVersion = businessVersion;
      const initialSequence = commandGeneration;
      const stopListening = await gateway.onStatus(handleStatus);
      if (disposed) {
        stopListening();
        return;
      }
      unlisten = stopListening;
      try {
        const serviceReadVersion = serviceStatusVersion;
        const snapshot = await gateway.snapshot();
        if (!disposed && initialVersion === businessVersion && initialSequence === commandGeneration) {
          coreStatus = "normal";
          adoptSnapshot(snapshot, serviceReadVersion);
          displayError = initialDisplayError(snapshot);
          writeState();
        }
      } catch (error) {
        if (!disposed && initialVersion === businessVersion && initialSequence === commandGeneration) {
          environmentReadError = reportDisplayReadError(error, environmentReadError);
          environmentPending = false;
          writeState();
        }
      }
      await refreshConnectionInfo();

      if (disposed) return;
      if (latestSnapshot.runtime.state === "connected") startLiveSnapshotRefresh();
      startEnvironmentRefresh();
    })();

    try {
      await startPromise;
    } catch (error) {
      startPromise = null;
      throw error;
    }
  }

  async function connect(credentials?: ConnectCredentials, atrustReauthenticate?: string): Promise<string | void> {
    const generation = resetForUserOperation("connect");
    try {
      // 先派发连接请求，身份/校内地址刷新不能阻塞用户看到连接中的即时反馈。
      const reply = await gateway.connect({
        ...NO_PROFILE_CONNECT_INTENT,
        ...(credentials ? { credentials } : {}),
        ...(atrustReauthenticate ? { atrust_reauthenticate: atrustReauthenticate } : {}),
      });
      if (disposed || generation !== commandGeneration) return;
      coreStatus = "normal";
      businessVersion += 1;
      // C2 窗口生命周期：RPC 回复登记 activeOpId 即关闭窗口（受理快照先到的常态下
      // 窗口已在 handleStatus 解除；此处覆盖回复先到、事件后到的时序，见 F4）。
      correlation.activeOpId = operationIdOf(reply);
      correlation = { ...correlation, inFlightAction: correlation.activeOpId ? null : pendingAction };
      throwIfOperationFailed(reply);
      writeState();
      void refreshConnectionInfo();
      return correlation.activeOpId ?? undefined;
    } catch (error) {
      if (disposed || generation !== commandGeneration) return;
      releasePendingAction();
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
    if (latestStatusSnapshot?.sequence === commandGeneration && operationIdOf(reply) &&
        operationIdOf(reply) === latestStatusSnapshot.snapshot.operation_id &&
        operationIdOf(reply) === latestSnapshot.operation_id && isTerminalState(latestStatusSnapshot.snapshot.runtime)) {
      if (completion.errors.length) {
        const details = completion.errors.map(error => [error.message, error.code].filter(Boolean).join(" ")).join("; ");
        console.error("较早停止回执报告错误", completion);
        pushToast("较早停止回执报告错误：" + details, "warning");
      }
      return;
    }
    // Pending 可携中间摘要；只据原 stopped 事实裁隧道停止。
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
    businessVersion += 1;
    if (completion.stopped || failed) {
      releasePendingAction();
      stopLiveSnapshotRefresh();
      correlation.terminalReached = true;
    }
    writeState();
  }

  async function stop(): Promise<void> {
    const generation = resetForUserOperation("stop");
    try {
      const reply = await gateway.stop();
      if (disposed || generation !== commandGeneration) return;
      coreStatus = "normal";
      businessVersion += 1;
      correlation.activeOpId = operationIdOf(reply);
      correlation = { ...correlation, inFlightAction: correlation.activeOpId ? null : pendingAction };
      applyStopCompletion(reply);
      throwIfOperationFailed(reply);
      if (reply.result.result === "succeeded" && (operationIdOf(reply) || !reply.stop_completion)) {
        // 缺 ID/摘要不证明隧道停止，也不无限延长局部忙碌覆盖。
        releasePendingAction(!operationIdOf(reply));
      }
      writeState();
    } catch (error) {
      if (disposed || generation !== commandGeneration) return;
      releasePendingAction();
      writeState();
      throw error;
    }
  }

  /**
   * T1：请求当前会话刷新，延迟仍由后续 Stats 到达；不把拉取快照当成探测回包。
   * 不触碰错误/关联状态——这是周期探测，不是用户业务操作。
   */
  async function triggerLatencyRefresh(): Promise<void> {
    const operationId = latestSnapshot.operation_id;
    if (latestSnapshot.runtime.state !== "connected" || !operationId) {
      throw { kind: "latency_not_connected", message: "当前没有已确认的连接，无法刷新时延。" };
    }
    await gateway.triggerLatencyRefresh(operationId);
    if (latestSnapshot.operation_id !== operationId || latestSnapshot.runtime.state !== "connected") {
      throw { kind: "latency_session_changed", message: "连接已变化，本次刷新结果不适用于当前会话。" };
    }
  }

  function applyServiceReply(reply: ServiceControlReply): void {
    if (!reply.ok) throw new Error(reply.message || "服务状态读取失败。");
    if (reply.service_status == null) throw new Error("服务状态回复缺少状态数据。");
    serviceStatusVersion += 1;
    latestSnapshot = { ...latestSnapshot, service_status: reply.service_status };
    serviceStatusReadError.value = "";
    writeState();
  }

  function reportServiceReadError(error: unknown): void {
    serviceStatusVersion += 1;
    latestSnapshot = { ...latestSnapshot, service_status: null };
    const detail = serviceInternalDiagnostic(error) ?? commandErrorMessage(error, "请重试读取。");
    serviceStatusReadError.value = detail.startsWith("服务状态读取失败") ? detail : `服务状态读取失败：${detail}`;
    writeState();
  }

  /** 服务命令结果与独立状态读取分别报告；状态读取失败保留原命令成功。 */
  async function serviceControl(action: ServiceControlAction): Promise<ServiceControlReply> {
    if (action === "query") {
      try {
        const reply = await gateway.serviceControl(action);
        applyServiceReply(reply);
        return reply;
      } catch (error) {
        reportServiceReadError(error);
        throw error;
      }
    }
    const reply = await gateway.serviceControl(action);
    if (!reply.ok) return reply;
    try {
      const observed = reply.service_status == null ? await gateway.serviceControl("query") : reply;
      applyServiceReply(observed);
      return { ...reply, service_status: observed.service_status,
        ...(observed.service_version !== undefined ? { service_version: observed.service_version } : {}) };
    } catch (error) {
      reportServiceReadError(error);
    }
    return reply;
  }

  function dispose(): void {
    disposed = true;
    stopLiveSnapshotRefresh();
    stopEnvironmentRefresh();
    unlisten?.();
    unlisten = null;
  }

  return {
    source: "real",
    state,
    serviceStatusReadError,
    start,
    connect,
    stop,
    configurationChanged,
    triggerLatencyRefresh,
    serviceControl,
    dispose,
  };
}
