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
import ConfigReadFailureModal from "./components/ConfigReadFailureModal.vue";
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
import {
  configReadFailureView,
  configReadUnreachableView,
  type ConfigReadFailureView,
} from "./product/config-read-failure";
import { configRecoveryNotice, decideQuickStartOnStartup, loadQuickStartSeen, markQuickStartSeen } from "./product/quick-start";
import { pushToast } from "./lib/toast";
import { PRODUCT_RUNTIME_KEY } from "./product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "./product/service-operations";
import { WINDOW_CHROME_PORT_KEY } from "./product/window-chrome";

const props = withDefaults(defineProps<{
  /** 仅开发模拟主入口传入；真实启动使用连接页默认值。 */
  initialPage?: "connect" | "logs" | "settings" | "about";
  /** 主入口在 Core gateway 就绪后读取的配置健康结论；测试可注入真实形状的回复。 */
  loadQuickStart?: () => Promise<ConfigPayload>;
  /** T9：core 生命周期事件订阅源；测试可注入，缺省订阅真实 tauri 事件。 */
  subscribeCoreLifecycle?: (callback: (event: CoreLifecycleEvent) => void) => Promise<() => void>;
}>(), {
  initialPage: "connect",
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

const current = ref<PageKey>(props.initialPage);
const currentComponent = computed(() => pages.find((p) => p.key === current.value)!.component);
const appearance = inject(APPEARANCE_KEY)!;
const chrome = inject(WINDOW_CHROME_PORT_KEY)!;
const mode = computed(() => appearance.state.value.mode);
const quickStartOpen = ref(false);
const quickStartItems = ref<ConfigPayload["items"]>([]);
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
  quickStartOpen.value = false;
  // 已保存/安装的结果先完成；已见标记和独立显示读取不再阻断关窗。
  void markQuickStartSeen();
  await runtime.configurationChanged({ strict: true });
}

/** 跳过时机写「已见」标记；不再依赖 core bootstrap 是否成功落盘默认配置。 */
function skipQuickStart(): void {
  void markQuickStartSeen();
  quickStartOpen.value = false;
}

// ---- S4（2026-10）：config.json 读失败明确报错（不再静默 console.error） ----

/** 启动配置读取失败投影（null = 无失败/已恢复）。 */
const configReadFailure = ref<ConfigReadFailureView | null>(null);
/** 修复（runas）/重试进行中。 */
const configReadBusy = ref<"repair" | "retry" | null>(null);
/** 修复失败的壳侧说明（用户取消 UAC 等）；成功或重试前清除。 */
const configRepairError = ref<string | null>(null);

/** 启动配置读取与快速入门判定（挂载时与「重试读取」共用同一条路径）。
 *
 * 读失败（payload.read_failure）短路在判定**之前**：不写「已见」标记（读不了
 * ≠已有合法配置，播种会永久压掉首启快速入门）、不弹快速入门、不弹损坏通知。
 */
async function refreshStartupConfig(): Promise<void> {
  const seen = await loadQuickStartSeen();
  const payload = await props.loadQuickStart();
  const failure = configReadFailureView(payload);
  if (failure) {
    configReadFailure.value = failure;
    return;
  }
  configReadFailure.value = null;
  quickStartItems.value = payload.items;
  const decision = decideQuickStartOnStartup(payload, seen);
  if (decision === "show") {
    quickStartOpen.value = true;
  } else if (decision === "seed") {
    // 迁移播种：已有合法配置但无标记（≤4.1.0 升级用户）→ 补写标记防二次弹出。
    await markQuickStartSeen();
  } else if (decision === "quiet") {
    // R-D3-C（裁定实施）：老用户（seen）不重弹快速入门——当次启动的损坏修复
    // 改走一次性 toast 通知（信息源是当次损坏分支，下次启动 config 已合法，
    // 天然不再触发）。show 路径走快速入门引导，不进入本分支（无双提示）。
    const notice = configRecoveryNotice(payload);
    if (notice) pushToast(notice, "info");
  }
}

/** 「重试读取」：同一读取路径重跑（占用类失败关闭占用程序后用）。 */
async function retryStartupConfig(): Promise<void> {
  if (configReadBusy.value !== null) return;
  configReadBusy.value = "retry";
  try {
    await refreshStartupConfig();
  } catch (error) {
    // 重试仍拿不到 ConfigGet 回复（core 不可达）：模态切到 unreachable 档。
    console.error("配置状态读取失败", error);
    configReadFailure.value = configReadUnreachableView();
  } finally {
    configReadBusy.value = null;
  }
}

/** 「以管理员身份修复权限」（仅 ACL 档显示）：壳 runas 重置 ACL 后自动重试读取。 */
async function repairConfigPermissions(): Promise<void> {
  if (configReadBusy.value !== null) return;
  configReadBusy.value = "repair";
  configRepairError.value = null;
  try {
    await kernel.configRepairPermissions();
  } catch (error) {
    // 壳侧拒绝（用户取消 UAC / icacls 失败）：保持模态并明示原因，可再试。
    console.error("配置权限修复失败", error);
    configRepairError.value =
      typeof error === "object" && error !== null && "message" in error && typeof (error as { message?: unknown }).message === "string"
        ? (error as { message: string }).message
        : String(error);
    if (typeof error === "object" && error !== null && "code" in error) {
      configRepairError.value += `（${String(error.code)}）`;
    }
    return;
  } finally {
    configReadBusy.value = null;
  }
  // 修复原调用已经返回；本次读取失败只进入读取错误模态。
  await retryStartupConfig();
}

/** 「稍后再说」：关闭模态保留主界面可用（下次启动/重开设置仍会触发读取）。 */
function dismissConfigReadFailure(): void {
  if (configReadBusy.value !== null) return;
  configReadFailure.value = null;
  configRepairError.value = null;
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
    // S4：读失败（含 core 不可达）在 refreshStartupConfig 内统一转明确错误模态。
    await refreshStartupConfig();
  } catch (error) {
    // Core gateway 不可用时不以账户、连接状态或默认值猜测首次启动；S4 起不再
    // 静默——同一明确错误模态（unreachable 档，无修复按钮），主界面保持可用。
    console.error("快速入门配置状态读取失败", error);
    configReadFailure.value = configReadUnreachableView();
  } finally {
    startupReady.value = true;
    emit("startupReady");
  }
});
</script>

<template>
  <ConnectionInteractionHost v-if="startupReady" v-slot="{ state, action, run, minimal, protocol, initialUsername, initialServer, rememberPassword, hasStoredPassword, requestCredentials }">
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
        @run="run"
        @request-credentials="requestCredentials"
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
    :initial-items="quickStartItems"
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
  <ConfigReadFailureModal
    :visible="configReadFailure !== null"
    :view="configReadFailure"
    :busy="configReadBusy"
    :repair-error="configRepairError"
    :compact="mode === 'minimal'"
    @repair="repairConfigPermissions"
    @retry="retryStartupConfig"
    @dismiss="dismissConfigReadFailure"
  />
  <Teleport to="body">
    <ServiceOperationOverlay :visible="serviceOperations.busy.value" :label="serviceOperations.label.value" />
  </Teleport>
</template>

<style scoped>
.content { min-width:0; height:100%; overflow:hidden; }
</style>
