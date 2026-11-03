// 阶段④（D9 计划 §四，win32 树同构镜像）：失败终态通知出口——快照采纳钩子发现
// runtime 进入 failed_* 终态时弹一次状态面 toast。
//
// darwin 差异（E13 裁定 + 阶段② darwin 真注记）：darwin wire 无 R1/R2 重连
// reason lane（engine 侧无 reconnect.rs 对应物），本模块不参与 lane 互斥——
// 与 N-02（lifecycle-effects 的 disconnect_notify 托盘气泡）的单向互斥仍然成立：
// 本 episode 已弹失败 toast 时，main.ts 传 `failureEpisodeNotified` 抑制该次
// 「EXV 已断开」气泡（断开类 episode 后台不双通知）。
//
// 去重与重武装（复审 2.4-1 的空 operation_id 规则）：去重不依赖 operation_id——
// engine 主动终态的 operation_id 为空；空 id 跨 episode 同态碰撞由「离开 failed_*
// 家族即重武装」消解：进入 failed 家族（前一采纳相位非 failed_*）才弹一次；
// failed_clean → failed_dirty 家族内迁移与同态重放不重复弹；离开后再次进入即新
// episode、重新可弹。
//
// 渲染：前台 = 应用内 toast；后台 = 接线层升级为系统托盘通知（main.ts 的
// `isWindowForeground`/`invokeTrayNotify`）；前台抑制偏好沿用连接类语义。标题/
// 描述复用 presenter 的 `failureTitleFor`/`descriptionFor`——同一 episode 状态卡
// 与 toast 说同一句话（§3.4 纪律）。

import type { RuntimeSnapshot, RuntimeState } from "../lib/ipc";
import { pushToast, type ToastKind } from "../lib/toast";
import { disconnectSemanticsOf } from "../lib/status-map";
import { descriptionFor, failureTitleFor } from "./presenter";

/** 一次失败终态通知的产出（标题 + 一句话原因 + toast 类别）。 */
export interface FailedNotifyDecision {
  kind: ToastKind;
  title: string;
  message: string;
}

/**
 * 纯判定：由前后两个 runtime 相位计算待弹通知。
 *
 *   * 首观察（prev = null）只建基线，不弹（对齐 lifecycle-effects 首快照去抖）；
 *   * 非失败终态 → null（离开 failed_* 家族的快照由调用方完成重武装）；
 *   * 家族内（prev 已是 failed_*）→ 不弹（同 episode 延续）；
 *   * aTrust 本地用户取消 → 由原 Host 普通 toast 呈现，不发失败通知；
 *   * 其余（从非失败相位进入 failed_*）→ 弹一次。
 */
export function decideFailedNotify(
  prevState: RuntimeState["state"] | null,
  snapshot: RuntimeSnapshot,
): FailedNotifyDecision | null {
  if (prevState === null) return null;
  const runtime = snapshot.runtime;
  if (runtime.state !== "failed_clean" && runtime.state !== "failed_dirty") return null;
  if (prevState === "failed_clean" || prevState === "failed_dirty") return null;
  // 原 Host 已沿 browser-login cancelled 事件呈现本次用户取消。
  if (runtime.error?.code === "ERROR_CODE_CANCELLED_BEFORE_START"
    && runtime.error.reason === "atrust/auth_cancelled") return null;

  // darwin 无 reason lane，两判据恒 null（与 presenter 的同构约定一致）。
  const disconnect = disconnectSemanticsOf(runtime.error ?? null, prevState, null, null);
  const title = failureTitleFor(runtime, "连接失败", disconnect);
  const message = descriptionFor(runtime, disconnect);
  if (message === null) return null;
  return { kind: "error", title, message };
}

/** 通知分发端口（main.ts 注入真实前台探测与托盘升级；缺省仅应用内 toast）。 */
export interface FailedNotifyPort {
  /** 独立意外断开偏好；未提供时保留旧端口调用者行为。 */
  unexpectedDisconnectEnabled?(): boolean;
  /** 同一断开事件已由原断开通知覆盖时只发送一次。 */
  disconnectNotifyEnabled?(): boolean;
  /** 用户主动 Stop 在实际快照到达前即撤销意外通知资格。 */
  userStopping?(): boolean;
  /** 原采纳快照在当前产品运行态中的归属。 */
  isCurrent?(snapshot: RuntimeSnapshot): boolean;
  /** 主窗口是否在前台（异步探测防错序，由接线层负责）。 */
  isForeground(): Promise<boolean>;
  /** 前台抑制偏好（`suppress_notify_when_foreground`）。 */
  suppressWhenForeground(): boolean;
  /** 应用内 toast（前台路径）。 */
  toast(message: string, kind: ToastKind): void;
  /** 系统托盘通知（后台路径升级）。 */
  trayNotify(title: string, body: string, kind?: "unexpected-disconnect" | "failed", isCurrent?: () => boolean): void;
}

/** 缺省端口：恒前台、不抑制、只弹应用内 toast（测试与未接线环境的行为基线）。 */
const defaultPort: FailedNotifyPort = {
  isForeground: () => Promise.resolve(true),
  suppressWhenForeground: () => false,
  toast: (message, kind) => pushToast(message, kind),
  trayNotify: () => undefined,
};

// ---- 模块级单例（同 win32 failed-notify 形态）----

/** 前一采纳快照的 runtime 相位（null = 尚无首观察）。 */
let prevRuntimeState: RuntimeState["state"] | null = null;
/** 当前失败 episode 是否已有声音（N-02 托盘气泡的单向让位查询位）。 */
let failureEpisodeNotifiedFlag = false;
/** 分发端口（init 注入；缺省仅应用内 toast）。 */
let port: FailedNotifyPort = defaultPort;

/** 初始化通知分发端口（main.ts 启动调用；测试注入记录器）。 */
export function initFailedNotifyPort(next: FailedNotifyPort): void {
  port = next;
}

/** N-02 互斥查询（只读）：当前失败 episode 是否已有声音。 */
export function failureEpisodeNotified(): boolean {
  return failureEpisodeNotifiedFlag;
}

/** 当前已连接会话的局部通知归属；中间态不丢失，主动Stop/替代连接退休。 */
let connectedOwner: { operationId: string | null; identity: string | null } | null = null;
let unexpectedEpisodeHandled = false;
let notificationEpoch = 0;
let activeNotification: { operationId: string | null; unexpected: boolean } | null = null;
function snapshotIdentity(snapshot: RuntimeSnapshot): string | null {
  const identity = snapshot.session_identity;
  return identity ? JSON.stringify([identity.protocol, identity.username, identity.server]) : null;
}
function unexpectedDisconnectDecision(snapshot: RuntimeSnapshot): { handled: boolean; decision: FailedNotifyDecision | null } {
  // 原测试/嵌入端口未接新键时保持原模块行为；生产入口始终注入独立开关。
  if (!port.unexpectedDisconnectEnabled) return { handled: false, decision: null };
  const state = snapshot.runtime.state;
  if (state === "connected") {
    if (prevRuntimeState !== "connected" || snapshot.operation_id !== connectedOwner?.operationId) unexpectedEpisodeHandled = false;
    connectedOwner = { operationId: snapshot.operation_id ?? null, identity: snapshotIdentity(snapshot) };
    return { handled: false, decision: null };
  }
  if (state === "stopping" || state === "idle" || port.userStopping?.()) {
    connectedOwner = null;
    unexpectedEpisodeHandled = false;
    return { handled: false, decision: null };
  }
  if (!connectedOwner) return { handled: false, decision: null };
  const identity = snapshotIdentity(snapshot);
  const sameOwner = snapshot.operation_id == null || snapshot.operation_id === connectedOwner.operationId
    || (identity !== null && identity === connectedOwner.identity);
  if (!sameOwner) {
    if (state === "connecting") { connectedOwner = null; unexpectedEpisodeHandled = false; }
    return { handled: false, decision: null };
  }
  const error = snapshot.runtime.state === "reconciling" ? snapshot.runtime.blocking_error
    : snapshot.runtime.state === "failed_clean" || snapshot.runtime.state === "failed_dirty" ? snapshot.runtime.error : null;
  const reason = error?.reason ?? "";
  const loss = error?.stage === "ERROR_STAGE_DATA_PLANE"
    || reason === "session/engine_control_stream_ended" || reason.startsWith("stop/engine_lost:");
  if (!loss) return { handled: false, decision: null };
  if (unexpectedEpisodeHandled) return { handled: true, decision: null };
  unexpectedEpisodeHandled = true;
  // 本端口实际接管通知后同步置 episode 位，原 lifecycle 据此让位。
  // disconnect_notify 仅表示偏好，不能代表原端口已发送本次通知。
  if (snapshot.reconnect?.auto_reconnect || !port.unexpectedDisconnectEnabled()) return { handled: true, decision: null };
  return { handled: true, decision: { kind: "error", title: "连接意外断开", message: error?.message?.trim() || "与 VPN 引擎或服务器的连接中断，请重新连接。" } };
}

/**
 * 快照采纳钩子（产品运行时的唯一喂入口）。判定 → 分发；同步维护 N-02 互斥位。
 */
export function announceFailedSnapshot(snapshot: RuntimeSnapshot): void {
  const nextState = snapshot.runtime.state;
  const unexpected = unexpectedDisconnectDecision(snapshot);
  const decision = unexpected.handled ? unexpected.decision : decideFailedNotify(prevRuntimeState, snapshot);
  if (activeNotification && (activeNotification.operationId !== (snapshot.operation_id ?? null)
    || (activeNotification.unexpected ? !unexpected.handled : nextState !== "failed_clean" && nextState !== "failed_dirty"))) {
    activeNotification = null;
    notificationEpoch++;
  }
  prevRuntimeState = nextState;
  if (unexpected.handled || nextState === "failed_clean" || nextState === "failed_dirty") {
    // 同态重放不清零——置位粘性至离开 failed_* 家族（episode 内单次）。
    failureEpisodeNotifiedFlag = failureEpisodeNotifiedFlag || decision !== null;
  } else {
    // 离开 failed_* 家族：重武装。
    failureEpisodeNotifiedFlag = false;
    return;
  }
  if (decision === null) return;

  activeNotification = { operationId: snapshot.operation_id ?? null, unexpected: unexpected.handled };
  const epoch = ++notificationEpoch;
  const isCurrent = () => notificationEpoch === epoch && !port.userStopping?.()
    && (port.isCurrent?.(snapshot) ?? true);
  const body = `${decision.title}：${decision.message}`;
  void (async () => {
    try {
      const foreground = await port.isForeground();
      if (!isCurrent()) return;
      if (foreground && port.suppressWhenForeground()) return; // 前台抑制（状态卡已承载）。
      if (foreground) {
        port.toast(body, decision.kind);
      } else {
        port.trayNotify(decision.title, decision.message, unexpected.handled ? "unexpected-disconnect" : "failed", isCurrent); // 后台升级系统通知。
      }
    } catch (error) {
      console.error("窗口前台状态检测失败", error);
      if (!isCurrent()) return;
      port.toast(body, decision.kind);
      if (!port.suppressWhenForeground()) port.trayNotify(decision.title, decision.message, unexpected.handled ? "unexpected-disconnect" : "failed", isCurrent);
    }
  })();
}

/** 测试隔离：复位模块状态（生产路径不调用）。 */
export function resetFailedNotifyState(): void {
  prevRuntimeState = null;
  connectedOwner = null;
  unexpectedEpisodeHandled = false;
  activeNotification = null;
  notificationEpoch = 0;
  failureEpisodeNotifiedFlag = false;
  port = defaultPort;
}
