<script setup lang="ts">
/**
 * 时延数据唯一来自 engine 的隧道探测。这里不再维护 localStorage 开关或前端定时器：
 * 前者不能控制 engine，后者也会让 UI 声称发生了实际探测。用户可随时请求一次真实刷新。
 */
import { computed, inject, ref, watch } from "vue";

import { PRODUCT_RUNTIME_KEY } from "../product/runtime";

const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const refreshing = ref(false);
const refreshStatus = ref<"idle" | "requested" | "failed">("idle");
const failureMessage = ref("");
let refreshGeneration = 0;
watch([() => runtime?.state.value.status, () => runtime?.state.value.operationId], () => {
  refreshGeneration += 1;
  refreshing.value = false;
  refreshStatus.value = "idle";
  failureMessage.value = "";
}, { flush: "sync" });


const latencyValue = computed(() => {
  const metrics = runtime?.state.value.metrics;
  return metrics?.availability === "available" ? metrics.latency : null;
});

async function refreshLatency(): Promise<void> {
  if (refreshing.value || runtime === null) return;

  const generation = ++refreshGeneration;
  refreshing.value = true;
  refreshStatus.value = "idle";
  try {
    await runtime.triggerLatencyRefresh();
    if (generation === refreshGeneration) refreshStatus.value = "requested";
  } catch (error) {
    if (generation !== refreshGeneration) return;
    refreshStatus.value = "failed";
    failureMessage.value = typeof error === "object" && error !== null && "message" in error && typeof error.message === "string"
      ? error.message : "时延刷新未获受理，请重试。";
  } finally {
    if (generation === refreshGeneration) refreshing.value = false;
  }
}
</script>

<template>
  <section class="latency-section" data-testid="latency-control">
    <h2>时延</h2>
    <dl class="info-list">
      <div class="info-row">
        <dt>当前时延</dt>
        <dd data-testid="latency-value">{{ latencyValue ?? "检测中" }}</dd>
      </div>
    </dl>

    <p
      v-if="refreshStatus !== 'idle' || refreshing"
      class="latency-request-status"
      data-testid="latency-request-status"
      role="status"
    >
      {{
        refreshing
          ? "正在发送刷新请求…"
          : refreshStatus === "requested"
            ? "探测已受理，等待隧道回包更新时延。"
            : failureMessage
      }}
    </p>
    <button
      class="latency-refresh"
      :disabled="refreshing || runtime === null"
      data-testid="latency-refresh"
      @click="refreshLatency"
    >
      {{ refreshing ? "发送中…" : "立即刷新" }}
    </button>
  </section>
</template>

<style scoped>
.latency-section {
  padding-top: var(--space-4);
  border-top: 1px solid var(--border-subtle);
}

.latency-section h2 {
  margin-bottom: var(--space-3);
  font-size: var(--font-size-heading);
  font-weight: 650;
}

.info-list {
  display: grid;
  gap: var(--space-2);
  margin: 0;
}

.info-row {
  display: grid;
  grid-template-columns: minmax(0, 0.95fr) minmax(0, 1.05fr);
  gap: var(--space-2);
  align-items: baseline;
  min-width: 0;
}

.info-row dt {
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
}

.info-row dd {
  min-width: 0;
  margin: 0;
  overflow-wrap: anywhere;
  color: var(--text-primary);
  font-size: var(--font-size-body);
  font-variant-numeric: tabular-nums;
  text-align: right;
}

.latency-request-status {
  margin: var(--space-2) 0 0;
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
  line-height: 1.4;
}

.latency-refresh {
  width: 100%;
  margin-top: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--surface-subtle);
  color: var(--text-primary);
  font-size: var(--font-size-caption);
  cursor: pointer;
}

.latency-refresh:hover:not(:disabled) {
  border-color: var(--accent);
}

.latency-refresh:disabled {
  opacity: 0.6;
  cursor: default;
}

@media (max-width: 900px) {
  .latency-section {
    grid-column: 1 / -1;
  }
}
</style>
