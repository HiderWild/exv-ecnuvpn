import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import type { ProductUiState } from "./types";

type ConnectionVisual = { state: "idle" | "working" | "connected"; paused: boolean };
/** 可视化只投影现有事实；引擎恢复成功不等于 VPN 重新连接成功。 */
export function connectionVisualFor(state: ProductUiState): ConnectionVisual {
  if (state.coreStatus === "stopped") return { state: "idle", paused: true };
  if (state.selfHeal.active) {
    if (state.selfHeal.stage === "failed") return { state: "idle", paused: true };
    if (state.selfHeal.stage === "respawning") return { state: "working", paused: false };
    if (state.selfHeal.stage === "unknown") return { state: "working", paused: true };
  }
  if (state.status === "failed") return { state: "idle", paused: true };
  if (state.status === "connected") return { state: "connected", paused: false };
  if (["connecting", "awaiting", "stopping", "reconciling"].includes(state.status) || state.reconnect.active) {
    return { state: "working", paused: state.status === "awaiting" };
  }
  return { state: "idle", paused: false };
}

export function useConnectionVisual(state: () => ProductUiState) {
  const hidden = ref(document.hidden);
  const unfocused = ref(!document.hasFocus());
  const updateVisibility = () => { hidden.value = document.hidden; };
  const pause = () => { unfocused.value = true; };
  const resume = () => { unfocused.value = false; updateVisibility(); };
  onMounted(() => {
    document.addEventListener("visibilitychange", updateVisibility);
    window.addEventListener("blur", pause);
    window.addEventListener("focus", resume);
  });
  onBeforeUnmount(() => {
    document.removeEventListener("visibilitychange", updateVisibility);
    window.removeEventListener("blur", pause);
    window.removeEventListener("focus", resume);
  });
  const visual = computed(() => connectionVisualFor(state()));
  return { visualState: computed(() => visual.value.state), paused: computed(() => hidden.value || unfocused.value || visual.value.paused) };
}
