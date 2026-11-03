<script setup lang="ts">
import { computed } from "vue";
import type { ProductStage } from "../product/types";
const props = withDefaults(defineProps<{ stages: readonly ProductStage[]; paused?: boolean; awaiting?: boolean; active?: boolean; fallbackLabel?: string }>(), { active: true, fallbackLabel: "正在连接" });
const rows = computed(() => props.stages.length ? props.stages : [{ phase: "unknown", label: props.fallbackLabel, visual: "current" as const }]);
const currentIndex = computed(() => {
  const current = rows.value.findIndex(stage => stage.visual === "current");
  if (current >= 0) return current;
  const complete = rows.value.map(stage => stage.visual).lastIndexOf("complete");
  return Math.max(0, complete);
});
const current = computed(() => rows.value[currentIndex.value]);
const announcement = computed(() => current.value ? `${current.value.label}，${!props.active ? "进度已结束" : props.awaiting ? "等待确认" : current.value.visual === "complete" ? "已完成" : current.value.visual === "current" ? "进行中" : "等待中"}` : "");
</script>

<template>
  <div class="stage-reel" :data-paused="paused" data-testid="stage-reel" aria-label="连接阶段">
    <div class="stage-reel__window" aria-hidden="true">
      <div class="stage-reel__track" :style="{ transform: `translateY(calc(${1 - currentIndex} * var(--stage-row-height)))` }">
        <div v-for="(stage, index) in rows" :key="stage.phase" class="stage-reel__row" :class="{ 'stage-reel__row--current': index === currentIndex }" data-testid="stage-item" :data-stage-visual="stage.visual">
          <span class="stage-reel__mark">{{ stage.visual === 'complete' ? '✓' : index === currentIndex ? '•' : '·' }}</span>
          <span>{{ stage.label }}</span>
        </div>
      </div>
    </div>
    <span class="visually-hidden" :role="active ? 'status' : undefined" :aria-live="active ? 'polite' : 'off'" aria-atomic="true">{{ announcement }}</span>
  </div>
</template>

<style scoped>
.stage-reel { --stage-row-height:34px; position:relative; width:min(100%,320px); margin:0 auto; }
.stage-reel__window { height:calc(3 * var(--stage-row-height)); overflow:hidden; mask-image:linear-gradient(transparent,black 18%,black 82%,transparent); }
.stage-reel__track { transition:transform 360ms cubic-bezier(.22,.7,.2,1); }
.stage-reel__row { height:var(--stage-row-height); display:flex; align-items:center; justify-content:flex-start; gap:10px; color:var(--text-secondary); opacity:.45; font-size: var(--font-size-caption); text-align:left; white-space:nowrap; transition:font-size 360ms cubic-bezier(.22,.7,.2,1), opacity 360ms cubic-bezier(.22,.7,.2,1), color 360ms cubic-bezier(.22,.7,.2,1); }
.stage-reel__row > span:last-child { min-width:0; overflow:hidden; text-overflow:ellipsis; }
.stage-reel__row--current { color:var(--text-primary); opacity:1; font-size: var(--font-size-heading); font-weight:600; }
.stage-reel__mark { width:14px; font-size: var(--font-size-caption); text-align:center; flex:none; }
.stage-reel__row--current .stage-reel__mark { color:var(--accent); }
.visually-hidden { position:absolute; width:1px; height:1px; overflow:hidden; clip-path:inset(50%); white-space:nowrap; }
[data-paused="true"] .stage-reel__track, [data-paused="true"] .stage-reel__row,
:global(:root.motion-reduced .stage-reel__track), :global(:root.motion-reduced .stage-reel__row) { transition:none; }
@media (prefers-reduced-motion:reduce) {
  :global(:root:not([data-motion]) .stage-reel__track), :global(:root:not([data-motion]) .stage-reel__row) { transition:none; }
}
</style>
