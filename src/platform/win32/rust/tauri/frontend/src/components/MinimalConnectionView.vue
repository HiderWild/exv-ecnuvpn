<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import type { ProductConnectionAction } from "../product/connection-action";
import type { Protocol } from "../product/core-config";
import type { MinimalCredentialDraft } from "../product/minimal-credentials";
import { planMinimalCredentials } from "../product/minimal-credentials";
import type { ProductUiState } from "../product/types";
import AtrustBrowserFallback from "./AtrustBrowserFallback.vue";
import ConnectionProgressCard from "./ConnectionProgressCard.vue";
import ConnectionActionContent from "./ConnectionActionContent.vue";

// 328×136宿主预算减去48px标题栏；只呈现状态、说明、唯一动作和紧凑进度。
// 连接前凭据仍由同一宿主模态处理，不复制表单、保存状态或用户偏好。
const props = withDefaults(defineProps<{
  state: ProductUiState;
  action: ProductConnectionAction;
  protocol?: Protocol;
  initialUsername?: string;
  initialServer?: string;
  rememberPassword?: boolean;
  hasStoredPassword?: boolean;
}>(), { protocol:"cisco", initialUsername:"", initialServer:"", rememberPassword:false, hasStoredPassword:false });
const emit = defineEmits<{
  run: [draft?: MinimalCredentialDraft];
  "request-credentials": [request: { missingUsername: boolean; username: string }];
}>();
const plan = computed(() => planMinimalCredentials(
  { username:props.initialUsername, password:"", remember:true },
  { hasStoredPassword:props.hasStoredPassword, storedUsername:props.initialUsername },
));
const description = computed(() => {
  if (props.state.description?.trim()) return props.state.description.trim();
  if (props.state.status === "failed") return props.state.disconnectSemantics
    ? "连接已断开，请重新连接。" : "连接失败，请重试；多次失败请导出日志反馈。";
  return {
    idle:"连接校园 VPN，访问图书馆、课程平台与校内资源。",
    connecting:"正在建立校园 VPN 连接，请稍候。",
    awaiting:"请完成账户确认。",
    connected:"安全隧道已建立，可访问校内资源。",
    stopping:"正在安全撤销连接配置。",
    reconciling:"正在核对连接状态。",
  }[props.state.status];
});
const actionButton = ref<HTMLButtonElement | null>(null);
const statusTitle = ref<HTMLHeadingElement | null>(null);
async function restoreActionFocus(): Promise<void> {
  await nextTick();
  (props.action.enabled ? actionButton.value : statusTitle.value)?.focus();
}
function requestAction(): void {
  if (!props.action.enabled) return;
  if (props.protocol === "atrust") { emit("run", undefined); return; }
  // 取消和断开不受连接前凭据预检影响。
  if (plan.value.blocked && props.action.kind === "connect") {
    emit("request-credentials", { missingUsername:!props.initialUsername.trim(), username:props.initialUsername });
    return;
  }
  emit("run", { username:props.initialUsername, password:"", remember:true });
}
</script>

<template>
  <section class="minimal-connection-view" aria-label="极简连接">
    <div class="minimal-connection-view__body">
      <h1 ref="statusTitle" :data-status="state.status" :data-disconnected="state.disconnectSemantics === true" class="minimal-connection-view__title" data-testid="minimal-status" aria-live="polite" aria-atomic="true" tabindex="-1" :title="state.title">{{ state.title }}</h1>
      <div class="minimal-connection-view__description">
        <p data-testid="minimal-description" :title="description">{{ description }}</p>
        <AtrustBrowserFallback />
      </div>
      <button ref="actionButton" class="minimal-connection-view__action" data-testid="minimal-action" type="button" :disabled="!action.enabled" @click="requestAction"><ConnectionActionContent :action="action" /></button>
    </div>
    <ConnectionProgressCard :state="state" compact @return-focus="restoreActionFocus" />
  </section>
</template>

<style scoped>
/* 按钮祖先不建立滚动裁切；左右余量同时容纳外焦点圈和悬停阴影。 */
.minimal-connection-view { display:flex; width:100%; height:100%; min-width:0; min-height:0; flex-direction:column; padding:6px 16px; color:var(--text-primary); }
.minimal-connection-view__body { display:grid; flex:1 1 0; min-width:0; min-height:0; grid-template-columns:minmax(0,1fr) auto; align-content:center; align-items:center; gap:2px 12px; }
.minimal-connection-view__title { grid-column:1; align-self:end; min-width:0; margin:0; overflow:hidden; font-size: var(--font-size-heading); line-height:1.2; font-weight:600; text-overflow:ellipsis; white-space:nowrap; }
.minimal-connection-view__title[data-disconnected="true"] { color:var(--state-danger); }
.minimal-connection-view__description { grid-column:1; align-self:start; min-width:0; color:var(--text-secondary); font-size: var(--font-size-caption); line-height:1.3; overflow-wrap:anywhere; }
.minimal-connection-view__description > p { margin:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.minimal-connection-view__action { display:inline-flex; align-items:center; justify-content:center; gap:6px; grid-column:2; grid-row:1 / span 2; min-width:0; min-height:32px; padding:6px 8px; border:1px solid var(--accent); border-radius:8px; background:var(--accent); color:var(--accent-on); font-size: var(--font-size-body); font-weight:600; line-height:1.1; }
.minimal-connection-view__action:hover:not(:disabled) { border-color:var(--accent-strong); background:var(--accent-strong); }
.minimal-connection-view__action:disabled { border-color:var(--border-subtle); background:var(--surface-subtle); color:var(--text-secondary); }
</style>
