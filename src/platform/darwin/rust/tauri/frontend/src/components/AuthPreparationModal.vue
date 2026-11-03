<script setup lang="ts">
import { receiverStepSummaries, type AuthPreparationAction, type AuthPreparationEvent } from "../product/auth-preparation";
import ServiceOperationOverlay from "./ServiceOperationOverlay.vue";
defineProps<{ event: AuthPreparationEvent | null; busy: boolean; error: string; pendingAction: AuthPreparationAction | null }>();
const emit = defineEmits<{ action: [action: AuthPreparationAction] }>();
</script>
<template>
  <ServiceOperationOverlay :visible="event !== null" :label="event?.kind === 'certificate' && event.certificate_approval_required ? '安装本地登录证书' : '准备系统浏览器认证'" :spinner="busy" label-id="auth-preparation-title" test-id="auth-preparation-overlay" @escape="!busy && emit('action', 'cancel')">
    <div v-if="event" class="auth-preparation-body">
      <p v-if="event.kind === 'exclusive' && (event.receiver?.running || event.receiver?.can_start)">系统浏览器认证通道独占，继续需停止并禁用 aTrust 服务。停用将持续到您在设置中主动恢复，退出 EXV 后也会保持。</p>
      <p v-if="event.kind === 'certificate' && event.certificate_approval_required">系统浏览器认证需要安装 EXV 本地登录证书。继续后，系统将请求把证书加入当前用户的信任存储。本次批准只用于当前登录。</p>
      <p v-if="event.message" data-testid="auth-preparation-message" aria-live="polite">{{ event.message }}</p>
      <p v-if="event.native_code != null">系统错误码：{{ event.native_code }}</p>
      <p v-if="error" role="alert" class="auth-preparation-error">{{ error }}</p>
      <ul v-if="event.completed_steps?.length"><li v-for="(step, index) in receiverStepSummaries(event.completed_steps)" :key="index">{{ step }}</li></ul>
      <div class="auth-preparation-actions">
        <button v-if="event.kind === 'certificate' && event.certificate_approval_required" type="button" data-testid="auth-preparation-approve_certificate" :disabled="busy" @click="emit('action', 'approve_certificate')"><span v-if="busy && pendingAction === 'approve_certificate'" class="action-spinner" data-testid="auth-preparation-action-spinner" aria-hidden="true" />安装并继续</button>
        <button v-if="event.kind === 'exclusive' && (event.receiver?.running || event.receiver?.can_start)" type="button" data-testid="auth-preparation-disable_official_and_retry" :disabled="busy" @click="emit('action', 'disable_official_and_retry')"><span v-if="busy && pendingAction === 'disable_official_and_retry'" class="action-spinner" data-testid="auth-preparation-action-spinner" aria-hidden="true" />停止 aTrust 并重试</button>
        <button v-if="event.kind === 'exclusive' || !event.certificate_approval_required" type="button" data-testid="auth-preparation-retry" :disabled="busy" @click="emit('action', 'retry')"><span v-if="busy && pendingAction === 'retry'" class="action-spinner" data-testid="auth-preparation-action-spinner" aria-hidden="true" />重试</button>
        <button type="button" data-testid="auth-preparation-use_embedded" :disabled="busy" @click="emit('action', 'use_embedded')"><span v-if="busy && pendingAction === 'use_embedded'" class="action-spinner" data-testid="auth-preparation-action-spinner" aria-hidden="true" />使用内嵌浏览器</button>
        <button v-if="event.saved_login_available" type="button" data-testid="auth-preparation-try_saved_login" :disabled="busy" @click="emit('action', 'try_saved_login')"><span v-if="busy && pendingAction === 'try_saved_login'" class="action-spinner" data-testid="auth-preparation-action-spinner" aria-hidden="true" />尝试自动登录</button>
        <button type="button" data-testid="auth-preparation-cancel" :disabled="busy" @click="emit('action', 'cancel')"><span v-if="busy && pendingAction === 'cancel'" class="action-spinner" data-testid="auth-preparation-action-spinner" aria-hidden="true" />取消</button>
      </div>
    </div>
  </ServiceOperationOverlay>
</template>
<style scoped>
.auth-preparation-body { color: var(--text-secondary); font-size: var(--font-size-body); line-height: 1.6; overflow-wrap: anywhere; }
.auth-preparation-body p { margin: 0 0 var(--space-3); }
.auth-preparation-error { color: var(--state-danger); }
.auth-preparation-actions { display: flex; flex-wrap: wrap; gap: var(--space-2); }
.auth-preparation-actions button { min-height: 36px; padding: 6px 12px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); color: var(--text-primary); background: var(--surface-panel); font: inherit; cursor: pointer; }
.auth-preparation-actions button:first-child { background: var(--accent); color: var(--accent-on); }
.action-spinner { display: inline-block; width: 12px; height: 12px; margin-right: 6px; border: 2px solid currentColor; border-right-color: transparent; border-radius: 50%; animation: spin 1s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .action-spinner { animation: none; } }
.auth-preparation-actions button:disabled { opacity: .55; cursor: default; }
</style>
