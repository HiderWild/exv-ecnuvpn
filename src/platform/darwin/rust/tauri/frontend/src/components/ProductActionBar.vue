<script setup lang="ts">
import { computed } from "vue";
import { isServiceRunning, serviceStateLabel, type ProductUiState } from "../product/types";

const props = defineProps<{ state: ProductUiState }>();

const systemProxyEnabled = computed(() => ["manual", "automatic", "mixed"].includes(props.state.systemProxy?.status));
const proxyTunEnabled = computed(() => props.state.proxyTun?.status === "detected");

const serviceStatus = computed(() => {
  const status = props.state.service.status;
  if (status.kind === "unknown") return "状态未知";
  if (status.kind === "not_installed") return "未安装";
  return serviceStateLabel(status.scmState);
});

/** S6b 胶囊状态色：服务仅「运行中」着色；未安装/状态未知/中间态一律灰。 */
const serviceRunning = computed(() => {
  const status = props.state.service.status;
  return status.kind === "installed" && isServiceRunning(status.scmState);
});

</script>

<template>
  <section class="product-action-bar" data-testid="product-action-bar" aria-label="连接环境摘要">
    <div class="product-action-bar__item" data-testid="product-service-status" :data-enabled="serviceRunning">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16M4 12h16M4 18h16"/><circle cx="8" cy="6" r="2"/><circle cx="16" cy="12" r="2"/><circle cx="10" cy="18" r="2"/></svg>
      <span>服务</span>
      <strong>{{ serviceStatus }}</strong>
    </div>
    <div class="product-action-bar__item" data-testid="system-proxy-status" :data-enabled="systemProxyEnabled">
      <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/></svg>
      <span>系统代理</span>
      <strong>{{ systemProxyEnabled ? '已开启' : '未开启' }}</strong>
    </div>
    <div class="product-action-bar__item" data-testid="tun-status" :data-enabled="proxyTunEnabled">
      <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="5" r="2"/><rect x="3" y="16" width="6" height="4" rx="1"/><rect x="15" y="16" width="6" height="4" rx="1"/><path d="M12 7v5M6 16v-4h12v4"/></svg>
      <span>TUN</span>
      <strong>{{ proxyTunEnabled ? '已开启' : '未开启' }}</strong>
    </div>
  </section>
</template>

<style scoped>
.product-action-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  min-width: 0;
  gap: 10px;
  padding: 0;

}

.product-action-bar__item {
  display: inline-flex;
  min-width: 0;
  align-items: center;
  gap: 7px;
  padding: 8px 11px;
  border: 1px solid var(--border-subtle);
  border-radius: 999px;
  background: var(--surface-panel);
  color: var(--text-secondary);
  font-size: var(--font-size-body);
  line-height: 1.6;
}

.product-action-bar__item svg { flex:none; width:14px; height:14px; fill:none; stroke:currentColor; stroke-width:1.6; stroke-linecap:round; stroke-linejoin:round; }
.product-action-bar__item > span {
  color: var(--text-secondary);
  font-size: inherit;
}

.product-action-bar__item > strong {
  min-width: 0;
  font-size: var(--font-size-body);
  font-weight: 500;
  overflow-wrap: anywhere;
}

.product-action-bar__item[data-enabled="true"] { color:var(--text-primary); }
/* S6b 状态色映射：开启/运行中 = --state-success（绿），未开启/未安装/中间态 = 默认灰。 */
.product-action-bar__item[data-enabled="true"] > strong { color:var(--state-success); }
@container connection-content (max-width:420px) {
  .product-action-bar { gap:5px; }
  .product-action-bar__item { padding:6px 8px; font-size: var(--font-size-body); }
}

</style>
