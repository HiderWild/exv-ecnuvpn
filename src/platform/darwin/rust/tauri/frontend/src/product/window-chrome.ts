// Darwin 适配（单一接缝）：不导入宿主 SDK；invoke/listen 默认实现经壳注入的
// `__EXV_DARWIN_COMMAND_ADAPTER__` 全局 adapter（product/command-adapter-global.ts）。
import { darwinCommandAdapter } from "./command-adapter-global";
import type { InjectionKey } from "vue";

export type WindowMode = "advanced" | "minimal";
export type NativeWindowControl = "minimize" | "maximize" | "close";
export interface NativeWindowControlState {
  control: NativeWindowControl | null;
  pressed: boolean;
}
export type WindowChromeEventListener = (event: { payload: unknown }) => void;
export type WindowChromeListen = (
  event: string,
  listener: WindowChromeEventListener,
) => Promise<() => void>;

export interface WindowChromePort {
  setMode(mode: WindowMode): Promise<void>;
  control(control: NativeWindowControl): Promise<void>;
  /** 窗口显隐原语（连接后最小化到托盘 / 托盘唤出）；纯显隐，不携带关闭语义。 */
  setVisible(visible: boolean): Promise<void>;
  subscribeControlState(listener: (state: NativeWindowControlState) => void): Promise<() => void>;
}
export const WINDOW_CHROME_PORT_KEY: InjectionKey<WindowChromePort> = Symbol("exv.product.window-chrome");
export interface WindowChromeOptions {
  browserPreview?: boolean;
  invoke?: (
    command: string,
    args: { mode: WindowMode } | { control: NativeWindowControl } | { visible: boolean },
  ) => Promise<unknown>;
  listen?: WindowChromeListen;
}

function isNativeWindowControl(value: unknown): value is NativeWindowControl {
  return value === "minimize" || value === "maximize" || value === "close";
}

function parseControlState(payload: unknown): NativeWindowControlState {
  if (typeof payload !== "object" || payload === null) {
    return { control: null, pressed: false };
  }
  const raw = payload as { control?: unknown; pressed?: unknown };
  return {
    control: isNativeWindowControl(raw.control) ? raw.control : null,
    pressed: raw.pressed === true,
  };
}

export function createWindowChromePort(options: WindowChromeOptions = {}): WindowChromePort {
  const browserPreview = options.browserPreview ?? (options.invoke === undefined && options.listen === undefined && darwinCommandAdapter() === null);
  const invoke = options.invoke ?? ((command, args) => {
    const adapter = darwinCommandAdapter();
    if (!adapter) return Promise.reject(new Error("窗口壳 adapter 未注入（纯浏览器预览）"));
    return adapter.call(command, args);
  });
  const listen = options.listen ?? ((event, listener) => {
    const adapter = darwinCommandAdapter();
    if (!adapter) return Promise.reject(new Error("窗口壳 adapter 未注入（纯浏览器预览）"));
    return adapter.listen<unknown>(event, listener);
  });
  return {
    async setMode(mode) {
      if (browserPreview) return;
      await invoke("window_chrome_set_mode", { mode });
    },
    async control(control) {
      if (browserPreview) return;
      await invoke("window_chrome_control", { control });
    },
    async setVisible(visible) {
      if (browserPreview) return;
      await invoke("window_set_visible", { visible });
    },
    async subscribeControlState(listener) {
      if (browserPreview) return () => undefined;
      return listen("window-control-state", (event) => listener(parseControlState(event.payload)));
    },
  };
}
