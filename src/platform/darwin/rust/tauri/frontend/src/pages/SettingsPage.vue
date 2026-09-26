<script setup lang="ts">
import { computed, inject, onMounted, onUnmounted, ref, watch } from "vue";

import RoutesModal from "../components/RoutesModal.vue";
import PreUninstallSection from "../components/PreUninstallSection.vue";
import ServicePanel from "../components/ServicePanel.vue";
import SettingsRow from "../components/SettingsRow.vue";
import PasswordField from "../components/PasswordField.vue";
import SettingsSectionAxis from "../components/SettingsSectionAxis.vue";
import { pushToast } from "../lib/toast";
import { APPEARANCE_KEY, type Appearance } from "../product/appearance";
import { WINDOW_CHROME_PORT_KEY } from "../product/window-chrome";
import { PRODUCT_RUNTIME_KEY, type ProductRuntime } from "../product/runtime";
import {
  CORE_CONFIG_FIELDS,
  CORE_CONFIG_GATEWAY_KEY,
  createCoreConfigGateway,
  credentialConfigFor,
  revealSavedPassword,
  isCoreConfigKey,
  isVpnServerPreset,
  normalizeCoreConfigValue,
  normalizeServerValue,
  VPN_SERVERS,
  type CoreConfigGateway,
  type CoreConfigKey,
} from "../product/core-config";
import {
  activeSection,
  drafts,
  feedback,
  items,
  loadedOnce,
  loadError,
  loading,
  original,
  saving,
} from "../product/settings-state";
import {
  CLOSE_PREFERENCE_LABELS,
  captureUiPreferenceChanges,
  editUiPreference,
  loadUiPreferences,
  uiPrefErrors,
  uiPrefSaving,
  uiPrefsDirty,
  uiPrefsDraft,
  updateUiPreferences,
  type UiPreferences,
  type UiPrefKey,
} from "../product/ui-prefs";

const props = defineProps<{
  gateway?: CoreConfigGateway;
  appearance?: Appearance;
  /** 自动保存防抖窗口（毫秒）。默认 1000；测试注入 0 以便同步驱动。 */
  autoSaveDelayMs?: number;
}>();

const injectedAppearance = inject(APPEARANCE_KEY, null);
const resolvedAppearance = props.appearance ?? injectedAppearance;
if (resolvedAppearance === null) throw new Error("设置页需要本地外观状态。");
const appearance: Appearance = resolvedAppearance;
const appearanceDraft = appearance.draft;
const chrome = inject(WINDOW_CHROME_PORT_KEY, null);

const injectedGateway = inject(CORE_CONFIG_GATEWAY_KEY, null);
const gateway = props.gateway ?? injectedGateway ?? createCoreConfigGateway();
const credentialConfig = credentialConfigFor(gateway);
const runtime = inject(PRODUCT_RUNTIME_KEY, null) as ProductRuntime | null;
// 状态来自模块级 store（内存持久化：切页重挂载不丢草稿/浏览位置，不重新读取覆盖）。

/** 分区标题由常显文字锚点与页内 h2 共用。 */
const sections = [
  { id: "connection", label: "连接" },
  { id: "startup", label: "应用" },
  { id: "appearance", label: "外观" },
  { id: "diagnostics", label: "通知" },
  { id: "experimental", label: "实验性" },
] as const;

/** 连接与网络分栏：core 配置中留在本栏的字段（user_agent / mtu 迁入实验性功能）。 */
const CONNECTION_KEYS: readonly CoreConfigKey[] = [
  "server",
  "username",
  "password",
  "remember_password",
  "routes",
  "auto_reconnect",
  "auto_reconnect_max_attempts",
  "auto_reconnect_backoff",
];

const accents = [
  { id: "azure", label: "蔚蓝" },
  { id: "violet", label: "紫罗兰" },
  { id: "jade", label: "青玉" },
  { id: "amber", label: "琥珀" },
] as const;

const knownFields = computed(() =>
  CORE_CONFIG_FIELDS.filter((field) => Object.prototype.hasOwnProperty.call(drafts.value, field.key)),
);

function fieldDraft(key: CoreConfigKey): string {
  return drafts.value[key] ?? "";
}

/** 渲染用字段：密码行仅在「记住密码」勾选时展示（未勾选无从编辑已保存密码）。 */
const visibleFields = computed(() =>
  knownFields.value.filter(
    (field) => field.key !== "password" || drafts.value["remember_password"] === "true",
  ),
);

  const connectionFields = computed(() =>
    visibleFields.value.filter(
      (field) =>
        (CONNECTION_KEYS as readonly string[]).includes(field.key) ||
        field.key === "password",
    ).filter(
      (field) =>
        field.key === "auto_reconnect" ||
        (field.key !== "auto_reconnect_max_attempts" &&
          field.key !== "auto_reconnect_backoff") ||
        drafts.value.auto_reconnect === "true",
    ),
  );

function experimentalDraft(key: "mtu" | "user_agent"): string {
  return fieldDraft(key);
}

function updateText(key: CoreConfigKey, event: Event): void {
  drafts.value[key] = (event.target as HTMLInputElement).value;
  feedback.value[key] = undefined;
  scheduleAutoSave();
}

function updateBoolean(key: CoreConfigKey, event: Event): void {
  drafts.value[key] = (event.target as HTMLInputElement).checked ? "true" : "false";
  feedback.value[key] = undefined;
  scheduleAutoSave();
}

/**
 * VPN 服务器下拉：预设地址或「自定义」。
 * 初始按草稿归一（命中预设选预设，否则自定义）；用户显式切换用 serverChoice 覆盖；
 * 手填命中预设时归一回到预设项（与旧 C++ applyServerChoice 一致）。
 */
function deriveServerChoice(): string {
  const value = normalizeServerValue(drafts.value.server ?? "");
  return isVpnServerPreset(value) ? value : "custom";
}

const serverChoice = ref(deriveServerChoice());

watch(
  () => drafts.value.server,
  () => {
    serverChoice.value = deriveServerChoice();
  },
);

/** 下拉切换：选预设直接写回草稿；选「自定义」保留当前值交由文本框续编辑。 */
function onServerChoiceChange(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  feedback.value.server = undefined;
  serverChoice.value = value;
  if (value !== "custom") {
    drafts.value.server = value;
    scheduleAutoSave();
  }
}

/** 密码框写入：与其它表单输入同等对待，进入自动保存的防抖窗口。 */
function updatePassword(value: string): void {
  drafts.value.password = value;
  feedback.value.password = undefined;
  scheduleAutoSave();
}

/** 自动重连次数输入框仅在自动重连开关开启时可设置。 */
function reconnectAttemptsDisabled(): boolean {
  return drafts.value.auto_reconnect !== "true";
}

/** 自动重连退避开关仅在自动重连开关开启时可设置（auto_reconnect 的从属选项）。 */
function reconnectBackoffDisabled(): boolean {
  return drafts.value.auto_reconnect !== "true";
}

/** 路由编辑模态开关（路由行只展示条目数 + 修改按钮，编辑在模态内完成）。 */
const routesModalOpen = ref(false);
const routesSaving = ref(false);
const routesError = ref("");

/** 当前 routes 草稿拆分为条目列表（逗号分隔）。 */
const routesList = computed(() =>
  (drafts.value.routes ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean),
);

const routesSummary = computed(() => `${routesList.value.length} 条路由`);

/**
 * 路由模态确认只编辑设置草稿；由自动保存统一提交。
 */
async function onRoutesSave(routes: string[]): Promise<void> {
  if (routesSaving.value) return;
  routesSaving.value = true;
  routesError.value = "";
  const value = routes.join(",");
  // 模态仅编辑主设置草稿；真正的 ConfigSet 与其他设置一并由自动保存分流提交。
  drafts.value.routes = value;
  feedback.value.routes = undefined;
  routesModalOpen.value = false;
  routesSaving.value = false;
  scheduleAutoSave();
}

function openRoutesModal(): void {
  routesError.value = "";
  routesModalOpen.value = true;
}

function closeRoutesModal(): void {
  if (routesSaving.value) return;
  routesModalOpen.value = false;
  routesError.value = "";
}

function isUiPrefSaving(key: UiPrefKey): boolean {
  return uiPrefSaving.value[key] === true;
}

function uiPrefError(key: UiPrefKey): string {
  return uiPrefErrors.value[key] ?? "";
}

async function saveUiPreference<K extends UiPrefKey>(key: K, value: UiPreferences[K]): Promise<void> {
  editUiPreference(key, value);
  scheduleAutoSave();
}

function retryUiPreference(_key?: UiPrefKey): void {
  void saveAll();
}

async function load(): Promise<void> {
  loading.value = true;
  loadError.value = null;
  feedback.value = {};
  try {
    const nextItems = await gateway.configGet();
    credentialConfig.apply(nextItems);
    const nextDrafts: Record<string, string> = {};
    for (const item of nextItems) if (isCoreConfigKey(item.key)) nextDrafts[item.key] = item.value;
    // 密码是 AES-GCM 密文 blob，core 不回显：始终以空起始，由用户显式输入新密码。
    nextDrafts["password"] = "";
    items.value = nextItems;
    drafts.value = nextDrafts;
    original.value = { ...nextDrafts };
    loadedOnce.value = true;
  } catch {
    items.value = [];
    drafts.value = {};
    original.value = {};
    loadError.value = "暂时无法读取核心配置。";
  } finally {
    loading.value = false;
    // 内容加载完成后高度变化，重新定位当前分区。
    requestAnimationFrame(updateActiveSectionFromScroll);
  }
}

/** core 配置是否存在未保存的修改。 */
const isCoreDirty = computed(() =>
  knownFields.value.some((field) => {
    const raw = drafts.value[field.key] ?? "";
    const loaded = original.value[field.key] ?? "";
    return raw !== loaded;
  }),
);

const isAppearanceDirty = appearance.motionDirty;

const isSettingsDirty = computed(() => isCoreDirty.value || uiPrefsDirty.value || isAppearanceDirty.value);

/** 设置自动保存的默认防抖窗口：最后一次变更后 1 秒提交。 */
const DEFAULT_AUTO_SAVE_DELAY_MS = 1000;
let autoSaveTimer: ReturnType<typeof setTimeout> | null = null;

function clearAutoSaveTimer(): void {
  if (autoSaveTimer !== null) {
    clearTimeout(autoSaveTimer);
    autoSaveTimer = null;
  }
}

/**
 * 设置变更后安排一次自动保存：**每次新变更都重置计时**，用户停手 1 秒才提交一次。
 *
 * 只由用户变更入口调用（表单输入、路由模态确认、外观动效、前端偏好），不由
 * 「保存完成后的草稿收口」触发——否则保存失败时会每 1 秒重试一次，形成报错循环。
 * 计时到点仍复查脏态：期间被其它入口保存掉（或用户改回原值）则不再重复提交。
 */
function scheduleAutoSave(): void {
  clearAutoSaveTimer();
  if (!isSettingsDirty.value) return;
  autoSaveTimer = setTimeout(runAutoSave, props.autoSaveDelayMs ?? DEFAULT_AUTO_SAVE_DELAY_MS);
}

/**
 * 兑现一次自动保存：已无脏项则跳过（改回原值、被其它入口抢先保存都落在这里）。
 * 有保存在途时**顺延重排**而不是丢弃——保存期间产生的新编辑必须还能落到下一次提交上；
 * 顺延只在保存真正进行中发生，保存结束即收敛，不构成失败重试循环。
 */
function runAutoSave(): void {
  clearAutoSaveTimer();
  if (!isSettingsDirty.value) return;
  if (saving.value || appearance.saving.value) {
    autoSaveTimer = setTimeout(runAutoSave, props.autoSaveDelayMs ?? DEFAULT_AUTO_SAVE_DELAY_MS);
    return;
  }
  void saveAll();
}

/** 离页时立即兑现待发的自动保存：计时尚未到点也不该丢掉用户刚改的值。 */
function flushPendingAutoSave(): void {
  if (autoSaveTimer === null) return;
  runAutoSave();
}

/** 主题/强调色即时生效并落盘（D5 裁决 §8.3：用户主动切换持久化）。 */
async function commitAppearanceTheme(theme: "system" | "light" | "dark"): Promise<void> {
  if (!(await appearance.commitTheme(theme))) {
    pushToast(appearance.saveError.value ?? "外观保存失败，请重试。", "error");
  }
}

async function commitAppearanceAccent(accent: "azure" | "violet" | "jade" | "amber"): Promise<void> {
  if (!(await appearance.commitAccent(accent))) {
    pushToast(appearance.saveError.value ?? "外观保存失败，请重试。", "error");
  }
}

/** 动效仍走草稿 + 自动保存；主题/强调色已在模板中直接走即时提交入口（D5）。 */
function setAppearanceMotion(value: "normal" | "reduced"): void {
  appearance.edit("motion", value);
  scheduleAutoSave();
}

/**
 * 所有设置只改草稿；变更停顿 1 秒后由 `scheduleAutoSave` 调用本函数，按 core / UI 偏好 /
 * 外观的归属分流应用。校验错误仍内联提示且不提交（脏态保留，下次变更再重试）。
 */
async function saveAll(): Promise<void> {
  if (saving.value || appearance.saving.value) return;

  // 分流一：core 配置脏键。
  const pending: { key: CoreConfigKey; value: string }[] = [];
  const pendingRaw = { ...drafts.value };
  for (const field of knownFields.value) {
    const raw = drafts.value[field.key] ?? "";
    if (raw === (original.value[field.key] ?? "")) continue;
    const normalized = normalizeCoreConfigValue(field.key, raw);
    if (!normalized.ok) {
      feedback.value[field.key] = normalized.message;
      return; // 校验错误内联；不提交
    }
    pending.push({ key: field.key, value: normalized.value });
  }

  const pendingUiPrefs = uiPrefsDirty.value;
  const pendingUiValues = captureUiPreferenceChanges();
  // 主题/强调色/模式已即时落盘，保存设置只提交动效键（D5；否则"保存设置"会因
  // 已生效的外观项常亮，且不得把内部强制展开的呈现模式写进偏好文件）。
  const pendingAppearanceValues: Partial<typeof appearanceDraft.value> = {
    motion: appearanceDraft.value.motion,
  };
  const pendingAppearance = isAppearanceDirty.value;
  if (pending.length === 0 && !pendingUiPrefs && !pendingAppearance) return;

  saving.value = true;
  const completed: string[] = [];
  let stage = "核心配置";
  let coreOk = true;
  try {
    if (pending.length > 0) {
      credentialConfig.markSaved();
      const saved = await credentialConfig.write(() => gateway.configSet(pending));
      if (!saved) {
        coreOk = false;
      }
    }
    if (!coreOk) {
      pushToast("保存失败。", "error");
      return;
    }
    // core 已确认落盘时立即收口其草稿；后续某个独立 UI 存储失败不能让下一次保存
    // 重复提交已经成功的核心配置。
    for (const item of pending) {
      const savedValue = item.key === "password" ? "" : item.value;
      if (drafts.value[item.key] === pendingRaw[item.key]) drafts.value[item.key] = savedValue;
      original.value[item.key] = savedValue;
      feedback.value[item.key] = undefined;
    }
    if (pending.length > 0) {
      completed.push("核心配置已保存");
      await credentialConfig.refresh();
    }
    if (pending.some((item) => item.key === "username" || item.key === "server")) {
      stage = "配置显示刷新";
      await runtime?.configurationChanged();
    }
    stage = "前端偏好";
    if (pendingUiPrefs) {
      if (!(await updateUiPreferences(undefined, pendingUiValues))) {
        const detail = Object.values(uiPrefErrors.value).filter(Boolean).join(" ");
        pushToast(completed.concat("前端偏好保存失败：" + detail + "；未完成草稿已保留，请重试。").join("；"), "error");
        return;
      }
      completed.push("前端偏好已保存");
    }
    stage = "外观";
    if (pendingAppearance && !(await appearance.saveDraft(chrome ? (mode) => chrome.setMode(mode) : undefined, pendingAppearanceValues))) {
      pushToast(completed.concat(appearance.saveError.value ?? "外观保存失败，请重试。").join("；"), "error");
      return;
    }
    pushToast("设置已保存。", "success");
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    pushToast(completed.concat(stage + "保存失败：" + detail + "；未完成草稿已保留，请重试。").join("；"), "error");
  } finally {
    saving.value = false;
  }
}

function selectSection(id: string): void {
  if (!sections.some((section) => section.id === id)) return;
  activeSection.value = id;
  const target = sectionElements.get(id) ?? document.getElementById(`settings-${id}`);
  // 锚点跳转期间的滚动同步抑制：smooth 滚动过程中检测线会先扫过中间分区，
  // 把 activeSection 临时改到「目标下方的中间分区」，反复打断轴小球的飞行方向。
  // 抑制窗口覆盖整个平滑滚动（滚动期间每次 scroll 续期、scrollend 即清、
  // 硬上限兜底），使飞行只发生一次、方向恒为从旧锚点到目标；滚动触发不受影响。
  armScrollSuppression(2000);
  target?.scrollIntoView({
    behavior: appearance.state.value.motion === "reduced" ? "auto" : "smooth",
    block: "start",
  });
}

/**
 * 滚动位置观察（替代窄横带相交观察策略，修复滑块抽动与最后分区卡位）：
 *   * 单调判定：检测线（滚动容器顶部 + SECTION_READ_OFFSET）之上「最靠后」的分区为当前分区，
 *     分区滑过检测线才切换一次，不再随相交比来回抖动；
 *   * 底部回退：滚动到底时强制激活最后分区，短内容分区（高级）也能命中。
 */
const SECTION_READ_OFFSET = 88;
const BOTTOM_EPSILON = 4;
const pageRoot = ref<HTMLElement | null>(null);
let scrollContainer: HTMLElement | null = null;
/** 锚点点击后的程序化平滑滚动抑制标记：置位期间滚动同步不更新 activeSection。 */
const suppressScrollTracking = ref(false);
let scrollSuppressTimer: ReturnType<typeof setTimeout> | null = null;

function clearScrollSuppressTimer(): void {
  if (scrollSuppressTimer !== null) {
    clearTimeout(scrollSuppressTimer);
    scrollSuppressTimer = null;
  }
}

/**
 * 武装抑制：置位并挂一个硬上限兜底定时器（scrollend 未触发时也能自愈，
 * 防止抑制泄漏导致滚动同步永久失效）。
 */
function armScrollSuppression(maxMs: number): void {
  suppressScrollTracking.value = true;
  clearScrollSuppressTimer();
  scrollSuppressTimer = setTimeout(() => {
    if (suppressScrollTracking.value) suppressScrollTracking.value = false;
  }, maxMs);
}

/** 释放抑制：scrollend 触发即清（顺带清兜底定时器）。 */
function releaseScrollTrackingSuppression(): void {
  suppressScrollTracking.value = false;
  clearScrollSuppressTimer();
}
/** 分区元素缓存：挂载时从组件根收集（不依赖全局 document 查询，便于独立挂载测试）。 */
const sectionElements = new Map<string, HTMLElement>();

function collectSectionElements(): void {
  sectionElements.clear();
  const root = pageRoot.value;
  if (!root) return;
  for (const section of sections) {
    const element = root.querySelector<HTMLElement>(`#settings-${section.id}`);
    if (element) sectionElements.set(section.id, element);
  }
}

/** 解析实际滚动容器：设置页两分区后，内容在 .settings-page__layout 内滚动；
 *  独立环境（测试/预览）回退到滚动祖先/文档根。 */
function resolveScrollContainer(): HTMLElement | null {
  const inner = pageRoot.value?.querySelector<HTMLElement>(".settings-page__layout");
  if (inner) return inner;
  const host = document.querySelector<HTMLElement>(".product-content--scrollable");
  if (host) return host;
  let node: HTMLElement | null = pageRoot.value?.parentElement ?? null;
  while (node) {
    const overflowY = window.getComputedStyle(node).overflowY;
    if (overflowY === "auto" || overflowY === "scroll") return node;
    node = node.parentElement;
  }
  return document.scrollingElement as HTMLElement | null;
}

function setActiveSection(id: string): void {
  if (sections.some((section) => section.id === id)) activeSection.value = id;
}

function updateActiveSectionFromScroll(): void {
  const container = scrollContainer;
  if (!container) return;
  // 锚点跳转的平滑滚动期间不更新 activeSection（见 selectSection 的抑制说明）。
  if (suppressScrollTracking.value) {
    // 平滑滚动仍在进行：续期兜底定时器（250ms 无新滚动事件即自清），
    // 使抑制窗口与动画实际时长一致，而非固定猜测值。
    clearScrollSuppressTimer();
    scrollSuppressTimer = setTimeout(() => {
      if (suppressScrollTracking.value) suppressScrollTracking.value = false;
    }, 250);
    return;
  }
  const { scrollTop, clientHeight, scrollHeight } = container;
  // 测量未就绪或内容不可滚动：不做判定，保持当前分区。
  if (clientHeight <= 0 || scrollHeight <= clientHeight) return;
  if (scrollTop + clientHeight >= scrollHeight - BOTTOM_EPSILON) {
    setActiveSection(sections[sections.length - 1].id);
    return;
  }
  const readLine = container.getBoundingClientRect().top + SECTION_READ_OFFSET;
  let current: string = sections[0].id;
  for (const section of sections) {
    const element = sectionElements.get(section.id);
    if (element && element.getBoundingClientRect().top <= readLine) current = section.id;
    else break;
  }
  setActiveSection(current);
}

function bindScrollTracking(): void {
  scrollContainer = resolveScrollContainer();
  if (!scrollContainer) return;
  scrollContainer.addEventListener("scroll", updateActiveSectionFromScroll, { passive: true });
  window.addEventListener("resize", updateActiveSectionFromScroll);
  // 锚点平滑滚动结束后释放抑制（scrollend 对程序化 smooth 滚动可靠；
  // 兜底定时器防 scrollend 未触发/被吞导致抑制泄漏）。
  scrollContainer.addEventListener("scrollend", releaseScrollTrackingSuppression);
  requestAnimationFrame(updateActiveSectionFromScroll);
}

function unbindScrollTracking(): void {
  if (!scrollContainer) return;
  scrollContainer.removeEventListener("scroll", updateActiveSectionFromScroll);
  scrollContainer.removeEventListener("scrollend", releaseScrollTrackingSuppression);
  window.removeEventListener("resize", updateActiveSectionFromScroll);
  clearScrollSuppressTimer();
  scrollContainer = null;
}

onMounted(() => {
  collectSectionElements();
  void loadUiPreferences();
  if (loadedOnce.value) {
    // 从其它页切回：不重新读取（保留草稿/浏览位置），恢复滚动到上次分区锚点。
    // 该恢复是程序化滚动——抑制滚动同步到恢复完成（scrollend 即清；短硬上限兜底），
    // 避免首帧锚点跳变打乱轴小球方向。
    armScrollSuppression(800);
    requestAnimationFrame(() => {
      const target = sectionElements.get(activeSection.value) ?? document.getElementById(`settings-${activeSection.value}`);
      target?.scrollIntoView({ block: "start" });
    });
  } else {
    void load();
  }
  bindScrollTracking();
});

onUnmounted(() => {
  flushPendingAutoSave();
  unbindScrollTracking();
});
</script>

<template>
  <section
    ref="pageRoot"
    class="settings-page"
    aria-labelledby="settings-title"
    :data-settings-dirty="isSettingsDirty"
  >
    <header class="settings-page__header">
      <div>
        <h1 id="settings-title">设置</h1>
      </div>
    </header>

    <div class="settings-page__layout" data-testid="settings-page-layout">
      <SettingsSectionAxis :sections="sections" :active-section="activeSection" @select="selectSection" />
      <div class="settings-page__content">
        <section id="settings-connection" class="settings-section" data-testid="settings-connection">
          <header class="settings-section__header">
            <div><h2>连接</h2></div>
          </header>

          <div v-if="loading" class="settings-state">正在读取核心配置…</div>
          <div v-else-if="loadError" class="settings-state settings-state--error">
            <span>{{ loadError }}</span><button type="button" data-testid="retry-settings" @click="load">重试</button>
          </div>
          <template v-else>
            <div v-if="connectionFields.length" class="settings-fields-grid">
              <SettingsRow
                v-for="field in connectionFields"
                :key="field.key"
                :class="{ 'settings-row--dense': true, 'settings-row--wide': field.kind === 'routes' }"
                :label="field.label"
                :description="field.description"
                :data-testid="`setting-${field.key}`"
              >
                <div class="config-control">
                  <template v-if="field.kind === 'routes'">
                    <span class="routes-summary" data-testid="routes-summary">{{ routesSummary }}</span>
                    <button
                      type="button"
                      class="routes-edit-button"
                      data-testid="routes-edit"
                      @click="openRoutesModal"
                    >
                      修改
                    </button>
                  </template>
                  <div v-else-if="field.kind === 'server'" class="server-choice">
                    <select
                      class="settings-server-preference"
                      :data-testid="`input-${field.key}`"
                      :value="serverChoice"
                      :aria-label="field.label"
                      @change="onServerChoiceChange($event)"
                    >
                      <option v-for="server in VPN_SERVERS" :key="server.value" :value="server.value">{{ server.value }}</option>
                      <option value="custom">自定义</option>
                    </select>
                    <input
                      v-if="serverChoice === 'custom'"
                      :data-testid="`input-${field.key}-custom`"
                      type="text"
                      autocomplete="off"
                      :value="drafts[field.key] ?? ''"
                      :aria-label="`${field.label}（自定义）`"
                      @input="updateText(field.key, $event)"
                    >
                  </div>
                  <input
                    v-else-if="field.kind === 'number'"
                    :data-testid="`input-${field.key}`"
                    type="number"
                    min="0"
                    step="1"
                    autocomplete="off"
                    :value="drafts[field.key] ?? ''"
                    :aria-label="field.label"
                    :disabled="reconnectAttemptsDisabled()"
                    @input="updateText(field.key, $event)"
                  >
                  <PasswordField
                    v-else-if="field.kind === 'password'"
                    :model-value="drafts.password ?? ''"
                    data-testid="input-password"
                    :stored="credentialConfig.state.value.stored && drafts.username === credentialConfig.state.value.username && drafts.server === credentialConfig.state.value.server"
                    :identity="`${drafts.username}|${drafts.server}`"
                    :reveal-stored="() => revealSavedPassword(gateway, credentialConfig.state.value.username, credentialConfig.state.value.server)"
                    class="settings-password"
                    @update:model-value="updatePassword"
                  />
                  <input
                    v-else-if="field.kind !== 'boolean'"
                    :data-testid="`input-${field.key}`"
                    type="text"
                    autocomplete="off"
                    :value="drafts[field.key] ?? ''"
                    :aria-label="field.label"
                    @input="updateText(field.key, $event)"
                  >
                  <label v-else-if="field.kind === 'boolean'" class="boolean-control">
                    <input
                      :data-testid="`input-${field.key}`"
                      type="checkbox"
                      :checked="drafts[field.key] === 'true'"
                      :aria-label="field.label"
                      :disabled="field.key === 'auto_reconnect_backoff' ? reconnectBackoffDisabled() : undefined"
                      @change="updateBoolean(field.key, $event)"
                    >
                    <span>{{ drafts[field.key] === "true" ? "开启" : "关闭" }}</span>
                  </label>
                  <span v-if="feedback[field.key]" :data-testid="`feedback-${field.key}`" class="config-feedback">{{ feedback[field.key] }}</span>
                </div>
              </SettingsRow>
            </div>
            <p v-else class="settings-state">暂无</p>
          </template>

        </section>

        <section id="settings-service" class="settings-section" data-testid="settings-service" aria-labelledby="settings-service-title">
          <header class="settings-section__header"><div><h2 id="settings-service-title">服务</h2></div></header>
          <ServicePanel
            v-if="runtime !== null"
            :service="runtime.state.value.service"
            :auto-install="false"
            :busy="false"
            :show-connect-options="false"
            data-testid="settings-service-control"
          />
          <p v-else class="settings-state">运行时不可用，无法管理服务。</p>
        </section>

        <section id="settings-startup" class="settings-section" data-testid="settings-startup">
           <header class="settings-section__header"><div><h2>应用</h2></div></header>
           <div class="settings-fields-grid settings-fields-grid--static">
            <SettingsRow class="settings-row--dense" label="开机自动运行" description="登录 macOS 后自动启动 EXV。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-launch_at_login" type="checkbox" :checked="uiPrefsDraft.launch_at_login" :disabled="isUiPrefSaving('launch_at_login')" aria-label="开机自动运行" @change="saveUiPreference('launch_at_login', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.launch_at_login ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('launch_at_login')" class="preference-error" data-testid="ui-pref-error-launch_at_login" role="alert"><span>{{ uiPrefError('launch_at_login') }}</span><button type="button" data-testid="ui-pref-retry-launch_at_login" @click="retryUiPreference('launch_at_login')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="静默启动" description="启动时不弹出主窗口，仅托盘驻留，可从托盘唤出。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-silent_startup" type="checkbox" :checked="uiPrefsDraft.silent_startup" :disabled="isUiPrefSaving('silent_startup')" aria-label="静默启动" @change="saveUiPreference('silent_startup', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.silent_startup ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('silent_startup')" class="preference-error" data-testid="ui-pref-error-silent_startup" role="alert"><span>{{ uiPrefError('silent_startup') }}</span><button type="button" data-testid="ui-pref-retry-silent_startup" @click="retryUiPreference('silent_startup')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="连接后最小化到托盘" description="连接成功后自动把主窗口最小化到托盘。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-minimize_to_tray_on_connect" type="checkbox" :checked="uiPrefsDraft.minimize_to_tray_on_connect" :disabled="isUiPrefSaving('minimize_to_tray_on_connect')" aria-label="连接后最小化到托盘" @change="saveUiPreference('minimize_to_tray_on_connect', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.minimize_to_tray_on_connect ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('minimize_to_tray_on_connect')" class="preference-error" data-testid="ui-pref-error-minimize_to_tray_on_connect" role="alert"><span>{{ uiPrefError('minimize_to_tray_on_connect') }}</span><button type="button" data-testid="ui-pref-retry-minimize_to_tray_on_connect" @click="retryUiPreference('minimize_to_tray_on_connect')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="启动时自动连接" description="应用启动且空闲时自动发起连接。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-auto_connect_on_launch" type="checkbox" :checked="uiPrefsDraft.auto_connect_on_launch" :disabled="isUiPrefSaving('auto_connect_on_launch')" aria-label="启动时自动连接" @change="saveUiPreference('auto_connect_on_launch', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.auto_connect_on_launch ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('auto_connect_on_launch')" class="preference-error" data-testid="ui-pref-error-auto_connect_on_launch" role="alert"><span>{{ uiPrefError('auto_connect_on_launch') }}</span><button type="button" data-testid="ui-pref-retry-auto_connect_on_launch" @click="retryUiPreference('auto_connect_on_launch')">重试</button></div></div>
            </SettingsRow>

            <SettingsRow class="settings-row--dense" label="关闭按钮行为" description="智能判断：连接期间保持后台运行，空闲时退出。也可选择始终后台运行或直接退出。">
              <div class="preference-control"><select class="settings-close-preference" data-testid="ui-pref-close_preference" :value="uiPrefsDraft.close_preference" :disabled="isUiPrefSaving('close_preference')" aria-label="关闭按钮行为" @change="saveUiPreference('close_preference', ($event.target as HTMLSelectElement).value as 'smart' | 'tray' | 'quit')">
                <option value="smart">{{ CLOSE_PREFERENCE_LABELS.smart }}</option><option value="tray">{{ CLOSE_PREFERENCE_LABELS.tray }}</option><option value="quit">{{ CLOSE_PREFERENCE_LABELS.quit }}</option>
              </select><div v-if="uiPrefError('close_preference')" class="preference-error" data-testid="ui-pref-error-close_preference" role="alert"><span>{{ uiPrefError('close_preference') }}</span><button type="button" data-testid="ui-pref-retry-close_preference" @click="retryUiPreference('close_preference')">重试</button></div></div>
            </SettingsRow>
           </div>
         </section>

        <section id="settings-appearance" class="settings-section" data-testid="settings-appearance">
          <header class="settings-section__header"><div><h2>外观</h2></div></header>
          <SettingsRow class="settings-row--dense" label="主题" description="跟随系统或固定为浅色、深色。">
            <select data-testid="appearance-theme" :value="appearanceDraft.theme" aria-label="主题" @change="commitAppearanceTheme(($event.target as HTMLSelectElement).value as 'system' | 'light' | 'dark')">
              <option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option>
            </select>
          </SettingsRow>
          <SettingsRow class="settings-row--dense" label="强调色" description="用于主操作与当前状态，不改变语义颜色。">
            <div class="accent-options" role="group" aria-label="强调色">
              <button v-for="accent in accents" :key="accent.id" :data-testid="`appearance-accent-${accent.id}`" class="accent-option" :class="{ 'accent-option--active': appearanceDraft.accent === accent.id }" type="button" :aria-pressed="appearanceDraft.accent === accent.id" @click="commitAppearanceAccent(accent.id)">{{ accent.label }}</button>
            </div>
          </SettingsRow>
          <SettingsRow class="settings-row--dense" label="动效" description="减少动效以静态场景图显示连接状态，不加载三维组件。">
            <select data-testid="appearance-motion" :value="appearanceDraft.motion" aria-label="动效" @change="setAppearanceMotion(($event.target as HTMLSelectElement).value as 'normal' | 'reduced')">
              <option value="normal">正常</option><option value="reduced">减少动效</option>
            </select>
          </SettingsRow>
        </section>

        <section id="settings-diagnostics" class="settings-section" data-testid="settings-diagnostics">
          <header class="settings-section__header"><div><h2>通知</h2></div></header>
          <div class="settings-fields-grid settings-fields-grid--static">
            <SettingsRow class="settings-row--dense" label="建立连接时发送通知" description="连接成功建立后显示系统通知弹窗。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-connect_notify" type="checkbox" :checked="uiPrefsDraft.connect_notify" :disabled="isUiPrefSaving('connect_notify')" aria-label="建立连接时发送通知" @change="saveUiPreference('connect_notify', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.connect_notify ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('connect_notify')" class="preference-error" data-testid="ui-pref-error-connect_notify" role="alert"><span>{{ uiPrefError('connect_notify') }}</span><button type="button" data-testid="ui-pref-retry-connect_notify" @click="retryUiPreference('connect_notify')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="断开连接时发送通知" description="连接断开后显示系统通知弹窗。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-disconnect_notify" type="checkbox" :checked="uiPrefsDraft.disconnect_notify" :disabled="isUiPrefSaving('disconnect_notify')" aria-label="断开连接时发送通知" @change="saveUiPreference('disconnect_notify', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.disconnect_notify ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('disconnect_notify')" class="preference-error" data-testid="ui-pref-error-disconnect_notify" role="alert"><span>{{ uiPrefError('disconnect_notify') }}</span><button type="button" data-testid="ui-pref-retry-disconnect_notify" @click="retryUiPreference('disconnect_notify')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="触发重连时发送通知" description="触发重连时显示系统通知弹窗；仅在开启自动重连后真正生效。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-reconnect_notify" type="checkbox" :checked="uiPrefsDraft.reconnect_notify" :disabled="isUiPrefSaving('reconnect_notify')" aria-label="触发重连时发送通知" @change="saveUiPreference('reconnect_notify', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.reconnect_notify ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('reconnect_notify')" class="preference-error" data-testid="ui-pref-error-reconnect_notify" role="alert"><span>{{ uiPrefError('reconnect_notify') }}</span><button type="button" data-testid="ui-pref-retry-reconnect_notify" @click="retryUiPreference('reconnect_notify')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="窗口在前台时不发送通知" description="主窗口可见时无需发送连接类通知；优先级最高。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-suppress_notify_when_foreground" type="checkbox" :checked="uiPrefsDraft.suppress_notify_when_foreground" :disabled="isUiPrefSaving('suppress_notify_when_foreground')" aria-label="窗口在前台时不发送通知" @change="saveUiPreference('suppress_notify_when_foreground', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.suppress_notify_when_foreground ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('suppress_notify_when_foreground')" class="preference-error" data-testid="ui-pref-error-suppress_notify_when_foreground" role="alert"><span>{{ uiPrefError('suppress_notify_when_foreground') }}</span><button type="button" data-testid="ui-pref-retry-suppress_notify_when_foreground" @click="retryUiPreference('suppress_notify_when_foreground')">重试</button></div></div>
            </SettingsRow>
          </div>
        </section>

        <section id="settings-experimental" class="settings-section settings-experimental" data-testid="settings-experimental">
          <header class="settings-section__header"><div><h2>实验性功能</h2><p>以下选项可能影响连接兼容性，建议保持默认；连接时延显示在 macOS 上仍有限制。</p></div></header>
          <div class="settings-fields-grid settings-fields-grid--static">
            <SettingsRow class="settings-row--dense" label="MTU" description="网络接口 MTU。默认值已适配校园网；非专业场景建议保持默认。">
              <input
                data-testid="input-mtu"
                inputmode="numeric"
                type="text"
                autocomplete="off"
                :value="experimentalDraft('mtu')"
                aria-label="MTU"
                @input="updateText('mtu', $event)"
              >
              <span v-if="feedback['mtu']" class="config-feedback">{{ feedback['mtu'] }}</span>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="User-Agent" description="连接请求的客户端标识。修改可能导致服务端拒绝连接；建议保持默认。">
              <input
                data-testid="input-user_agent"
                type="text"
                autocomplete="off"
                :value="experimentalDraft('user_agent')"
                aria-label="User-Agent"
                @input="updateText('user_agent', $event)"
              >
              <span v-if="feedback['user_agent']" class="config-feedback">{{ feedback['user_agent'] }}</span>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="显示连接时延" description="显示收到的连接时延采样。macOS 暂不支持手动刷新；无采样时显示横杠，默认关闭。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-show_latency" type="checkbox" :checked="uiPrefsDraft.show_latency" :disabled="isUiPrefSaving('show_latency')" aria-label="显示连接时延" @change="saveUiPreference('show_latency', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.show_latency ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('show_latency')" class="preference-error" data-testid="ui-pref-error-show_latency" role="alert"><span>{{ uiPrefError('show_latency') }}</span><button type="button" data-testid="ui-pref-retry-show_latency" @click="retryUiPreference('show_latency')">重试</button></div></div>
            </SettingsRow>
          </div>

          <!-- 卸载并入实验性功能分区：单侧裁决，卸载属于「实验性」范畴而非独立一节。 -->
          <PreUninstallSection />
        </section>

      </div>

    </div>

    <RoutesModal
      :routes="routesList"
      :open="routesModalOpen"
      :busy="routesSaving"
      :error-message="routesError"
      @save="onRoutesSave"
      @close="closeRoutesModal"
    />
  </section>
</template>

<style scoped>
.settings-page {
  min-width: 0;
  /* 两分区骨架：banner 固定顶部（flex:none），设置项在下方独立滚动容器内滚动。
     header 不随内容滚走、内容不从 header 旁穿帮。 */
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  padding-top: var(--product-page-top-gap);
  --settings-header-height: 52px;
  --settings-select-width: 160px;
  /* 壳层 .product-content 已取消上下内边距；banner 直接落在内容顶部，不再被
     外层留白推离或被 overflow 裁剪。 */
  margin-top: 0;
}
.settings-page__header {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  height: var(--settings-header-height);
  margin: 0;
  padding: 0;
  /* 无圆角无缝隙：顶部完整横条，不设 border-bottom，与下方设置项容器自然衔接。 */
  background: var(--surface-canvas);
}
.settings-page__header h1 { margin-bottom: 0; font-size: clamp(22px, 2.2vw, 30px); letter-spacing: -0.02em; }
.settings-section__header p { margin-bottom: 0; color: var(--text-secondary); }
.settings-page__layout {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-width: none;
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  grid-template-rows: max-content max-content;
  align-content: start;
  gap: 0;
  align-items: start;
  padding: 0 0 var(--space-6);
}
.settings-page__layout::-webkit-scrollbar { display: none; }
.settings-page__content {
  display: grid;
  grid-column: 1;
  grid-row: 2;
  gap: 0;
  min-width: 0;
  width: 100%;
  padding-bottom: var(--space-6);
}
.settings-section { scroll-margin-top: calc(51px + var(--space-4)); padding: var(--space-4) 0 var(--space-6); }
.settings-service-section { scroll-margin-top: calc(51px + var(--space-4)); min-width: 0; padding: var(--space-4) 0 var(--space-6); }
.settings-service-section__unavailable { margin: 0; padding: var(--space-3) 0; }
/* 金色框直接画在原容器最外围，不引入额外间距：
   * 不再用 margin-block —— 该分区与相邻分区的垂直间距回到与普通分区一致；
   * 水平改为向外扩 16px、同时保留 16px 内边距 —— 内容仍落在与其他分区相同的 x 上
     （零位移），框则画到页面内容列的最外沿；壳层左右各留 24px，不会溢出。 */
.settings-experimental {
  margin-inline: calc(var(--space-4) * -1);
  padding: var(--space-4) var(--space-4) var(--space-6);
  border: 1px solid #d29922;
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, #d29922 6%, var(--surface-panel));
}
.settings-experimental .settings-section__header h2 { color: var(--text-primary); }
.settings-section__header { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-3); margin-bottom: var(--space-2); padding-bottom: var(--space-2); }
.settings-section__header h2 { margin: 0 0 2px; font-size: 16px; }
.settings-section__header p { font-size: 12px; }
.settings-section__lead { padding: var(--space-1) 0 0; }
.settings-fields-grid { display: grid; grid-template-columns: minmax(0, 1fr); }
.settings-fields-grid .settings-row--wide { grid-column: 1 / -1; }
.settings-state { margin: 0; padding: var(--space-3) 0; color: var(--text-secondary); font-size: 13px; }
.settings-state--error { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); color: var(--state-danger); }
.accent-options, .config-control { display: flex; min-width: 0; align-items: center; justify-content: flex-end; gap: 6px; }
.accent-options { flex-wrap: wrap; }
.accent-option { min-width: 38px; min-height: 32px; padding: 5px 8px; color: var(--text-secondary); font-size: 12px; }
.accent-option--active { border-color: var(--accent); background: var(--accent-subtle); color: var(--text-primary); }
.config-control > input { flex: 1 1 0; min-width: 0; width: auto; }
.settings-password { width: 172px; max-width: 100%; }
.settings-page select { width: var(--settings-select-width); min-width: 0; flex: 0 0 var(--settings-select-width); }
.settings-page select.settings-close-preference,
.settings-page select.settings-server-preference { width: 208px; flex: 0 0 208px; }
.server-choice { display: flex; min-width: 0; align-items: center; justify-content: flex-end; gap: 6px; }
.server-choice select { min-height: 34px; }
.server-choice input[type="text"] { flex: 1 1 0; min-width: 0; width: auto; }
.boolean-control { display: inline-flex; align-items: center; gap: 7px; min-height: 34px; color: var(--text-primary); white-space: nowrap; }
.boolean-control input { width: 18px; height: 18px; accent-color: var(--accent); }
.config-feedback { flex: 0 0 auto; color: var(--state-danger); font-size: 11px; }
.preference-control { display: grid; min-width: 0; justify-items: end; gap: 4px; }
.preference-error { display: flex; align-items: center; justify-content: flex-end; gap: var(--space-2); color: var(--state-danger); font-size: 11px; line-height: 1.35; }
.preference-error button { min-height: 24px; padding: 0 var(--space-2); border-color: var(--state-danger); background: transparent; color: var(--state-danger); font-size: 11px; }
.routes-summary { flex: none; color: var(--text-secondary); white-space: nowrap; font-size: 13px; }
.routes-edit-button { flex: none; min-height: 32px; padding: 4px 14px; border-color: var(--accent); background: var(--accent); color: var(--accent-on); font-weight: 600; cursor: pointer; }
.routes-edit-button:hover { border-color: var(--accent-strong); background: var(--accent-strong); }
.settings-readonly { color: var(--text-secondary); white-space: nowrap; }

@media (max-width: 1000px) {
  .settings-fields-grid { grid-template-columns: 1fr; }
  .settings-fields-grid .settings-row--wide { grid-column: auto; }
  .settings-section__header { align-items: flex-start; }
  .server-choice { width: 100%; flex-direction: column; align-items: stretch; }
  .settings-page .server-choice select.settings-server-preference,
  .server-choice input[type="text"] { width: 100%; flex-basis: auto; }
}

@media (max-width: 480px) {
  .settings-section__header { display: block; }
  .settings-section__count { display: block; margin-top: 4px; }
}

.motion-reduced .settings-page *, [data-motion="reduced"] .settings-page * { scroll-behavior: auto; transition: none !important; animation: none !important; }
</style>
