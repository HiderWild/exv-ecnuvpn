<script setup lang="ts">
import { computed, inject, onMounted, provide, ref, type Component } from "vue";
import ConnectPage from "./pages/ConnectPage.vue";
import LogsPage from "./pages/LogsPage.vue";
import SettingsPage from "./pages/SettingsPage.vue";
import AboutPage from "./pages/AboutPage.vue";
import ProductWindowFrame from "./components/ProductWindowFrame.vue";
import ConnectionInteractionHost from "./components/ConnectionInteractionHost.vue";
import CoreCrashBanner from "./components/CoreCrashBanner.vue";
import CoreWakeFailureModal from "./components/CoreWakeFailureModal.vue";
import MinimalConnectionView from "./components/MinimalConnectionView.vue";
import ToastStack from "./components/ToastStack.vue";
import QuickStartDialog from "./components/QuickStartDialog.vue";
import ServiceOperationOverlay from "./components/ServiceOperationOverlay.vue";
import { kernel, onCoreLifecycle, type ConfigPayload, type CoreLifecycleEvent } from "./lib/ipc";
import { APPEARANCE_KEY } from "./product/appearance";
import {
  announceCoreLifecycle,
  useCoreCrashBanner,
  useCoreWakeFailureModal,
} from "./product/core-crash";
import { decideQuickStartOnStartup, loadQuickStartSeen, markQuickStartSeen } from "./product/quick-start";
import { PRODUCT_RUNTIME_KEY } from "./product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "./product/service-operations";
import { WINDOW_CHROME_PORT_KEY } from "./product/window-chrome";

const props = withDefaults(defineProps<{
  /** 主入口在 Core gateway 就绪后读取的配置健康结论；测试可注入真实形状的回复。 */
  loadQuickStart?: () => Promise<ConfigPayload>;
  /** T9：core 生命周期事件订阅源；测试可注入，缺省订阅真实 tauri 事件。 */
  subscribeCoreLifecycle?: (callback: (event: CoreLifecycleEvent) => void) => Promise<() => void>;
}>(), {
  loadQuickStart: () => kernel.configGet(),
  subscribeCoreLifecycle: (callback: (event: CoreLifecycleEvent) => void) => {
    // 纯浏览器/测试预览无 tauri 事件桥（与 main.ts 的 runtime 启动门同一判定）：
    // 无订阅即无横幅，不报错。
    if (typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window)) {
      return Promise.resolve(() => undefined);
    }
    return onCoreLifecycle(callback);
  },
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
// T9（§2.9）：core 崩溃弱提示横幅 + 唤醒失败模态（全局挂载，状态在 product/core-crash）。
const coreCrash = useCoreCrashBanner();
const coreWakeFailure = useCoreWakeFailureModal();
const requestAppRestart = (): Promise<void> => coreWakeFailure.restartApp();
const requestAppExit = (): Promise<void> => coreWakeFailure.exitApp();

async function finishQuickStart(): Promise<void> {
  await runtime.configurationChanged({ strict: true });
  // 完成时机写「已见」标记（KI-13：有且仅有首次运行弹出；尽力而为，
  // 失败不阻断关闭——后续启动的 seed 分支会补写）。
  await markQuickStartSeen();
  quickStartOpen.value = false;
}

/** 跳过时机写「已见」标记；不再依赖 core bootstrap 是否成功落盘默认配置。 */
function skipQuickStart(): void {
  void markQuickStartSeen();
  quickStartOpen.value = false;
}

onMounted(async () => {
  // T9：core 生命周期事件订阅（crashed/recovered 驱动弱提示横幅显隐）。
  try {
    await props.subscribeCoreLifecycle(announceCoreLifecycle);
  } catch (error) {
    // 纯浏览器预览无 tauri 事件桥；横幅在该环境下不启用，不影响其余界面。
    console.error("core 生命周期事件订阅失败", error);
  }
  try {
    // 标记读取是纯本地文件（不经 Core），先行完成不干扰首次 ConfigGet 竞态约束
    // （2026-09-14 修复：首次配置结论必须由本次挂载的第一次 ConfigGet 消费）。
    const seen = await loadQuickStartSeen();
    const decision = decideQuickStartOnStartup(await props.loadQuickStart(), seen);
    if (decision === "show") {
      quickStartOpen.value = true;
    } else if (decision === "seed") {
      // 迁移播种：已有合法配置但无标记（≤4.1.0 升级用户）→ 补写标记防二次弹出。
      await markQuickStartSeen();
    }
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
  <ConnectionInteractionHost v-if="startupReady" v-slot="{ state, action, run, minimal, protocol, initialUsername, initialServer, rememberPassword, hasStoredPassword, credentialError }">
    <ProductWindowFrame :mode="mode" :chrome="chrome" :appearance="appearance" :current-page="current" @navigate="current = $event">
      <MinimalConnectionView
        v-if="minimal && state"
        :state="state"
        :action="action"
        :protocol="protocol"
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
  <CoreCrashBanner
    :visible="coreCrash.visible.value"
    :busy="coreCrash.busy.value"
    :mode="mode"
    @restart="coreCrash.restart()"
    @dismiss="coreCrash.dismiss()"
  />
  <QuickStartDialog
    :open="quickStartOpen"
    :after-core-saved="finishQuickStart"
    @skip="skipQuickStart"
  />
  <CoreWakeFailureModal
    :visible="coreWakeFailure.visible.value"
    :message="coreWakeFailure.message.value"
    :compact="mode === 'minimal'"
    @restart-app="requestAppRestart"
    @exit-app="requestAppExit"
  />
  <Teleport to="body">
    <ServiceOperationOverlay :visible="serviceOperations.busy.value" :label="serviceOperations.label.value" />
  </Teleport>
</template>

<style scoped>
.content { min-width:0; height:100%; overflow:hidden; }
</style>
