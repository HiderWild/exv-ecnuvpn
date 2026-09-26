<script setup lang="ts">
import { computed, inject, onMounted, provide, ref, type Component } from "vue";
import ConnectPage from "./pages/ConnectPage.vue";
import LogsPage from "./pages/LogsPage.vue";
import SettingsPage from "./pages/SettingsPage.vue";
import AboutPage from "./pages/AboutPage.vue";
import ProductWindowFrame from "./components/ProductWindowFrame.vue";
import ConnectionInteractionHost from "./components/ConnectionInteractionHost.vue";
import MinimalConnectionView from "./components/MinimalConnectionView.vue";
import ToastStack from "./components/ToastStack.vue";
import QuickStartDialog from "./components/QuickStartDialog.vue";
import ServiceOperationOverlay from "./components/ServiceOperationOverlay.vue";
import { kernel, type ConfigPayload } from "./lib/ipc";
import { APPEARANCE_KEY } from "./product/appearance";
import { quickStartShouldOpen } from "./product/quick-start";
import { PRODUCT_RUNTIME_KEY } from "./product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "./product/service-operations";
import { WINDOW_CHROME_PORT_KEY } from "./product/window-chrome";

const props = withDefaults(defineProps<{
  /** 主入口在 Core gateway 就绪后读取的配置健康结论；测试可注入真实形状的回复。 */
  loadQuickStart?: () => Promise<ConfigPayload>;
}>(), {
  loadQuickStart: () => kernel.configGet(),
});
const emit = defineEmits<{ startupReady: [] }>();

type PageKey = "connect" | "logs" | "settings" | "about";

const pages: { key: PageKey; label: string; component: Component }[] = [
  { key: "connect", label: "连接", component: ConnectPage },
  { key: "logs", label: "日志", component: LogsPage },
  { key: "settings", label: "设置", component: SettingsPage },
  { key: "about", label: "关于", component: AboutPage },
];

const current = ref<PageKey>("connect");
const currentComponent = computed(() => pages.find((p) => p.key === current.value)!.component);
const appearance = inject(APPEARANCE_KEY)!;
const chrome = inject(WINDOW_CHROME_PORT_KEY)!;
const mode = computed(() => appearance.state.value.mode);
const quickStartOpen = ref(false);
// ConfigGet 会把缺失配置初始化落盘；先保留首次结论，再允许子组件读取。
const startupReady = ref(false);
const runtime = inject(PRODUCT_RUNTIME_KEY)!;
const serviceOperations = createServiceOperationCoordinator((action) => runtime.serviceControl(action));
provide(SERVICE_OPERATION_KEY, serviceOperations);

async function finishQuickStart(): Promise<void> {
  await runtime.configurationChanged({ strict: true });
  quickStartOpen.value = false;
}

onMounted(async () => {
  try {
    quickStartOpen.value = quickStartShouldOpen(await props.loadQuickStart());
  } catch (error) {
    // Core gateway 不可用时不以账户、连接状态或默认值猜测首次启动；保持主界面可用。
    console.error("快速入门配置状态读取失败", error);
  } finally {
    startupReady.value = true;
    emit("startupReady");
  }
});
</script>

<template>
  <ConnectionInteractionHost v-if="startupReady" v-slot="{ state, action, run, minimal, initialUsername, initialServer, rememberPassword, hasStoredPassword, credentialError }">
    <ProductWindowFrame :mode="mode" :chrome="chrome" :appearance="appearance" :current-page="current" @navigate="current = $event">
      <MinimalConnectionView
        v-if="minimal && state"
        :state="state"
        :action="action"
        :initial-username="initialUsername"
        :initial-server="initialServer"
        :remember-password="rememberPassword"
        :has-stored-password="hasStoredPassword"
        :credential-error="credentialError"
        @run="run"
      />
      <div v-else class="content"><component :is="currentComponent" @navigate="current = $event" /></div>
    </ProductWindowFrame>
  </ConnectionInteractionHost>
  <ToastStack :mode="mode" />
  <QuickStartDialog
    :open="quickStartOpen"
    :after-core-saved="finishQuickStart"
    @skip="quickStartOpen = false"
  />
  <Teleport to="body">
    <ServiceOperationOverlay :visible="serviceOperations.busy.value" :label="serviceOperations.label.value" />
  </Teleport>
</template>

<style scoped>
.content { min-width:0; height:100%; overflow:hidden; }
</style>
