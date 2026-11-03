<script setup lang="ts">
import type { ProductUiState } from "../product/types";
import type { ProductConnectionAction } from "../product/connection-action";
import ConnectionActionButton from "./ConnectionActionButton.vue";
import ConnectionActionContent from "./ConnectionActionContent.vue";

const props = withDefaults(defineProps<{
  state: ProductUiState;
  description: string;
  busy: boolean;
  /**
   * 修复 3b：宿主预计算的连接动作（连接在飞期间任意相位为「取消」）；缺省由
   * ConnectionActionButton 按快照现算（独立挂载兼容）。
   */
  action?: ProductConnectionAction;
  /** 由页面按“服务明确未安装且连接空闲”这一业务条件决定是否展示。 */
  showAutoInstallOption?: boolean;
  autoInstall?: boolean;
}>(), {
  showAutoInstallOption: false,
  autoInstall: false,
});

const emit = defineEmits<{
  action: [];
  settings: [];
  logs: [];
  "update:autoInstall": [value: boolean];
}>();

function updateAutoInstall(event: Event): void {
  emit("update:autoInstall", (event.target as HTMLInputElement).checked);
}
</script>

<template>
  <section
    class="product-connection-hero"
    data-testid="product-connection-hero"
    :data-status="state.status"
    :data-severity="state.severity"
    :data-disconnected="state.disconnectSemantics"
    aria-labelledby="connection-state-title"
  >
    <div class="product-connection-hero__copy">
      <h1 id="connection-state-title" tabindex="-1" aria-live="polite" aria-atomic="true">{{ state.title }}</h1>
      <p v-if="description" aria-live="polite" aria-atomic="true">{{ description }}</p>
      <div v-if="state.status === 'failed'" class="connection-target">
        <button type="button" @click="emit('settings')">连接设置 <span aria-hidden="true">↗</span></button>
        <button
          v-if="state.status === 'failed'"
          data-testid="connection-error-logs"
          type="button"
          @click="emit('logs')"
        >查看日志</button>
      </div>
      <label
        v-if="props.showAutoInstallOption"
        class="product-connection-hero__auto-install"
        data-testid="hero-auto-install-option"
      >
        <input
          type="checkbox"
          data-testid="hero-auto-install"
          :checked="props.autoInstall"
          :disabled="busy"
          @change="updateAutoInstall"
        >
        <span>安装服务后连接</span>
      </label>
    </div>

    <div class="product-connection-hero__action">

      <ConnectionActionButton :state="state" :busy="busy" :action="action" @action="emit('action')">
        <template #default="{ action: resolved }">
          <ConnectionActionContent :action="resolved" />
        </template>
      </ConnectionActionButton>
    </div>
  </section>
</template>

<style scoped>
.product-connection-hero { display:flex; flex-direction:column; flex:none; min-height:0; align-items:flex-start; gap:17px; padding:0; }
.product-connection-hero__copy { min-width:0; min-height:0; }
.product-connection-hero h1 { margin:0; font-size: var(--font-size-display); letter-spacing:-.025em; line-height:1.25; font-weight:600; }
.product-connection-hero__copy > p { max-width:290px; margin:9px 0 0; color:var(--text-secondary); font-size: var(--font-size-caption); line-height:1.6; overflow-wrap:anywhere; }
.product-connection-hero__auto-install { display:inline-flex; align-items:center; gap:8px; margin-top:12px; color:var(--text-secondary); font-size: var(--font-size-caption); }
.product-connection-hero__auto-install input { width:16px; height:16px; margin:0; accent-color:var(--accent); }
.product-connection-hero__action { flex:none; padding:3px; margin:0; }
.product-connection-hero__action :deep(.connection-action) { display:inline-flex; align-items:center; justify-content:center; gap:8px; min-width:0; min-height:36px; padding:8px 13px; font-size: var(--font-size-body); font-weight:600; border-radius:8px; }
.product-connection-hero[data-severity="error"] h1,.product-connection-hero[data-severity="blocking"] h1,.product-connection-hero[data-disconnected="true"] h1 { color:var(--state-danger); }
.connection-target { display:flex; align-items:center; flex-wrap:wrap; gap:6px 12px; margin-top:10px; color:var(--text-secondary); font-size: var(--font-size-caption); }
.connection-target > button { background:transparent; border:0; min-height:26px; padding:2px 0; color:var(--accent-strong); font-size: var(--font-size-body); }
@container connection-content (max-height:420px) { .product-connection-hero h1 { font-size: var(--font-size-heading); } .product-connection-hero { gap:12px; } }
@container connection-content (max-width:420px) { .product-connection-hero h1 { font-size: var(--font-size-heading); } .product-connection-hero__copy > p { font-size: var(--font-size-caption); } }
</style>
