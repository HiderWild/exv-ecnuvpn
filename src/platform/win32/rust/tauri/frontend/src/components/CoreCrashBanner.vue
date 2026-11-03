<script setup lang="ts">
import { computed, inject } from "vue";

import { APPEARANCE_KEY } from "../product/appearance";
import type { WindowModePreference } from "../product/appearance";

/**
 * T9（§2.9）未连接态 core 崩溃弱提示横幅。
 *
 * 触发：壳侧保守死亡判定（管道断 + 子进程退出信号）广播 `exv://core-lifecycle`
 * （crashed）。非阻塞：固定在窗口顶部居中、只占自身尺寸，用户可忽略（忽略按钮），
 * 也可点「重启 core」走壳侧确认死亡后的受控重拉。恢复广播（recovered）撤下横幅。
 */
const props = defineProps<{
  visible: boolean;
  /** 「重启 core」进行中（防连点；由 App 壳从 core-crash 状态绑定）。 */
  busy?: boolean;
  /** 极简窗内紧凑渲染变体。 */
  compact?: boolean;
  /** 显式窗口模式；缺省时回落到注入的产品外观状态。 */
  mode?: WindowModePreference;
}>();

const emit = defineEmits<{
  /** 用户点「重启 core」（壳侧确认死亡才重拉；活着幂等成功）。 */
  restart: [];
  /** 用户忽略弱提示（横幅隐藏；core 再崩时壳会重新广播）。 */
  dismiss: [];
}>();

const appearance = inject(APPEARANCE_KEY, null);
// 注：Vue 对缺省的 boolean prop 做 false 铸造，`compact` 只能显式强制紧凑态
// （true），不能用它强制关闭——未传时回落 mode/外观注入。
const minimal = computed(() => {
  if (props.compact) return true;
  return props.mode
    ? props.mode === "minimal"
    : appearance?.state.value.mode === "minimal";
});
</script>

<template>
  <div
    v-if="visible"
    class="core-crash-banner"
    :class="{ 'core-crash-banner--minimal': minimal }"
    data-testid="core-crash-banner"
    role="status"
    aria-live="polite"
  >
    <span class="core-crash-banner__icon" aria-hidden="true">⚠</span>
    <span class="core-crash-banner__text">
      后台服务已崩溃，连接与设置保存暂不可用。
      <span class="core-crash-banner__hint">可点击重启，或忽略此提示。</span>
    </span>
    <button
      type="button"
      class="core-crash-banner__restart"
      data-testid="core-crash-restart"
      :disabled="busy"
      @click="emit('restart')"
    >
      {{ busy ? "正在重启…" : "重启 core" }}
    </button>
    <button
      type="button"
      class="core-crash-banner__dismiss"
      data-testid="core-crash-dismiss"
      :disabled="busy"
      aria-label="忽略此提示"
      @click="emit('dismiss')"
    >
      忽略
    </button>
  </div>
</template>

<style scoped>
.core-crash-banner {
  position: fixed;
  top: 12px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 55;
  display: flex;
  align-items: center;
  gap: 10px;
  max-width: min(92vw, 460px);
  padding: 8px 12px;
  border: 1px solid var(--state-danger, #b3363b);
  border-radius: 10px;
  background: var(--surface-panel, #2a2f3a);
  color: var(--text-primary, #fff);
  font-size: var(--font-size-body);
  line-height: 1.4;
  box-shadow: 0 6px 20px rgb(0 0 0 / 0.28);
}

.core-crash-banner__icon {
  flex: none;
  color: var(--state-danger, #b3363b);
}

.core-crash-banner__text {
  min-width: 0;
  overflow-wrap: anywhere;
}

.core-crash-banner__hint {
  color: var(--text-secondary, rgb(255 255 255 / 0.66));
}

.core-crash-banner__restart {
  flex: none;
  min-height: 28px;
  padding: 0 12px;
  border: 1px solid var(--border-subtle, rgb(255 255 255 / 0.2));
  border-radius: 8px;
  background: var(--state-danger, #b3363b);
  color: #fff;
  font-size: var(--font-size-caption);
  cursor: pointer;
}

.core-crash-banner__dismiss {
  flex: none;
  min-height: 28px;
  padding: 0 10px;
  border: 1px solid var(--border-subtle, rgb(255 255 255 / 0.2));
  border-radius: 8px;
  background: transparent;
  color: var(--text-secondary, rgb(255 255 255 / 0.66));
  font-size: var(--font-size-caption);
  cursor: pointer;
}

.core-crash-banner__restart:disabled,
.core-crash-banner__dismiss:disabled {
  opacity: 0.55;
  cursor: default;
}

/* 极简窗（328px 宽）：横幅贴顶部、允许换行、按钮不换行截断。 */
.core-crash-banner--minimal {
  top: 8px;
  right: 8px;
  left: 8px;
  max-width: none;
  transform: none;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 8px;
  font-size: var(--font-size-caption);
}

.core-crash-banner--minimal .core-crash-banner__restart,
.core-crash-banner--minimal .core-crash-banner__dismiss {
  min-height: 22px;
  padding: 0 8px;
  font-size: var(--font-size-caption);
}
</style>
