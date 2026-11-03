<script setup lang="ts">
import type { ProductUiState } from "../product/types";
import { useConnectionVisual } from "../product/connection-visual";
import ConnectionArtwork from "./ConnectionArtwork.vue";
const props = defineProps<{ state: ProductUiState }>();
const { visualState, paused } = useConnectionVisual(() => props.state);
</script>

<template>
  <section class="connection-stage" :data-paused="paused" data-testid="product-connection-visual-stage" aria-label="连接状态可视化">
    <div class="connection-visual" data-testid="connection-visual" :data-visual-state="visualState" :data-paused="paused">
      <ConnectionArtwork :state="visualState" :paused="paused" :stopping="state.status === 'stopping'" />
    </div>
    <div class="connection-copy">
      <slot />
    </div>
  </section>
</template>

<style scoped>
.connection-stage {
  gap: 24px;
  padding: 24px;
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr);
  grid-template-rows: minmax(0, 1fr);
  align-items: center;
  flex: 1 1 0;
  min-width: 0;
  min-height: 0;
  position: relative;
}
.connection-copy { min-width:0; min-height:0; max-height:100%; overflow-y:auto; overflow-x:hidden; display:flex; flex-direction:column; justify-content:safe center; gap:16px; scrollbar-width:thin; }
.connection-visual { width:100%; height:100%; max-height:320px; min-width:0; min-height:0; overflow:clip; }
@container connection-content (max-height:420px) {
  .connection-stage { gap:14px; padding:14px; }
}
@container connection-content (max-width:420px) {
  .connection-stage { grid-template-columns:minmax(0,1fr) minmax(0,1fr); gap:10px; padding:14px; }
}
</style>
