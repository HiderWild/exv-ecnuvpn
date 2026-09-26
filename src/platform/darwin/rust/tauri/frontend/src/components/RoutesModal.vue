<script setup lang="ts">
import RoutesEditor from "./RoutesEditor.vue";
import { useDialogFocus } from "./use-dialog-focus";

/**
 * 设置页的独立路由编辑容器。路由编辑能力由 RoutesEditor 唯一提供；此处只保留
 * 模态承载、取消与“确认后再由 SettingsPage 统一保存”的契约。
 */
const props = withDefaults(defineProps<{
  routes: string[];
  open: boolean;
  busy?: boolean;
  errorMessage?: string;
}>(), {
  busy: false,
  errorMessage: "",
});

const emit = defineEmits<{
  save: [routes: string[]];
  close: [];
}>();

function requestClose(): void {
  if (!props.busy) emit("close");
}
const { dialog, onDialogKeydown } = useDialogFocus(() => props.open, requestClose);
</script>

<template>
  <div
    v-if="open"
    class="modal-overlay"
    data-testid="routes-modal"
    role="dialog"
    aria-modal="true"
    aria-labelledby="routes-modal-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
    :aria-busy="busy"
    @click.self="requestClose"
  >
    <div class="modal-card routes-modal-card">
      <h2 id="routes-modal-title">路由设置</h2>
      <p class="modal-hint">经过 VPN 的目标网段；支持 CIDR（如 10.0.0.0/8）或单个 IP（视为 /32）。确认修改后，请在设置页点击“保存设置”应用。</p>
      <p v-if="errorMessage" class="routes-save-error" data-testid="routes-save-error" role="alert">{{ errorMessage }}</p>
      <RoutesEditor
        :routes="routes"
        :busy="busy"
        side-by-side
        show-actions
        @save="emit('save', $event)"
        @close="requestClose"
      />
    </div>
  </div>
</template>

<style scoped>
.modal-overlay { position: fixed; inset: 0; z-index: 50; display: flex; align-items: center; justify-content: center; padding: var(--space-5); background: rgb(0 0 0 / 0.45); overflow: hidden; }
.routes-modal-card { position: relative; width: min(100%, 820px); max-height: min(86vh, 820px); display: flex; flex-direction: column; overflow: hidden; padding: var(--space-6); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: var(--surface-panel); box-shadow: 0 12px 32px rgb(0 0 0 / 0.28); color: var(--text-primary); }
.routes-modal-card h2 { margin: 0 0 var(--space-2); font-size: 18px; }
.modal-hint { margin: 0 0 var(--space-4); color: var(--text-secondary); font-size: 13px; }
.routes-save-error { margin: 0 0 var(--space-3); padding: var(--space-2) var(--space-3); border-left: 2px solid var(--state-danger); background: color-mix(in srgb, var(--state-danger) 8%, transparent); color: var(--state-danger); font-size: 12px; line-height: 1.45; }
@media (max-width: 700px) { .routes-modal-card { padding: var(--space-4); } }
</style>
