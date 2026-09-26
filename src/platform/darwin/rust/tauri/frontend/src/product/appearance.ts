import { computed, ref, type ComputedRef, type InjectionKey, type Ref } from "vue";

export type ThemePreference = "system" | "light" | "dark";
export type AccentPreference = "azure" | "violet" | "jade" | "amber";
export type MotionPreference = "normal" | "reduced";
export type WindowModePreference = "advanced" | "minimal";

export interface AppearanceState {
  theme: ThemePreference;
  accent: AccentPreference;
  motion: MotionPreference;
  mode: WindowModePreference;
}

export interface AppearanceStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  /** 显式保存必须报告真实持久化失败，不能降级为会话缓存。 */
  persistItem?(key: string, value: string): void;
}

export interface Appearance {
  readonly state: Readonly<Ref<AppearanceState>>;
  readonly documentClass: Readonly<ComputedRef<string>>;
  readonly draft: Readonly<Ref<AppearanceState>>;
  readonly dirty: Readonly<ComputedRef<boolean>>;
  /** 仅动效是否需要保存（主题/强调色/模式已即时落盘，不再计入）。 */
  readonly motionDirty: Readonly<ComputedRef<boolean>>;
  readonly saving: Readonly<Ref<boolean>>;
  readonly saveError: Readonly<Ref<string | null>>;
  edit<K extends keyof AppearanceState>(key: K, value: AppearanceState[K]): void;
  saveDraft(applyMode?: (mode: WindowModePreference) => Promise<void>, pending?: Partial<AppearanceState>): Promise<boolean>;
  /** 用户主动切换主题：立即落盘 + 应用；失败返回 false 并留下可见错误。 */
  commitTheme(theme: ThemePreference): Promise<boolean>;
  /** 用户主动切换强调色：立即落盘 + 应用；失败返回 false 并留下可见错误。 */
  commitAccent(accent: AccentPreference): Promise<boolean>;
  /**
   * 用户主动切换窗口模式：先 `applyMode`（宿主 chrome.setMode）后落盘；
   * 宿主应用失败必须回滚到上一个已生效模式并报错，不得假装成功。
   */
  commitMode(
    mode: WindowModePreference,
    applyMode: (mode: WindowModePreference) => Promise<void>,
  ): Promise<boolean>;
  setTheme(theme: ThemePreference): void;
  setAccent(accent: AccentPreference): void;
  setMotion(motion: MotionPreference): void;
  /** 展示层原语：只改呈现，不落盘（内部强制展开用；用户操作走 commitMode）。 */
  setMode(mode: WindowModePreference): void;
  applyDocument(root: HTMLElement): void;
}

export const APPEARANCE_KEY: InjectionKey<Appearance> = Symbol("exv.product.appearance");

export const APPEARANCE_STORAGE_KEY = "exv.ui.appearance";

type AppearanceStorageGetter = () => AppearanceStorage | null | undefined;

const DEFAULT_APPEARANCE: AppearanceState = {
  theme: "system",
  accent: "azure",
  motion: "normal",
  mode: "advanced",
};

/**
 * 为浏览器存储提供只管理外观数据的安全适配器。
 *
 * 某些隐私环境会让 `window.localStorage` 的属性访问本身抛出异常；其他环境则只会
 * 拒绝具体的读取或写入。无论哪一种，当前会话都应继续保留用户刚刚选择的外观，
 * 而不能让应用在挂载前失败。
 */
export function createBrowserSafeAppearanceStorage(
  getStorage: AppearanceStorageGetter,
): AppearanceStorage {
  const sessionValues = new Map<string, string>();
  let browserStorage: AppearanceStorage | null | undefined;

  function getBrowserStorage(): AppearanceStorage | null {
    if (browserStorage !== undefined) return browserStorage;

    try {
      browserStorage = getStorage() ?? null;
    } catch {
      browserStorage = null;
    }

    return browserStorage;
  }

  return {
    getItem(key) {
      if (key !== APPEARANCE_STORAGE_KEY) return null;

      const sessionValue = sessionValues.get(key);
      if (sessionValue !== undefined) return sessionValue;

      try {
        return getBrowserStorage()?.getItem(key) ?? null;
      } catch {
        return null;
      }
    },
    persistItem(key, value) {
      if (key !== APPEARANCE_STORAGE_KEY) return;
      const target = getBrowserStorage();
      if (!target) throw new Error("浏览器外观存储不可用");
      target.setItem(key, value);
      sessionValues.set(key, value);
    },
    setItem(key, value) {
      if (key !== APPEARANCE_STORAGE_KEY) return;

      sessionValues.set(key, value);
      try {
        getBrowserStorage()?.setItem(key, value);
      } catch {
        // 浏览器存储不可写时，内存副本仍保留本次会话的外观选择。
      }
    },
  };
}

function isThemePreference(value: unknown): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

function isAccentPreference(value: unknown): value is AccentPreference {
  return value === "azure" || value === "violet" || value === "jade" || value === "amber";
}

function isMotionPreference(value: unknown): value is MotionPreference {
  return value === "normal" || value === "reduced";
}

function isWindowModePreference(value: unknown): value is WindowModePreference {
  return value === "advanced" || value === "minimal";
}

function readAppearance(storage: AppearanceStorage): AppearanceState {
  try {
    const raw = storage.getItem(APPEARANCE_STORAGE_KEY);
    if (!raw) return { ...DEFAULT_APPEARANCE };

    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return { ...DEFAULT_APPEARANCE };

    const saved = parsed as Partial<AppearanceState>;
    return {
      theme: isThemePreference(saved.theme) ? saved.theme : DEFAULT_APPEARANCE.theme,
      accent: isAccentPreference(saved.accent) ? saved.accent : DEFAULT_APPEARANCE.accent,
      motion: isMotionPreference(saved.motion) ? saved.motion : DEFAULT_APPEARANCE.motion,
      mode: isWindowModePreference(saved.mode) ? saved.mode : DEFAULT_APPEARANCE.mode,
    };
  } catch {
    return { ...DEFAULT_APPEARANCE };
  }
}

/**
 * 仅管理本地视觉偏好。它不读取、保存或派生任何 VPN 运行状态、统计或凭据。
 */
export function createAppearance(storage: AppearanceStorage): Appearance {
  const state = ref<AppearanceState>(readAppearance(storage));
  const documentClass = computed(() => (state.value.motion === "reduced" ? "motion-reduced" : ""));
  let documentRoot: HTMLElement | null = null;
  const saved = ref<AppearanceState>({ ...state.value });
  const draft = ref<AppearanceState>({ ...saved.value });
  const saving = ref(false);
  const saveError = ref<string | null>(null);
  const modeRequested = ref(false);
  const dirty = computed(() => saveError.value !== null ||
    (modeRequested.value && draft.value.mode !== state.value.mode) ||
    (Object.keys(draft.value) as (keyof AppearanceState)[]).some((key) => draft.value[key] !== saved.value[key]));
  // D5 裁决（计划 §8.3）：主题/强调色/模式即时落盘，只有动效仍需显式保存。
  const motionDirty = computed(() => draft.value.motion !== saved.value.motion);

  async function saveDraft(
    applyMode?: (mode: WindowModePreference) => Promise<void>,
    pending: Partial<AppearanceState> = { ...draft.value },
  ): Promise<boolean> {
    if (saving.value) return false;
    // D5：只提交显式给出的键（设置页/快速入门现在只提交动效），其余键沿用已保存值；
    // 这样"仅保存动效"不会把内部强制展开的呈现模式写进偏好文件。
    const target: AppearanceState = { ...saved.value, ...pending };
    const pendingKeys = Object.keys(pending) as (keyof AppearanceState)[];
    const modeRequestedByCaller = pending.mode !== undefined;
    if (saveError.value === null &&
      pendingKeys.every((key) => pending[key] === saved.value[key])) return true;
    saving.value = true;
    saveError.value = null;
    let persisted = false;
    try {
      const modeChanged = modeRequestedByCaller && target.mode !== state.value.mode;
      if (modeChanged && !applyMode) throw new Error("窗口模式应用接口不可用");
      (storage.persistItem ?? storage.setItem).call(storage, APPEARANCE_STORAGE_KEY, JSON.stringify(target));
      persisted = true;
      if (modeChanged) await applyMode!(target.mode);
      state.value = { ...state.value, ...pending } as AppearanceState;
      applyToDocument();
      saved.value = { ...target };
      if (draft.value.mode === target.mode) modeRequested.value = false;
      return true;
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      saveError.value = (persisted ? "外观已落盘，但应用失败：" : "外观保存失败：") + detail + "；草稿已保留，请重试。";
      return false;
    } finally {
      saving.value = false;
    }
  }

  function persist(): void {
    const { theme, accent, motion, mode } = state.value;
    try {
      storage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify({ theme, accent, motion, mode }));
    } catch {
      // 本地存储不可用时保留当前会话中的外观；不得影响 VPN 业务流程。
    }
  }

  function applyToDocument(): void {
    if (!documentRoot) return;

    const { theme, accent, motion, mode } = state.value;
    documentRoot.dataset.theme = theme;
    documentRoot.dataset.accent = accent;
    documentRoot.dataset.motion = motion;
    documentRoot.dataset.mode = mode;
    documentRoot.classList.toggle("motion-reduced", motion === "reduced");
  }

  function update(next: AppearanceState): void {
    const previous = state.value;
    draft.value = Object.fromEntries((Object.keys(next) as (keyof AppearanceState)[])
      .map((key) => [key, draft.value[key] === previous[key] ? next[key] : draft.value[key]])) as unknown as AppearanceState;
    state.value = next;
    persist();
    saved.value = { ...next };
    applyToDocument();
  }

  /** 即时提交（不涉及窗口模式）：落盘失败不改变任何已生效状态，只留下可见错误。 */
  function commitKnown<K extends keyof AppearanceState>(
    key: K,
    value: AppearanceState[K],
  ): boolean {
    const next = { ...state.value, [key]: value };
    try {
      (storage.persistItem ?? storage.setItem).call(
        storage,
        APPEARANCE_STORAGE_KEY,
        JSON.stringify(next),
      );
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      saveError.value = "外观保存失败：" + detail + "；未生效，请重试。";
      return false;
    }
    state.value = next;
    saved.value = { ...next };
    draft.value = { ...draft.value, [key]: value };
    saveError.value = null;
    applyToDocument();
    return true;
  }

  return {
    state,
    documentClass,
    draft,
    dirty,
    motionDirty,
    saving,
    saveError,
    edit(key, value) {
      draft.value = { ...draft.value, [key]: value };
      // 再选已保存模式也可能是在请求结束一次临时展开；仍需显式保存后应用。
      if (key === "mode") modeRequested.value = value !== state.value.mode;
    },
    saveDraft,
    commitTheme(theme) {
      return Promise.resolve(commitKnown("theme", theme));
    },
    commitAccent(accent) {
      return Promise.resolve(commitKnown("accent", accent));
    },
    async commitMode(mode, applyMode) {
      if (saving.value) return false;
      if (mode === state.value.mode) return true;
      saving.value = true;
      saveError.value = null;
      const previous = state.value;
      const next = { ...state.value, mode };
      let persisted = false;
      try {
        // 顺序：先落盘（记录用户意图），再让宿主切换原生窗口；宿主失败则把落盘值
        // 与内存状态一并回滚到 previous——不得留下"窗口没变但偏好已变"的假成功。
        (storage.persistItem ?? storage.setItem).call(
          storage,
          APPEARANCE_STORAGE_KEY,
          JSON.stringify(next),
        );
        persisted = true;
        await applyMode(mode);
        state.value = next;
        saved.value = { ...next };
        draft.value = { ...draft.value, mode };
        modeRequested.value = false;
        applyToDocument();
        return true;
      } catch (error) {
        if (persisted) {
          try {
            (storage.persistItem ?? storage.setItem).call(
              storage,
              APPEARANCE_STORAGE_KEY,
              JSON.stringify(previous),
            );
          } catch {
            // 回滚落盘失败只影响下次启动的呈现；内存状态仍按本节回滚。
          }
        }
        state.value = previous;
        saved.value = { ...previous };
        draft.value = { ...draft.value, mode: previous.mode };
        modeRequested.value = false;
        applyToDocument();
        const detail = error instanceof Error ? error.message : String(error);
        saveError.value = "外观保存失败：" + detail + "；未生效，请重试。";
        return false;
      } finally {
        saving.value = false;
      }
    },
    setTheme(theme) {
      update({ ...state.value, theme });
    },
    setAccent(accent) {
      update({ ...state.value, accent });
    },
    setMotion(motion) {
      update({ ...state.value, motion });
    },
    setMode(mode) {
      // 展示层原语（D5 裁决 §8.3）：内部强制展开认证/详情窗口只更新实际呈现，
      // 不落盘、不更新 saved；用户主动切换走 commitMode。
      state.value = { ...state.value, mode };
      applyToDocument();
    },
    applyDocument(root) {
      documentRoot = root;
      applyToDocument();
    },
  };
}
