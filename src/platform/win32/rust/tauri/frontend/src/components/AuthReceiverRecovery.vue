<script setup lang="ts">
import { computed, inject, ref } from "vue";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";
import { authPreparationError, receiverStepSummaries, receiverConfigLabel, useAuthPreparationGateway, type AuthReceiverReply } from "../product/auth-preparation";
const props = defineProps<{ operationBusy?: boolean }>();
const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const restoreBlocked = computed(() => props.operationBusy || runtime?.browserLoginActive?.value === true
  || ["connecting", "awaiting", "connected", "stopping", "reconciling"].includes(runtime?.state.value.status ?? ""));
const gateway = useAuthPreparationGateway();
const expanded = ref(false);
const busy = ref(false);
const result = ref<AuthReceiverReply | null>(null);
const error = ref("");
const restored = ref(false);
async function run(restore = false): Promise<void> {
  if (busy.value || (restore && restoreBlocked.value)) return;
  expanded.value = true;
  busy.value = true;
  error.value = "";
  restored.value = false;
  try {
    const original = restore ? result.value?.receiver?.original_config : null;
    const reply = await (restore ? gateway.restore() : gateway.status());
    result.value = restore && original && reply.receiver && !reply.receiver.original_config
      ? { ...reply, receiver: { ...reply.receiver, original_config: original } }
      : reply;
    if (reply.error) error.value = authPreparationError(reply.error);
    else restored.value = restore;
  } catch (cause) { error.value = authPreparationError(cause); }
  finally { busy.value = false; }
}
</script>
<template>
  <section class="auth-receiver-recovery" aria-label="aTrust 启动设置">
    <button type="button" data-testid="auth-receiver-open" :disabled="busy" :aria-expanded="expanded" @click="expanded ? expanded = false : run()">查看/恢复 aTrust 启动设置</button>
    <div v-if="expanded" class="auth-receiver-detail" :aria-busy="busy">
      <p>为使用系统浏览器认证而禁用的 aTrust 服务，在退出 EXV 后会保持禁用。您可以主动恢复原启动设置；原来已禁用的项目会保持禁用。</p>
      <p v-if="busy" role="status">{{ result ? '正在恢复/读取启动设置…' : '正在读取启动设置…' }}</p>
      <template v-if="result?.receiver">
        <dl><dt>原启动设置</dt><dd data-testid="auth-receiver-original">{{ receiverConfigLabel(result.receiver.original_config) }}</dd><dt>当前启动设置</dt><dd data-testid="auth-receiver-current">{{ receiverConfigLabel(result.receiver.current_config) }}</dd><dt>当前服务状态</dt><dd>{{ result.receiver.running ? '运行中' : '已停止' }} · {{ result.receiver.can_start ? '可以启动' : '当前不会自行启动' }}</dd></dl>
        <p v-if="!result.receiver.recovery_available">没有待恢复的原启动设置。</p>
      </template>
      <ul v-if="result?.completed_steps.length"><li v-for="(step, index) in receiverStepSummaries(result.completed_steps)" :key="index">{{ step }}</li></ul>
      <p v-if="error" role="alert">{{ error }}</p>
      <p v-if="restoreBlocked">请先断开连接后恢复。</p>
      <p v-if="restored" role="status">已恢复 aTrust 原启动设置。</p>
      <div class="auth-receiver-actions"><button type="button" data-testid="auth-receiver-refresh" :disabled="busy" @click="run()">重新读取</button><button v-if="result?.receiver?.recovery_available" type="button" data-testid="auth-receiver-restore" :disabled="busy || restoreBlocked" @click="run(true)">恢复 aTrust 原启动设置</button></div>
    </div>
  </section>
</template>
<style scoped>
.auth-receiver-recovery { margin-top: var(--space-4); color: var(--text-secondary); font-size: var(--font-size-body); overflow-wrap: anywhere; }
.auth-receiver-detail { margin-top: var(--space-3); line-height: 1.6; }
dl { display: grid; grid-template-columns: auto 1fr; gap: var(--space-2) var(--space-4); }
dd { margin: 0; color: var(--text-primary); }
.auth-receiver-actions { display: flex; flex-wrap: wrap; gap: var(--space-2); }
button { min-height: 32px; padding: 4px 12px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-panel); color: var(--text-primary); font: inherit; cursor: pointer; }
button:disabled { opacity: .55; cursor: default; }
[role="alert"] { color: var(--state-danger); }
</style>
