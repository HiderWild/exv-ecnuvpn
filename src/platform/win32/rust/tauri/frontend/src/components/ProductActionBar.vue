<script setup lang="ts">
import { computed } from "vue";
import { serviceStateLabel, type ProductUiState } from "../product/types";

const props = defineProps<{ state: ProductUiState }>();

const systemProxyEnabled = computed(() => ["manual", "automatic", "mixed"].includes(props.state.systemProxy.status));
const proxyTunEnabled = computed(() => props.state.proxyTun.status === "detected");

const serviceStatus = computed(() => {
  const status = props.state.service.status;
  if (status.kind === "unknown") return "状态未知";
  if (status.kind === "not_installed") return "未安装";
  return serviceStateLabel(status.scmState);
});

const serviceMode = computed(() => {
  const labels = { auto: "自动", service: "服务", oneshot: "一次性", unknown: "未知" } as const;
  return labels[props.state.service.mode];
});

</script>

<template>
  <section class="product-action-bar" data-testid="product-action-bar" aria-label="连接环境摘要">
    <div class="product-action-bar__item" data-testid="product-service-status">
      <span>服务</span>
      <strong>{{ serviceStatus }}</strong>
      <small>{{ serviceMode }}</small>
    </div>
    <div class="product-action-bar__item" data-testid="system-proxy-status" :data-enabled="systemProxyEnabled">
      <span>系统代理</span>
      <strong>{{ state.systemProxy.status === 'unknown' ? '未知' : systemProxyEnabled ? '开启' : '关闭' }}</strong>
    </div>
    <div class="product-action-bar__item" data-testid="tun-status" :data-enabled="proxyTunEnabled">
      <span>TUN</span>
      <strong>{{ state.proxyTun.status === 'unknown' ? '未知' : proxyTunEnabled ? '已检测' : '未检测' }}</strong>
    </div>
  </section>
</template>

<style scoped>
.product-action-bar {
  display: grid;
  min-width: 0;
  grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  gap: var(--space-3);
  padding: var(--space-3) 0;

}

.product-action-bar__item {
  display: grid;
  min-width: 0;
  grid-template-columns: auto minmax(0, 1fr);
  align-items: baseline;
  column-gap: var(--space-2);
}

.product-action-bar__item > span {
  color: var(--text-secondary);
  font-size: 12px;
}

.product-action-bar__item > strong {
  min-width: 0;
  overflow: hidden;
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.product-action-bar__item > small {
  grid-column: 2;
  min-width: 0;
  overflow: hidden;
  color: var(--text-tertiary);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

</style>
