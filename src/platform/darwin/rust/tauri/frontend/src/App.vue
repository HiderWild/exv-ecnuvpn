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
import ConfigReadFailureNotice from "./components/ConfigReadFailureNotice.vue";
import ServiceUpdateNotice from "./components/ServiceUpdateNotice.vue";
import ServiceOperationOverlay from "./components/ServiceOperationOverlay.vue";
import { kernel, type ConfigPayload, type ConfigReadFailure, type ConfigRecovery } from "./lib/ipc";
import { APPEARANCE_KEY } from "./product/appearance";
import {
  decideQuickStartOnStartup,
  loadQuickStartSeen,
  markQuickStartSeen,
} from "./product/quick-start";
import { pushToast } from "./lib/toast";
import { type CoreConfigItem, CORE_CONFIG_GATEWAY_KEY, createCoreConfigGateway, credentialConfigFor } from "./product/core-config";
import { PRODUCT_RUNTIME_KEY } from "./product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "./product/service-operations";
import { WINDOW_CHROME_PORT_KEY } from "./product/window-chrome";

const props = withDefaults(defineProps<{
  /** 主入口在 Core gateway 就绪后读取的配置健康结论；测试可注入真实形状的回复。 */
  loadQuickStart?: () => Promise<ConfigPayload>;
  /** 仅由 DEV 预览入口提供；正式入口不传。 */
  initialPage?: "connect" | "logs" | "settings" | "about";
}>(), {
  loadQuickStart: () => kernel.configGet(),
  initialPage: "connect",
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
// ConfigGet 会把缺失配置初始化落盘；先保留首次结论，再允许子组件读取。
const startupReady = ref(false);
const runtime = inject(PRODUCT_RUNTIME_KEY)!;
// 与 InteractionHost、设置页共享同一配置网关及非秘密凭据投影。
const configGateway = inject(CORE_CONFIG_GATEWAY_KEY, null) ?? createCoreConfigGateway();
provide(CORE_CONFIG_GATEWAY_KEY, configGateway);
const credentialConfig = credentialConfigFor(configGateway);
const configReadFailure = ref<ConfigReadFailure | null>(null);
const configReadBusy = ref(false);
const configReadError = ref("");
const configReadUnavailable = computed(() => configReadFailure.value !== null || configReadError.value.length > 0);
const quickStartInitialItems = ref<ReadonlyArray<CoreConfigItem>>([]);
const quickStartRecovery = ref<ConfigRecovery | null>(null);
const shownRecoveries = new Set<string>();
const serviceOperations = createServiceOperationCoordinator((action) => runtime.serviceControl(action));
provide(SERVICE_OPERATION_KEY, serviceOperations);
const serviceReadError = computed(() => runtime.serviceStatusReadError?.value ?? "");
const serviceReadBusy = ref(false);
async function retryServiceStatus(): Promise<void> {
  serviceReadBusy.value = true;
  try { await serviceOperations.run("query"); }
  catch { /* 本次读取错误由runtime独立呈现。 */ }
  finally { serviceReadBusy.value = false; }
}

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

/** 首次读取和显式重试共用；读失败优先于向导与「已见」标记。 */
async function refreshStartupConfig(): Promise<void> {
  const seen = await loadQuickStartSeen();
  const payload = await props.loadQuickStart();
  if (payload.read_failure) {
    configReadFailure.value = payload.read_failure;
    quickStartOpen.value = false;
    quickStartRecovery.value = null;
    return;
  }
  credentialConfig.apply(payload.items);
  // 初次挂载由 startupReady 的既有入口启动 runtime；恢复读取后刷新当前展示。
  if (startupReady.value) await runtime.configurationChanged();
  configReadFailure.value = null;
  const decision = decideQuickStartOnStartup(payload, seen);
  const recovery = payload.config_recovery?.corrupted ? payload.config_recovery : null;
  if (decision === "show") {
    quickStartInitialItems.value = payload.items;
    quickStartRecovery.value = recovery;
    quickStartOpen.value = true;
    if (recovery) shownRecoveries.add(recovery.backup_file);
  } else {
    quickStartRecovery.value = null;
    if (decision === "seed") await markQuickStartSeen();
    if (recovery && !shownRecoveries.has(recovery.backup_file)) {
      shownRecoveries.add(recovery.backup_file);
      pushToast(`配置文件损坏，已保留原文件并恢复默认设置。备份文件：${recovery.backup_file}（配置目录内）。请到设置检查配置，或从备份找回原配置。`, "info");
    }
  }
}

function showConfigReadError(error: unknown): void {
  // RPC 无回复时不能编造路径；重试失败保留上次 Core 返回的真实路径。
  const detail = error instanceof Error ? error.message : String(error);
  configReadError.value = `配置请求未完成：${detail}。请确认 EXV 核心可用后重试，或查看日志。`;
}

async function retryStartupConfig(): Promise<void> {
  if (configReadBusy.value) return;
  configReadBusy.value = true;
  try {
    await refreshStartupConfig();
    configReadError.value = "";
  } catch (error) {
    showConfigReadError(error);
  } finally {
    configReadBusy.value = false;
  }
}

async function openConfigLogs(): Promise<void> {
  if (mode.value === "minimal") {
    if (!(await appearance.commitMode("advanced", (next) => chrome.setMode(next)))) {
      configReadError.value = appearance.saveError.value ?? "无法展开日志窗口，请重试。";
      return;
    }
  }
  current.value = "logs";
}

onMounted(async () => {
  // 覆盖 App 后启动即读取安装版本，不等待用户进入设置或翻日志。
  void serviceOperations.run("query").catch(() => undefined);
  // C3：常驻本地 Agent（浏览器登录票据接收面）离线时按事实提示（启动失败原因
  // 来自壳层，如端口被官方客户端占用）；在线不打扰。
  void kernel.nativeAgentStatus().then((status) => {
    if (status.phase !== "idle" && !status.online && status.message) {
      pushToast(`本地登录接收服务未启动：${status.message}。浏览器登录模式在处理前不可用，其余功能不受影响。`, "error");
    }
  }).catch(() => undefined);
  await retryStartupConfig();
  startupReady.value = true;
  emit("startupReady");
});
</script>

<template>
  <ConnectionInteractionHost @resolver-conflict-adjust="current = 'settings'" v-if="startupReady" v-slot="{ state, action, run, busy, minimal, initialUsername, initialServer, rememberPassword, hasStoredPassword, requestCredentials, protocol }">
    <ProductWindowFrame :mode="mode" :chrome="chrome" :appearance="appearance" :current-page="current" @navigate="current = $event">
      <div class="workspace-view">
        <div v-if="serviceReadError" class="service-read-error" role="status" data-testid="service-status-read-error">
          <span>{{ serviceReadError }}</span>
          <button type="button" :disabled="serviceReadBusy || serviceOperations.busy.value" data-testid="service-status-retry" @click="retryServiceStatus">重试读取</button>
        </div>
      <ConfigReadFailureNotice
        :failure="configReadFailure"
        :busy="configReadBusy"
        :compact="minimal"
        :error-message="configReadError"
        @retry="retryStartupConfig"
        @open-logs="openConfigLogs"
      />
      <ServiceUpdateNotice v-if="current !== 'settings' && !(minimal && configReadUnavailable)" :operations="serviceOperations" :busy="busy" />
      <template v-if="minimal && state">
      <MinimalConnectionView
        v-if="!configReadUnavailable"
        :state="state"
        :action="action"
        :initial-username="initialUsername"
        :initial-server="initialServer"
        :remember-password="rememberPassword"
        :has-stored-password="hasStoredPassword"
        :protocol="protocol"
        @run="run"
        @request-credentials="requestCredentials"
      />
      </template>
      <div v-else class="content"><component :is="currentComponent" @navigate="current = $event" /></div>
      </div>
    </ProductWindowFrame>
  </ConnectionInteractionHost>
  <ToastStack :mode="mode" />
  <QuickStartDialog
    :open="quickStartOpen"
    :recovery="quickStartRecovery"
    :initial-items="quickStartInitialItems"
    :after-core-saved="finishQuickStart"
    @skip="skipQuickStart"
  />
  <Teleport to="body">
    <ServiceOperationOverlay :visible="serviceOperations.busy.value" :label="serviceOperations.label.value" />
  </Teleport>
</template>

<style scoped>
.workspace-view { display:flex; flex-direction:column; height:100%; min-height:0; }
.content { min-width:0; flex:1; min-height:0; overflow:hidden; }
.service-read-error { display:flex; align-items:center; gap:var(--space-3); padding:var(--space-3); color:var(--text-secondary); font-size:var(--font-size-caption); }
</style>
