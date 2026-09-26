// 预卸载（darwin 单侧）：设置页「危险操作」区的调用与状态层。
//
// 边界：本模块只负责「调用壳命令 + 规范化返回结果 + 暴露给组件」；不含任何删除逻辑
// （删除在壳侧白名单删除器与 core 提权段内完成，前端无从绕过）。
//
// 结果判定契约：壳命令返回**分项结果**，每项状态由删除尝试之后的后置核对得出。
// 前端不做二次推断（不得把 failed 显示成成功、不得把 skipped 吞掉）。

import { darwinCommandAdapter } from "./command-adapter-global";

/** 单项状态（与壳侧 `ItemOutcome.status` 逐字对应）。 */
export type PreUninstallItemStatus = "removed" | "absent" | "skipped" | "failed" | "not_run";

export interface PreUninstallItem {
  label: string;
  status: PreUninstallItemStatus;
  detail: string;
}

export interface PreUninstallReply {
  items: PreUninstallItem[];
  /** 需要用户手动处理的项（目前仅"应用本体删不掉"）。 */
  manual_actions: string[];
  /** 提权段是否整体未运行（用户取消密码 / 无管理员凭据）。 */
  elevation_skipped: boolean;
  /** 是否因"当前连接无法断开"而中止（中止时除断开一项外未执行任何删除）。 */
  aborted: boolean;
}

/** 状态码 → 中文标签（用于逐项结果呈现）。 */
export const PRE_UNINSTALL_STATUS_LABELS: Readonly<Record<PreUninstallItemStatus, string>> = {
  removed: "已删除",
  // "未扫描到"而非"本就不存在"：本机确实没有该产物，与"扫描过但没找到"是同一事实的
  // 两种说法，前者更贴近卸载器的实际动作（扫描 → 未命中）。
  absent: "未扫描到",
  skipped: "已跳过",
  failed: "失败",
  not_run: "未运行",
};

function isStatus(value: unknown): value is PreUninstallItemStatus {
  return (
    value === "removed" ||
    value === "absent" ||
    value === "skipped" ||
    value === "failed" ||
    value === "not_run"
  );
}

/**
 * 规范化壳命令返回：形状不符或出现未知状态码时**如实报错**，不猜测、不降级成成功。
 */
export function normalizePreUninstallReply(raw: unknown): PreUninstallReply {
  if (typeof raw !== "object" || raw === null) throw new Error("预卸载返回结果形状非法");
  const candidate = raw as Record<string, unknown>;
  const rawItems = candidate.items;
  if (!Array.isArray(rawItems)) throw new Error("预卸载返回结果缺少逐项列表");
  const items: PreUninstallItem[] = rawItems.map((entry) => {
    if (typeof entry !== "object" || entry === null) throw new Error("预卸载逐项结果形状非法");
    const item = entry as Record<string, unknown>;
    if (typeof item.label !== "string" || !isStatus(item.status)) {
      throw new Error("预卸载逐项结果含未知状态码");
    }
    return {
      label: item.label,
      status: item.status,
      detail: typeof item.detail === "string" ? item.detail : "",
    };
  });
  const manual = candidate.manual_actions;
  return {
    items,
    manual_actions: Array.isArray(manual)
      ? manual.filter((value): value is string => typeof value === "string")
      : [],
    elevation_skipped: candidate.elevation_skipped === true,
    aborted: candidate.aborted === true,
  };
}

/** 调用壳命令执行预卸载（不退出应用；退出由调用方在展示结果之后决定）。 */
export async function runPreUninstall(): Promise<PreUninstallReply> {
  const adapter = darwinCommandAdapter();
  if (!adapter) throw new Error("当前不在 EXV 应用内，无法执行卸载");
  return normalizePreUninstallReply(await adapter.call("pre_uninstall"));
}

/**
 * 退出应用（卸载收尾）。
 *
 * 走壳侧 `pre_uninstall_quit`（复用壳的唯一退出入口），**不使用 `chrome.control("close")`**：
 * 后者受 `close_preference` 驱动，用户若设为"最小化到托盘"，卸载后进程仍在——与
 * "卸载完成"的语义矛盾。
 */
export async function quitAfterPreUninstall(): Promise<void> {
  const adapter = darwinCommandAdapter();
  if (!adapter) return;
  await adapter.call("pre_uninstall_quit");
}

/** 汇总：是否存在需要人工处理的项（用于决定是否显示"手动处理"区块）。 */
export function hasManualActions(reply: PreUninstallReply): boolean {
  return reply.manual_actions.length > 0;
}
