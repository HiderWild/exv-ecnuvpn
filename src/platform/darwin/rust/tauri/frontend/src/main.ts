import { createApp, watch } from "vue";
import App from "./App.vue";
import { kernel, type ConfigPayload } from "./lib/ipc";
import { darwinCommandAdapter } from "./product/command-adapter-global";
import {
  APPEARANCE_KEY,
  createAppearance,
  createBrowserSafeAppearanceStorage,
} from "./product/appearance";
import { CORE_CONFIG_GATEWAY_KEY, createCoreConfigGateway } from "./product/core-config";
import { installNativeContextMenuBan } from "./product/context-menu";
import {
  announceFailedSnapshot,
  failureEpisodeNotified,
  initFailedNotifyPort,
} from "./product/failed-notify";
import { dispatchLifecycleEffects, dispatchSystemNotification, type SystemNotificationKind, computeLifecycleEffects, computeSelfHealNotify, shouldAutoConnectOnLaunch, createReconnectNotificationTracker, reconnectPauseCopyOf, RECONNECTING_TITLE, RECONNECTING_BODY } from "./product/lifecycle-effects";
import { LOGS_GATEWAY_KEY } from "./product/logs";
import { createMockCoreConfigGateway, createMockLogsGateway } from "./product/mock-page-data";
import { createMockRuntime, selectRuntimeSource, selectPreviewStatus, selectPreviewScenario, selectPreviewPhase, selectPreviewPage, selectPreviewModal, selectProgressPreview } from "./product/mock-runtime";
import { createProductRuntime, PRODUCT_RUNTIME_KEY } from "./product/runtime";
import { pushToast } from "./lib/toast";
import {
  loadUiPreferences,
  provideUiPrefsGateway,
  createTauriUiPrefsGateway,
  uiPreferencesState,
} from "./product/ui-prefs";
import type { ProductStatus } from "./product/types";
import { createWindowChromePort, WINDOW_CHROME_PORT_KEY } from "./product/window-chrome";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/motion.css";
import "./styles/ui-first-shell.css";
import "./styles/dialogs.css";

// S6b：全域禁用原生右键菜单（release 无 devtools 消费；日志条目自制菜单见 LogsPage）。
installNativeContextMenuBan();

const appearance = createAppearance(createBrowserSafeAppearanceStorage(() => window.localStorage));
appearance.applyDocument(document.documentElement);
const chrome = createWindowChromePort();
const source = selectRuntimeSource(import.meta.env.DEV, window.location.search);
if (source === "mock") {
  const compact = new URLSearchParams(window.location.search).get("compact");
  if (compact === "1") appearance.setMode("minimal");
  if (compact === "0") appearance.setMode("advanced");
}
// 阶段④：真实运行时的快照采纳钩子喂失败终态通知（darwin 无 R1/R2 lane，无互斥项）。
const mockRuntime = source === "mock" ? createMockRuntime(selectPreviewStatus(import.meta.env.DEV, window.location.search), undefined,
  selectPreviewScenario(import.meta.env.DEV, window.location.search), selectPreviewPhase(import.meta.env.DEV, window.location.search)) : null;
const runtime = mockRuntime ?? createProductRuntime(undefined, undefined, announceFailedSnapshot);
/**
 * Core 在 Rust setup 中完成 bootstrap 后才暴露 Tauri gateway；主入口把这次读取注入
 * App，由 App 在首次挂载时只按 `requires_quick_start` 决定是否显示对话框。
 */
const loadQuickStart = (): Promise<ConfigPayload> => {
  if (source === "mock") return Promise.resolve({ items: [], requires_quick_start: false });
  return kernel.configGet();
};
const modalScene = selectPreviewModal(import.meta.env.DEV, window.location.search);
const progressScene = selectProgressPreview(import.meta.env.DEV, window.location.search);
const root = import.meta.env.DEV && modalScene ? (await import("./components/DevModalScene.vue")).default
  : import.meta.env.DEV && progressScene ? (await import("./components/DevProgressScene.vue")).default : App;
const app = createApp(root, {
  ...(mockRuntime && (modalScene || progressScene) ? { previewRuntime: mockRuntime } : {}),
  ...(source === "mock" ? { initialPage: selectPreviewPage(import.meta.env.DEV, window.location.search) } : {}),
  loadQuickStart,
  onStartupReady: () => {
    // 运行时的身份刷新也读取 ConfigGet，必须等首次启动结论已被 App 接收。
    if (source === "mock" || "__TAURI_INTERNALS__" in window) {
      void runtime.start().catch((error: unknown) => {
        console.error("产品运行时初始化失败", error);
      });
    }
  },
});

app.provide(PRODUCT_RUNTIME_KEY, runtime);
app.provide(APPEARANCE_KEY, appearance);
app.provide(WINDOW_CHROME_PORT_KEY, chrome);
app.provide(CORE_CONFIG_GATEWAY_KEY, source === "mock" ? createMockCoreConfigGateway() : createCoreConfigGateway());
if (source === "mock") {
  app.provide(LOGS_GATEWAY_KEY, createMockLogsGateway());
}
try {
  await chrome.setMode(appearance.state.value.mode);
} catch (error) {
  console.error("窗口模式初始化失败", error);
  appearance.setMode("advanced");
}

// 前端自有设置：启动即加载（Tauri 外壳内走真实网关；预览环境保持默认值）。
if (source !== "mock") {
  provideUiPrefsGateway(createTauriUiPrefsGateway());
}
await loadUiPreferences();

// 阶段④（D9 计划 §4.2-3/4）：失败终态通知的分发端口——前台用应用内 toast，后台
// 经 `invokeTrayNotify` 升级为系统通知；前台抑制偏好沿用连接类语义。
if (source !== "mock") {
  initFailedNotifyPort({
    unexpectedDisconnectEnabled: () => uiPreferencesState().value.unexpected_disconnect_notify,
    disconnectNotifyEnabled: () => uiPreferencesState().value.disconnect_notify,
    userStopping: () => runtime.state.value.status === "stopping",
    isForeground: () => isWindowForeground(),
    suppressWhenForeground: () => uiPreferencesState().value.suppress_notify_when_foreground,
    toast: (message, kind) => pushToast(message, kind),
    isCurrent: snapshot => (snapshot.operation_id == null || runtime.state.value.operationId === snapshot.operation_id)
      && runtime.state.value.status === (snapshot.runtime.state.startsWith("failed_") ? "failed" : snapshot.runtime.state),
    trayNotify: (title, body, kind = "failed", isCurrent) => {
      void invokeTrayNotify(title, body, kind, isCurrent);
    },
  });
}

// 连接过渡效果分发：状态流 → hide-window / tray-notify / 启动自动连接。
let previousStatus: ProductStatus | null = null;
const reconnectNotifications = createReconnectNotificationTracker();
// 2026-09-05 host 自愈（计划 §4.5）：selfHeal stage 去重游标（与 previousStatus 同模式；
// null = 首快照只记录基线）。
let prevSelfHealStage: string | null = null;
// S5b：重连暂停提示去重游标——同一暂停事实只在出现沿提示一次；reason 清空或切换
// 后重新武装。
let prevReconnectPauseCopy: string | null = null;
watch(
  runtime.state,
  (productState) => {
    const prefs = uiPreferencesState().value;
    const nextStatus = productState.status;
    const prevStatus = previousStatus;
    const isFirstSnapshot = prevStatus === null;
    const observed = reconnectNotifications.observe(productState);
    previousStatus = nextStatus;
    if (observed.reconnectTrigger) {
      // 应用内提示始终可见；系统通知单独遵循用户偏好及前台抑制。
      pushToast(RECONNECTING_BODY, "warning");
      const trigger = observed.reconnectTrigger;
      void invokeTrayNotify(RECONNECTING_TITLE, RECONNECTING_BODY, "reconnecting",
        () => reconnectNotifications.isCurrent(trigger, runtime.state.value));
    }

    // S5b：重连暂停事实（owner 失配/凭据清除）出现沿的应用内提示。
    const pauseCopy = reconnectPauseCopyOf(productState.reconnect.reason);
    if (pauseCopy !== null && pauseCopy !== prevReconnectPauseCopy) {
      pushToast(pauseCopy, "warning");
    }
    prevReconnectPauseCopy = pauseCopy;

    // host 自愈失败气泡（§4.5 冻结）：仅非 failed → failed 进入时一次；respawning/
    // succeeded 不弹；系统发送同样读取最新偏好与前台事实。
    const nextSelfHealStage = productState.selfHeal.active ? productState.selfHeal.stage : null;
    for (const effect of computeSelfHealNotify(prevSelfHealStage, nextSelfHealStage)) {
      if (effect.kind === "tray-notify") {
        void invokeTrayNotify(effect.title, effect.body, "self-heal-failed",
          () => runtime.state.value.selfHeal.active && runtime.state.value.selfHeal.stage === "failed"
            && runtime.state.value.operationId === productState.operationId);
      }
    }
    prevSelfHealStage = nextSelfHealStage;

    // 事件与窗口动作仍在原生命周期分发；系统发送入口单独读取即时前台。
    {
      // 同一操作的展示刷新会重建对象；只在业务相位或操作归属改变时撤销旧效果。
      if (runtime.state.value.status !== nextStatus || runtime.state.value.operationId !== productState.operationId) return;

      const effects = computeLifecycleEffects(prevStatus, nextStatus, prefs, {
        // 此处生成事件候选；真实前台事实仅由统一发送入口读取。
        foreground: false,
        // 阶段④ N-02 互斥：失败 episode 已有声音时「EXV 已断开」气泡让位。
        failureEpisodeNotified: failureEpisodeNotified(),
        automaticReconnect: observed.automaticReconnect,
      });
      void dispatchLifecycleEffects(effects, {
        isCurrent: () => runtime.state.value.status === nextStatus && runtime.state.value.operationId === productState.operationId,
        notify: (title, body) => invokeTrayNotify(title, body, nextStatus === "connected" ? "connected" : "disconnected",
          () => runtime.state.value.status === nextStatus && runtime.state.value.operationId === productState.operationId),
        hide: () => chrome.setVisible(false).catch((error: unknown) => {
            console.error("隐藏窗口失败", error);
          }),
      });

    }

    if (shouldAutoConnectOnLaunch(isFirstSnapshot, nextStatus, prefs)) {
      void runtime.connect();
    }
  },
  { immediate: true, flush: "sync" },
);

/** Darwin 沿原宿主 adapter 读取焦点；探测错误保留原始错误。 */
async function isWindowForeground(): Promise<boolean> {
  const adapter = darwinCommandAdapter();
  if (!adapter?.isWindowFocused) throw new Error("窗口壳前台查询接口未注入");
  return adapter.isWindowFocused();
}

async function invokeTrayNotify(title: string, body: string, kind: SystemNotificationKind, isCurrent = () => true): Promise<void> {
  const adapter = darwinCommandAdapter();
  if (source === "mock" || adapter === null) return;
  try {
    await dispatchSystemNotification(kind, title, body, {
      readPreferences: () => uiPreferencesState().value, isForeground: isWindowForeground, isCurrent,
      send: async (title, body) => { await adapter.call("tray_notify", { title, body }); },
    });
  } catch (error) {
    console.error("托盘通知失败", error);
  }
}

app.mount("#app");

if (source === "mock") {
  const badge = document.createElement("div");
  badge.textContent = "模拟数据";
  badge.setAttribute("role", "status");
  badge.style.cssText = [
    "position:fixed",
    "right:12px",
    "bottom:12px",
    "z-index:2147483647",
    "pointer-events:none",
    "padding:4px 8px",
    "border:1px solid currentColor",
    "border-radius:999px",
    "background:rgba(255,255,255,.9)",
    "color:#4b5563",
    "font: var(--font-size-caption)/1.2 system-ui,sans-serif",
  ].join(";");
  document.body.append(badge);
}

window.addEventListener("beforeunload", () => runtime.dispose(), { once: true });
