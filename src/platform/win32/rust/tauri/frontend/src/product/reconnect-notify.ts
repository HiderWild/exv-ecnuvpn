// T12（计划 §2.8 / §2.9 缺口③）通知出口：R1/R2 重连事件 → 应用内 toast。
//
// 事件源（全部复用既有 wire，不新增 proto 字段）：
//   * R2（core 侧恢复，T6b）：`RuntimeSnapshot.reconnect.reason` 稳定 id
//     （engine_lost_service / engine_lost_oneshot / engine_lost_oneshot_gated_off）
//     随 Transition 快照到达（EOF 分类发布/受理发布）；
//   * R1（engine 侧重连，T6a）：`RuntimeSnapshot.engine_reconnect.reason` 稳定 id
//     （tunnel_lost / healed / heal_assembly_failed / heal_timeout / auth_rejected /
//     attempts_exhausted）经 engine `ObserveOwnedState` 快照 → core `GetSnapshot`
//     源化偏好到达（连接存续期 1s 轮询承载）。
// 两条 lane 都在快照上，入口是产品运行时的快照采纳钩子（事件与轮询同一点）。
//
// 门控（§2.8「自动重连时发送通知」，config `reconnect_notify`，默认开）：
//   * 过程类（重连触发/恢复中/恢复成功）受门控压制；
//   * 终态类（认证失效/重连耗尽/oneshot 崩溃断开/崩溃环）不受压制——设置项语义是
//     「自动重连**触发时**发送通知」（重连过程提示），终态是用户必须知道的断开事实，
//     与既有「断开连接时发送通知」同一语义级别。
//   * 门控值经既有 config 链（`configGet`）读取：启动时读一次 + 每次事件判定时后台
//     best-effort 刷新（缓存值即时生效）。会话冻结语义同 T4：信封冻结值不可回查，
//     以当前配置值为准（设置行本身仅在主开关开启时可改，见 SettingsPage 门控）。
//
// 渲染：右下角一次性 toast（`lib/toast`，自动消失），层级低于 T9 崩溃横幅与模态
// （模态 > 横幅 > toast），互不抢占。

import { pushToast, type ToastKind } from "../lib/toast";
import type { ConfigPayload, RuntimeSnapshot } from "../lib/ipc";

/** 单条重连通知文案（生产数据：稳定 reason id → 用户可读文案，集中一处）。 */
export interface ReconnectNotifyCopy {
  /** 是否弹 toast（false = 过程延续 id：只推进游标，不重复打扰）。 */
  notify: boolean;
  /** 终态类（true = 不受 reconnect_notify 压制；false = 过程类，受门控）。 */
  terminal: boolean;
  /** toast 类别（颜色语义，`lib/toast`）。 */
  kind: ToastKind;
  /** 用户可读文案（中文；gated_off 遵守 D13 裁定句式，不含 UAC 等级句）。 */
  message: string;
}

/**
 * 稳定 reason id → 文案表（生产数据）。未知 id 由 [`unknownReconnectCopy`] 兜底。
 *
 * `crash_loop`（engine 崩溃环终态）今日无实时到达路径：engine 发布 Idle 终态事件时
 * core 已死（无转发者），事后摘要查询不在本计划 wire 内——T9 core-lifecycle 崩溃
 * 横幅是该症状的既有呈现。文案先备：终态摘要查询落地后按 id 零改动接线。
 */
export const RECONNECT_NOTIFY_COPY: Readonly<Record<string, ReconnectNotifyCopy>> = {
  // ---- R1（engine 侧隧道自愈；id 契约 = win32 engine reconnect.rs）----
  tunnel_lost: {
    notify: true,
    terminal: false,
    kind: "warning",
    message: "隧道中断，正在自动重连…",
  },
  heal_assembly_failed: {
    notify: false,
    terminal: false,
    kind: "warning",
    message: "自愈组装失败，按退避继续重试。",
  },
  heal_timeout: {
    notify: false,
    terminal: false,
    kind: "warning",
    message: "自愈组装超时，按退避继续重试。",
  },
  healed: {
    notify: true,
    terminal: false,
    kind: "success",
    message: "连接已自动恢复。",
  },
  auth_rejected: {
    notify: true,
    terminal: true,
    kind: "error",
    message: "登录凭据已失效，自动重连已停止；请重新连接。",
  },
  attempts_exhausted: {
    notify: true,
    terminal: true,
    kind: "error",
    message: "自动重连次数已用完，连接已断开；请重新连接。",
  },
  // ---- 崩溃环终态（engine 善后收口；见模块文档——今日无实时 wire 路径）----
  crash_loop: {
    notify: true,
    terminal: true,
    kind: "error",
    message: "后台核心反复崩溃，已停止自动恢复；连接已断开并完成清理。",
  },
  // ---- R2（core 侧 engine 失联恢复；id 契约 = win32 host R2Reason）----
  engine_lost_service: {
    notify: true,
    terminal: false,
    kind: "warning",
    message: "后台服务连接中断，正在尝试恢复连接…",
  },
  engine_lost_oneshot: {
    notify: true,
    terminal: false,
    kind: "warning",
    message: "后台引擎连接中断，正在重新提权恢复连接（会重新弹 UAC）…",
  },
  engine_lost_oneshot_gated_off: {
    notify: true,
    terminal: true,
    kind: "error",
    message: "后台引擎已退出，连接已断开。重新连接会重新弹 UAC；若打扰请改用安装服务形态。",
  },
};

/** 未知 reason id 的兜底文案（fail-safe：通用变化提示 + 原始 id 便于查日志）。 */
export function unknownReconnectCopy(id: string): ReconnectNotifyCopy {
  return {
    notify: true,
    // 未知 id 的类别不可判定：按过程类处理（受门控压制，避免不可关的未知噪声）。
    terminal: false,
    kind: "warning",
    message: `连接状态发生变化（${id}）；详情请查看日志。`,
  };
}

/** 判定输入：两条 lane 的当前 reason id（空/缺省归一为 null）。 */
export interface ReconnectNotifyCursor {
  /** R1 lane：`engine_reconnect.reason`（null = 无 R1 语义）。 */
  r1: string | null;
  /** R2 lane：`reconnect.reason`（null = 无 R2 事件）。 */
  r2: string | null;
  /** 原恢复已结束时，消费真实连接／失败终态。 */
  r2Result?: "connected" | "failed";
}

/** 一次判定的产出：待弹的 toast（空数组 = 无通知）。 */
export interface ReconnectToastDecision {
  kind: ToastKind;
  message: string;
}

/** 归一化 reason 字段：空串/空白 = null（与 proto3 缺省语义一致）。 */
function normalizeReason(raw: string | null | undefined): string | null {
  const trimmed = raw?.trim();
  return trimmed ? trimmed : null;
}

/** 实际 R2 恢复结束并进入失败；终态 reason 可由 Core 正常清空。 */
function r2Failed(prev: ReconnectNotifyCursor | null, next: ReconnectNotifyCursor): boolean {
  return prev !== null && prev.r2 !== null && prev.r2 !== "engine_lost_oneshot_gated_off"
    && next.r2 === null && next.r2Result === "failed";
}

/**
 * 纯判定：由前后两个游标计算待弹通知（生产逻辑；无副作用，测试直接覆盖）。
 *
 * 规则：
 *   * `prev === null` = 首次观察：只记录基线，不弹（对齐生命周期效果的首快照去抖；
 *     应用启动时已在重连中，由状态区呈现，不补弹历史）。
 *   * lane 内 id 不变 → 不弹（engine 侧 lane 反映最近一次状态事件，连接存续期会
 *     持续携带同一 id——去重必须按「变化」而非「出现」）。
 *   * id 变为 null → 只清游标（恢复完成/引擎换装）。
 *   * `notify: false` 的延续 id（heal_assembly_failed / heal_timeout）→ 不弹：
 *     退避重试是同一 episode 的延续，逐次弹会刷屏。
 *   * `healed` 仅在前一观察已有 R1 游标时弹（首见 healed = 基线，不弹）。
 *   * 过程类受 `notifyEnabled` 压制；终态类恒弹。
 */
export function decideReconnectToasts(
  prev: ReconnectNotifyCursor | null,
  next: ReconnectNotifyCursor,
  notifyEnabled: boolean,
): ReconnectToastDecision[] {
  if (prev === null) return [];
  const decisions: ReconnectToastDecision[] = [];

  if (prev.r2 !== null && next.r2 === null && prev.r2 !== "engine_lost_oneshot_gated_off") {
    if (next.r2Result === "connected" && notifyEnabled) {
      decisions.push({ kind: "success", message: "连接已自动恢复。" });
    } else if (r2Failed(prev, next)) {
      decisions.push({ kind: "error", message: "自动恢复连接失败，请重新连接；仍失败请导出日志反馈。" });
    }
  }

  // R1 lane（engine 自愈）。
  if (next.r1 !== prev.r1 && next.r1 !== null) {
    const copy = RECONNECT_NOTIFY_COPY[next.r1] ?? unknownReconnectCopy(next.r1);
    if (copy.notify && (next.r1 !== "healed" || prev.r1 !== null)) {
      const suppressed = !copy.terminal && !notifyEnabled;
      if (!suppressed) {
        decisions.push({ kind: copy.kind, message: copy.message });
      }
    }
  }

  // R2 lane（core 侧恢复）：进入/换 id 即弹一次。
  if (next.r2 !== prev.r2 && next.r2 !== null) {
    const copy = RECONNECT_NOTIFY_COPY[next.r2] ?? unknownReconnectCopy(next.r2);
    if (copy.notify) {
      const suppressed = !copy.terminal && !notifyEnabled;
      if (!suppressed) {
        decisions.push({ kind: copy.kind, message: copy.message });
      }
    }
  }

  return decisions;
}

// ---- 模块级单例（同 ui-transient / core-crash 模式；App 壳接线，页面不接触）----

/** 游标（null = 尚无首观察——首观察只建基线）。 */
let cursor: ReconnectNotifyCursor | null = null;
/** 门控缓存（T4 默认开；设置链读取后更新）。 */
let reconnectNotifyEnabled = true;
/** 门控读取源（init 注入；null = 未初始化，保持默认值）。 */
let gateLoader: (() => Promise<ConfigPayload>) | null = null;
/** 门控后台刷新在途标记（防重入）。 */
let gateRefreshInFlight = false;
/**
 * 阶段④ failed-notify 互斥游标（2026-10-09 裁定 D9 #4：互斥收敛**单向**——
 * failed-notify 让位于 lane 终态；lane 终态恒弹是既有语义，不做反向抑制）：
 * 当前 episode 内 R1/R2 是否已弹过终态通知。reason 终态先到、Failed 后到时
 * 沿原 lane id 关联；R2 的无 reason 失败终态也沿此位消费。失败家族内保持，
 * 离开失败家族且 lane 已离开终态 id 时重新武装。
 */
let laneTerminalEpisodeOpen = false;

/** 从快照提取两条 lane 的 reason id。 */
export function reconnectCursorOf(snapshot: RuntimeSnapshot): ReconnectNotifyCursor {
  const reason = normalizeReason(snapshot.reconnect?.reason);
  const recovering = snapshot.reconnect?.auto_reconnect === true && snapshot.reconnect.active;
  const runtime = snapshot.runtime;
  const state = runtime.state;
  const failed = runtime.state === "failed_clean" || runtime.state === "failed_dirty";
  // 本地用户取消由原 Host 普通 toast 呈现，仍沿原游标结束本次恢复。
  const authCancelled = failed
    && runtime.error?.code === "ERROR_CODE_CANCELLED_BEFORE_START"
    && runtime.error.reason === "atrust/auth_cancelled";
  return {
    r1: normalizeReason(snapshot.engine_reconnect?.reason),
    // reason表示失联来源；只有实际恢复active才承诺正在恢复。
    r2: recovering || reason === "engine_lost_oneshot_gated_off" ? reason : null,
    ...(state === "connected" ? { r2Result: "connected" as const } :
      failed && !authCancelled ? { r2Result: "failed" as const } : {}),
  };
}

/**
 * 初始化门控读取源（main.ts 启动调用；测试注入内存 loader）。
 *
 * **不做立即读取**（2026-09-28 复审 F1 / 计划 v6.1 第一批）：入口层 eager
 * ConfigGet 会与 App 的快速入门首读竞态——core 侧 config_get 在配置缺失时当场
 * bootstrap 落盘且仅该次返回 requires=true，入口层抢先读取会把干净机器的首次
 * 判定永久翻成「已配置」（2026-09-14 修过的同类启动顺序竞态在 T12 入口层重
 * 开）。门控缓存保持默认值（开），首次刷新由 lane 变化触发；首个过程类 toast
 * 判定前首观察只建基线不弹，门届时已完成刷新（终态类恒不受门控）。
 */
export function initReconnectNotifyGate(loader: () => Promise<ConfigPayload>): void {
  gateLoader = loader;
}

/** 后台 best-effort 刷新门控缓存（config `reconnect_notify`；缺省 = 开）。 */
async function refreshReconnectNotifyGate(): Promise<void> {
  if (gateRefreshInFlight || gateLoader === null) return;
  gateRefreshInFlight = true;
  try {
    const payload = await gateLoader();
    const item = payload.items.find((entry) => entry.key === "reconnect_notify");
    if (item) {
      reconnectNotifyEnabled = item.value.trim() !== "false";
    }
  } catch {
    // 读取失败沿用缓存值（默认开）；下一次判定再试。
  } finally {
    gateRefreshInFlight = false;
  }
}

/** 测试/接线注入门控当前值（生产路径经 config 链；直接注入用于确定性测试）。 */
export function setReconnectNotifyGate(enabled: boolean): void {
  reconnectNotifyEnabled = enabled;
}

/**
 * 快照采纳钩子（产品运行时的唯一喂入口——事件与 GetSnapshot 轮询都汇到快照采纳）。
 * 判定 → 弹 toast；lane 游标变化时后台刷新门控（罕见路径，成本可忽略）。
 * 终态类 id 实际弹出时置 lane 终态 episode 游标（供 failed-notify 单向让位查询）。
 */
export function announceReconnectSnapshot(snapshot: RuntimeSnapshot, options: { lifecycleOwnsProgress?: boolean } = {}): void {
  const next = reconnectCursorOf(snapshot);
  const decisions = decideReconnectToasts(cursor, next, reconnectNotifyEnabled);
  const r2FailureNotified = r2Failed(cursor, next) && decisions.length > 0;
  const laneChanged =
    cursor === null || cursor.r1 !== next.r1 || cursor.r2 !== next.r2;
  cursor = next;
  if (laneChanged) {
    void refreshReconnectNotifyGate();
  }
  for (const decision of decisions) {
    if (!options.lifecycleOwnsProgress || decision.kind !== "warning") pushToast(decision.message, decision.kind);
  }
  // reason 终态或 R2 实际失败已发声时置位；失败家族内保持，避免 reason 清空
  // 导致紧随其后的 failed-notify 再次通知。离开失败且无终态 lane 时重新武装。
  const isTerminalId = (id: string | null): boolean =>
    id !== null && (RECONNECT_NOTIFY_COPY[id]?.terminal ?? false);
  if (r2FailureNotified || (decisions.length > 0 && (isTerminalId(next.r1) || isTerminalId(next.r2)))) {
    laneTerminalEpisodeOpen = true;
  } else if (!isTerminalId(next.r1) && !isTerminalId(next.r2) && next.r2Result !== "failed") {
    laneTerminalEpisodeOpen = false;
  }
}

/**
 * 阶段④ failed-notify 互斥查询（只读）：当前 episode 内 lane 终态是否已弹过
 * 通知——true 时 failed-notify 对同一 episode 不再弹通用失败 toast（让位于携带
 * 具体原因句的 lane 终态 toast；同一 episode 一个声音）。
 */
export function laneTerminalNotifiedInEpisode(): boolean {
  return laneTerminalEpisodeOpen;
}

/** 测试隔离：复位模块状态（生产路径不调用）。 */
export function resetReconnectNotifyState(): void {
  cursor = null;
  reconnectNotifyEnabled = true;
  gateLoader = null;
  gateRefreshInFlight = false;
  laneTerminalEpisodeOpen = false;
}
