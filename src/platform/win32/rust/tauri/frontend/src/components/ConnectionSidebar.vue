<script setup lang="ts">
import { computed } from "vue";

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
const account = computed(() => connectionInfo.value.accountSource === "unavailable" ? "通过系统认证，无法获取" : connectionInfo.value.account ?? "—");
const displayMetric = (value: string | null | undefined): string => value ?? "—";
/** 连接时延显示开关（已生效偏好）：关闭时不渲染该行（默认关闭）。 */
const showLatency = computed(() => uiPreferencesState().value.show_latency);
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
          <dd :title="displayMetric(availableMetrics?.latency)" tabindex="0">{{ displayMetric(availableMetrics?.latency) }}</dd>
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
/* 标识保持单行；全文保留在文本节点、title 和可访问名称。 */
.info-row--vpn-server dd { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
</style>
