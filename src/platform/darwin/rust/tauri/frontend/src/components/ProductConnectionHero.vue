<script setup lang="ts">
import type { ProductUiState } from "../product/types";
import ConnectionActionButton from "./ConnectionActionButton.vue";

const props = withDefaults(defineProps<{
  state: ProductUiState;
  description: string;
  busy: boolean;
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
    aria-labelledby="connection-state-title"
  >
    <div class="product-connection-hero__copy">
      <h1 id="connection-state-title" tabindex="-1" aria-live="polite" aria-atomic="true">{{ state.title }}</h1>
      <p v-if="description" aria-live="polite" aria-atomic="true">{{ description }}</p>
      <div v-if="state.status === 'idle' || state.status === 'failed'" class="connection-target">
        <button type="button" @click="emit('settings')">连接设置 <span aria-hidden="true">↗</span></button>
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

      <ConnectionActionButton :state="state" :busy="busy" @action="emit('action')" />
    </div>
  </section>
</template>

<style scoped>
.product-connection-hero { margin-top:var(--product-page-top-gap); display:flex; min-height:88px; flex:none; align-items:center; justify-content:space-between; gap:24px; padding:12px 0 24px; border-bottom:1px solid var(--border-subtle); }
.product-connection-hero__copy { min-width:0; }
.product-connection-hero h1 { margin:0; font-size:28px; letter-spacing:-.025em; line-height:1.25; font-weight:650; }
.product-connection-hero__copy > p { max-width:44rem; margin:8px 0 0; color:var(--text-secondary); font-size:13px; line-height:1.5; overflow-wrap:anywhere; }
.product-connection-hero__auto-install { display:inline-flex; align-items:center; gap:8px; margin-top:12px; color:var(--text-secondary); font-size:13px; }
.product-connection-hero__auto-install input { width:16px; height:16px; margin:0; accent-color:var(--accent); }
.product-connection-hero__action { flex:none; }
.product-connection-hero__action :deep(.connection-action) { min-width:104px; min-height:40px; }
.product-connection-hero[data-status="connected"] :deep(.connection-action) { background:var(--surface-panel); color:var(--text-primary); border-color:var(--border-strong); }
.product-connection-hero[data-severity="error"] h1,.product-connection-hero[data-severity="blocking"] h1 { color:var(--state-danger); }
.connection-target { display:flex; align-items:center; flex-wrap:wrap; gap:6px 12px; margin-top:10px; color:var(--text-secondary); font-size:12px; }
.connection-target > button { background:transparent; border:0; min-height:26px; padding:2px 0; color:var(--accent-strong); font-size:12px; }
</style>
