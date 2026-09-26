<script setup lang="ts">
import { computed, inject } from "vue";
import type { ProductUiState } from "../product/types";
import { APPEARANCE_KEY } from "../product/appearance";
import { useConnectionVisual } from "../product/connection-visual";
import ConnectionRuntimePanel from "./ConnectionRuntimePanel.vue";
import ConnectionArtwork from "./ConnectionArtwork.vue";
const props = defineProps<{ state: ProductUiState }>();
const { visualState, paused } = useConnectionVisual(() => props.state);
const appearance = inject(APPEARANCE_KEY, null);
const reduced = computed(() => appearance?.state.value.motion === "reduced");
const showRuntime = computed(() => ["connecting", "awaiting", "connected"].includes(props.state.status) || props.state.reconnect.active);
// 收起期间保留最后一帧数据，避免内容先变成未连接状态再淡出。
const runtimeState = computed<ProductUiState>((previous) => showRuntime.value ? props.state : previous ?? props.state);
</script>

<template>
  <section class="connection-stage" :data-paused="paused" data-testid="product-connection-visual-stage" aria-label="连接状态可视化" :class="{ 'connection-stage--with-data': showRuntime, 'connection-stage--reduced': reduced }">
    <div class="connection-visual" data-testid="connection-visual" :data-visual-state="visualState" :data-paused="paused">
      <ConnectionArtwork :state="visualState" :paused="paused" :stopping="state.status === 'stopping'" />
    </div>
    <div class="connection-runtime" :inert="!showRuntime" :aria-hidden="!showRuntime">
      <Transition name="runtime-reveal" :css="!reduced">
        <ConnectionRuntimePanel v-if="showRuntime" :state="runtimeState" :paused="paused" />
      </Transition>
    </div>
  </section>
</template>

<style scoped>
.connection-stage {
  --runtime-width: 280px;
  --runtime-gap: 24px;
  --layout-duration: 360ms;
  --layout-easing: cubic-bezier(.22, 1, .36, 1);
  display: grid;
  grid-template-columns: minmax(0, 1fr) 0px;
  grid-template-rows: minmax(0, 1fr);
  min-width: 0;
  min-height: 0;
  height: 100%;
  max-height: 560px;
  position: relative;
  transition: grid-template-columns var(--layout-duration) var(--layout-easing);
}
.connection-stage--with-data { grid-template-columns: minmax(0, 1fr) calc(var(--runtime-width) + var(--runtime-gap)); }
.connection-visual { width:100%; height:100%; min-width:0; min-height:0; overflow:clip; }
.connection-runtime { display:grid; align-content:center; min-width:0; min-height:0; overflow:clip; }
.connection-runtime > :deep(.runtime-panel) { width:var(--runtime-width); margin-left:var(--runtime-gap); }
.runtime-reveal-enter-active,
.runtime-reveal-leave-active { transition: opacity var(--layout-duration) var(--layout-easing), transform var(--layout-duration) var(--layout-easing); }
.runtime-reveal-enter-from,
.runtime-reveal-leave-to { opacity:0; transform:translateX(20px); }
.connection-stage--reduced { --layout-duration:0ms; }
@media (prefers-reduced-motion: reduce) { .connection-stage { --layout-duration:0ms; } }
@container connection-main (max-width:760px) {
  .connection-stage { --runtime-width:220px; --runtime-gap:16px; }
}
</style>
