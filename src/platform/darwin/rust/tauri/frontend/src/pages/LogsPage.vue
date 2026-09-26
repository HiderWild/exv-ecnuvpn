<script setup lang="ts">
import { computed, inject, nextTick, onBeforeUnmount, onMounted, onUnmounted, ref, useId, watch } from "vue";

import type { LogEvent } from "../lib/ipc";
import { LOGS_GATEWAY_KEY, createLogsGateway, type LogsGateway } from "../product/logs";
import {
  entries,
  error,
  expandedLogKeys,
  followLatest,
  isVisibleLogEvent,
  loadedOnce,
  loading,
  logCursor,
  logEventKey,
  logViewport,
  LOG_WINDOW_LIMIT,
  searchQuery,
  selectedLevels,
} from "../product/logs-state";
import { copyLogText, matchesLogSearch, preciseLogTime, serializeLogs } from "../product/logs-presentation";
import { exportLogFile } from "../product/logs-export";

const BASE_LOG_LEVELS = ["debug", "info", "warn", "error"] as const;

const props = defineProps<{
  gateway?: LogsGateway;
}>();

const injectedGateway = inject(LOGS_GATEWAY_KEY, null);
const gateway = props.gateway ?? injectedGateway ?? createLogsGateway();
const clearing = ref(false);
const clearError = ref(false);
const copying = ref(false);
const copyStatus = ref("");
const copyFailed = ref(false);
const exporting = ref(false);
const exportStatus = ref("");
const exportFailed = ref(false);
const levelFilterOpen = ref(false);
const levelFilterRef = ref<HTMLElement | null>(null);
const levelButtonRef = ref<HTMLButtonElement | null>(null);
const levelPanelRef = ref<HTMLElement | null>(null);
const levelPanelId = `logs-levels-${useId()}`;
const bodyRef = ref<HTMLElement | null>(null);
let unlisten: (() => void) | null = null;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let disposed = false;
let scrollRequestGeneration = 0;
let pointerScrollInProgress = false;
let lastTouchY: number | null = null;

const filteredEntries = computed(() =>
  entries.value.filter((entry) => isLevelSelected(entry.level.trim()) && matchesLogSearch(entry, searchQuery.value)),
);
const levelSummary = computed(() => selectedLevels.value === null ? "全部" : selectedLevels.value.length === 0 ? "未选择" : selectedLevels.value.join("、"));

/** 初次无数据时才用加载提示替换表格；后台增量轮询必须保留既有日志节点。 */
const isInitialLoading = computed(() => loading.value && !loadedOnce.value && entries.value.length === 0);

const availableLevels = computed(() => {
  const levels = new Set<string>(BASE_LOG_LEVELS);
  for (const entry of entries.value) {
    const level = entry.level.trim();
    if (level) levels.add(level);
  }
  return [...levels];
});

function formatTime(timestampMs: number): string {
  if (preciseLogTime(timestampMs) === "时间未知") return "—";
  return new Date(timestampMs).toLocaleTimeString("zh-CN", {
    hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit", fractionalSecondDigits: 3,
  });
}

function isLevelSelected(level: string): boolean {
  return selectedLevels.value === null || selectedLevels.value.includes(level);
}

function toggleLevel(level: string): void {
  const next = new Set(selectedLevels.value ?? availableLevels.value);
  if (next.has(level)) next.delete(level);
  else next.add(level);
  selectedLevels.value = availableLevels.value.every((candidate) => next.has(candidate)) ? null : [...next];
}

function closeLevelFilter(restoreFocus = false): void {
  if (!levelFilterOpen.value) return;
  levelFilterOpen.value = false;
  if (restoreFocus) void nextTick(() => levelButtonRef.value?.focus());
}

function toggleLevelFilter(): void {
  if (levelFilterOpen.value) closeLevelFilter(true);
  else {
    levelFilterOpen.value = true;
    void nextTick(() => levelPanelRef.value?.querySelector<HTMLInputElement>('input[type="checkbox"]')?.focus());
  }
}

function handleOutsidePointer(event: Event): void {
  if (event.target instanceof Node && !levelFilterRef.value?.contains(event.target)) closeLevelFilter();
}

function handleEscape(event: KeyboardEvent): void {
  if (levelFilterOpen.value && event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closeLevelFilter(true);
  }
}

function handleFilterFocusOut(event: FocusEvent): void {
  if (event.relatedTarget instanceof Node && !levelFilterRef.value?.contains(event.relatedTarget)) closeLevelFilter();
}

async function copyFilteredLogs(): Promise<void> {
  if (copying.value || filteredEntries.value.length === 0) return;
  const snapshot = serializeLogs(filteredEntries.value);
  const count = filteredEntries.value.length;
  copying.value = true;
  exportStatus.value = "";
  copyStatus.value = "";
  copyFailed.value = false;
  try {
    await copyLogText(snapshot);
    copyStatus.value = `已复制 ${count} 条日志`;
  } catch {
    copyFailed.value = true;
    copyStatus.value = "复制失败，请重试。";
  } finally {
    copying.value = false;
  }
}

async function exportFilteredLogs(): Promise<void> {
  if (exporting.value || filteredEntries.value.length === 0) return;
  const contents = serializeLogs(filteredEntries.value) + "\n";
  const count = filteredEntries.value.length;
  exporting.value = true;
  copyStatus.value = "";
  exportStatus.value = "";
  exportFailed.value = false;
  try {
    const result = await exportLogFile(contents);
    if (result === "saved") exportStatus.value = `已导出 ${count} 条日志`;
    else if (result === "download") exportStatus.value = `已发起 ${count} 条日志的下载`;
  } catch (cause) {
    exportFailed.value = true;
    const reason = cause instanceof Error ? cause.message : typeof cause === "string" ? cause : "请重试";
    exportStatus.value = `导出失败：${reason}`;
  } finally {
    exporting.value = false;
  }
}

function isAtLatest(element: HTMLElement): boolean {
  return element.scrollHeight - element.clientHeight - element.scrollTop <= 8;
}

function pauseFollowLatest(): void {
  followLatest.value = false;
  rememberViewport();
  // 失效已经排队的 nextTick 追尾，避免展开详情后又被旧任务拉回底部。
  scrollRequestGeneration += 1;
}

function resumeFollowLatest(): void {
  followLatest.value = true;
  scrollToLatest();
}

function isEditableTarget(target: EventTarget | null): boolean {
  return target instanceof Element
    && target.closest("input, textarea, select, [contenteditable='true']") !== null;
}

function handleDirectedUserScroll(direction: number, target: EventTarget | null): void {
  if (direction === 0 || isEditableTarget(target)) return;
  const body = bodyRef.value;
  if (body === null) return;
  if (direction > 0 && isAtLatest(body)) {
    resumeFollowLatest();
    return;
  }
  pauseFollowLatest();
}

function handleWheel(event: WheelEvent): void {
  handleDirectedUserScroll(event.deltaY, event.target);
}

function handleBodyKeydown(event: KeyboardEvent): void {
  const direction = event.key === "ArrowDown" || event.key === "PageDown" || event.key === "End"
    ? 1
    : event.key === "ArrowUp" || event.key === "PageUp" || event.key === "Home"
      ? -1
      : 0;
  handleDirectedUserScroll(direction, event.target);
}

function handleTouchStart(event: TouchEvent): void {
  if (isEditableTarget(event.target)) return;
  lastTouchY = event.touches[0]?.clientY ?? null;
}

function handleTouchMove(event: TouchEvent): void {
  if (isEditableTarget(event.target)) return;
  const currentY = event.touches[0]?.clientY;
  if (currentY === undefined || lastTouchY === null) {
    lastTouchY = currentY ?? null;
    return;
  }
  handleDirectedUserScroll(lastTouchY - currentY, event.target);
  lastTouchY = currentY;
}

function handleTouchEnd(): void {
  lastTouchY = null;
}

function handleBodyPointerDown(event: PointerEvent): void {
  if (event.pointerType !== "touch" && !isEditableTarget(event.target)) pointerScrollInProgress = true;
}

function handleBodyPointerEnd(): void {
  pointerScrollInProgress = false;
}

function handleWindowBlur(): void {
  // 鼠标在窗口外松开不会触发 document 的 pointerup；不让旧手势污染之后的 scroll。
  pointerScrollInProgress = false;
  lastTouchY = null;
}

function handleScroll(event: Event): void {
  const body = event.currentTarget as HTMLElement;
  rememberViewport();
  // 仅把按住鼠标拖动滚动条导致的离底视为用户暂停；布局或程序滚动不能改变状态。
  if (pointerScrollInProgress && !isAtLatest(body)) pauseFollowLatest();
}

function rememberViewport(withAnchor = false): void {
  const body = bodyRef.value;
  if (disposed || body === null) return;
  const viewport = { top: body.scrollTop, left: body.scrollLeft, anchorKey: null as string | null, anchorOffset: 0 };
  if (withAnchor && !followLatest.value) {
    const top = body.getBoundingClientRect().top + body.clientTop;
    const anchor = Array.from(body.querySelectorAll<HTMLElement>("[data-log-key]"))
      .find((row) => row.getBoundingClientRect().bottom > top);
    if (anchor) {
      viewport.anchorKey = anchor.dataset.logKey ?? null;
      viewport.anchorOffset = anchor.getBoundingClientRect().top - top;
    }
  }
  logViewport.value = viewport;
}

function restoreViewport(): void {
  const body = bodyRef.value;
  if (disposed || body === null) return;
  body.scrollLeft = logViewport.value.left;
  if (followLatest.value) {
    body.scrollTop = body.scrollHeight;
    return;
  }
  const { top, anchorKey, anchorOffset } = logViewport.value;
  const anchor = anchorKey === null ? undefined : Array.from(body.querySelectorAll<HTMLElement>("[data-log-key]"))
    .find((row) => row.dataset.logKey === anchorKey);
  // 增量拉取淘汰窗口头部时，优先找回原来阅读的条目；已淘汰则退回可用位置。
  body.scrollTop = anchor
    ? body.scrollTop + anchor.getBoundingClientRect().top - body.getBoundingClientRect().top - body.clientTop - anchorOffset
    : top;
}

function scrollToLatest(): void {
  if (!followLatest.value) return;
  const requestGeneration = ++scrollRequestGeneration;
  void nextTick(() => {
    if (!disposed && followLatest.value && requestGeneration === scrollRequestGeneration && bodyRef.value !== null) {
      bodyRef.value.scrollTop = bodyRef.value.scrollHeight;
    }
  });
}

function handleDetailsSummaryClick(event: Event, key: string): void {
  const details = (event.currentTarget as HTMLElement).closest("details");
  if (!(details instanceof HTMLDetailsElement)) return;
  if (details.open) expandedLogKeys.value.delete(key);
  else {
    pauseFollowLatest();
    expandedLogKeys.value.add(key);
  }
}

function handleDetailsToggle(event: Event, key: string): void {
  if (disposed) return;
  const open = (event.currentTarget as HTMLDetailsElement).open;
  // 恢复 open 属性也会产生原生 toggle，不能把状态还原误判成用户新展开。
  if (open === expandedLogKeys.value.has(key)) return;
  if (open) {
    pauseFollowLatest();
    expandedLogKeys.value.add(key);
  } else expandedLogKeys.value.delete(key);
}

function handleFollowLatestChange(event: Event): void {
  if ((event.currentTarget as HTMLInputElement).checked) resumeFollowLatest();
  else pauseFollowLatest();
}

function appendUnique(rawEntries: ReadonlyArray<LogEvent>): void {
  const seen = new Set(entries.value.map(logEventKey));
  const fresh: LogEvent[] = [];
  for (const entry of rawEntries) {
    if (!isVisibleLogEvent(entry)) continue;
    const key = logEventKey(entry);
    if (seen.has(key)) continue;
    seen.add(key);
    fresh.push(entry);
  }
  if (fresh.length === 0) return;
  rememberViewport(true);
  entries.value = [...entries.value, ...fresh].slice(-LOG_WINDOW_LIMIT);
  const retainedKeys = new Set(entries.value.map(logEventKey));
  for (const key of expandedLogKeys.value) {
    if (!retainedKeys.has(key)) expandedLogKeys.value.delete(key);
  }
  scrollToLatest();
}

/**
 * 懒加载 + 增量：首次打开从 seq 0 全量拉取，之后以日志尾游标取增量。
 * 当前 wire 已有 LogsList，但没有可直接复用的跨进程 StreamLogs；因此用短周期增量轮询
 * 保证页面实时追尾，同时保留 onLogs seam 作为事件到达时的低延迟入口。
 */
async function loadHistory(): Promise<void> {
  if (loading.value || clearing.value) return;

  loading.value = true;
  try {
    let afterSeq = loadedOnce.value ? logCursor.value : 0;
    let hasMore = true;
    const nextEntries: LogEvent[] = [];

    while (hasMore) {
      const chunk = await gateway.logsList(afterSeq, 500);
      nextEntries.push(...chunk.events);
      if (nextEntries.length > LOG_WINDOW_LIMIT) nextEntries.splice(0, nextEntries.length - LOG_WINDOW_LIMIT);
      hasMore = chunk.has_more && chunk.next_after_seq !== afterSeq;
      afterSeq = chunk.next_after_seq;
    }

    appendUnique(nextEntries);
    if (afterSeq > logCursor.value) logCursor.value = afterSeq;
    loadedOnce.value = true;
    error.value = false;
  } catch {
    error.value = true;
  } finally {
    loading.value = false;
  }
}

async function subscribeToFutureEntries(): Promise<void> {
  try {
    const stop = await gateway.onLogs((entry) => { if (!disposed) appendUnique([entry]); });
    if (disposed) stop();
    else unlisten = stop;
  } catch {
    unlisten = null;
  }
}

async function clearLogs(): Promise<void> {
  if (clearing.value) return;
  clearing.value = true;
  clearError.value = false;
  try {
    const reply = await gateway.logsClear();
    if (!reply.cleared) throw new Error("logs_clear rejected");
    entries.value = [];
    expandedLogKeys.value.clear();
    logViewport.value = { top: 0, left: 0, anchorKey: null, anchorOffset: 0 };
    logCursor.value = 0;
    loadedOnce.value = false;
    scrollToLatest();
  } catch {
    clearError.value = true;
  } finally {
    clearing.value = false;
  }
}

function startPolling(): void {
  if (pollTimer !== null) return;
  pollTimer = setInterval(() => {
    void loadHistory();
  }, 1_000);
}

onMounted(() => {
  document.addEventListener("pointerdown", handleOutsidePointer, true);
  document.addEventListener("keydown", handleEscape, true);
  document.addEventListener("pointerup", handleBodyPointerEnd, true);
  document.addEventListener("pointercancel", handleBodyPointerEnd, true);
  window.addEventListener("blur", handleWindowBlur);
  void loadHistory();
  void subscribeToFutureEntries();
  startPolling();
});

onBeforeUnmount(() => {
  rememberViewport(true);
  disposed = true;
  scrollRequestGeneration += 1;
});

onUnmounted(() => {
  if (pollTimer !== null) clearInterval(pollTimer);
  pollTimer = null;
  unlisten?.();
  unlisten = null;
  document.removeEventListener("pointerdown", handleOutsidePointer, true);
  document.removeEventListener("keydown", handleEscape, true);
  document.removeEventListener("pointerup", handleBodyPointerEnd, true);
  document.removeEventListener("pointercancel", handleBodyPointerEnd, true);
  window.removeEventListener("blur", handleWindowBlur);
});

// DOM 首次出现、缓存重入和跨页面返回的异步历史都在布局完成后恢复，无需等待网络。
watch([bodyRef, filteredEntries], restoreViewport, { flush: "post" });

watch([selectedLevels, searchQuery], () => {
  copyStatus.value = "";
  exportStatus.value = "";
  scrollToLatest();
});
</script>

<template>
  <section class="logs-page" aria-labelledby="logs-title">
    <header class="logs-page__header">
      <div>
        <h1 id="logs-title">日志</h1>
      </div>
      <div class="logs-actions">
        <div ref="levelFilterRef" class="logs-filter" @focusout="handleFilterFocusOut">
          <button ref="levelButtonRef" type="button" data-testid="log-level-filter" aria-label="日志等级筛选"
            :aria-expanded="levelFilterOpen" :aria-controls="levelPanelId" aria-haspopup="dialog" @click="toggleLevelFilter">
            <span class="logs-filter__summary">等级：{{ levelSummary }}</span>
            <svg class="logs-filter__chevron" viewBox="0 0 16 16" fill="none" aria-hidden="true" focusable="false">
              <path d="m4.5 6.25 3.5 3.5 3.5-3.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
          <div v-if="levelFilterOpen" :id="levelPanelId" ref="levelPanelRef" class="logs-level-panel" role="dialog" aria-label="日志等级筛选">
            <div class="logs-level-panel__actions">
              <button type="button" data-testid="log-level-all" @click="selectedLevels = null">全部</button>
              <button type="button" data-testid="log-level-none" @click="selectedLevels = []">清空</button>
            </div>
            <fieldset>
              <legend>显示等级（可多选）</legend>
              <label v-for="level in availableLevels" :key="level" class="logs-level-option">
                <input type="checkbox" :checked="isLevelSelected(level)" :data-testid="`log-level-${level}`" @change="toggleLevel(level)" />
                <span class="log-level" :class="`log-level--${level}`">{{ level }}</span>
              </label>
            </fieldset>
          </div>
        </div>
        <label class="logs-follow-latest">
          <input
            :checked="followLatest"
            type="checkbox"
            data-testid="logs-follow-latest"
            @change="handleFollowLatestChange"
          />
          <span>跟随最新</span>
        </label>
        <button type="button" data-testid="copy-logs" :disabled="copying || filteredEntries.length === 0" @click="copyFilteredLogs">
          {{ copying ? "复制中…" : `复制当前筛选（${filteredEntries.length} 条）` }}
        </button>
        <button type="button" data-testid="export-logs" :disabled="exporting || filteredEntries.length === 0" title="导出当前页面已保留且符合筛选条件的日志，包含全部详情字段（JSONL）" @click="exportFilteredLogs">
          {{ exporting ? "导出中…" : "导出当前筛选" }}
        </button>
        <button type="button" data-testid="clear-logs" :disabled="clearing" title="清空全部持久化日志，不受当前筛选影响" @click="clearLogs">
          {{ clearing ? "清空中…" : "清空日志" }}
        </button>
      </div>
    </header>

    <div class="logs-toolbar">
      <input v-model="searchQuery" type="search" data-testid="logs-search" class="logs-search" aria-label="搜索日志" placeholder="搜索消息、组件、代码或字段…" />
      <span class="logs-window" data-testid="logs-window">显示 {{ filteredEntries.length }} / 已保留 {{ entries.length }} 条 · 最多保留最近 2,000 条</span>
    </div>
    <p v-show="!exportStatus" class="logs-copy-status" :class="{ 'logs-copy-status--error': copyFailed }" data-testid="logs-copy-status" role="status" aria-live="polite">{{ copyStatus }}</p>
    <p v-show="exportStatus" class="logs-copy-status" :class="{ 'logs-copy-status--error': exportFailed }" data-testid="logs-export-status" role="status" aria-live="polite">{{ exportStatus }}</p>
    <div v-if="clearError" class="logs-message logs-message--error" role="alert">
      <span>清空日志失败，请重试。</span>
      <button type="button" :disabled="clearing" @click="clearLogs">重试清空</button>
    </div>
    <div v-if="error && entries.length > 0" class="logs-message logs-message--error" role="status">
      <span>日志更新失败，当前显示已读取记录。</span>
      <button type="button" :disabled="loading" @click="loadHistory">重试</button>
    </div>

    <div v-if="error && entries.length === 0" class="logs-message logs-message--error">
      <span>暂时无法读取日志。</span>
      <button type="button" data-testid="retry-logs" @click="loadHistory">重试</button>
    </div>
    <p v-else-if="isInitialLoading" class="logs-message">正在读取历史记录…</p>

    <div v-else class="logs-page__table-shell" data-testid="logs-table-shell">
      <div class="log-table" data-testid="log-table">
        <div class="log-row log-row--head" data-testid="logs-table-header" aria-hidden="true">
          <span>时间</span>
          <span>级别</span>
          <span>组件</span>
          <span>消息</span>
        </div>
        <div ref="bodyRef" class="logs-page__body" data-testid="logs-scroll-body" tabindex="0" aria-label="日志列表"
          @scroll="handleScroll" @wheel="handleWheel" @keydown="handleBodyKeydown"
          @touchstart="handleTouchStart" @touchmove="handleTouchMove" @touchend="handleTouchEnd" @touchcancel="handleTouchEnd"
          @pointerdown="handleBodyPointerDown">
          <p v-if="filteredEntries.length === 0" class="logs-empty">
            {{ selectedLevels?.length === 0 ? "请选择至少一个日志等级" : entries.length === 0 ? "暂无日志" : "没有符合筛选条件的日志" }}
          </p>
          <details v-for="entry in filteredEntries" :key="logEventKey(entry)" :data-log-key="logEventKey(entry)" :open="expandedLogKeys.has(logEventKey(entry))" class="log-details" data-testid="log-entry" @toggle="handleDetailsToggle($event, logEventKey(entry))">
            <summary class="log-row" data-testid="log-details-toggle" @click.prevent="handleDetailsSummaryClick($event, logEventKey(entry))">
              <time class="log-time" :datetime="preciseLogTime(entry.timestamp_ms) === '时间未知' ? undefined : preciseLogTime(entry.timestamp_ms)" :title="preciseLogTime(entry.timestamp_ms)">{{ formatTime(entry.timestamp_ms) }}</time>
              <span class="log-level" :class="`log-level--${entry.level}`">{{ entry.level }}</span>
              <span class="log-component">{{ entry.component || "—" }}</span>
              <span class="log-message">{{ entry.message }}</span>
              <svg class="log-details__triangle" viewBox="0 0 12 12" aria-hidden="true"><path d="M4 2 9 6 4 10Z" fill="currentColor" /></svg>
            </summary>
            <dl data-testid="log-details">
              <dt>完整时间</dt><dd>{{ preciseLogTime(entry.timestamp_ms) }}</dd>
              <dt>事件代码</dt><dd>{{ entry.code || "—" }}</dd>
              <template v-for="(value, key) in entry.fields" :key="key"><dt>{{ key }}</dt><dd>{{ value }}</dd></template>
            </dl>
          </details>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.logs-page {
  min-width: 0;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  margin-top: 0;
  padding: var(--product-page-top-gap) 0 var(--space-4);
}

.logs-page__header {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-5);
  min-height: 48px;
  margin: 0;
  padding: 0 0 var(--space-3);
  background: var(--surface-canvas);
}

.logs-page__header h1 {
  margin: 0;
  white-space: nowrap;
  font-size: clamp(22px, 2.2vw, 30px);
  letter-spacing: -0.02em;
}

.logs-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-2) var(--space-3);
}

.logs-filter,
.logs-follow-latest {
  display: inline-flex;
  align-items: center;
  gap: var(--space-1);
  color: var(--text-secondary);
  font-size: 12px;
  white-space: nowrap;
}

.logs-filter { position: relative; }

.logs-filter > button {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  min-height: 32px;
  padding: 0 var(--space-2);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--surface-panel);
  color: var(--text-primary);
}

.logs-filter__summary { max-width: 220px; overflow: hidden; text-overflow: ellipsis; }
.logs-filter__chevron { display: block; flex: none; width: 14px; height: 14px; color: var(--text-secondary); }
.logs-filter > button[aria-expanded="true"] .logs-filter__chevron { transform: rotate(180deg); }
.logs-level-panel {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  z-index: 40;
  width: 240px;
  max-width: calc(100vw - 40px);
  max-height: min(420px, 65vh);
  overflow: auto;
  padding: var(--space-3);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--surface-panel);
  box-shadow: var(--shadow-raised);
}
.logs-level-panel__actions { display: flex; justify-content: flex-end; gap: var(--space-2); margin-bottom: var(--space-2); }
.logs-level-panel__actions button { min-height: 28px; padding: 2px var(--space-2); font-size: 12px; }
.logs-level-panel fieldset { margin: 0; padding: 0; border: 0; }
.logs-level-panel legend { margin-bottom: var(--space-2); color: var(--text-secondary); font-size: 12px; }
.logs-level-option { display: flex; align-items: center; gap: var(--space-2); min-height: 34px; padding: 3px var(--space-2); border-radius: var(--radius-md); cursor: pointer; }
.logs-level-option:hover { background: var(--surface-subtle); }
.logs-level-option input { width: 15px; height: 15px; accent-color: var(--accent); }
.logs-filter button:focus-visible, .logs-level-option input:focus-visible, .logs-search:focus-visible, .log-details summary:focus-visible { outline: 2px solid var(--focus-ring); outline-offset: 2px; }
.logs-toolbar { display: flex; flex: none; flex-wrap: wrap; align-items: center; gap: var(--space-2) var(--space-3); }
.logs-search { flex: 1; min-width: 200px; max-width: 450px; min-height: 34px; padding: 5px var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-panel); color: var(--text-primary); font: inherit; font-size: 13px; }
.logs-search::placeholder { color: var(--text-secondary); }
.logs-window { color: var(--text-secondary); font-size: 12px; }
.logs-copy-status { flex: none; min-height: 20px; margin: 3px 0; color: var(--text-secondary); font-size: 12px; }
.logs-copy-status--error { color: var(--state-danger); }

.logs-follow-latest input {
  width: 15px;
  height: 15px;
  accent-color: var(--accent);
}

.logs-message {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  margin: 0 0 var(--space-4);
  padding: var(--space-3) var(--space-4);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--surface-panel);
  color: var(--text-secondary);
  font-size: 13px;
}

.logs-message--error { color: var(--state-danger); }

.logs-page__table-shell {
  flex: 1;
  min-height: 0;
}

.logs-page__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
  scrollbar-width: thin;
}

.log-table {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--surface-panel);
}

.log-row {
  position: relative;
  display: grid;
  grid-template-columns: 106px 62px minmax(88px, 0.32fr) minmax(260px, 2.68fr);
  gap: var(--space-2);
  align-items: start;
  min-width: 0;
  padding: 7px calc(var(--space-3) + 28px) 7px var(--space-3);
  font-size: 13px;
  line-height: 1.45;
}

.log-row--head {
  flex: none;
  border-bottom: 1px solid var(--border-subtle);
  background: var(--surface-subtle);
  color: var(--text-secondary);
  font-weight: 650;
}

.logs-page__body .log-row:hover { background: var(--surface-subtle); }

.log-time,
.log-component {
  min-width: 0;
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: 12px;
  overflow-wrap: anywhere;
}

.log-level {
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: 12px;
  text-transform: uppercase;
}

.log-level--warn { color: var(--state-warning); }
.log-level--error { color: var(--state-danger); }
.log-level--debug { color: var(--text-secondary); }
.log-message {
  min-width: 0;
  display: block;
  /* 折叠态只占一行：超出部分用省略号截断，完整内容仍留在 DOM 中供展开查看。 */
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 展开态恢复自由换行，完整显示消息原文（含内部换行）。 */
.log-details[open] .log-message {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.log-details { margin: 0; white-space: normal; }
.log-details summary { cursor: pointer; list-style: none; }
.log-details summary::-webkit-details-marker { display: none; }
.log-details__triangle { position: absolute; top: 11px; right: calc(var(--space-3) + 6px); width: 12px; height: 12px; color: var(--text-secondary); }
.log-details[open] .log-details__triangle { transform: rotate(90deg); }
.log-details[open] .log-details__triangle { color: var(--accent); }
.log-details dl { display: grid; grid-template-columns: minmax(90px, 0.3fr) minmax(0, 1fr); gap: 4px var(--space-3); margin: 0 var(--space-3) var(--space-2); padding: var(--space-2) var(--space-3); border-left: 2px solid var(--border-subtle); background: var(--surface-subtle); font-family: var(--font-mono); font-size: 12px; }
.log-details dt { color: var(--text-secondary); overflow-wrap: anywhere; }
.log-details dd { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }

.logs-empty {
  margin: 0;
  padding: var(--space-6);
  color: var(--text-secondary);
  text-align: center;
}

@media (max-width: 700px) {
  .logs-page__header {
    flex-direction: column;
    align-items: flex-start;
    padding: var(--space-3) 0 var(--space-4);
  }

  .logs-actions { justify-content: flex-start; }

  .log-row { grid-template-columns: 100px 52px minmax(0, 1fr); }
  .log-row > :nth-child(3) { display: none; }
  .logs-level-panel { left: 0; right: auto; }
  .logs-filter__summary { max-width: 170px; }
  .log-details dl { grid-template-columns: 1fr; }
}
</style>
