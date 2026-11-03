<script setup lang="ts">
import CredentialFields from "./CredentialFields.vue";
import { computed, ref, watch } from "vue";
import { useDialogFocus } from "./use-dialog-focus";

import type { ConnectCredentials } from "../lib/ipc";

/**
 * 统一凭据模态（Area F：唯一密码输入面）。
 *
 * 两种形态：
 *  - `mode="credentials"`：账户 + 密码均缺，双字段；
 *  - `mode="password"`：仅缺密码，账户行为只读投影。
 *
 * 「记住密码」为勾选（连接场景），确定后一次性 emit，草稿随即清空；
 * 密码框只接受存在性布尔（`hasSavedPassword` → 占位二态），没有读取已存值的通道。
 */
const props = withDefaults(defineProps<{
  visible: boolean;
  /** 设置入口只写配置；新密码自动保存，留空保留原密码。 */
  purpose?: "connect" | "settings";
  /** credentials = 账户密码双字段；password = 账户只读 + 仅输密码。 */
  mode?: "credentials" | "password";
  busy?: boolean;
  cancelling?: boolean;
  errorMessage?: string;
  /** 本次业务说明。 */
  notice?: string;
  resetKey?: number;
  initialUsername?: string;
  /** 已存密码存在性（仅驱动占位二态，非值）。 */
  hasSavedPassword?: boolean;
  /** 是否显示「记住密码」勾选；设置入口固定保存语义时置 false。 */
  showRemember?: boolean;
  /** 勾选初值（连接场景取当前 remember_password 配置）。 */
  initialRemember?: boolean;
  /** 极简窗内紧凑渲染变体；不改变字段与提交语义。 */
  compact?: boolean;
}>(), {
  purpose: "connect",
  mode: "credentials",
  busy: false,
  cancelling: false,
  errorMessage: "",
  notice: "",
  resetKey: 0,
  initialUsername: "",
  hasSavedPassword: false,
  showRemember: true,
  initialRemember: false,
  compact: false,
});

const emit = defineEmits<{
  submit: [credentials: ConnectCredentials & { passwordAction?: "keep" | "replace" | "clear" }];
  clearSavedPassword: [request: { username: string }];
  close: [];
}>();

const username = ref("");
const password = ref("");
const remember = ref(false);
const validationMessage = ref("");
const clearRequested = ref(false);
let draftInitialized = false;

function initializeDraft(): void {
  username.value = props.initialUsername.trim();
  password.value = "";
  clearRequested.value = false;
  remember.value = props.initialRemember;
  validationMessage.value = "";
  draftInitialized = true;
}

function clearDraft(): void {
  username.value = "";
  password.value = "";
  clearRequested.value = false;
  remember.value = false;
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

/** 设置入口自动保存新密码；连接入口沿用本次勾选。 */
const persist = computed(() => (props.purpose === "settings" || !props.showRemember ? true : remember.value));
watch(password, value => {
  if (props.purpose === "connect" && value && clearRequested.value) { clearRequested.value = false; remember.value = props.initialRemember; }
});

function clearPassword(): void {
  if (props.busy) return;
  password.value = "";
  clearRequested.value = true;
  remember.value = false;
  validationMessage.value = "";
}

function submit(): void {
  if (props.busy) return;
  const account = username.value.trim();
  const passwordAction = password.value ? "replace" : clearRequested.value ? "clear" : "keep";
  if (!account) { validationMessage.value = "请填写账户和密码。"; return; }
  if (props.purpose === "settings") {
    validationMessage.value = "";
    emit("submit", { username: account, password: password.value, persist: passwordAction === "clear" ? false : persist.value, passwordAction });
    return;
  }
  if (passwordAction === "clear") {
    validationMessage.value = "";
    emit("clearSavedPassword", { username: account });
    return;
  }
  if (!password.value) {
    validationMessage.value = props.mode === "password" ? "请输入密码。" : "请填写账户和密码。";
    return;
  }
  validationMessage.value = "";
  emit("submit", { username: account, password: password.value, persist: persist.value });
}

function close(): void {
  if (props.busy) return;
  emit("close");
}
const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, close, { initialFocus: "input:not([readonly]):not([type=checkbox])" });
</script>

<template>
  <div
    v-if="visible"
    class="credential-modal-overlay exv-dialog"
    data-testid="credential-required-modal"
    :data-compact="compact ? 'true' : 'false'"
    role="dialog"
    aria-modal="true"
    aria-labelledby="credential-required-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
  >
    <form class="credential-modal-card exv-dialog-card" @submit.prevent="submit">
      <h2 id="credential-required-title">{{ purpose === "settings" ? "修改账户与密码" : mode === "password" ? "输入密码" : "需要账户凭据" }}</h2>
      <div class="credential-modal-body" :tabindex="compact ? 0 : undefined" :role="compact ? 'region' : undefined" :aria-label="compact ? '详细说明' : undefined">
        <p class="credential-modal-intro">{{ purpose === "settings" ? "密码留空时保留已保存密码；也可输入新密码或点击清除。保存后生效。" : mode === "password" ? "输入密码后继续本次操作。" : "填写后即可继续本次连接。勾选“记住密码”才会保存到本机配置。" }}</p>
        <p v-if="notice" class="credential-modal-notice" data-testid="credential-notice" role="status">{{ notice }}</p>
        <p v-if="cancelling" class="credential-modal-cancelling" data-testid="credential-cancelling" role="status">正在取消连接，等待运行时确认…</p>

        <CredentialFields v-model:username="username" v-model:password="password"
          :readonly-username="mode === 'password' && purpose !== 'settings'"
          :has-saved-password="hasSavedPassword && !clearRequested" :busy="busy" :compact="compact"
          @clear-password="clearPassword" />
        <label v-if="purpose === 'connect' && showRemember" class="credential-modal-remember">
          <input
            v-model="remember"
            data-testid="credential-remember"
            type="checkbox"
            :disabled="busy"
          >
          <span>记住密码</span>
        </label>

        <p v-if="validationMessage || errorMessage" class="credential-modal-error" data-testid="credential-error" role="alert">
          {{ validationMessage || errorMessage }}
        </p>
      </div>

      <div class="credential-modal-actions">
        <button type="button" class="credential-modal-cancel" data-testid="credential-cancel" :disabled="busy" @click="close">{{ cancelling ? "取消中…" : "取消" }}</button>
        <button type="button" class="credential-modal-primary" :disabled="busy" data-testid="credential-submit" @click="submit">{{ purpose === "settings" ? "保存" : "确定" }}</button>
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
.credential-modal-notice,
.credential-modal-error,
.credential-modal-cancelling { margin: 0; }
.credential-modal-card h2 { font-size: var(--font-size-heading); }
.credential-modal-intro { color: var(--text-secondary); font-size: var(--font-size-caption); line-height: 1.6; }
.credential-modal-notice { color: var(--text-primary); font-size: var(--font-size-caption); line-height: 1.5; }
.credential-modal-password-row { display:flex; align-items:center; gap:8px; min-width:0; }
.credential-clear-password { flex:none; min-height:34px; padding:0 10px; border:1px solid var(--border-subtle); border-radius:var(--radius-md); background:var(--surface-raised); color:var(--text-secondary); }
.credential-modal-field { display: grid; gap: var(--space-2); color: var(--text-secondary); font-size: var(--font-size-body); }
.credential-modal-field input { min-height: 36px; padding: 0 var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-primary); }
/* password 形态的只读账户投影：灰显、不可聚焦。 */
.credential-modal-username-readonly { color: var(--text-secondary); background: var(--surface-subtle); cursor: default; }
.credential-modal-remember { display: inline-flex; align-items: center; gap: var(--space-2); color: var(--text-secondary); font-size: var(--font-size-body); }
.credential-modal-remember input { width: 16px; height: 16px; accent-color: var(--accent); }
.credential-modal-error { color: var(--state-danger); font-size: var(--font-size-caption); line-height: 1.5; }
.credential-modal-cancelling { color: var(--text-secondary); font-size: var(--font-size-caption); line-height: 1.5; }
.credential-modal-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--space-2); }
.credential-modal-actions button { min-height: 34px; padding: var(--space-1) var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-primary); cursor: pointer; }
.credential-modal-actions .credential-modal-primary { border-color: var(--accent); background: var(--accent); color: var(--accent-on); }
.credential-modal-actions button:disabled { cursor: wait; opacity: 0.65; }

/* 极简紧凑变体：面板收窄到 min(100%, 294px) 且内容区可滚动（M4：极简窗 136px
   硬预算下模态内容完整可达），动作区始终可见不参与滚动。 */
.credential-modal-overlay[data-compact="true"] {
  padding: 8px;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-card {
  display: flex;
  width: min(100%, 294px);
  max-height: calc(100vh - 28px);
  flex-direction: column;
  gap: 6px;
  padding: 8px;
  border-radius: var(--radius-md);
}

/* 内容区滚动（M4 预案），动作区固定可见（对齐历史 ModalShell 的 body/actions 分工）。 */
.credential-modal-overlay[data-compact="true"] .credential-modal-body {
  display: grid;
  min-height: 0;
  flex: 1 1 auto;
  gap: 4px;
  overflow-y: auto;
  scrollbar-width: thin;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-card h2 {
  flex: none;
  font-size: var(--font-size-heading);
  line-height: 1.2;
  overflow-wrap: anywhere;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-intro {
  font-size: var(--font-size-caption);
  line-height: 1.25;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-notice,
.credential-modal-overlay[data-compact="true"] .credential-modal-error,
.credential-modal-overlay[data-compact="true"] .credential-modal-cancelling {
  font-size: var(--font-size-caption);
  line-height: 1.2;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-password-row { display:flex; align-items:center; gap:8px; min-width:0; }
.credential-clear-password { flex:none; min-height:34px; padding:0 10px; border:1px solid var(--border-subtle); border-radius:var(--radius-md); background:var(--surface-raised); color:var(--text-secondary); }
.credential-modal-field {
  gap: 2px;
  font-size: var(--font-size-body);
}

.credential-modal-overlay[data-compact="true"] .credential-modal-field input {
  min-height: 24px;
  padding: 2px 6px;
  font-size: var(--font-size-body);
}

.credential-modal-overlay[data-compact="true"] .credential-modal-remember {
  font-size: var(--font-size-body);
  gap: 4px;
}

.credential-modal-overlay[data-compact="true"] .credential-modal-remember input {
  width: 12px;
  height: 12px;
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
  font-size: var(--font-size-body);
  line-height: 1.1;
  white-space: nowrap;
}

@media (max-width: 360px), (max-height: 180px) {
  .credential-modal-overlay[data-compact="true"] .credential-modal-actions button {
    flex: 0 0 auto;
  }
}

/* 标题与动作固定，长说明独立滚动；不占用 Darwin 原生标题栏预算。 */
.credential-modal-overlay.exv-dialog .credential-modal-card { display: flex; flex-direction: column; overflow: hidden; min-height: 0; }
.credential-modal-overlay.exv-dialog h2 { flex: none; }
.credential-modal-overlay.exv-dialog .credential-modal-body { min-height: 1.3em; min-width: 0; overflow-y: auto; overflow-wrap: anywhere; overscroll-behavior: contain; padding: 3px; margin: 0 -3px; }
.credential-modal-overlay.exv-dialog :is(.modal-actions, .credential-modal-actions) { flex: none; padding: 5px 5px 8px; margin-left: -5px; margin-right: -5px; }
</style>
