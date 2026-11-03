<script setup lang="ts">
import { computed, inject, nextTick, ref } from "vue";
import ProductActionBar from "../components/ProductActionBar.vue";
import ProductConnectionHero from "../components/ProductConnectionHero.vue";
import ProductConnectionVisualStage from "../components/ProductConnectionVisualStage.vue";
import ConnectionSessionDetails from "../components/ConnectionSessionDetails.vue";
import ConnectionProgressCard from "../components/ConnectionProgressCard.vue";
import { CONNECTION_INTERACTION_KEY } from "../product/connection-interaction";
const emit = defineEmits<{ navigate: [page: 'settings' | 'logs'] }>();
const page = ref<HTMLElement | null>(null);
async function restoreConnectionFocus() {
  await nextTick();
  const target = page.value?.querySelector<HTMLElement>('[data-testid="connection-action"]:not(:disabled)')
    ?? page.value?.querySelector<HTMLElement>('#connection-state-title');
  target?.focus();
}

const { state: productState, action: connectionAction, busy: actionBusy, autoInstallService, runAction } = inject(CONNECTION_INTERACTION_KEY)!;
const stateDescription = computed(() => {
  const state = productState.value;
  if (state === null) return "";
  if (state.description?.trim()) return state.description.trim();
  // 阶段①（「需要处理」分流）：失败兜底句按断开语义给出行动指引；旧句
  // 「请查看需要处理的原因」指向一个不存在的入口，已退役。
  if (state.status === "failed") {
    return state.disconnectSemantics === true
      ? "连接已断开，请重新连接。"
      : "连接失败，请重试；多次失败请导出日志反馈。";
  }
  const descriptions = {
    idle: "连接校园 VPN，访问图书馆、课程平台与校内资源。",
    connecting: "正在建立校园 VPN 连接，请稍候。",
    awaiting: "请完成账户确认。",
    connected: "安全隧道已建立，可访问校内资源。",
    stopping: "正在安全撤销连接配置。",
    reconciling: "正在核对连接状态。",
  } as const;
  return descriptions[state.status];
});
</script>

<template>
  <section ref="page" class="connect-page" aria-labelledby="connection-state-title">
    <p v-if="productState === null" class="runtime-unavailable" data-testid="runtime-unavailable">
      暂时无法取得连接状态。
    </p>

    <template v-else>
      <div class="connection-layout">
        <ProductConnectionVisualStage :state="productState">
          <ProductConnectionHero
            :state="productState"
            :description="stateDescription"
            :busy="actionBusy"
            :action="connectionAction"
            :show-auto-install-option="productState.service.status.kind === 'not_installed' && productState.status === 'idle'"
            :auto-install="autoInstallService"
            @action="runAction"
            @settings="emit('navigate', 'settings')"
            @logs="emit('navigate', 'logs')"
            @update:auto-install="autoInstallService = $event"
          />
        </ProductConnectionVisualStage>
        <ConnectionSessionDetails :state="productState" :expanded="productState.status === 'connected'" @return-focus="restoreConnectionFocus" />
      </div>

      <ConnectionProgressCard :state="productState" @return-focus="restoreConnectionFocus" />
      <ProductActionBar :state="productState" class="connect-page__action-bar" />
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
  gap: 0;
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
  font-size: var(--font-size-body);
}

.operation-pending {
  margin-bottom: var(--space-4);
}


.connection-layout {
  display: flex;
  flex-direction: column;
  flex: 1 1 0;
  margin-top: 0;
  min-height: 0;
  min-width: 0;
  overflow: clip;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: var(--surface-panel);
}

.connect-page__action-bar {
  margin-top: 16px;
  margin-bottom: 0;
  flex: none;
}

</style>
