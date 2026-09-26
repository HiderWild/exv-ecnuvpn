<script setup lang="ts">
import { computed, ref, watch } from "vue";

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
const displayMetric = (value: string | null | undefined): string => value ?? "—";
/** 连接时延显示开关（已生效偏好）：关闭时不渲染该行（默认关闭）。 */
const showLatency = computed(() => uiPreferencesState().value.show_latency);
const serverExpanded = ref(false);
watch(connected, (value) => { if (!value) serverExpanded.value = false; });
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
          <dd>{{ displayMetric(state.metrics?.online) }}</dd>
        </div>
        <div class="info-row">
          <dt>配置账号</dt>
          <dd>{{ connectionInfo.account ?? "—" }}</dd>
        </div>
      </dl>

      <dl class="info-list connection-details-list">
        <div v-if="showLatency" class="info-row" data-testid="connection-latency">
          <dt>连接时延</dt>
          <dd>{{ displayMetric(availableMetrics?.latency) }}</dd>
        </div>
        <div class="info-row">
          <dt>校内地址</dt>
          <dd>{{ connectionInfo.campusIp ?? "—" }}</dd>
        </div>
        <div v-if="state.reconnect.enabled" class="info-row">
          <dt>重连次数</dt>
          <dd>{{ state.reconnect.currentAttempt }}</dd>
        </div>
        <div class="info-row info-row--vpn-server" data-testid="connection-vpn-server">
          <dt>配置服务器</dt>
          <dd :title='connectionInfo.vpnServer ?? "—"'><details class="server-address" @toggle="serverExpanded = ($event.target as HTMLDetailsElement).open"><summary>{{ serverExpanded ? '收起地址' : connectionInfo.vpnServer ?? '—' }}</summary><span v-if="serverExpanded">{{ connectionInfo.vpnServer ?? '—' }}</span></details></dd>
        </div>
      </dl>

      <TrafficMetricsPanel :metrics="availableMetrics" :show-totals="true" />
    </section>

  </aside>
</template>

<style scoped>
.connection-sidebar {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: var(--space-4);
  /* 侧栏本身直接落在 product-rail 的灰色底上，不再套白色卡片。 */
  padding: 0;
  border: 0;
  border-radius: 0;
  background: transparent;
}

.connection-sidebar__status-pill {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: 0;
  font-size: 13px;
  color: var(--text-secondary);
}

.connection-sidebar__status-dot {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--text-tertiary);
}

.connection-sidebar__status-pill--connected .connection-sidebar__status-dot {
  background: var(--state-success);
}

.connection-sidebar__core-status {
  color: var(--state-warning);
  font-size: 12px;
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
  font-size: 13px;
}

.info-row dd {
  min-width: 0;
  margin: 0;
  overflow-wrap: anywhere;
  color: var(--text-primary);
  font-size: 14px;
  font-variant-numeric: tabular-nums;
  text-align: right;
}

/* 服务器地址是连接信息中的长标识：任何 rail 宽度都只占一行，鼠标悬停可查看全量值。 */
.info-row--vpn-server dd {
  overflow: hidden;
  overflow-wrap: normal;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.connection-summary-list,
.connection-details-list {
  gap: var(--space-2);
}

.connection-details-list {
  margin-top: var(--space-3);
}

.connection-summary-list .info-row dd,
.connection-details-list .info-row dd {
  font-weight: 600;
}

.sidebar-section { min-height:0; overflow:auto; scrollbar-width:thin; }
.info-row { grid-template-columns:80px minmax(0,1fr); gap:6px; }
.info-row dd { font-size:13px; }
.info-row dt { font-size:12px; }
.connection-sidebar__status-pill { flex:none; }
.server-address summary { overflow:hidden; text-overflow:ellipsis; cursor:pointer; list-style:none; }
.server-address[open] { white-space:normal; overflow-wrap:anywhere; }
.server-address[open] summary { color:var(--accent); white-space:normal; overflow-wrap:anywhere; }
.server-address > span { display:block; margin-top:4px; font-size:12px; font-weight:400; }
</style>
