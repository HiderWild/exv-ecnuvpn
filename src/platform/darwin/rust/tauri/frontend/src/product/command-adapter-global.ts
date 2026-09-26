/**
 * Darwin 壳注入的最小 command/event 边界（darwin 前端唯一 IPC 接缝）。
 *
 * win32 UI 全量接入后，前端不导入任何宿主 SDK：`lib/ipc.ts`、`product/window-chrome.ts`、
 * `product/ui-prefs.ts`、`pages/AboutPage.vue` 等处的 invoke/listen 语义都收敛到本全局
 * adapter（call = Tauri invoke，listen = 事件订阅，回调收到带 `payload` 字段的事件对象，
 * 与宿主事件 API 形状等价）。
 */

/** 事件回调收到的形状（与宿主 event API 的 `Event<T>` 等价：只消费 `payload`）。 */
export interface TauriAdapterEvent<Payload> {
  payload: Payload;
}

/** 事件退订函数（与宿主 event API 的 `UnlistenFn` 等价）。 */
export type TauriAdapterUnlisten = () => void | Promise<void>;

/** 由 Darwin Tauri 壳在加载前注入的最小命令/事件边界。 */
export interface TauriCommandAdapter {
  call<Result>(command: string, args?: Readonly<Record<string, unknown>>): Promise<Result>;
  listen<Payload>(
    event: string,
    listener: (event: TauriAdapterEvent<Payload>) => void,
  ): Promise<TauriAdapterUnlisten>;
  /** 主窗口是否聚焦（壳经宿主 window API 提供；未注入时按「不在前台」处理）。 */
  isWindowFocused?(): Promise<boolean>;
}

declare global {
  interface Window {
    /** 仅由 Darwin Tauri 壳在加载前注入；普通浏览器没有此值。 */
    __EXV_DARWIN_COMMAND_ADAPTER__?: TauriCommandAdapter;
  }
}

/**
 * 读取壳注入的 command adapter（`__EXV_DARWIN_COMMAND_ADAPTER__` 全局声明的单一来源）。
 *
 * 浏览器/预览入口没有该全局：返回 null，调用方按「外壳不可用」诚实降级，
 * 不导入任何宿主 SDK、不猜测宿主 API。
 */
export function darwinCommandAdapter(): TauriCommandAdapter | null {
  if (typeof window === "undefined") return null;
  return window.__EXV_DARWIN_COMMAND_ADAPTER__ ?? null;
}
