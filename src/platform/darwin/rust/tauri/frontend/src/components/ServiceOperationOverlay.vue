<script setup lang="ts">
import { computed, inject, nextTick, ref, watch } from "vue";

import { APPEARANCE_KEY } from "../product/appearance";

const props = defineProps<{
  visible: boolean;
  label: string;
}>();

/**
 * 极简适配：服务安装/卸载遮罩在 328px 窗口内也要可见。这是第三套独立适配
 * （与模态紧凑面板、右下角一次性提醒的定位都不同），不复用它们的样式类。
 */
const appearance = inject(APPEARANCE_KEY, null);
const minimal = computed(() => appearance?.state.value.mode === "minimal");

const dialog = ref<HTMLElement | null>(null);
let previousActiveElement: HTMLElement | null = null;

function keepFocusInDialog(event: KeyboardEvent): void {
  if (event.key !== "Tab") return;
  event.preventDefault();
  dialog.value?.focus();
}

watch(
  () => props.visible,
  async (visible) => {
    if (visible) {
      previousActiveElement = document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
      await nextTick();
      dialog.value?.focus();
      return;
    }

    const focusTarget = previousActiveElement;
    previousActiveElement = null;
    await nextTick();
    if (focusTarget?.isConnected) focusTarget.focus();
  },
);
</script>

<template>
  <div
    v-if="visible"
    class="service-overlay"
    :class="{ 'service-overlay--minimal': minimal }"
    data-testid="service-operation-overlay"
    role="dialog"
    aria-modal="true"
    aria-live="polite"
    aria-labelledby="service-operation-overlay-label"
    tabindex="-1"
    ref="dialog"
    @keydown="keepFocusInDialog"
  >
    <div class="service-overlay__card service-overlay__card--contained">
      <div class="service-overlay__spinner" aria-hidden="true" />
      <p id="service-operation-overlay-label" class="service-overlay__label service-overlay__label--wrap">{{ label }}</p>
    </div>
  </div>
</template>

<style scoped>
.service-overlay {
  position: fixed;
  inset: 0;
  z-index: 70;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: max(var(--space-4), env(safe-area-inset-top)) max(var(--space-4), env(safe-area-inset-right)) max(var(--space-4), env(safe-area-inset-bottom)) max(var(--space-4), env(safe-area-inset-left));
  background: rgb(0 0 0 / 0.52);
}

.service-overlay__card {
  box-sizing: border-box;
  width: min(calc(100vw - (2 * var(--space-4))), 440px);
  max-height: calc(100dvh - (2 * var(--space-4)));
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-4);
  overflow: auto;
  padding: var(--space-6);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: var(--surface-panel);
  box-shadow: var(--shadow-raised);
}

.service-overlay__label {
  margin: 0;
  color: var(--text-primary);
  font-size: 14px;
  line-height: 1.5;
  text-align: center;
}

.service-overlay__label--wrap {
  max-width: 100%;
  overflow-wrap: anywhere;
  word-break: break-word;
}

.service-overlay__spinner {
  width: 28px;
  height: 28px;
  border: 3px solid var(--border-subtle);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: service-spin 0.8s linear infinite;
}

/* 极简：卡片收进 328px 宽窗口（左右各留 8px），内边距与文案同步收敛。 */
.service-overlay--minimal {
  padding: 8px;
}

.service-overlay--minimal .service-overlay__card {
  width: min(calc(100vw - 16px), 296px);
  max-height: calc(100vh - 16px);
  gap: 6px;
  padding: 10px 8px;
}

.service-overlay--minimal .service-overlay__label {
  font-size: 12px;
  line-height: 1.3;
}

.service-overlay--minimal .service-overlay__spinner {
  width: 20px;
  height: 20px;
  border-width: 2px;
}

@keyframes service-spin {
  to {
    transform: rotate(360deg);
  }
}

.motion-reduced .service-overlay__spinner,
[data-motion="reduced"] .service-overlay__spinner {
  animation: none;
  border-color: var(--accent);
}

@media (max-width: 420px) {
  .service-overlay__card {
    padding: var(--space-5);
  }
}
</style>
