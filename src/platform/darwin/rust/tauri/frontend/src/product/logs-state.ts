// 日志页跨页面切换的模块级状态（内存持久化 + 懒加载/增量）。
//
// 要求（用户明确）：日志遵从懒加载——首次打开一次性全量加载；后续打开向 core 传回
// 前端日志尾（游标），core 只回增量。模块级状态让日志在页面切走再切回时保留，且
// 只做增量拉取。

import { ref } from "vue";

import type { LogEvent } from "../lib/ipc";

/** 日志条目（有界，保留最近 2000 条）。 */
export const entries = ref<LogEvent[]>([]);
/** 前端日志尾游标（最后一次 `logsList` 的 `next_after_seq`；增量加载起点）。 */
export const logCursor = ref(0);
/** 本会话是否已加载过（首次全量；后续只取增量）。 */
export const loadedOnce = ref(false);
export const loading = ref(false);
export const error = ref(false);
/** null 表示全部等级（包括后来出现的新等级）；空数组表示没有选择。 */
export const selectedLevels = ref<string[] | null>(null);
export const searchQuery = ref("");
/** 浏览状态与日志缓存同寿命，切页或切换窗口模式不会重置。 */
export const followLatest = ref(true);
export const expandedLogKeys = ref(new Set<string>());
export const logViewport = ref({ top: 0, left: 0, anchorKey: null as string | null, anchorOffset: 0 });
export const LOG_WINDOW_LIMIT = 2_000;

/** 事件去重键（增量与实时订阅合并时的重复保护）。 */
export function logEventKey(entry: LogEvent): string {
  return JSON.stringify([
    entry.timestamp_ms,
    entry.level,
    entry.component,
    entry.code,
    entry.message,
    Object.entries(entry.fields).sort(([left], [right]) => left.localeCompare(right)),
  ]);
}

/**
 * 兼容旧持久化文件中的诊断噪声记录。
 *
 * engine 已不再产生这些业务无关的心跳/计数 dump，但用户可能仍会看到清理前的历史
 * 记录；前端在历史、事件和轮询三条入口统一屏蔽，避免诊断噪声重新呈现出来。
 */
const SUPPRESSED_LOG_PATTERNS = [
  /keepalive\s+heartbeat\s+received(?:\s*\(diagnostic\))?/i,
  /data\s+plane\s+ring\s+counters(?:\s*\(diagnostic\))?/i,
];

export function isVisibleLogEvent(entry: LogEvent): boolean {
  const searchable = [
    entry.level,
    entry.component,
    entry.code,
    entry.message,
    ...Object.entries(entry.fields).flat(),
  ].join(" ");
  return !SUPPRESSED_LOG_PATTERNS.some((pattern) => pattern.test(searchable));
}

/** 测试重置（生产路径不调用）。 */
export function resetLogsState(): void {
  entries.value = [];
  logCursor.value = 0;
  loadedOnce.value = false;
  loading.value = false;
  error.value = false;
  selectedLevels.value = null;
  searchQuery.value = "";
  followLatest.value = true;
  expandedLogKeys.value = new Set();
  logViewport.value = { top: 0, left: 0, anchorKey: null, anchorOffset: 0 };
}
