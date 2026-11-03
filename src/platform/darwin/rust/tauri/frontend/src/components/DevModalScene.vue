<script setup lang="ts">
// 仅由 main 的 DEV + preview=1 门动态加载；本组件再次校验，动作只作用于内存。
import { computed, inject, provide, ref } from "vue";
import App from "../App.vue";
import CredentialRequiredModal from "./CredentialRequiredModal.vue";
import AuthenticationFailureModal from "./AuthenticationFailureModal.vue";
import BrowserLoginWaitModal from "./BrowserLoginWaitModal.vue";
import ServiceConnectFailureModal from "./ServiceConnectFailureModal.vue";
import ResolverConflictModal from "./ResolverConflictModal.vue";
import ServiceOperationOverlay from "./ServiceOperationOverlay.vue";
import QuickStartDialog from "./QuickStartDialog.vue";
import RoutesModal from "./RoutesModal.vue";
import { selectPreviewModal, createMockRuntime, type MockProductRuntime } from "../product/mock-runtime";
import { createMockCoreConfigGateway } from "../product/mock-page-data";
import { CORE_CONFIG_GATEWAY_KEY } from "../product/core-config";
import { APPEARANCE_KEY } from "../product/appearance";
import { PRODUCT_RUNTIME_KEY, type ProductRuntime } from "../product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "../product/service-operations";
import type { QuickStartApplyRequest, QuickStartApplyReply } from "../lib/ipc";

defineOptions({ inheritAttrs: false });
const props = defineProps<{ previewRuntime: MockProductRuntime }>();
const params = new URLSearchParams(window.location.search);
const scene = selectPreviewModal(import.meta.env.DEV, window.location.search);
const enabled = scene !== null && props.previewRuntime.source === "mock";
const busy = enabled && params.get("busy") === "1";
const error = enabled && params.get("error") === "1";
const long = enabled && params.get("long") === "1";
const open = ref(true);
const event = ref("尚未派发动作");
const appearance = inject(APPEARANCE_KEY, null);
if (enabled && params.get("compact") === "1") appearance?.setMode("minimal");
if (enabled && params.get("compact") === "0") appearance?.setMode("advanced");
const compact = computed(() => appearance?.state.value.mode === "minimal");
// 不信任外围注入的网关是否为模拟源；本场景始终拥有自己的内存配置与服务控制。
const gateway = createMockCoreConfigGateway();
provide(CORE_CONFIG_GATEWAY_KEY, gateway);
const runtime: ProductRuntime = {
  ...(enabled ? props.previewRuntime : createMockRuntime()),
  async serviceControl() {
    if (busy) return new Promise<never>(() => {});
    return scene === "service"
      ? { ok: false, service_status: null, message: "模拟：服务安装未完成，请重试。" }
      : { ok: true, service_status: null, message: "模拟服务动作完成（内存）" };
  },
};
provide(PRODUCT_RUNTIME_KEY, runtime);
provide(SERVICE_OPERATION_KEY, createServiceOperationCoordinator(action => runtime.serviceControl(action)));
function action(name: string, close = true): void {
  if (!enabled) return;
  event.value = `模拟组件事件：${name}`;
  if (close) open.value = false;
}
async function apply(request: QuickStartApplyRequest): Promise<QuickStartApplyReply> {
  if (!enabled) return { ok: false, message: "开发预览未启用。" };
  if (busy) return new Promise<never>(() => {});
  if (error) return { ok: false, message: "模拟：配置保存失败，请重试。" };
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
const detail = "模拟：连接请求已到达认证服务，但本次隧道建立未完成，请检查服务器设置并重试。";
const reason = long ? detail.repeat(12) : detail;
</script>

<template>
  <App v-if="enabled" v-bind="$attrs" />
  <template v-if="enabled">
    <div class="dev-modal-scene" data-testid="dev-modal-scene">
      <strong>模拟模态 · {{ scene }}</strong>
      <span role="status">{{ event }}</span>
      <button v-if="!open" type="button" @click="open = true">重新打开模拟模态</button>
    </div>
    <CredentialRequiredModal v-if="scene === 'credentials'" :visible="open" :compact="compact" :busy="busy" :error-message="error ? reason : ''" :notice="long ? reason : ''" initial-username="preview-user" @submit="action('submit（不记录凭据）')" @close="action('close')" />
    <AuthenticationFailureModal v-if="scene === 'auth-failure' || scene === 'auth-invalidated'" :visible="open" :compact="compact" :web-login="scene === 'auth-invalidated'" :session-invalidated="scene === 'auth-invalidated'" @close="action('close')" @reenter="action('reenter')" />
    <BrowserLoginWaitModal v-if="scene === 'auth-wait'" :visible="open" :compact="compact" mode="embedded" :cancelling="busy" @cancel="action('cancel')" />
    <ServiceConnectFailureModal v-if="scene === 'service'" :visible="open" :compact="compact" :error-message="reason" @close="action('close')" @recovered="action('recovered')" />
    <ResolverConflictModal v-if="scene === 'resolver'" :visible="open" :compact="compact" :conflict="{ domain: 'preview.example.edu', message: reason }" @close="action('close')" @adjust="action('adjust')" />
    <ServiceOperationOverlay v-if="scene === 'overlay'" :visible="open" :label="long ? reason : '模拟：正在安装后台服务…'" />
    <QuickStartDialog v-if="scene === 'quick' || scene === 'quick-retry'" :open="open" :apply="apply" :after-core-saved="refresh" @skip="action('skip')" @complete="action('complete')" />
    <RoutesModal v-if="scene === 'routes'" :open="open" :busy="busy" :routes="['10.0.0.0/8', '172.16.0.0/12']" @save="action('save')" @close="action('close')" />
  </template>
</template>

<style scoped>
.dev-modal-scene { position: fixed; bottom: 6px; left: 6px; z-index: 40; display: flex; gap: 8px; align-items: center; flex-wrap: wrap; max-width: calc(100vw - 12px); padding: 5px 8px; background: var(--surface-panel); border: 1px dashed var(--state-warning); border-radius: 7px; color: var(--text-secondary); font-size: var(--font-size-caption); }
.dev-modal-scene button { min-height: 24px; padding: 2px 6px; font-size: var(--font-size-body); }
</style>
