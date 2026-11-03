import type { ProductUiState } from "./types";
import type { ConnectPhase } from "../lib/ipc";
import { CONNECT_PHASE_ORDER, phaseIndexOf } from "../lib/status-map";

export type ConnectionProgressView =
  | { expanded: boolean; mode: "complete"; percent: 100; label: string }
  | { expanded: boolean; mode: "indeterminate"; label: string };

/** 只读呈现：阶段的 complete 是顺序投影，不能据此计算确定比例。 */
export function presentConnectionProgress(state: ProductUiState): ConnectionProgressView {
  if (state.status === "connected") {
    return { expanded: false, mode: "complete", percent: 100, label: "连接完成" };
  }
  const expanded = state.status === "connecting" || state.status === "awaiting";
  return {
    expanded,
    mode: "indeterminate",
    label: state.status === "awaiting" ? "等待账户确认，进度未定" : "连接进度未定",
  };
}

export type FlowConnectionProgressView = ConnectionProgressView | {
  expanded: true;
  mode: "flow";
  phase: ConnectPhase;
  percent: number;
  cap: number;
  label: string;
};

/** 主窗口与极简共用的内部阶段锚点；只表达流程位置，不统计完成工作量。 */
export function presentFlowConnectionProgress(state: ProductUiState): FlowConnectionProgressView {
  const base = presentConnectionProgress(state);
  if (!base.expanded) return base;
  const current = state.stages.filter(stage => stage.visual === "current");
  const index = current.length === 1 ? phaseIndexOf(current[0].phase) : -1;
  if (index < 0) return base;
  const count = CONNECT_PHASE_ORDER.length;
  return {
    expanded: true,
    mode: "flow",
    phase: current[0].phase,
    percent: Math.min(99, Math.round(index / count * 100)),
    cap: index === count - 1 ? 99 : Math.round((index + 1) / count * 100) - 1,
    label: state.status === "awaiting" ? "等待账户确认" : "",
  };
}

/** 每250ms前台活动tick按剩差渐缓；不代表后台实际工作量。 */
export function advanceFlowProgress(value: number, cap: number): number {
  return Math.min(cap, value + (cap - value) * .035);
}
