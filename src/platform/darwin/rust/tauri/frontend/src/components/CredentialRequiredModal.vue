<script setup lang="ts">
import PasswordField from "./PasswordField.vue";
import { ref, watch } from "vue";
import { useDialogFocus } from "./use-dialog-focus";

import type { ConnectCredentials } from "../lib/ipc";

const props = withDefaults(defineProps<{
  visible: boolean;
  busy?: boolean;
  cancelling?: boolean;
  errorMessage?: string;
  resetKey?: number;
  initialUsername?: string;
  /** 极简窗内紧凑渲染变体；不改变字段与提交语义。 */
  compact?: boolean;
}>(), {
  busy: false,
  cancelling: false,
  errorMessage: "",
  resetKey: 0,
  initialUsername: "",
  compact: false,
});

const emit = defineEmits<{
  submit: [credentials: ConnectCredentials];
  close: [];
}>();

const username = ref("");
const password = ref("");
const validationMessage = ref("");
let draftInitialized = false;

function initializeDraft(): void {
  username.value = props.initialUsername.trim();
  password.value = "";
  validationMessage.value = "";
  draftInitialized = true;
}

function clearDraft(): void {
  username.value = "";
  password.value = "";
  validationMessage.value = "";
  draftInitialized = false;
}

// 只在一段新草稿或父级明确重置时取安全的用户名投影；切换完整/极简模式会暂时
// 隐藏模态，但不能抹掉用户尚未提交的草稿。密码始终从不回显。
watch(() => props.visible, (visible) => {
  if (visible && !draftInitialized) initializeDraft();
}, { immediate: true });
watch(() => props.resetKey, () => {
  if (props.visible) initializeDraft();
  else clearDraft();
});

function submit(persist: boolean): void {
  if (!username.value.trim() || !password.value) {
    validationMessage.value = "请填写账户和密码。";
    return;
  }
  validationMessage.value = "";
  emit("submit", { username: username.value.trim(), password: password.value, persist });
}

function close(): void {
  if (props.busy) return;
  emit("close");
}
const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, close);
</script>

<template>
  <div
    v-if="visible"
    class="credential-modal-overlay"
    data-testid="credential-required-modal"
    :data-compact="compact ? 'true' : 'false'"
    role="dialog"
    aria-modal="true"
    aria-labelledby="credential-required-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
  >
    <form class="credential-modal-card" @submit.prevent="submit(false)">
      <div class="credential-modal-body">
        <h2 id="credential-required-title">需要账户凭据</h2>
        <p class="credential-modal-intro">填写后即可继续本次连接。选择“记住密码”才会保存到本机配置。</p>
        <p v-if="cancelling" class="credential-modal-cancelling" data-testid="credential-cancelling" role="status">正在取消连接，等待运行时确认…</p>

        <label class="credential-modal-field">
          <span>账户</span>
          <input
            v-model="username"
            data-testid="credential-username"
            autocomplete="username"
            :disabled="busy"
          >
        </label>
        <label class="credential-modal-field">
          <span>密码</span>
          <PasswordField
            v-model="password"
            data-testid="credential-password"
            :disabled="busy"
            :compact="compact"
          />
        </label>

        <p v-if="validationMessage || errorMessage" class="credential-modal-error" data-testid="credential-error" role="alert">
          {{ validationMessage || errorMessage }}
        </p>
      </div>

      <div class="credential-modal-actions">
        <button type="button" class="credential-modal-cancel" data-testid="credential-cancel" :disabled="busy" @click="close">{{ cancelling ? "取消中…" : "取消" }}</button>
        <button type="button" :disabled="busy" data-testid="credential-submit-once" @click="submit(false)">本次使用</button>
        <button type="button" class="credential-modal-primary" :disabled="busy" data-testid="credential-submit-remember" @click="submit(true)">
          记住密码
        </button>
      </div>
    </form>
  </div>
</template>

<style scoped>
.credential-modal-overlay {
  position: fixed;
  inset: 0;
  z-index: 56;
  display: grid;
  place-items: center;
  padding: var(--space-5);
  background: rgb(0 0 0 / 0.45);
}

.credential-modal-card {
  max-height: calc(100dvh - 40px);
  overflow-y: auto;
  overflow-wrap: anywhere;
  display: grid;
  width: min(100%, 440px);
  gap: var(--space-4);
  padding: var(--space-6);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: var(--surface-panel);
  box-shadow: 0 12px 32px rgb(0 0 0 / 0.28);
  color: var(--text-primary);
}

.credential-modal-card h2,
.credential-modal-intro,
.credential-modal-error,
.credential-modal-cancelling { margin: 0; }
.credential-modal-card h2 { font-size: 18px; }
.credential-modal-intro { color: var(--text-secondary); font-size: 13px; line-height: 1.6; }
.credential-modal-field { display: grid; gap: var(--space-2); color: var(--text-secondary); font-size: 13px; }
.credential-modal-field input { min-height: 36px; padding: 0 var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-primary); }
.credential-modal-error { color: var(--state-danger); font-size: 13px; line-height: 1.5; }
.credential-modal-cancelling { color: var(--text-secondary); font-size: 13px; line-height: 1.5; }
.credential-modal-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--space-2); }
.credential-modal-actions button { min-height: 34px; padding: var(--space-1) var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-primary); cursor: pointer; }
.credential-modal-actions .credential-modal-primary { border-color: var(--accent); background: var(--accent); color: var(--accent-on); }
.credential-modal-actions button:disabled { cursor: wait; opacity: 0.65; }

/* 极简紧凑变体：面板收窄到 min(100%, 294px) 且内容区裁切，动作按钮不换行。
   覆盖本模态自己的 440px 宽 / space-6 内边距 / 18px 标题。 */
.credential-modal-overlay[data-compact="true"] {
  padding: 8px;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-card {
  display: flex;
  width: min(100%, 294px);
  max-height: calc(100vh - 28px);
  flex-direction: column;
  gap: 6px;
  overflow: hidden;
  padding: 8px;
  border-radius: var(--radius-md);
}

/* 内容区裁切，动作区始终可见（对齐历史 ModalShell 的 body/actions 分工）。 */
.credential-modal-overlay[data-compact="true"] .credential-modal-body {
  display: grid;
  min-height: 0;
  flex: 1 1 auto;
  gap: 4px;
  overflow: hidden;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-card h2 {
  overflow: hidden;
  font-size: 12px;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-intro {
  font-size: 11px;
  line-height: 1.25;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-field {
  gap: 2px;
  font-size: 11px;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-field input {
  min-height: 24px;
  padding: 2px 6px;
  font-size: 11px;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-error,
.credential-modal-overlay[data-compact="true"] .credential-modal-cancelling {
  font-size: 11px;
  line-height: 1.2;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-actions {
  flex: 0 0 auto;
  flex-wrap: nowrap;
  gap: 4px;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-actions button {
  min-width: 0;
  min-height: 22px;
  padding: 2px 7px;
  font-size: 11px;
  line-height: 1.1;
  white-space: nowrap;
}

@media (max-width: 360px), (max-height: 180px) {
  .credential-modal-overlay[data-compact="true"] .credential-modal-actions button {
    flex: 0 0 auto;
  }
}
</style>
