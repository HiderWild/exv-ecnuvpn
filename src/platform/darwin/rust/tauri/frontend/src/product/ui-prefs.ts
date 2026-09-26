// 前端自有设置（UI preferences）：纯 UI 行为偏好的状态层。
//
// 边界切割（与 app/src/ui_prefs.rs 头注释同源）：
//   * core 配置（server/username/…）走 core-config.ts 的 config_get/set——本模块绝不触碰；
//   * 本模块的键是纯前端行为偏好，存 `~/Library/Application Support/EXV/ui-preferences.json`
//     （字段名照抄宿主偏好契约，两端键集一致；Rust 侧负责持久化）。
//
// 保存模型：页面编辑只写 `draft`；显式保存时才按归属分流到 LaunchAgent 自启 / 偏好文件。
// `draft` 同时承载待保存值与失败后可重试的用户选择。

import { computed, ref } from "vue";

import { darwinCommandAdapter } from "./command-adapter-global";

export type ClosePreference = "smart" | "tray" | "quit";

export interface UiPreferences {
  /** 关闭按钮行为："smart"（连接期间后台运行，空闲时退出）| "tray" | "quit"。 */
  close_preference: ClosePreference;
  /** 连接成功后自动隐藏窗口到托盘。 */
  minimize_to_tray_on_connect: boolean;
  /** 开机自动运行。 */
  launch_at_login: boolean;
  /** 启动静默：所有启动方式都不弹窗，仅托盘驻留。 */
  silent_startup: boolean;
  /** 遗留键（存储兼容）：旧版「连接状态通知」单开关。新版本不再渲染/消费，
   * 仅保留读写以兼容旧偏好文件；其 true 不再驱动任何通知（由下方分事件键接管）。 */
  connection_state_notifications: boolean;
  /** 建立连接时发送通知。 */
  connect_notify: boolean;
  /** 断开连接时发送通知。 */
  disconnect_notify: boolean;
  /** 触发重连时发送通知（C5 自动重连落地前仅开关+文案，效果待接线）。 */
  reconnect_notify: boolean;
  /** 窗口在前台（聚焦）时抑制所有连接类通知；优先级最高。 */
  suppress_notify_when_foreground: boolean;
  /** 应用启动且空闲时自动发起连接。 */
  auto_connect_on_launch: boolean;
  /** 连接页/快速入门「安装服务」默认勾选意图（纯前端偏好：只写偏好文件，无宿主副作用）。 */
  install_service_on_connect: boolean;
  /** 连接后显示实时时延（毫秒）；纯前端显示偏好，默认关闭。 */
  show_latency: boolean;
}

/** 全部可保存键（页面据此渲染控件；旧 connection_state_notifications 不再渲染/可编辑）。 */
export const UI_PREF_KEYS = [
  "close_preference",
  "minimize_to_tray_on_connect",
  "launch_at_login",
  "silent_startup",
  "connect_notify",
  "disconnect_notify",
  "reconnect_notify",
  "suppress_notify_when_foreground",
  "auto_connect_on_launch",
  "install_service_on_connect",
  "show_latency",
] as const;

export type UiPrefKey = (typeof UI_PREF_KEYS)[number];

/** 按项保存状态与失败反馈；和草稿同属模块级状态，页面重挂载后仍可继续处理。 */
export const uiPrefSaving = ref<Partial<Record<UiPrefKey, boolean>>>({});
export const uiPrefErrors = ref<Partial<Record<UiPrefKey, string>>>({});

export const CLOSE_PREFERENCE_VALUES: readonly ClosePreference[] = ["smart", "tray", "quit"];

export const DEFAULT_UI_PREFERENCES: UiPreferences = {
  close_preference: "smart",
  minimize_to_tray_on_connect: false,
  launch_at_login: false,
  silent_startup: false,
  connection_state_notifications: false,
  connect_notify: false,
  disconnect_notify: false,
  reconnect_notify: false,
  suppress_notify_when_foreground: true,
  auto_connect_on_launch: false,
  install_service_on_connect: true,
  show_latency: false,
};

export const CLOSE_PREFERENCE_LABELS: Record<ClosePreference, string> = {
  smart: "智能判断最小化与退出",
  tray: "最小化到托盘",
  quit: "直接退出",
};

export type UiPreferencesPatch = Partial<UiPreferences>;

/** 后端 Command 的返回/入参形状：字段可缺省（读 = 有效值视图；写 = 补丁）。 */
export type UiPreferencesWire = UiPreferencesPatch;

export interface UiPrefsGateway {
  get(): Promise<UiPreferencesWire>;
  set(patch: UiPreferencesPatch): Promise<UiPreferencesWire>;
  /** 设置开机自启（LaunchAgent 执行层，写 ~/Library/LaunchAgents plist）；返回 ok + message。 */
  setAutostart(enabled: boolean): Promise<{ ok: boolean; message?: string }>;
}

/** 校验 wire 值并回落默认（坏值不进状态）。 */
export function normalizeUiPreferences(raw: UiPreferencesWire | null | undefined): UiPreferences {
  const merged: UiPreferences = { ...DEFAULT_UI_PREFERENCES };
  if (!raw || typeof raw !== "object") return merged;

  if (
    typeof raw.close_preference === "string" &&
    (CLOSE_PREFERENCE_VALUES as readonly string[]).includes(raw.close_preference)
  ) {
    merged.close_preference = raw.close_preference;
  }
  for (const key of [
    "minimize_to_tray_on_connect",
    "launch_at_login",
    "silent_startup",
    "connection_state_notifications",
    "connect_notify",
    "disconnect_notify",
    "reconnect_notify",
    "suppress_notify_when_foreground",
    "auto_connect_on_launch",
    "install_service_on_connect",
    "show_latency",
  ] as const) {
    const value = raw[key];
    if (typeof value === "boolean") merged[key] = value;
  }
  return merged;
}

// Darwin 适配（单一接缝）：不导入宿主 SDK；invoke 经壳注入的全局 adapter 等价映射。
async function invokeCommand<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const adapter = darwinCommandAdapter();
  if (!adapter) {
    throw new Error("ui prefs 仅在 Tauri 外壳内可用");
  }
  return adapter.call<T>(cmd, args);
}

/** 生产网关：走 Tauri Command（ui_prefs.rs / autostart.rs）。 */
export function createTauriUiPrefsGateway(): UiPrefsGateway {
  return {
    get() {
      return invokeCommand<UiPreferencesWire>("ui_prefs_get");
    },
    set(patch) {
      return invokeCommand<UiPreferencesWire>("ui_prefs_set", { patch });
    },
    async setAutostart(enabled) {
      return invokeCommand<{ ok: boolean; message?: string }>("autostart_set", { enabled });
    },
  };
}

// ---- 模块级单例：已生效值 + 草稿 ----

const state = ref<UiPreferences>({ ...DEFAULT_UI_PREFERENCES });
const draft = ref<Partial<UiPreferences>>({});
let gateway: UiPrefsGateway | null = null;
let loadedOnce = false;
let loadPromise: Promise<void> | null = null;
let saveTail: Promise<void> = Promise.resolve();
const pendingKeys = new Set<UiPrefKey>();
let stateEpoch = 0;

/** 草稿视图（页面控件读这里；缺键回落已生效值）。 */
export const uiPrefsDraft = computed<UiPreferences>(() => ({ ...state.value, ...draft.value }));

/** 是否存在未保存的偏好修改。 */
export const uiPrefsDirty = computed(() => Object.keys(draft.value).length > 0);

/** 测试重置（生产路径不调用）。 */
export function resetUiPrefsState(): void {
  stateEpoch += 1;
  state.value = { ...DEFAULT_UI_PREFERENCES };
  draft.value = {};
  gateway = null;
  loadedOnce = false;
  loadPromise = null;
  saveTail = Promise.resolve();
  pendingKeys.clear();
  uiPrefSaving.value = {};
  uiPrefErrors.value = {};
}

/** 注入网关（测试注入内存实现；生产在 main.ts 调用一次装真实网关）。
 * 换网关后允许重新 load（新网关的存储才是真相源）。 */
export function provideUiPrefsGateway(impl: UiPrefsGateway): void {
  stateEpoch += 1;
  gateway = impl;
  loadedOnce = false;
  loadPromise = null;
  saveTail = Promise.resolve();
  pendingKeys.clear();
  uiPrefSaving.value = {};
  uiPrefErrors.value = {};
}

/**
 * 加载一次有效偏好（重复调用复用在途/已完成的结果）。
 * Tauri 外壳之外（纯浏览器预览）保持默认值，不视为错误。
 * 加载会丢弃草稿（存储是真相源；仅在启动加载场景发生）。
 */
export async function loadUiPreferences(): Promise<void> {
  if (loadedOnce) return;
  if (loadPromise !== null) return loadPromise;

  loadPromise = (async () => {
    const impl = gateway ?? createTauriUiPrefsGateway();
    try {
      const raw = await impl.get();
      state.value = normalizeUiPreferences(raw);
      draft.value = {};
      uiPrefErrors.value = {};
      loadedOnce = true;
    } catch {
      // 外壳之外 / 后端不可用：保持默认值（诚实降级，不阻断 UI）。
    }
  })();

  try {
    await loadPromise;
  } finally {
    loadPromise = null;
  }
}

/** 编辑草稿（页面控件调用；保存成功前不视为已生效）。 */
export function editUiPreference<K extends UiPrefKey>(key: K, value: UiPreferences[K]): void {
  if (value === state.value[key] && !pendingKeys.has(key)) {
    // 改回原值 = 该键不再脏。
    const next = { ...draft.value };
    delete next[key];
    draft.value = next;
    return;
  }
  draft.value = { ...draft.value, [key]: value };
}

/**
 * 「安装服务」偏好的即时提交（R2/R3）：用户勾选/取消无需点保存，直接写偏好文件。
 *
 * 该键是**纯前端偏好**——只走 `set`（偏好文件），绝不经过 `launch_at_login` 的
 * LaunchAgent 分支；无宿主副作用。失败返回 false 并保留已生效旧值（由
 * [`updateUiPreferences`] 的错误面记录），调用方据此选择报错或仅记录。
 */
export async function commitInstallServicePreference(
  enabled: boolean,
): Promise<boolean> {
  const patch: UiPreferencesPatch = { install_service_on_connect: enabled };
  editUiPreference("install_service_on_connect", enabled);
  return updateUiPreferences(["install_service_on_connect"], patch);
}

/** 当前已保存的「安装服务」偏好（默认 true；未加载完成时也是默认值）。 */
export function installServicePreference(): boolean {
  return uiPrefsDraft.value.install_service_on_connect;
}

function effectiveImpl(): UiPrefsGateway {
  return gateway ?? createTauriUiPrefsGateway();
}

/** 捕获用户点击保存时的脏值；等待其他分支期间的新编辑仍留在草稿。 */
export function captureUiPreferenceChanges(): UiPreferencesPatch {
  return { ...draft.value };
}

/**
 * 保存动作执行器：只截取调用方指定的脏键，并按调用顺序串行持久化。
 *
 * 分流规则：
 *   * `launch_at_login` → 先写 LaunchAgent（执行真相源），再持久化偏好文件（显示态）；
 *   * 其余键 → 直接持久化偏好文件（UI 进程启动时读取生效）。
 *
 * 失败时保留本次草稿，并尽力把已改的 LaunchAgent 值写回；成功时只清除仍等于本次快照的
 * 草稿，避免吞掉排队期间的新编辑。
 */
export function updateUiPreferences(
  requestedKeys: readonly UiPrefKey[] = UI_PREF_KEYS,
  requestedValues: UiPreferencesPatch = draft.value,
): Promise<boolean> {
  const patch: UiPreferencesPatch = {};
  const keys: UiPrefKey[] = [];
  for (const key of requestedKeys) {
    if (pendingKeys.has(key) || requestedValues[key] === undefined) continue;
    (patch as Record<UiPrefKey, UiPreferences[UiPrefKey]>)[key] = requestedValues[key] as UiPreferences[UiPrefKey];
    keys.push(key);
  }

  if (keys.length === 0) {
    // 同一键在前一项保存途中再次被编辑时，等队列落稳后保存最新草稿。
    if (requestedKeys.some((key) => pendingKeys.has(key) && draft.value[key] !== undefined)) {
      return saveTail.then(() => updateUiPreferences(requestedKeys));
    }
    return Promise.resolve(true);
  }

  for (const key of keys) pendingKeys.add(key);
  const nextErrors = { ...uiPrefErrors.value };
  const nextSaving = { ...uiPrefSaving.value };
  for (const key of keys) {
    delete nextErrors[key];
    nextSaving[key] = true;
  }
  uiPrefErrors.value = nextErrors;
  uiPrefSaving.value = nextSaving;
  const operationEpoch = stateEpoch;
  const impl = effectiveImpl();

  const run = async (): Promise<boolean> => {
    const previous = state.value;
    let autostartChanged = false;
    try {
      if (patch.launch_at_login !== undefined && patch.launch_at_login !== previous.launch_at_login) {
        const result = await impl.setAutostart(patch.launch_at_login);
        if (!result.ok) throw new Error(result.message ?? "autostart rejected");
        autostartChanged = true;
      }

      await impl.set(patch);
      if (operationEpoch !== stateEpoch) return false;

      // 用户可能在 Core 等待期间改回旧基线，稀疏草稿会删掉该键；先保留完整意图，
      // 再采用本次快照，不能把这种“撤回”误认为没有后续编辑。
      const latestIntent = { ...uiPrefsDraft.value };
      state.value = normalizeUiPreferences({ ...state.value, ...patch });
      const remaining = { ...draft.value };
      for (const key of keys) {
        if (latestIntent[key] === patch[key]) delete remaining[key];
        else (remaining as Record<UiPrefKey, UiPreferences[UiPrefKey]>)[key] = latestIntent[key];
      }
      draft.value = remaining;
      const errorsAfterSave = { ...uiPrefErrors.value };
      for (const key of keys) delete errorsAfterSave[key];
      uiPrefErrors.value = errorsAfterSave;
      return true;
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      let rollbackError = "";
      if (autostartChanged) {
        try {
          const rollback = await impl.setAutostart(previous.launch_at_login);
          if (!rollback.ok) throw new Error(rollback.message ?? "自启恢复被拒绝");
        } catch (error) {
          rollbackError = "；自启可能已变更，恢复失败：" + (error instanceof Error ? error.message : String(error));
        }
      }
      if (operationEpoch === stateEpoch) {
        const errorsAfterFailure = { ...uiPrefErrors.value };
        for (const key of keys) errorsAfterFailure[key] = "保存失败，未确认生效：" + detail + rollbackError;
        uiPrefErrors.value = errorsAfterFailure;
      }
      return false;
    }
  };

  const result = saveTail.then(run, run);
  saveTail = result.then(
    () => undefined,
    () => undefined,
  );
  return result.finally(() => {
    for (const key of keys) pendingKeys.delete(key);
    if (operationEpoch === stateEpoch) {
      const savingAfter = { ...uiPrefSaving.value };
      for (const key of keys) savingAfter[key] = false;
      uiPrefSaving.value = savingAfter;
    }
  });
}

/** 只读视图（运行时效果消费 lifecycle-effects 等）。 */
export function uiPreferencesState() {
  return state;
}
