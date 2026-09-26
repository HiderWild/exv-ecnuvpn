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
import { computeLifecycleEffects, computeSelfHealNotify, shouldAutoConnectOnLaunch } from "./product/lifecycle-effects";
import { LOGS_GATEWAY_KEY } from "./product/logs";
import { createMockCoreConfigGateway, createMockLogsGateway } from "./product/mock-page-data";
import { createMockRuntime, selectRuntimeSource, selectPreviewStatus } from "./product/mock-runtime";
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

const appearance = createAppearance(createBrowserSafeAppearanceStorage(() => window.localStorage));
appearance.applyDocument(document.documentElement);
const chrome = createWindowChromePort();
const source = selectRuntimeSource(import.meta.env.DEV, window.location.search);
const runtime = source === "mock" ? createMockRuntime(selectPreviewStatus(import.meta.env.DEV, window.location.search)) : createProductRuntime();
/**
 * Core 在 Rust setup 中完成 bootstrap 后才暴露 Tauri gateway；主入口把这次读取注入
 * App，由 App 在首次挂载时只按 `requires_quick_start` 决定是否显示对话框。
 */
const loadQuickStart = (): Promise<ConfigPayload> => {
  if (source === "mock") return Promise.resolve({ items: [], requires_quick_start: false });
  return kernel.configGet();
};
const app = createApp(App, {
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

// 连接过渡效果分发：状态流 → hide-window / tray-notify / 启动自动连接。
let previousStatus: ProductStatus | null = null;
// 2026-09-05 host 自愈（计划 §4.5）：selfHeal stage 去重游标（与 previousStatus 同模式；
// null = 首快照只记录基线）。
let prevSelfHealStage: string | null = null;
watch(
  runtime.state,
  (productState) => {
    const prefs = uiPreferencesState().value;
    const nextStatus = productState.status;
    const prevStatus = previousStatus;
    const isFirstSnapshot = prevStatus === null;

    // host 自愈失败气泡（§4.5 冻结）：仅非 failed → failed 进入时一次；respawning/
    // succeeded 不弹。不参与前台异步探测（失败是必须知道的事件），同步分发即可。
    const nextSelfHealStage = productState.selfHeal.active ? productState.selfHeal.stage : null;
    for (const effect of computeSelfHealNotify(prevSelfHealStage, nextSelfHealStage)) {
      if (effect.kind === "tray-notify") {
        void invokeTrayNotify(effect.title, effect.body);
      }
    }
    prevSelfHealStage = nextSelfHealStage;

    // 前台状态异步探测后分发效果；等待期间状态再次变化则由后续回调接管（丢弃本次，避免错序）。
    void (async () => {
      const foreground = await isWindowForeground();
      if (previousStatus !== prevStatus) return;

      for (const effect of computeLifecycleEffects(prevStatus, nextStatus, prefs, { foreground })) {
        if (effect.kind === "hide-window") {
          void chrome.setVisible(false).catch((error: unknown) => {
            console.error("隐藏窗口失败", error);
          });
        } else if (effect.kind === "tray-notify") {
          void invokeTrayNotify(effect.title, effect.body);
        }
      }

      previousStatus = nextStatus;
    })();

    if (shouldAutoConnectOnLaunch(isFirstSnapshot, nextStatus, prefs)) {
      void runtime.connect();
    }
  },
  { immediate: true },
);

/** 主窗口是否在前台（聚焦）：Darwin 壳经注入 adapter 的 isWindowFocused 提供；
 * mock/浏览器环境回退 false（不抑制）；探测失败也按「不在前台」处理（不阻断通知）。 */
async function isWindowForeground(): Promise<boolean> {
  if (source === "mock") return false;
  const adapter = darwinCommandAdapter();
  if (adapter === null || adapter.isWindowFocused === undefined) return false;
  try {
    return await adapter.isWindowFocused();
  } catch (error) {
    console.error("窗口前台状态检测失败", error);
    return false;
  }
}

async function invokeTrayNotify(title: string, body: string): Promise<void> {
  const adapter = darwinCommandAdapter();
  if (source === "mock" || adapter === null) return;
  try {
    await adapter.call("tray_notify", { title, body });
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
    "font:12px/1.2 system-ui,sans-serif",
  ].join(";");
  document.body.append(badge);
}

window.addEventListener("beforeunload", () => runtime.dispose(), { once: true });
