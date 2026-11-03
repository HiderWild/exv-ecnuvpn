<script setup lang="ts">
import { computed } from "vue";

import type { ProductUiState } from "../product/types";
import { connectionActionFor, type ProductConnectionAction } from "../product/connection-action";

const props = defineProps<{
  state: ProductUiState;
  busy: boolean;
  /**
   * 修复 3b（2026-10-08 UI 重试脱同步）：宿主预计算的连接动作。提供时以它为准
   * （连接在飞期间任意相位呈现「取消」）；缺省回落按快照现算（独立挂载/旧用法
   * 兼容）。不得在按钮层绕开宿主的在飞覆盖重新决策。
   */
  action?: ProductConnectionAction;
}>();

const emit = defineEmits<{
  action: [];
}>();

const resolvedAction = computed<ProductConnectionAction>(() =>
  props.action ?? connectionActionFor(props.state));

function requestAction(): void {
  if (!props.busy && resolvedAction.value.enabled) emit("action");
}
</script>

<template>
  <button
    class="connection-action"
    data-testid="connection-action"
    :data-action="resolvedAction.kind"
    type="button"
    :disabled="busy || !resolvedAction.enabled"
    :aria-busy="busy ? 'true' : 'false'"
    @click="requestAction"
  >
    <slot :action="resolvedAction">{{ resolvedAction.label }}</slot>
  </button>
</template>

<style scoped>
.connection-action {
  min-width: 112px;
  min-height: 44px;
  border-color: var(--accent);
  background: var(--accent);
  color: var(--accent-on);
  font-weight: 650;
  letter-spacing: 0.02em;
}

.connection-action:hover:not(:disabled) {
  border-color: var(--accent-strong);
  background: var(--accent-strong);
}

.connection-action:disabled {
  border-color: var(--border-subtle);
  background: var(--surface-subtle);
  color: var(--text-secondary);
}
</style>
