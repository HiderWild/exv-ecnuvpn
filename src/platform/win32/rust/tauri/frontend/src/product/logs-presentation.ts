import type { LogEvent } from "../lib/ipc";

/** JSONL 保留 wire 全字段；换行消息被 JSON 转义，每条记录只占一行。 */
export function serializeLogs(entries: ReadonlyArray<LogEvent>): string {
  return entries.map((entry) => JSON.stringify(entry)).join("\n");
}

export function matchesLogSearch(entry: LogEvent, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  return !needle || [entry.message, entry.code, entry.component, ...Object.entries(entry.fields).flat()]
    .some((value) => value.toLocaleLowerCase().includes(needle));
}

export function preciseLogTime(timestampMs: number): string {
  if (!timestampMs || !Number.isFinite(timestampMs)) return "时间未知";
  const date = new Date(timestampMs);
  return Number.isNaN(date.getTime()) ? "时间未知" : date.toISOString();
}

/** 优先采用 WebView Clipboard API；兼容缺少该 API 的宿主并核实回退结果。 */
export async function copyLogText(text: string): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // 部分 WebView 禁用异步剪贴板，继续尝试宿主提供的编辑复制命令。
  }
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const selection = window.getSelection();
  const ranges = selection ? Array.from({ length: selection.rangeCount }, (_, index) => selection.getRangeAt(index).cloneRange()) : [];
  const field = document.createElement("textarea");
  field.value = text;
  field.readOnly = true;
  field.setAttribute("aria-label", "待复制日志");
  field.style.cssText = "position:fixed;left:-10000px;top:0;width:1px;height:1px;opacity:0;";
  document.body.appendChild(field);
  try {
    field.focus({ preventScroll: true });
    field.select();
    if (typeof document.execCommand !== "function" || !document.execCommand("copy")) {
      throw new Error("clipboard write failed");
    }
  } finally {
    field.remove();
    focused?.focus({ preventScroll: true });
    if (selection) {
      selection.removeAllRanges();
      ranges.forEach((range) => selection.addRange(range));
    }
  }
}
