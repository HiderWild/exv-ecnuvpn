<script setup lang="ts">
import { computed, inject } from "vue";
import { useDialogFocus } from "./use-dialog-focus";

import { APPEARANCE_KEY } from "../product/appearance";

const props = withDefaults(defineProps<{
  visible: boolean;
  label: string;
  spinner?: boolean;
  labelId?: string;
  testId?: string;
}>(), { spinner: true, labelId: "service-operation-overlay-label", testId: "service-operation-overlay" });
const emit = defineEmits<{ escape: [] }>();

/**
 * 极简适配：服务安装/卸载遮罩在 328px 窗口内也要可见。这是第三套独立适配
 * （与模态紧凑面板、右下角一次性提醒的定位都不同），不复用它们的样式类。
 */
const appearance = inject(APPEARANCE_KEY, null);
const minimal = computed(() => appearance?.state.value.mode === "minimal");

const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, () => emit("escape"), { initialFocus: "button, input, select, textarea, a[href]" });
</script>

<template>
  <div
    v-if="visible"
    class="service-overlay exv-dialog"
    :class="{ 'service-overlay--minimal': minimal }"
    :data-testid="testId"
    role="dialog"
    aria-modal="true"
    :aria-live="spinner ? 'polite' : undefined"
    :aria-labelledby="labelId"
    tabindex="-1"
    ref="dialog"
    @keydown="onDialogKeydown"
  >
    <div class="service-overlay__card service-overlay__card--contained exv-dialog-card">
      <div v-if="spinner" class="service-overlay__spinner" data-testid="service-overlay-spinner" aria-hidden="true" />
      <p :id="labelId" class="service-overlay__label service-overlay__label--wrap" tabindex="0" role="region" aria-label="操作进度说明">{{ label }}</p>
      <div v-if="$slots.default" class="service-overlay__extra"><slot /></div>
    </div>
  </div>
</template>

<style scoped>
.service-overlay__extra { min-height: 0; width: 100%; overflow-y: auto; }
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
  font-size: var(--font-size-heading);
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
  font-size: var(--font-size-heading);
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
/* 长操作说明可独立回读，加载图形保留在滚动区外。 */
.service-overlay.exv-dialog .service-overlay__card { min-height: 0; overflow: hidden; }
.service-overlay.exv-dialog .service-overlay__spinner { flex: none; }
.service-overlay.exv-dialog .service-overlay__label { min-height: 1.3em; overflow-y: auto; padding: 3px; }
.service-overlay.exv-dialog .service-overlay__label:focus-visible { outline-offset: -2px; }
</style>
