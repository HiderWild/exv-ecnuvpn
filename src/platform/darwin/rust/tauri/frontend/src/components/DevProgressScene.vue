<script setup lang="ts">
// 仅 DEV + preview=1 + scene=progress 动态加载；可见控件只替换模拟内存样本。
import { computed } from "vue";
import App from "../App.vue";
import { CONNECT_PHASE_ORDER } from "../lib/status-map";
import type { ConnectPhase } from "../lib/ipc";
import { PRODUCT_PHASE_LABELS } from "../product/presenter";
import { selectProgressPreview, type MockProductRuntime } from "../product/mock-runtime";

defineOptions({ inheritAttrs:false });
const props = defineProps<{ previewRuntime: MockProductRuntime }>();
const enabled = selectProgressPreview(import.meta.env.DEV, window.location.search) && props.previewRuntime.source === "mock";
const phase = computed(() => props.previewRuntime.state.value.stages.find(stage => stage.visual === "current")?.phase ?? "");
function newAttempt() {
  props.previewRuntime.showPreview("connecting", "observing_owned_state");
}
function showPhase(event: Event) {
  props.previewRuntime.showPreview("connecting", (event.target as HTMLSelectElement).value as ConnectPhase);
}
</script>

<template>
  <App v-if="enabled" v-bind="$attrs" />
  <aside v-if="enabled" class="dev-progress-scene" data-testid="dev-progress-scene" aria-label="模拟连接进度控制">
    <strong>模拟进度</strong>
    <button type="button" @click="newAttempt">新尝试</button>
    <select :value="phase" aria-label="模拟连接阶段" @change="showPhase">
      <option value="" disabled>选择连接阶段</option>
      <option v-for="item in CONNECT_PHASE_ORDER" :key="item" :value="item">{{ PRODUCT_PHASE_LABELS[item] }}</option>
    </select>
    <button type="button" @click="previewRuntime.showPreview('awaiting')">等待确认</button>
    <button type="button" @click="previewRuntime.showPreview('connected')">连接成功</button>
    <button type="button" @click="previewRuntime.showPreview('failed')">连接失败</button>
    <button type="button" @click="previewRuntime.showPreview('connecting', 'observing_owned_state', 'reconnecting')">自动重连</button>
    <button type="button" @click="previewRuntime.showPreview('connecting', undefined, 'unknown-phase')">未知阶段</button>
    <button type="button" @click="previewRuntime.showPreview('stopping')">停止</button>
    <button type="button" @click="previewRuntime.showPreview('idle')">空闲</button>
  </aside>
</template>

<style scoped>
.dev-progress-scene { position:fixed; bottom:6px; left:6px; z-index:45; display:flex; align-items:center; flex-wrap:wrap; gap:5px; max-width:calc(100vw - 100px); padding:5px 8px; border:1px dashed var(--state-warning); border-radius:7px; background:var(--surface-panel); color:var(--text-secondary); font-size: var(--font-size-body); }
.dev-progress-scene button,.dev-progress-scene select { min-height:24px; padding:2px 6px; font-size: var(--font-size-body); }
</style>
