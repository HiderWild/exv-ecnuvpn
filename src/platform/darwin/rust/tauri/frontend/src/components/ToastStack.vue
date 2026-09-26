<script setup lang="ts">
import { computed, inject } from "vue";

import { useToasts } from "../lib/toast";
import { APPEARANCE_KEY } from "../product/appearance";
import type { WindowModePreference } from "../product/appearance";

/**
 * 全局一次性状态提醒。极简模式（328px 宽）的定位是**独立适配**，与模态的紧凑面板
 * 是两件事：这里只重新定位/收窄/换行，不复用任何模态类，也不与模态合并成通用小窗样式。
 */
const props = defineProps<{
  /** 显式窗口模式；缺省时回落到注入的产品外观状态。 */
  mode?: WindowModePreference;
}>();

const appearance = inject(APPEARANCE_KEY, null);
const minimal = computed(() =>
  props.mode ? props.mode === "minimal" : appearance?.state.value.mode === "minimal",
);

const { toasts, dismiss } = useToasts();
</script>

<template>
  <div
    class="toast-stack"
    :class="{ 'toast-stack--minimal': minimal }"
    data-testid="toast-stack"
    aria-live="polite"
  >
    <transition-group name="toast">
      <div
        v-for="toast in toasts"
        :key="toast.id"
        class="toast"
        :class="`toast--${toast.kind}`"
        data-testid="toast"
        role="status"
        @click="dismiss(toast.id)"
      >
        {{ toast.message }}
      </div>
    </transition-group>
  </div>
</template>

<style scoped>
.toast-stack {
  position: fixed;
  right: 16px;
  bottom: 16px;
  z-index: 60;
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-width: 320px;
  pointer-events: none;
}

/* 极简：窗口宽 328px，right:16 + max-width:320 会溢出 8px。改为左右各留 8px 并
   允许换行，底部贴近标题栏下方的动效条（8px）。 */
.toast-stack--minimal {
  right: 8px;
  bottom: 8px;
  left: 8px;
  max-width: none;
  gap: 4px;
}

.toast-stack--minimal .toast {
  padding: 6px 8px;
  border-radius: 8px;
  font-size: 12px;
  line-height: 1.35;
  overflow-wrap: anywhere;
}

.toast {
  pointer-events: auto;
  padding: 10px 14px;
  border-radius: 10px;
  border: 1px solid rgb(255 255 255 / 0.14);
  color: #fff;
  font-size: 13px;
  line-height: 1.5;
  box-shadow: 0 6px 20px rgb(0 0 0 / 0.28);
  cursor: pointer;
  white-space: pre-wrap;
}

.toast--info {
  background: #3b6ea8;
}
.toast--success {
  background: #2e7d4f;
}
.toast--warning {
  background: #b07a28;
}
.toast--error {
  background: #b3363b;
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(6px);
}
</style>
