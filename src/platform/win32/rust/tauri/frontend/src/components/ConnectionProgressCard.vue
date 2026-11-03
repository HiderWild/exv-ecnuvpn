<script setup lang="ts">
import { computed, inject, onBeforeUnmount, ref, watch } from "vue";
import { APPEARANCE_KEY } from "../product/appearance";
import { type FlowConnectionProgressView } from "../product/connection-progress";
import { useConnectionVisual } from "../product/connection-visual";
import { useFlowProgress } from "../product/use-flow-progress";
import type { ProductStage, ProductUiState } from "../product/types";
import ConnectionStageList from "./ConnectionStageList.vue";

const props = defineProps<{ state: ProductUiState; compact?: boolean }>();
const emit = defineEmits<{ "return-focus": [] }>();
const panel = ref<HTMLElement | null>(null);
const appearance = inject(APPEARANCE_KEY, null);
const reduced = computed(() => appearance
  ? appearance.state.value.motion === "reduced"
  : window.matchMedia("(prefers-reduced-motion: reduce)").matches);
const { paused, viewportPaused } = useConnectionVisual(() => props.state);
const flow = useFlowProgress(() => props.state, () => viewportPaused.value, () => reduced.value);
const view = computed(() => flow.view.value);
type ProgressFrame = { view: FlowConnectionProgressView; stages: ProductStage[]; awaiting: boolean; reconnect: string | null };
// 仅保留退出画面；不记录操作身份，也不复制连接/重连状态机。
const frame = ref<ProgressFrame | null>(null);
const frameLabel = computed(() => frame.value?.view.mode === "flow"
  ? `${frame.value.view.percent}%${frame.value.view.label ? `，${frame.value.view.label}` : ""}` : frame.value?.view.label);
const expanded = ref(false);
let collapseTimer: ReturnType<typeof setTimeout> | undefined;
let clearTimer: ReturnType<typeof setTimeout> | undefined;

function cancelExit() {
  clearTimeout(collapseTimer);
  clearTimeout(clearTimer);
}
function clearAfterCollapse() {
  expanded.value = false;
  if (reduced.value) frame.value = null;
  else clearTimer = setTimeout(() => { frame.value = null; }, 360);
}

watch([view, () => props.state.stages, () => props.state.reconnect, reduced], (_next, previous) => {
  const wasActive = previous?.[0]?.expanded === true;
  if (view.value.expanded) {
    cancelExit();
    frame.value = {
      view: view.value,
      stages: props.state.stages.map(stage => ({ ...stage })),
      awaiting: props.state.status === "awaiting",
      reconnect: props.state.reconnect.active ? `重连中 · 第 ${props.state.reconnect.currentAttempt} 次` : null,
    };
    expanded.value = true;
    return;
  }
  if (panel.value?.contains(document.activeElement)) emit("return-focus");
  if (reduced.value) {
    cancelExit();
    expanded.value = false;
    frame.value = null;
    return;
  }
  // 终态后替换快照不延长退出、不改写最后阶段，也不凭空闪现完成卡。
  if (!wasActive || !frame.value) {
    if (frame.value?.view.mode === "complete" && view.value.mode !== "complete") {
      cancelExit();
      expanded.value = false;
      frame.value = null;
    }
    return;
  }
  cancelExit();
  if (view.value.mode === "complete") {
    frame.value = { ...frame.value, view: view.value };
    collapseTimer = setTimeout(clearAfterCollapse, 180);
  } else {
    clearAfterCollapse();
  }
}, { immediate: true });

onBeforeUnmount(cancelExit);
</script>

<template>
  <section
    ref="panel"
    class="progress-panel"
    :class="{ 'progress-panel--compact': compact }"
    data-testid="connection-progress-card"
    aria-label="连接进度"
    :data-expanded="expanded"
    :data-active="view.expanded"
    :data-paused="paused"
    :data-reduced="reduced"
    :aria-hidden="!view.expanded"
    :inert="!view.expanded"
  >
    <div class="progress-clip">
      <div v-if="frame" class="progress-card" :data-mode="frame.view.mode">
        <div class="estimated-progress">
          <div
            class="progress-track"
            role="progressbar"
            :aria-label="compact ? '连接流程进度' : '连接进度'"
            aria-valuemin="0"
            aria-valuemax="100"
            :aria-valuenow="'percent' in frame.view ? frame.view.percent : undefined"
            :aria-valuetext="view.expanded || frame.view.mode === 'complete' ? frameLabel : '连接进度已结束'"
          ><span class="progress-fill" :style="'percent' in frame.view ? { width: `${frame.view.percent}%` } : undefined" /></div>
          <span data-testid="progress-percent" class="progress-percent" :title="frameLabel">{{ 'percent' in frame.view ? `${frame.view.percent}%` : '—' }}</span>
        </div>
        <div v-if="!compact" class="progress-caption">
          <ConnectionStageList
            :stages="frame.stages"
            :paused="paused || reduced || !view.expanded"
            :awaiting="frame.awaiting"
            :active="view.expanded"
            :fallback-label="frame.awaiting ? '等待账户确认' : '正在连接'"
          />
          <div class="progress-context">
            <span>{{ frame.view.mode === 'complete' ? frame.view.label : view.expanded ? (frame.view.mode === 'flow' ? frame.view.label : '进度未定') : '进度已结束' }}</span>
            <span v-if="frame.reconnect">{{ view.expanded ? frame.reconnect : '重连进度已结束' }}</span>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.progress-panel { display:grid; flex:none; min-width:0; grid-template-rows:0fr; margin-top:0; opacity:0; transition:grid-template-rows 360ms cubic-bezier(.22,.7,.2,1),margin-top 360ms cubic-bezier(.22,.7,.2,1),opacity 220ms ease; }
.progress-panel[data-expanded="true"] { grid-template-rows:1fr; margin-top:16px; opacity:1; }
.progress-clip { min-height:0; overflow:hidden; }
.progress-card { padding:17px 19px; border:1px solid var(--border-subtle); border-radius:var(--radius-lg); background:var(--surface-panel); }
.estimated-progress { display:flex; align-items:center; gap:14px; margin-bottom:12px; }
.progress-track { flex:1; height:6px; overflow:hidden; border-radius:99px; background:var(--border-subtle); }
.progress-fill { display:block; height:100%; width:36%; border-radius:inherit; background:var(--accent); animation:progress-flow 1500ms ease-in-out infinite alternate; transition:width 300ms ease; }
.progress-card[data-mode="complete"] .progress-fill { width:100%; animation:none; background:var(--state-success); }
.progress-card[data-mode="flow"] .progress-fill { animation:none; }
.progress-percent { min-width:38px; text-align:right; color:var(--text-secondary); font-size: var(--font-size-caption); font-variant-numeric:tabular-nums; }
.progress-caption { display:flex; align-items:center; gap:16px; }
.progress-caption :deep(.stage-reel) { flex:1; width:auto; margin:0; }
.progress-context { display:flex; flex-direction:column; align-items:flex-end; gap:6px; flex:none; color:var(--text-secondary); font-size: var(--font-size-caption); }
[data-paused="true"] .progress-fill { animation-play-state:paused; }
[data-active="false"] .progress-fill { animation:none; }
[data-reduced="true"].progress-panel { transition:none; }
[data-reduced="true"] .progress-fill { animation:none; transition:none; }
:global(:root[data-motion="reduced"] .progress-panel) { transition:none; }
:global(:root[data-motion="reduced"] .progress-fill) { animation:none; transition:none; }
@keyframes progress-flow { from { transform:translateX(-100%); } to { transform:translateX(280%); } }
@media (prefers-reduced-motion:reduce) {
  :global(:root:not([data-motion]) .progress-panel) { transition:none; }
  :global(:root:not([data-motion]) .progress-fill) { animation:none; transition:none; }
}
@container connection-content (max-height:420px) { .progress-card { padding:10px 14px; } .estimated-progress { margin-bottom:6px; } .progress-caption :deep(.stage-reel__window) { height:calc(2 * var(--stage-row-height)); } .progress-caption :deep(.stage-reel__track) { position:relative; top:calc(-.5 * var(--stage-row-height)); } }
@container connection-content (max-width:420px) { .progress-caption { gap:8px; } .progress-context { font-size: var(--font-size-caption); } }
.progress-panel--compact[data-expanded="true"] { margin-top:4px; }
.progress-panel--compact .progress-card { padding:2px 6px; }
.progress-panel--compact .estimated-progress { min-width:0; gap:8px; margin-bottom:0; }
.progress-panel--compact .progress-percent { min-width:32px; font-size: var(--font-size-caption); }
</style>
