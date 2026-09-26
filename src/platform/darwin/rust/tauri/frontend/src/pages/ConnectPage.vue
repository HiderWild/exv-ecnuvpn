<script setup lang="ts">
import { computed, inject } from "vue";
import ProductActionBar from "../components/ProductActionBar.vue";
import ProductConnectionHero from "../components/ProductConnectionHero.vue";
import ProductConnectionVisualStage from "../components/ProductConnectionVisualStage.vue";
import { CONNECTION_INTERACTION_KEY } from "../product/connection-interaction";
const emit = defineEmits<{ navigate: [page: 'settings'] }>();

const { state: productState, busy: actionBusy, autoInstallService, runAction } = inject(CONNECTION_INTERACTION_KEY)!;
const stateDescription = computed(() => {
  const state = productState.value;
  if (state === null) return "";
  if (state.description?.trim()) return state.description.trim();
  const descriptions = {
    idle: "",
    connecting: "",
    awaiting: "请完成账户确认。",
    connected: "",
    stopping: "正在安全撤销连接配置。",
    reconciling: "正在核对连接状态。",
    failed: "连接未完成，请查看需要处理的原因。",
  } as const;
  return descriptions[state.status];
});
</script>

<template>
  <section class="connect-page" aria-labelledby="connection-state-title">
    <p v-if="productState === null" class="runtime-unavailable" data-testid="runtime-unavailable">
      暂时无法取得连接状态。
    </p>

    <template v-else>
      <ProductConnectionHero
        :state="productState"
        :description="stateDescription"
        :busy="actionBusy"
        :show-auto-install-option="productState.service.status.kind === 'not_installed' && productState.status === 'idle'"
        :auto-install="autoInstallService"
        @action="runAction"
        @settings="emit('navigate', 'settings')"
        @update:auto-install="autoInstallService = $event"
      />

      <div class="connection-layout">
        <ProductConnectionVisualStage :state="productState" />
      </div>

      <footer class="connection-details" aria-label="连接详情"><ProductActionBar :state="productState" class="connect-page__action-bar" /></footer>
    </template>
  </section>
</template>

<style scoped>
.connect-page {
  display: flex;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  min-width: 0;
  --connect-section-gap: var(--space-3);
  gap: var(--connect-section-gap);
  padding-bottom: var(--connect-section-gap);
  color: var(--text-primary);
  overflow: clip;
}

.runtime-unavailable,
.operation-pending {
  margin: 0;
  padding: var(--space-4);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--surface-panel);
  color: var(--text-secondary);
  font-size: 14px;
}

.operation-pending {
  margin-bottom: var(--space-4);
}

.connection-layout {
  container:connection-main / inline-size;
  display: grid;
  flex: 1 1 0;
  margin-top: 0;
  min-height: 0;
  min-width: 0;
  overflow: clip;
}

.connect-page__action-bar {
  margin-top: 0;
  margin-bottom: 0;
  flex: none;
}

.connection-details { flex:none; border-top:1px solid var(--border-subtle); padding-top:8px; font-size:13px; color:var(--text-secondary); }
</style>
