// 连接过渡效果（纯函数 + 薄分发）：状态变化 → 窗口/托盘效果。
//
// 效果全部由前端自有设置驱动（ui-prefs），不触碰 core 配置：
//   * 进入 connected：`minimize_to_tray_on_connect` → 隐藏窗口；
//     `connect_notify` → 托盘气泡「已连接」。
//   * 离开 connected（任一非连接态）：`disconnect_notify` → 托盘气泡「已断开」（窗口不动）。
//   * 实际恢复触发由本模块局部游标记忆；应用内 toast 与系统通知分别分发。
//   * 前台抑制：窗口在前台（聚焦）且 `suppress_notify_when_foreground` 开启时，抑制全部
//     连接类通知（隐藏窗口动作不受影响）。前台状态由调用方（main.ts）判定后作为
//     `NotifyContext` 传入，本函数保持纯函数。
//   * 首快照去抖：应用启动即已连接时不发通知（对齐旧 webui 的
//     connectionNotificationState 初始化语义——首见只记录基线）。

import type { ProductStatus, ProductUiState } from "./types";
import type { UiPreferences } from "./ui-prefs";

/** 单个过渡效果的执行指令。 */
export type LifecycleEffect =
  | { kind: "hide-window" }
  | { kind: "tray-notify"; title: string; body: string };

/** 同一过渡先完成通知门控读取，再隐藏窗口；等待期间过渡失效则不隐藏旧窗口。 */
export async function dispatchLifecycleEffects(
  effects: LifecycleEffect[],
  port: { isCurrent(): boolean; notify(title: string, body: string): Promise<void>; hide(): Promise<void> },
): Promise<void> {
  if (!port.isCurrent()) return;
  try {
    for (const effect of effects) {
      if (effect.kind === "tray-notify") await port.notify(effect.title, effect.body);
    }
  } finally {
    if (effects.some(effect => effect.kind === "hide-window") && port.isCurrent()) await port.hide();
  }
}

export const CONNECTED_TITLE = "EXV 已连接";
export const CONNECTED_BODY = "校园网连接已建立。";
export const DISCONNECTED_TITLE = "EXV 已断开";
export const DISCONNECTED_BODY = "校园网连接已断开。";

/** host 自愈失败的托盘气泡（2026-09-05 计划 §4.5 冻结；文案与 presenter 文案表一致）。 */
export const SELF_HEAL_FAILED_TITLE = "EXV VPN";
export const SELF_HEAL_FAILED_BODY =
  "引擎自动恢复失败，请重启应用后重新连接；若重复失败，请查看日志并反馈。";

function isConnected(status: ProductStatus): boolean {
  return status === "connected";
}

/** 通知上下文（调用方注入的前台状态；纯函数不自行探测窗口）。 */
export interface NotifyContext {
  /** 主窗口当前是否在前台（聚焦）。 */
  foreground: boolean;
  /**
   * 阶段④ N-02 互斥（2026-10-09 裁定 D9 #4，单向收敛）：当前失败 episode 是否
   * 已有声音（failed-notify 或 R1/R2 lane 终态 toast/系统通知）。true 时下方
   * `disconnect_notify` 托盘气泡让位——断开类 episode 后台时不再出现
   * 「EXV 已断开」+「连接已断开：<原因>」双通知；缺省 false（无失败 episode /
   * 未接线时气泡行为与既有完全一致）。
   */
  failureEpisodeNotified?: boolean;
  /** 当前过渡由实际自动恢复产生，普通连接/断开通知让位。 */
  automaticReconnect?: boolean;
}

/** 前台抑制判定：窗口在前台 且 抑制开关开启 → 抑制连接类通知（优先级最高）。 */
function notifyBlocked(prefs: UiPreferences, context: NotifyContext): boolean {
  return context.foreground && prefs.suppress_notify_when_foreground;
}

/**
 * 由前后两个产品状态计算过渡效果（无副作用；分发由接线层完成）。
 *
 * @param prevStatus 前一状态（`null` = 应用启动后的首快照）。
 * @param nextStatus 当前状态。
 * @param prefs 生效中的前端偏好。
 * @param context 通知上下文（前台状态）；缺省按「不在前台」处理（不触发抑制）。
 */
export function computeLifecycleEffects(
  prevStatus: ProductStatus | null,
  nextStatus: ProductStatus,
  prefs: UiPreferences,
  context: NotifyContext = { foreground: false },
): LifecycleEffect[] {
  const effects: LifecycleEffect[] = [];
  const firstSnapshot = prevStatus === null;
  const wasConnected = !firstSnapshot && isConnected(prevStatus);
  const nowConnected = isConnected(nextStatus);

  if (nowConnected && !wasConnected) {
    const willHideWindow = !firstSnapshot && prefs.minimize_to_tray_on_connect;
    if (willHideWindow) {
      effects.push({ kind: "hide-window" });
    }
    if (!firstSnapshot && !context.automaticReconnect && prefs.connect_notify && !notifyBlocked(prefs, context)) {
      effects.push({ kind: "tray-notify", title: CONNECTED_TITLE, body: CONNECTED_BODY });
    }
    return effects;
  }

  if (
    !nowConnected &&
    wasConnected &&
    prefs.disconnect_notify &&
    !context.automaticReconnect &&
    !notifyBlocked(prefs, context) &&
    // 阶段④ N-02 让位：失败 episode 已由 failed-notify / lane 终态携带原因句发声，
    // 本通用气泡不再叠加（同一 episode 一个声音）。
    !context.failureEpisodeNotified
  ) {
    effects.push({ kind: "tray-notify", title: DISCONNECTED_TITLE, body: DISCONNECTED_BODY });
  }
  return effects;
}

/**
 * 启动自动连接的门禁判定：首快照空闲 + pref 开启 → 自动发起一次连接。
 * （旧 webui 还检查 remember_password/serviceInstalled；Rust 侧前端拿不到这些
 * 数据口，v1 以 idle 为门禁，失败按常规 failed 态诚实呈现，不静默重试。）
 */
export function shouldAutoConnectOnLaunch(
  isFirstSnapshot: boolean,
  status: ProductStatus,
  prefs: UiPreferences,
): boolean {
  return isFirstSnapshot && status === "idle" && prefs.auto_connect_on_launch;
}

/**
 * host 自愈进展的托盘气泡（2026-09-05 计划 §4.5，冻结）：仅 stage 从**非 failed**
 * 进入 `failed` 时给一次气泡（`respawning`/`succeeded` 不弹气泡）；`failed` →
 * `failed` 不重复（prevStage 去重由调用方 `prevSelfHealStage` 承担，与 previousStatus
 * 同模式）。首见（prevStage = null）只记录基线，不弹气泡。
 *
 * 此处仅生成自愈失败事件；实际系统发送沿统一入口读取偏好与前台事实。
 */
export function computeSelfHealNotify(
  prevStage: string | null,
  nextStage: string | null,
): LifecycleEffect[] {
  if (prevStage === null || nextStage === null) return [];
  if (prevStage === "failed" || nextStage !== "failed") return [];
  return [{ kind: "tray-notify", title: SELF_HEAL_FAILED_TITLE, body: SELF_HEAL_FAILED_BODY }];
}

/** 本窗口已采纳状态的恢复来源；沿原操作归属保留触发事实至成功或用户取消。 */
export function createReconnectNotificationTracker() {
  let previous: ProductUiState | null = null;
  let hadConnectedSession = false;
  let cycle: { operationId: ProductUiState["operationId"]; completed: boolean } | null = null;
  let trigger: { operationId: ProductUiState["operationId"] } | null = null;
  return {
    observe(current: ProductUiState, engineRecoveryActive = false) {
      const first = previous === null;
      const active = current.reconnect.enabled && (current.reconnect.active || engineRecoveryActive);
      const cancelled = current.status === "stopping" || current.status === "idle"
        || (current.status === "connecting" && !current.stopEnabled);
      const newOperation = cycle !== null && current.operationId !== cycle.operationId;
      const newDeparture = previous?.status === "connected" && current.status !== "connected";
      // 宿主恢复从旧会话的失联快照切换到真实恢复操作，仍属于同一轮。
      if (newOperation && active && !cancelled && cycle && !cycle.completed) {
        cycle.operationId = current.operationId;
        if (trigger) trigger.operationId = current.operationId;
      }
      if (cancelled || (newOperation && (!active || cycle?.completed)) || (cycle?.completed && (newDeparture || active))) {
        cycle = null;
        trigger = null;
      }
      if (cancelled) hadConnectedSession = false;
      let reconnectTrigger: typeof trigger = null;
      if (active && !cycle && !cancelled) {
        cycle = { operationId: current.operationId, completed: false };
        if (!first && hadConnectedSession) {
          trigger = { operationId: current.operationId };
          reconnectTrigger = trigger;
        }
      }
      const automaticReconnect = current.reconnect.enabled && (
        (cycle !== null && !cycle.completed)
        || (newDeparture && (current.status === "failed" || current.status === "reconciling"))
      );
      if (current.status === "connected") {
        hadConnectedSession = true;
        if (cycle && !active) cycle.completed = true;
      }
      previous = current;
      return { automaticReconnect, reconnectTrigger };
    },
    isCurrent(observed: { operationId: ProductUiState["operationId"] }, current: ProductUiState) {
      return observed === trigger && current.reconnect.enabled
        && observed.operationId === current.operationId
        && current.status !== "stopping" && current.status !== "idle";
    },
  };
}

export const RECONNECTING_TITLE = "EXV 正在重连";
export const RECONNECTING_BODY = "连接中断，正在自动重连。";

/**
 * S5b：重连暂停稳定 reason id → 应用内提示文案。仅映射暂停事实；其余 reason id
 * （R2 恢复事件类）返回 null，由各自消费路径处理。
 */
export function reconnectPauseCopyOf(reason: string): string | null {
  if (reason === "reconnect_paused_owner_mismatch") {
    return "已保存账户与当前连接不一致，自动重连已暂停。";
  }
  if (reason === "reconnect_paused_credentials_cleared") {
    return "保存的密码已清除，自动重连已暂停。";
  }
  return null;
}

/** 所有系统通知在原发送入口使用同一个无状态门控。 */
export type SystemNotificationKind = "connected" | "disconnected" | "reconnecting"
  | "unexpected-disconnect" | "failed" | "self-heal-failed";
export async function dispatchSystemNotification(
  kind: SystemNotificationKind,
  title: string,
  body: string,
  port: {
    readPreferences(): UiPreferences;
    isForeground(): Promise<boolean>;
    isCurrent(): boolean;
    send(title: string, body: string): Promise<void>;
  },
): Promise<void> {
  let foreground = false;
  // 抑制关闭时无须读取窗口；开启时探测错误保持原始拒绝，由原入口记录。
  if (port.readPreferences().suppress_notify_when_foreground) {
    try {
      foreground = await port.isForeground();
    } catch (error) {
      if (port.readPreferences().suppress_notify_when_foreground) throw error;
    }
  }
  const prefs = port.readPreferences();
  if (!port.isCurrent() || (foreground && prefs.suppress_notify_when_foreground)) return;
  const enabled = kind === "connected" ? prefs.connect_notify
    : kind === "disconnected" ? prefs.disconnect_notify
    : kind === "reconnecting" ? prefs.reconnect_notify
    : kind === "unexpected-disconnect" ? prefs.unexpected_disconnect_notify : true;
  if (enabled) await port.send(title, body);
}
