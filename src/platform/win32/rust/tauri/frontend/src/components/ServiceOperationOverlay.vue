<script setup lang="ts">
import { computed, inject } from "vue";

import { useDialogFocus } from "./use-dialog-focus";

import { APPEARANCE_KEY } from "../product/appearance";

const props = withDefaults(defineProps<{
  visible: boolean;
  label: string;
  /** 是否渲染加载指示器；错误/操作态可关闭（默认 true，服务操作保持原行为）。 */
  spinner?: boolean;
  /** 标题元素 id（aria-labelledby 目标）；默认服务遮罩自身 id。 */
  labelId?: string;
  /** 根节点 data-testid；默认 service-operation-overlay。 */
  testId?: string;
}>(), {
  spinner: true,
  labelId: "service-operation-overlay-label",
  testId: "service-operation-overlay",
});

/**
 * 极简适配：服务安装/卸载遮罩在 328px 窗口内也要可见。这是第三套独立适配
 * （与模态紧凑面板、右下角一次性提醒的定位都不同），不复用它们的样式类。
 */
const appearance = inject(APPEARANCE_KEY, null);
const minimal = computed(() => appearance?.state.value.mode === "minimal");

const emit = defineEmits<{ escape: [] }>();
const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, () => emit("escape"), {
  // 忙碌无可用动作时仍聚焦遮罩根；全文区可经 Tab 阅读，不抢占初始动作焦点。
  initialFocus: "button, input, select, textarea, a[href]",
});
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
      <p :id="labelId" class="service-overlay__label service-overlay__label--wrap">{{ label }}</p>
      <div v-if="$slots.default" class="service-overlay__extra">
        <slot />
      </div>
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

/* 可选插槽区：承载等待认证模态的取消/回退/三选项等操作与错误区，
   与遮罩共用同一份 token（--surface-panel/--text-primary/--state-danger）。 */
.service-overlay__extra {
  display: grid;
  width: 100%;
  justify-items: center;
  gap: var(--space-3);
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

.service-overlay--minimal .service-overlay__extra {
  gap: 6px;
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
