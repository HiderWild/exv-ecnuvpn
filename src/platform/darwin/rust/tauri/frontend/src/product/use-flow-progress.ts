import { computed, ref, watch } from "vue";
import { advanceFlowProgress, presentFlowConnectionProgress, type FlowConnectionProgressView } from "./connection-progress";
import type { ProductUiState } from "./types";

/** 只拥有进度画面的局部增长值；业务阶段和操作仍由已接纳快照提供。 */
export function useFlowProgress(
  state: () => ProductUiState,
  viewportPaused: () => boolean,
  reduced: () => boolean,
) {
  const anchor = computed(() => presentFlowConnectionProgress(state()));
  const phase = computed(() => anchor.value.mode === "flow" ? anchor.value.phase : null);
  const operation = () => state().operationId ?? null;
  const retry = () => state().reconnect.currentAttempt;
  const value = ref<number | null>(null);

  watch([phase, operation, retry, reduced, () => anchor.value.expanded], () => {
    value.value = anchor.value.mode === "flow" ? anchor.value.percent : null;
  }, { immediate: true, flush: "sync" });

  const running = computed(() => anchor.value.mode === "flow"
    && !viewportPaused() && !reduced() && !state().selfHeal.active);
  watch([running, phase, operation, retry], (_next, _previous, onCleanup) => {
    if (!running.value) return;
    const timer = setInterval(() => {
      if (anchor.value.mode === "flow" && value.value !== null) {
        value.value = advanceFlowProgress(value.value, anchor.value.cap);
      }
    }, 250);
    // watch停止（含卸载）或暂停时回收，不补后台墙钟时间。
    onCleanup(() => clearInterval(timer));
  }, { immediate: true, flush: "sync" });

  const view = computed<FlowConnectionProgressView>(() => anchor.value.mode === "flow"
    ? { ...anchor.value, percent: Math.min(anchor.value.cap, Math.floor(value.value ?? anchor.value.percent)) }
    : anchor.value);
  return { view };
}
