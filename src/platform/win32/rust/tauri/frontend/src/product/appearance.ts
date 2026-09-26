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
  /** 全部外观键的待保存草稿脏标记（快速入门主题等仍走草稿）。 */
  readonly dirty: Readonly<ComputedRef<boolean>>;
  /** 仅动效键的待保存脏标记；设置页“保存设置”据此决定是否提交外观。 */
  readonly motionDirty: Readonly<ComputedRef<boolean>>;
  readonly saving: Readonly<Ref<boolean>>;
  readonly saveError: Readonly<Ref<string | null>>;
  edit<K extends keyof AppearanceState>(key: K, value: AppearanceState[K]): void;
  saveDraft(applyMode?: (mode: WindowModePreference) => Promise<void>, pending?: AppearanceState): Promise<boolean>;
  /**
   * 用户主动切换窗口模式：先应用宿主窗口，成功才落盘并收敛草稿；
   * 失败回滚状态/草稿/文档属性并给出可见错误，不产生“假成功”。
   */
  commitMode(mode: WindowModePreference, applyMode: (mode: WindowModePreference) => Promise<void>): Promise<boolean>;
  setTheme(theme: ThemePreference): void;
  setAccent(accent: AccentPreference): void;
  setMotion(motion: MotionPreference): void;
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
  const motionDirty = computed(() => draft.value.motion !== saved.value.motion);

  async function saveDraft(
    applyMode?: (mode: WindowModePreference) => Promise<void>,
    pending: AppearanceState = { ...draft.value },
  ): Promise<boolean> {
    if (saving.value) return false;
    if (saveError.value === null && pending.mode === state.value.mode &&
      (Object.keys(pending) as (keyof AppearanceState)[]).every((key) => pending[key] === saved.value[key])) return true;
    saving.value = true;
    saveError.value = null;
    let persisted = false;
    try {
      if (pending.mode !== state.value.mode && !applyMode) throw new Error("窗口模式应用接口不可用");
      (storage.persistItem ?? storage.setItem).call(storage, APPEARANCE_STORAGE_KEY, JSON.stringify(pending));
      persisted = true;
      if (pending.mode !== state.value.mode) await applyMode!(pending.mode);
      state.value = pending;
      applyToDocument();
      saved.value = { ...pending };
      if (draft.value.mode === pending.mode) modeRequested.value = false;
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

  /** 显式提交路径：存储不可写必须抛出，由调用方回滚（主题/强调色仍走吞错的 persist）。 */
  function persistStrict(value: AppearanceState): void {
    (storage.persistItem ?? storage.setItem).call(storage, APPEARANCE_STORAGE_KEY, JSON.stringify(value));
  }

  /**
   * 用户主动切换窗口模式（标题栏模式控件）。
   *
   * 与展示层原语 `setMode` 的区别：本入口先请求宿主应用窗口模式，成功才落盘、
   * 更新 `state`/`saved`、应用到文档；失败回滚全部内存状态与文档属性并写入
   * `saveError`，绝不假装成功。
   */
  async function commitMode(
    mode: WindowModePreference,
    applyMode: (mode: WindowModePreference) => Promise<void>,
  ): Promise<boolean> {
    if (saving.value) return false;
    // 选择当前已生效模式不是一次切换：只收敛可能残留的待保存请求，不落盘、不调宿主。
    if (mode === state.value.mode) {
      draft.value = { ...draft.value, mode };
      modeRequested.value = false;
      if (saveError.value !== null) saveError.value = null;
      return true;
    }

    const previousState = { ...state.value };
    const previousDraft = { ...draft.value };
    saving.value = true;
    saveError.value = null;
    let applied = false;
    try {
      await applyMode(mode);
      applied = true;
      const next: AppearanceState = { ...state.value, mode };
      persistStrict(next);
      state.value = next;
      saved.value = { ...next };
      draft.value = { ...draft.value, mode };
      modeRequested.value = false;
      applyToDocument();
      return true;
    } catch (error) {
      // 宿主已切换时尽力恢复原生窗口，避免内存呈现与真实窗口分叉。
      if (applied) {
        try {
          await applyMode(previousState.mode);
        } catch {
          // 原生窗口恢复失败不再抛出；错误信息已如实说明，状态收敛为回滚后的值。
        }
      }
      state.value = previousState;
      // 草稿与待保存请求一并收敛回已提交模式，避免失败后留下会被其它入口
      // （如快速入门提交）再次消费的幽灵模式请求。
      draft.value = { ...previousDraft, mode: previousState.mode };
      modeRequested.value = false;
      applyToDocument();
      const detail = error instanceof Error ? error.message : String(error);
      saveError.value = "窗口模式切换失败：" + detail + "；已恢复原模式，请重试。";
      return false;
    } finally {
      saving.value = false;
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
    commitMode,
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
      // 临时展开认证/详情窗口只更新实际呈现；持久化模式由 edit + saveDraft 管理。
      state.value = { ...state.value, mode };
      applyToDocument();
    },
    applyDocument(root) {
      documentRoot = root;
      applyToDocument();
    },
  };
}
