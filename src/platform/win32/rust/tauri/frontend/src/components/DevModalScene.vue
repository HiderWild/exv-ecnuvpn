<script setup lang="ts">
// 只由 main.ts 的 DEV + preview=1 + 已知 modal 门动态加载；全部动作使用内存网关。
import { computed, inject, provide, ref } from "vue";
import App from "../App.vue";
import CredentialRequiredModal from "./CredentialRequiredModal.vue";
import AuthenticationFailureModal from "./AuthenticationFailureModal.vue";
import BrowserAuthWaitingModal from "./BrowserAuthWaitingModal.vue";
import ServiceConnectFailureModal from "./ServiceConnectFailureModal.vue";
import ConfigReadFailureModal from "./ConfigReadFailureModal.vue";
import CoreWakeFailureModal from "./CoreWakeFailureModal.vue";
import ServiceOperationOverlay from "./ServiceOperationOverlay.vue";
import QuickStartDialog from "./QuickStartDialog.vue";
import RoutesModal from "./RoutesModal.vue";
import { selectPreviewModal } from "../product/mock-modal-scene";
import { CORE_CONFIG_GATEWAY_KEY } from "../product/core-config";
import { APPEARANCE_KEY } from "../product/appearance";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";
import type { QuickStartApplyRequest, QuickStartApplyReply } from "../lib/ipc";

defineOptions({ inheritAttrs: false });
const params = new URLSearchParams(window.location.search);
const scene = selectPreviewModal(import.meta.env.DEV, window.location.search);
const busy = params.get("busy") === "1";
const error = params.get("error") === "1";
const throttleWarning = params.get("throttle") === "0" ? null : { attempts: 3, window_minutes: 30, threshold: 3 };
const open = ref(true);
const event = ref("尚未派发动作");
const appearance = inject(APPEARANCE_KEY, null);
if (params.get("compact") === "1") appearance?.setMode("minimal");
if (params.get("compact") === "0") appearance?.setMode("advanced");
const compact = computed(() => appearance?.state.value.mode === "minimal");
const gateway = inject(CORE_CONFIG_GATEWAY_KEY)!;
const runtime = inject(PRODUCT_RUNTIME_KEY)!;
// 服务场景在用户点击后显示恢复步骤；忙碌模拟停留，不冒充宿主运行结果。
provide(PRODUCT_RUNTIME_KEY, {
  ...runtime,
  async serviceControl() {
    if (busy) return new Promise<never>(() => {});
    return scene === "service" ? { ok: false, message: "模拟：服务安装未完成，请重试。" } : { ok: true, message: "模拟服务动作完成" };
  },
});
function action(name: string, close = true): void {
  event.value = `模拟组件事件：${name}`;
  if (close) open.value = false;
}
async function apply(request: QuickStartApplyRequest): Promise<QuickStartApplyReply> {
  // 不记录秘密，不使用真实 kernel；保存原回复由生产快速入门组件处理，显示刷新独立执行。
  if (busy) return new Promise<never>(() => {});
  if (error) return { ok: false, message: "模拟：配置保存失败，请重试。" };
  // items 由生产 quickStartItems 生成，全部属于 CoreConfigGateway 既有键。
  await gateway.configSet(request.items as Parameters<typeof gateway.configSet>[0]);
  event.value = "模拟配置已保存（内存）";
  return { ok: true };
}
let refreshFailed = false;
async function refresh(): Promise<void> {
  if (scene === "quick-retry" && !refreshFailed) {
    refreshFailed = true;
    throw new Error("模拟刷新失败");
  }
}
const configView = { kind: "acl" as const, title: "无法读取配置", message: "模拟：配置文件访问被拒绝，主界面暂时无法读取已有设置。", canRepairPermissions: true };
async function coreAction(name: string): Promise<void> {
  if (busy) return new Promise<never>(() => {});
  action(name);
}
</script>

<template>
  <App v-bind="$attrs" />
  <div class="dev-modal-scene" data-testid="dev-modal-scene">
    <strong>模拟模态 · {{ scene }}</strong>
    <span role="status">{{ event }}</span>
    <button v-if="!open" type="button" @click="open = true">重新打开模拟模态</button>
  </div>
  <CredentialRequiredModal v-if="scene === 'credentials'" :visible="open" :compact="compact" :busy="busy" :error-message="error ? '模拟：账户凭据未被接受，请重新输入。' : ''" initial-username="preview-user" @submit="action('submit（不记录凭据）')" @close="action('close')" />
  <AuthenticationFailureModal v-if="scene === 'auth-failure'" :visible="open" :compact="compact" @close="action('close')" @reenter="action('reenter')" />
  <BrowserAuthWaitingModal v-if="scene === 'auth-wait' || scene === 'auth-auto' || scene === 'auth-error'" :visible="open" :mode="scene === 'auth-auto' ? 'auto-login' : scene === 'auth-error' ? 'error' : 'waiting'" :allow-system-browser="true" :busy="busy" :error="scene === 'auth-error' ? '模拟：认证未完成，请选择重试或取消。' : ''" stage="submitting" :throttle-warning="throttleWarning" @cancel="action('cancel')" @retry="action('retry')" @system-browser="action('system-browser')" />
  <ServiceConnectFailureModal v-if="scene === 'service'" :visible="open" :compact="compact" error-message="模拟：服务引擎未能就绪。" @close="action('close')" @recovered="action('recovered')" />
  <ConfigReadFailureModal v-if="scene === 'config'" :visible="open" :compact="compact" :view="configView" :busy="busy ? 'repair' : null" :repair-error="error ? '模拟：管理员授权未完成，请重试。' : null" @repair="action('repair', false)" @retry="action('retry', false)" @dismiss="action('dismiss')" />
  <CoreWakeFailureModal v-if="scene === 'core'" :visible="open" :compact="compact" message="模拟：后台核心进程未能启动。" :on-restart-app="() => coreAction('restartApp')" :on-exit-app="() => coreAction('exitApp')" />
  <ServiceOperationOverlay v-if="scene === 'overlay'" :visible="open" label="模拟：正在安装后台服务…" />
  <QuickStartDialog v-if="scene === 'quick' || scene === 'quick-retry'" :open="open" :apply="apply" :after-core-saved="refresh" @skip="action('skip')" @complete="action('complete')" />
  <RoutesModal v-if="scene === 'routes'" :open="open" :busy="busy" :routes="['10.0.0.0/8', '172.16.0.0/12']" @save="action('save')" @close="action('close')" />
</template>

<style scoped>
.dev-modal-scene { position: fixed; bottom: 6px; left: 6px; z-index: 45; display: flex; gap: 8px; align-items: center; flex-wrap: wrap; max-width: calc(100vw - 12px); padding: 5px 8px; background: var(--surface-panel); border: 1px dashed var(--state-warning); border-radius: 7px; color: var(--text-secondary); font-size: var(--font-size-body); }
.dev-modal-scene button { min-height: 24px; padding: 2px 6px; font-size: var(--font-size-body); }
</style>
