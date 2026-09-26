<script setup lang="ts">
import { computed } from "vue";
import type { ProductUiState } from "../product/types";
import ConnectionStageList from "./ConnectionStageList.vue";
const props = defineProps<{ state:ProductUiState; paused?:boolean }>();
const connected = computed(() => props.state.status === "connected");
const metrics = computed(() => props.state.metrics?.availability === "available" ? props.state.metrics : null);
const hasStages = computed(() => props.state.stages.length > 0 && ["connecting","awaiting"].includes(props.state.status));
const reconnect = computed(() => props.state.reconnect.active ? `重连中 · 第 ${props.state.reconnect.currentAttempt} 次` : `已重连 ${props.state.reconnect.currentAttempt} 次`);
</script>

<template>
  <aside class="runtime-panel" data-testid="connection-runtime-panel" aria-label="本次连接">
    <div v-if="hasStages" class="runtime-panel__operation">
      <h2>连接进展</h2>
      <ConnectionStageList :stages="state.stages" :paused="paused" :awaiting="state.status === 'awaiting'" />
    </div>
    <div v-if="connected" class="runtime-panel__session">
      <h2>在线时长</h2>
      <strong class="runtime-panel__duration" data-testid="runtime-online">{{ state.metrics?.online ?? '—' }}</strong>
      <div class="runtime-panel__traffic" aria-label="本次上下行流量">
        <div><span>↑ 上传</span><strong>{{ metrics?.uploadRate ?? '—' }}</strong><small>累计 {{ metrics?.uploadTotal ?? '—' }}</small></div>
        <div><span>↓ 下载</span><strong>{{ metrics?.downloadRate ?? '—' }}</strong><small>累计 {{ metrics?.downloadTotal ?? '—' }}</small></div>
      </div>
    </div>
    <dl class="runtime-panel__identity">
      <div v-if="connected"><dt>校内地址</dt><dd>{{ state.connectionInfo.campusIp ?? '—' }}</dd></div>
      <div><dt>配置账户</dt><dd>{{ state.connectionInfo.account ?? '—' }}</dd></div>
      <div><dt>配置服务器</dt><dd>{{ state.connectionInfo.vpnServer ?? '—' }}</dd></div>
    </dl>
    <div v-if="state.reconnect.enabled || state.reconnect.active" class="runtime-panel__reconnect" data-testid="reconnect-status"><span>自动重连</span><strong>{{ reconnect }}</strong></div>
  </aside>
</template>

<style scoped>
.runtime-panel { min-width:0; width:100%; display:grid; grid-template-columns:minmax(0,1fr); align-content:center; gap:18px; padding:8px 0; }
h2 { margin:0 0 6px; font-size:12px; font-weight:500; color:var(--text-secondary); }
.runtime-panel__duration { font-size:27px; font-weight:600; letter-spacing:-.04em; font-variant-numeric:tabular-nums; line-height:1.3; }
.runtime-panel__traffic { display:grid; grid-template-columns:1fr 1fr; gap:12px; margin-top:12px; }
.runtime-panel__traffic > div { display:flex; flex-direction:column; gap:5px; min-width:0; }
.runtime-panel__traffic span { font-size:12px; color:var(--text-secondary); }
.runtime-panel__traffic strong { font-size:15px; font-weight:600; font-variant-numeric:tabular-nums; white-space:nowrap; }
.runtime-panel__traffic small { font-size:11px; color:var(--text-tertiary); }
.runtime-panel__identity { margin:0; display:grid; gap:10px; }
.runtime-panel__identity > div { display:grid; grid-template-columns:66px minmax(0,1fr); align-items:baseline; gap:10px; }
dt { font-size:11px; color:var(--text-secondary); }
dd { margin:0; font-size:13px; color:var(--text-primary); overflow-wrap:anywhere; font-variant-numeric:tabular-nums; }
.runtime-panel__reconnect { display:grid; grid-template-columns:66px minmax(0,1fr); align-items:baseline; gap:10px; font-size:12px; }
.runtime-panel__reconnect > span { color:var(--text-secondary); }
.runtime-panel__reconnect strong { font-weight:500; }
.runtime-panel__operation :deep(.stage-reel) { margin:0; }
.runtime-panel__operation :deep(.stage-reel__row) { justify-content:flex-start; }
@media (max-height:680px) {
  .runtime-panel { gap:12px; padding:4px 0; }
  .runtime-panel__identity { gap:7px; }
  .runtime-panel__traffic { margin-top:8px; }
}
</style>
