import { createApp, watch } from "vue";
import App from "./App.vue";
import { kernel, type ConfigPayload } from "./lib/ipc";
import { pushToast } from "./lib/toast";
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
import { createMockCoreConfigGateway, createMockLogsGateway, createMockUiPrefsGateway, selectPreviewPageFault } from "./product/mock-page-data";
import { createMockRuntime, selectRuntimeSource, selectPreviewStatus, selectPreviewPage, selectPreviewScenario, selectProgressPreview } from "./product/mock-runtime";
import { selectPreviewModal } from "./product/mock-modal-scene";
import { announceReconnectSnapshot } from "./product/reconnect-notify";
import { createProductRuntime, PRODUCT_RUNTIME_KEY } from "./product/runtime";
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
// T12：真实运行时的快照采纳钩子喂重连通知（R1/R2 reason lane → toast）。
// 阶段④：同一喂入口组合 failed-notify（失败终态单次 toast；与 lane 终态/N-02
// 托盘气泡单向互斥）——两个通知出口共用一次快照采纳，顺序无关（互斥位在
// failed-notify 内同步维护，reconnect-notify 先跑以先置 lane 终态游标）。
const mockRuntime = source === "mock"
  ? createMockRuntime(selectPreviewStatus(import.meta.env.DEV, window.location.search), undefined, selectPreviewScenario(import.meta.env.DEV, window.location.search))
  : null;
let engineRecoveryActive = false;
const runtime = mockRuntime ?? createProductRuntime(undefined, undefined, (snapshot) => {
      engineRecoveryActive = ["tunnel_lost", "heal_assembly_failed", "heal_timeout"].includes(snapshot.engine_reconnect?.reason ?? "");
      announceReconnectSnapshot(snapshot, { lifecycleOwnsProgress: true });
      announceFailedSnapshot(snapshot);
    });
/**
 * Core 在 Rust setup 中完成 bootstrap 后才暴露 Tauri gateway；主入口把这次读取注入
 * App，由 App 在首次挂载时按「已见标记 + requires_quick_start」决定是否显示对话框
 * （KI-13：有且仅有首次运行弹出；详见 product/quick-start.ts）。
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
  ...(progressScene && mockRuntime ? { previewRuntime:mockRuntime } : {}),
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
app.provide(CORE_CONFIG_GATEWAY_KEY, source === "mock" ? createMockCoreConfigGateway(selectPreviewPageFault(import.meta.env.DEV, window.location.search)) : createCoreConfigGateway());
if (source === "mock") {
  app.provide(LOGS_GATEWAY_KEY, createMockLogsGateway(undefined, selectPreviewPageFault(import.meta.env.DEV, window.location.search)));
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
} else if (selectPreviewPageFault(import.meta.env.DEV, window.location.search) === "settings-save-failure") {
  provideUiPrefsGateway(createMockUiPrefsGateway("settings-save-failure"));
} else if (modalScene) {
  provideUiPrefsGateway(createMockUiPrefsGateway());
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
    const observed = reconnectNotifications.observe(productState, engineRecoveryActive);
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
        // 阶段④ N-02 互斥：失败 episode 已有声音（failed-notify/lane 终态）时，
        // 本次 connected → failed_* 的「EXV 已断开」托盘气泡让位。快照采纳钩子
        // 先于本回调执行，此处读到的已是本快照结论。
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

/** Windows 读取即时原生前台窗口归属；读取错误由通知原入口处理。 */
async function isWindowForeground(): Promise<boolean> {
  return chrome.isForeground();
}

async function invokeTrayNotify(title: string, body: string, kind: SystemNotificationKind, isCurrent = () => true): Promise<void> {
  if (source === "mock" || !("__TAURI_INTERNALS__" in window)) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await dispatchSystemNotification(kind, title, body, {
      readPreferences: () => uiPreferencesState().value, isForeground: isWindowForeground, isCurrent,
      send: (title, body) => invoke("tray_notify", { title, body }),
    });
  } catch (error) {
    console.error("托盘通知失败", error);
  }
}

app.mount("#app");

if (source === "mock" && new URLSearchParams(window.location.search).get("capture") !== "1") {
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
  // DEV 紧凑模态标识留在左下，不覆盖右侧取消／重试动作；普通预览沿用原排版。
  if (import.meta.env.DEV && modalScene && appearance.state.value.mode === "minimal") {
    badge.style.right = "auto";
    badge.style.left = "4px";
    badge.style.bottom = "2px";
    badge.style.fontSize = "var(--font-size-caption)";
    badge.style.padding = "1px 3px";
  }
  document.body.append(badge);
}

window.addEventListener("beforeunload", () => runtime.dispose(), { once: true });
