<script setup lang="ts">
import { computed, inject, onUnmounted, ref, useId, watch } from "vue";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";

import type { ProductUiState } from "../product/types";
import { uiPreferencesState } from "../product/ui-prefs";
import TrafficMetricsPanel from "./TrafficMetricsPanel.vue";

const props = defineProps<{
  state: ProductUiState;
}>();

const connected = computed(() => props.state.status === "connected");
const availableMetrics = computed(() =>
  props.state.metrics?.availability === "available" ? props.state.metrics : null,
);
const connectionInfo = computed(() => props.state.connectionInfo);
const account = computed(() => connectionInfo.value.account?.trim() || (connectionInfo.value.accountSource === "unavailable" ? "认证服务未返回账户信息" : "—"));
const displayMetric = (value: string | null | undefined): string => value ?? "—";
/** 连接时延显示开关（已生效偏好）：关闭时不渲染该行（默认关闭）。 */
const showLatency = computed(() => uiPreferencesState().value.show_latency);
const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const latencyStatusId = useId();
const refreshing = ref(false);
const refreshMessage = ref("");
let refreshGeneration = 0;
const refreshAvailable = computed(() => connected.value && showLatency.value &&
  props.state.connectionInfo.protocol === "cisco" && !!props.state.operationId &&
  runtime !== null);
const latencyMessage = computed(() => {
  if (props.state.connectionInfo.protocol === "atrust") return "aTrust 暂不支持手动刷新时延。";
  if (refreshing.value) return "正在发送刷新请求…";
  if (!refreshAvailable.value) return "当前会话暂不能刷新时延。";
  return refreshMessage.value;
});
watch([() => props.state.operationId, () => props.state.status, () => props.state.connectionInfo.protocol, showLatency], () => {
  ++refreshGeneration;
  refreshing.value = false;
  refreshMessage.value = "";
}, { flush: "sync" });
onUnmounted(() => { ++refreshGeneration; });
async function refreshLatency(): Promise<void> {
  if (!refreshAvailable.value || refreshing.value || runtime === null) return;
  const generation = ++refreshGeneration;
  refreshing.value = true;
  refreshMessage.value = "";
  try {
    await runtime.triggerLatencyRefresh();
    if (generation === refreshGeneration) refreshMessage.value = "探测已受理，等待隧道回包更新时延。";
  } catch (error) {
    if (generation !== refreshGeneration) return;
    refreshMessage.value = typeof error === "object" && error !== null && "message" in error && typeof error.message === "string"
      ? error.message : "时延刷新未获受理，请重试。";
  } finally {
    if (generation === refreshGeneration) refreshing.value = false;
  }
}
</script>

<template>
  <aside class="connection-sidebar" aria-label="连接信息">
    <div
      class="connection-sidebar__status-pill"
      :class="{ 'connection-sidebar__status-pill--connected': connected }"
      data-testid="connection-status-pill"
    >
      <span class="connection-sidebar__status-dot" />
      <span class="connection-sidebar__status-text">{{ state.title }}</span>
      <span v-if="state.coreStatus === 'stopped'" class="connection-sidebar__core-status">核心已停止</span>
    </div>

    <section v-if="connected" class="sidebar-section" data-testid="traffic-metrics">
      <dl class="info-list connection-summary-list">
        <div class="info-row">
          <dt>在线时长</dt>
          <dd :title="displayMetric(state.metrics?.online)" :aria-label="`在线时长：${displayMetric(state.metrics?.online)}`" tabindex="0">{{ displayMetric(state.metrics?.online) }}</dd>
        </div>
        <div class="info-row">
          <dt>用户名</dt>
          <dd :title="account" :aria-label="`用户名：${account}`" tabindex="0">{{ account }}</dd>
        </div>

        <div v-if="showLatency" class="info-row" data-testid="connection-latency">
          <dt>连接时延</dt>
          <dd class="latency-value" :title="displayMetric(availableMetrics?.latency)" tabindex="0">
            <span data-testid="sidebar-latency-value">{{ displayMetric(availableMetrics?.latency) }}</span>
            <button type="button" class="latency-refresh" data-testid="sidebar-latency-refresh"
              aria-label="刷新连接时延" :title="latencyMessage || '刷新连接时延'"
              :aria-describedby="latencyMessage ? latencyStatusId : undefined"
              :disabled="!refreshAvailable || refreshing" @click="refreshLatency">
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M20 11a8 8 0 1 0-2 6M20 4v7h-7" /></svg>
            </button>
          </dd>
        </div>
        <div class="info-row">
          <dt>校内地址</dt>
          <dd :title="connectionInfo.campusIp ?? '—'" tabindex="0">{{ connectionInfo.campusIp ?? "—" }}</dd>
        </div>
        <div v-if="state.reconnect.enabled" class="info-row">
          <dt>重连次数</dt>
          <dd>{{ state.reconnect.currentAttempt }}</dd>
        </div>
        <div class="info-row info-row--vpn-server" data-testid="connection-vpn-server">
          <dt>服务器</dt>
          <dd :title="connectionInfo.vpnServer ?? '—'" :aria-label="`服务器：${connectionInfo.vpnServer ?? '—'}`" tabindex="0">{{ connectionInfo.vpnServer ?? "—" }}</dd>
        </div>
      </dl>

      <p v-if="showLatency && latencyMessage" :id="latencyStatusId" class="latency-status latency-status--description" role="status" data-testid="sidebar-latency-status">{{ latencyMessage }}</p>
      <TrafficMetricsPanel :metrics="availableMetrics" :show-totals="true" />
    </section>

  </aside>
</template>

<style scoped>
.connection-sidebar { display:flex; min-width:0; flex-direction:column; gap:0; padding:0; border:0; border-radius:0; background:transparent; }
.connection-sidebar__status-pill { display:flex; flex:none; align-items:center; gap:8px; padding:0; font-size: var(--font-size-caption); color:var(--text-secondary); }
.connection-sidebar__status-dot { width:7px; height:7px; flex:none; border-radius:50%; background:var(--text-tertiary); }
.connection-sidebar__status-pill--connected .connection-sidebar__status-dot { background:var(--state-success); }
.connection-sidebar__core-status { color:var(--state-warning); font-size: var(--font-size-caption); }
.sidebar-section { min-height:0; margin-top:15px; padding-top:13px; border-top:1px solid var(--border-subtle); }
.info-list { display:grid; gap:7px; margin:0; }
.info-row { display:grid; grid-template-columns:54px minmax(0,1fr); gap:7px; align-items:baseline; min-width:0; }
.info-row dt { color:var(--text-secondary); font-size: var(--font-size-caption); white-space:nowrap; }
.info-row dd { min-width:0; margin:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--text-primary); font-size: var(--font-size-body); font-weight:400; font-variant-numeric:tabular-nums; text-align:right; }
.info-row dd:focus-visible { outline:2px solid var(--focus-ring); outline-offset:2px; }
.info-row .latency-value { display:flex; justify-content:flex-end; align-items:center; gap:4px; }
.latency-value > span { min-width:0; overflow:hidden; text-overflow:ellipsis; }
.latency-refresh { display:inline-flex; flex:none; align-items:center; justify-content:center; width:22px; height:22px; min-height:22px; padding:3px; border:0; border-radius:4px; background:transparent; color:var(--text-secondary); cursor:pointer; }
.latency-refresh:hover:not(:disabled) { color:var(--text-primary); background:var(--surface-panel); }
.latency-refresh:disabled { opacity:.5; cursor:default; }
.latency-refresh:focus-visible { outline:2px solid var(--focus-ring); outline-offset:2px; }
.latency-status { margin:6px 0 0; color:var(--text-secondary); font-size: var(--font-size-caption); line-height:1.5; overflow-wrap:anywhere; }
/* 反馈全文由按钮 title 和 aria-describedby 保留，不占左栏布局高度。 */
.latency-status--description { position:absolute; width:1px; height:1px; padding:0; margin:-1px; overflow:hidden; clip-path:inset(50%); white-space:nowrap; border:0; }
/* 标识保持单行；全文保留在文本节点、title 和可访问名称。 */
.info-row--vpn-server dd { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
</style>
