<script setup lang="ts">
import { computed, inject, onMounted, onUnmounted, ref, watch } from "vue";

import TitlebarThemeModeControl from "../components/TitlebarThemeModeControl.vue";
import RoutesModal from "../components/RoutesModal.vue";
import ServicePanel from "../components/ServicePanel.vue";
import SettingsRow from "../components/SettingsRow.vue";
import CredentialRequiredModal from "../components/CredentialRequiredModal.vue";
import SettingsSectionAxis from "../components/SettingsSectionAxis.vue";
import {
  APPEARANCE_FIELD_COPY,
  APPLICATION_FIELD_COPY,
  EXPERIMENTAL_FIELD_COPY,
  EXPERIMENTAL_SECTION_COPY,
  SHARED_CORE_FIELD_COPY,
} from "../../../../../../../common/frontend/settings-fields";
import { clearedCredentialConfig } from "../product/minimal-credentials";
import { pushToast } from "../lib/toast";
import { configuredRoutes } from "../lib/route-rules";
import { handleCoreWakeFailure } from "../product/core-crash";
import { APPEARANCE_KEY, type Appearance } from "../product/appearance";
import { WINDOW_CHROME_PORT_KEY } from "../product/window-chrome";
import { PRODUCT_RUNTIME_KEY, type ProductRuntime } from "../product/runtime";
import {
  CONNECTION_MODE_OPTIONS,
  CORE_CONFIG_FIELDS,
  CORE_CONFIG_GATEWAY_KEY,
  DEFAULT_CONNECTION_MODE,
  createCoreConfigGateway,
  credentialConfigFor,
  connectionModeFromConfig,
  protocolFromConfig,
  isCoreConfigKey,
  isVpnServerPreset,
  defaultServerFor,
  normalizeCoreConfigValue,
  sanitizeCustomServerInput,
  normalizeServerValue,
  vpnServersFor,
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
  autostartStateUnavailable,
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

/** 六个内容宽度分区，选中后只显示对应面板（实验性分区文案双端共享）。 */
const sections = [
  { id: "connection", label: "连接" },
  { id: "service", label: "服务" },
  { id: "startup", label: "应用" },
  { id: "appearance", label: "外观" },
  { id: "diagnostics", label: "通知" },
  { id: "experimental", label: EXPERIMENTAL_SECTION_COPY.label },
] as const;

/** 连接与网络分栏；两侧协议专属服务器只保存在草稿中，当前活动地址通过 `server` 编辑。 */
const CONNECTION_KEYS: readonly CoreConfigKey[] = [
  "server",
  "username",
  "password",
  "remember_password",
  "connection_mode",
  "routes",
  "campus_dns_domains",
  "atrust_dns_enabled",
  "auto_reconnect",
  "auto_reconnect_max_attempts",
  "auto_reconnect_backoff",
  "reconnect_on_oneshot_engine_crash",
];

const accents = [
  { id: "ecnu", label: "品牌红", color: "#a92545" },
  { id: "azure", label: "蔚蓝", color: "#328fe5" },
  { id: "violet", label: "紫罗兰", color: "#9274dc" },
  { id: "jade", label: "青玉", color: "#29a78f" },
  { id: "amber", label: "琥珀", color: "#c79934" },
] as const;

const knownFields = computed(() =>
  CORE_CONFIG_FIELDS.filter((field) => Object.prototype.hasOwnProperty.call(drafts.value, field.key)),
);

function fieldDraft(key: CoreConfigKey): string {
  return drafts.value[key] ?? "";
}

/** 渲染用字段：密码不设内联输入（Area F：设置页只显示凭据状态行 + 模态修改）。 */
const visibleFields = computed(() =>
  knownFields.value.filter((field) => field.key !== "password" && field.key !== "remember_password"),
);

  const connectionFields = computed(() =>
    visibleFields.value.filter(
      (field) =>
        (CONNECTION_KEYS as readonly string[]).includes(field.key) ||
        field.key === "password",
    )
      .filter((field) => field.key !== "connection_mode" || protocol.value === "cisco")
      .filter((field) => visibleConnectionField(field.key)),
  );

/**
 * T4 新设置项的可见性门控（计划 §2.8）：
 * - `reconnect_on_oneshot_engine_crash`：仅**未安装服务**且主开关（auto_reconnect）
 *   开启时展示——服务形态的 engine 崩溃由 SCM 自恢复，与一次性提权无关；
 * - 既有 `auto_reconnect_max_attempts` / `auto_reconnect_backoff` 维持主开关门控。
 * 服务安装态取运行时快照（`ProductService.status`）；运行时不可用或未知时
 * 保守隐藏 oneshot 项（未知状态不展示 UAC 相关开关）。
 */
function visibleConnectionField(key: CoreConfigKey): boolean {
  const masterOn = drafts.value.auto_reconnect === "true";
  if (key === "auto_reconnect") return true;
  const slaveOfMaster = [
    "auto_reconnect_max_attempts",
    "auto_reconnect_backoff",
    "reconnect_on_oneshot_engine_crash",
  ];
  if (slaveOfMaster.includes(key) && !masterOn) return false;
  if (key === "reconnect_on_oneshot_engine_crash") {
    return runtime?.state.value.service.status.kind === "not_installed";
  }
  return true;
}

/** 主连接信息、独立流量控制卡、其余连接偏好按用户操作顺序呈现。 */
const connectionGroups = computed(() => {
  const mainKeys: readonly string[] = ["protocol", "atrust_auth_mode", "server", "atrust_server", "username"];
  const trafficKeys: readonly string[] = ["routes", "atrust_dns_enabled", "campus_dns_domains"];
  const fields = connectionFields.value;
  return [
    { id: "main", fields: fields.filter(field => mainKeys.includes(field.key)).sort((a, b) => mainKeys.indexOf(a.key) - mainKeys.indexOf(b.key)) },
    { id: "traffic", fields: trafficKeys.flatMap(key => fields.filter(field => field.key === key && (key !== "atrust_dns_enabled" || protocol.value === "atrust"))) },
    { id: "preferences", fields: fields.filter(field => !mainKeys.includes(field.key) && !trafficKeys.includes(field.key)) },
  ].filter(group => group.fields.length > 0);
});

const serverInputComposing = ref(false);
/** 编辑框呈现域名；历史配置中的协议外壳保留在草稿中，未编辑时不会重写。 */
function customServerDisplay(key: CoreConfigKey): string {
  return (drafts.value[key] ?? "").replace(/^https?:\/\//i, "").replace(/\/+$/, "");
}
function startServerComposition(): void {
  serverInputComposing.value = true;
  clearAutoSaveTimer();
}
function endServerComposition(key: CoreConfigKey, event: Event): void {
  serverInputComposing.value = false;
  updateCustomServer(key, event);
}
function updateCustomServer(key: CoreConfigKey, event: Event): void {
  if (serverInputComposing.value || (event as InputEvent).isComposing) return;
  const input = event.target as HTMLInputElement;
  const raw = input.value;
  const start = input.selectionStart;
  const end = input.selectionEnd;
  const value = sanitizeCustomServerInput(raw);
  input.value = value;
  if (start !== null && end !== null) {
    input.setSelectionRange(sanitizeCustomServerInput(raw.slice(0, start)).length, sanitizeCustomServerInput(raw.slice(0, end)).length);
  }
  updateText(key, event);
}

function experimentalDraft(key: "mtu" | "user_agent"): string {
  return fieldDraft(key);
}

function updateText(key: CoreConfigKey, event: Event): void {
  drafts.value[key] = (event.target as HTMLInputElement).value;
  if (key === "server") drafts.value[protocol.value === "atrust" ? "atrust_server" : "cisco_server"] = drafts.value.server;
  feedback.value[key] = undefined;
  scheduleAutoSave();
}

function updateBoolean(key: CoreConfigKey, event: Event): void {
  drafts.value[key] = (event.target as HTMLInputElement).checked ? "true" : "false";
  feedback.value[key] = undefined;
  scheduleAutoSave();
}

/** 当前草稿协议；缺失或未知取值保守按 Cisco 显示（与后端读路径一致），写入仍走严格校验。 */
const protocol = computed(() => protocolFromConfig(drafts.value.protocol));

/** 服务器预设只展示当前协议可用的地址（Cisco 三地与 aTrust 门户互不通用）。 */
const serverPresets = computed(() => vpnServersFor(protocol.value));

/**
 * VPN 服务器下拉：当前协议的预设地址或「自定义」。
 * 初始按草稿归一（命中预设选预设，否则自定义）；用户显式切换用 serverChoice 覆盖；
 * 手填命中预设时归一回到预设项（与旧 C++ applyServerChoice 一致）。
 */
function deriveServerChoice(): string {
  const value = normalizeServerValue(drafts.value.server ?? "", protocol.value);
  return isVpnServerPreset(value, protocol.value) ? value : "custom";
}

const serverChoice = ref(deriveServerChoice());

watch(
  () => drafts.value.server,
  () => {
    serverChoice.value = deriveServerChoice();
  },
);

/** 切换协议时将活动服务器保存到对应侧，并恢复另一侧已保存的服务器。 */
function onProtocolChange(next: "cisco" | "atrust"): void {
  if (next === protocol.value) return;
  const previous = protocol.value;
  drafts.value[previous === "atrust" ? "atrust_server" : "cisco_server"] = drafts.value.server ?? "";
  drafts.value.protocol = next;
  const key = next === "atrust" ? "atrust_server" : "cisco_server";
  drafts.value.server = drafts.value[key] || defaultServerFor(next);
  drafts.value[key] = drafts.value.server;
  feedback.value.protocol = undefined;
  feedback.value.server = undefined;
  scheduleAutoSave();
}

function setAtrustAuthMode(mode: "browser" | "embedded" | "headless"): void {
  drafts.value.atrust_auth_mode = mode;
  feedback.value.atrust_auth_mode = undefined;
  scheduleAutoSave();
}

/**
 * §7.10-3 渠道边界（S7 无头裁定后）：开启「尝试自动登录」时仅「浏览器」档不可用——
 * 系统浏览器不承担自动登录，连接时按网页内嵌解析；headless 保持无头、embedded
 * 保持内嵌（壳侧 resolved_atrust_auth_mode）。此处如实呈现（浏览器档置灰 + 说明），
 * 不改变配置存储语义。
 */

/** 下拉切换：选预设直接写回草稿；选「自定义」保留当前值交由文本框续编辑。 */
function onServerChoiceChange(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  feedback.value.server = undefined;
  serverChoice.value = value;
  if (value !== "custom") {
    drafts.value.server = value;
    drafts.value[protocol.value === "atrust" ? "atrust_server" : "cisco_server"] = value;
    scheduleAutoSave();
  }
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
const routesList = computed(() => configuredRoutes(drafts.value.routes));

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
  if (_key === "launch_at_login" && autostartStateUnavailable.value) {
    void loadUiPreferences();
    return;
  }
  void saveAll();
}

/**
 * 凭据状态行的密码修改模态（Area F：设置页不再有内联密码框）。
 *
 * M2：按当前用户名存在性选模态形态——有账户 → 仅密码（账户只读投影）；
 * 账户为空 → 合并输入（否则只读投影下无法补账户，提交校验死锁）。
 * 确定后单独 `config_set`（密码不进自动保存草稿 drafts）；设置保存语义
 * 为新密码自动保存；留空保留原密码，显式清除随本次提交生效。S5a
 * （2026-10-26）：换账户留空同样保留原密文——凭据对按 core 侧 owner
 * 匹配门判定有效性（失配暂停自动重连，改回原账户即恢复）。
 */
const credentialModalOpen = ref(false);
const credentialModalBusy = ref(false);
const credentialModalError = ref("");
const credentialModalResetKey = ref(0);
const credentialModalMode = computed(() => "credentials" as const);
const reconnectEligible = computed(() => !!credentialConfig.state.value.username.trim() && credentialConfig.state.value.stored);

function openPasswordModal(): void {
  credentialModalError.value = "";
  credentialModalResetKey.value += 1;
  credentialModalOpen.value = true;
}

function closePasswordModal(): void {
  if (credentialModalBusy.value) return;
  credentialModalOpen.value = false;
  credentialModalResetKey.value += 1;
  credentialModalError.value = "";
}

async function submitPasswordModal(credentials: { username: string; password: string; persist: boolean; passwordAction?: "keep" | "replace" | "clear" }): Promise<void> {
  if (credentialModalBusy.value) return;
  const action = credentials.passwordAction ?? (credentials.password ? "replace" : "keep");
  const usernameChanged = credentials.username.trim() !== credentialConfig.state.value.username.trim();
  const updates: { key: CoreConfigKey; value: string }[] = [];
  if (usernameChanged) updates.push({ key: "username", value: credentials.username.trim() });
  if (action === "clear") updates.push(...clearedCredentialConfig());
  else {
    if (action === "replace") updates.push({ key: "password", value: credentials.password });
    if (action === "replace") updates.push({ key: "remember_password", value: "true" });
  }
  if (updates.length === 0) { closePasswordModal(); return; }
  credentialModalBusy.value = true;
  credentialModalError.value = "";
  try {
    credentialConfig.markSaved();
    const saved = await credentialConfig.write(() => gateway.configSet(updates));
    if (!saved) throw new Error("Core 未确认写入");
    for (const item of updates) {
      const value = item.key === "password" ? "" : item.value;
      drafts.value[item.key] = value;
      original.value[item.key] = value;
    }
    credentialModalResetKey.value += 1;
    credentialModalOpen.value = false;
    await credentialConfig.refresh();
    if (usernameChanged) await runtime?.configurationChanged();
    // S5a：按动作如实区分——keep（只发 username）密码保持原密文；replace 更新
    // 密文；clear 物理清除。不再使用"账户与密码设置已保存。"半谎言文案。
    pushToast(
      action === "clear" ? "已清除保存的密码，自动重连已关闭。"
        : action === "replace" ? "账户与密码已更新。"
        : "账户已更新，密码保持不变。",
      "success",
    );
  } catch (error) {
    if (handleCoreWakeFailure(error)) return;
    credentialModalError.value = "密码设置保存失败，请重试。";
  } finally {
    credentialModalBusy.value = false;
  }
}

async function load(): Promise<void> {
  loading.value = true;
  loadError.value = null;
  feedback.value = {};
  try {
    const nextItems = await gateway.configGet();
    credentialConfig.apply(nextItems);
    const nextDrafts: Record<string, string> = { username: "", remember_password: "false", auto_reconnect: "false", atrust_dns_enabled: "false", campus_dns_domains: "", routes: "" };
    for (const item of nextItems) if (isCoreConfigKey(item.key)) nextDrafts[item.key] = item.value;
    nextDrafts["connection_mode"] = connectionModeFromConfig(nextDrafts["connection_mode"]);
    // 未知协议（更高版本写入）保守显示 Cisco；旧配置缺失协议时也按 Cisco 迁移。
    nextDrafts["protocol"] = protocolFromConfig(nextDrafts["protocol"]);
    const selectedProtocol = protocolFromConfig(nextDrafts["protocol"]);
    nextDrafts["cisco_server"] ||= selectedProtocol === "cisco" ? nextDrafts["server"] ?? defaultServerFor("cisco") : defaultServerFor("cisco");
    nextDrafts["atrust_server"] ||= selectedProtocol === "atrust" ? nextDrafts["server"] ?? defaultServerFor("atrust") : defaultServerFor("atrust");
    nextDrafts["server"] = nextDrafts[selectedProtocol === "atrust" ? "atrust_server" : "cisco_server"] ?? nextDrafts["server"] ?? defaultServerFor(selectedProtocol);
    // 三值原样保留（C1 放行 headless）；未知取值（更高版本写入）保守回落 browser 展示。
    const authMode = nextDrafts["atrust_auth_mode"];
    nextDrafts["atrust_auth_mode"] = authMode === "embedded" || authMode === "headless" ? authMode : "browser";
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

/** 动效仍走草稿 + 自动保存；主题/强调色在模板中直接调用即时提交入口（R1）。 */
function setAppearanceDraft(key: "motion", value: "normal" | "reduced"): void {
  appearance.edit(key, value);
  scheduleAutoSave();
}

/**
 * 所有设置只改草稿；变更停顿 1 秒后由 `scheduleAutoSave` 调用本函数，按 core / UI 偏好 /
 * 外观的归属分流应用。校验错误仍内联提示且不提交（脏态保留，下次变更再重试）。
 */
async function saveAll(): Promise<void> {
  if (serverInputComposing.value) return;
  if (saving.value || appearance.saving.value) return;

  // 分流一：core 配置脏键。
  const pending: { key: CoreConfigKey; value: string }[] = [];
  const pendingRaw = { ...drafts.value };
  for (const field of knownFields.value) {
    const raw = drafts.value[field.key] ?? "";
    if (raw === (original.value[field.key] ?? "")) continue;
    const normalized = normalizeCoreConfigValue(field.key, raw, { protocol: protocol.value });
    if (!normalized.ok) {
      feedback.value[field.key] = normalized.message;
      return; // 校验错误内联；不提交
    }
    pending.push({ key: field.key, value: normalized.value });
  }

  const pendingUiPrefs = uiPrefsDirty.value;
  const pendingUiValues = captureUiPreferenceChanges();
  // 外观侧仍有待保存语义的只剩动效。必须用完整 AppearanceState 提交（未变的键取当前
  // 已生效值），不能直接展开 draft：内部强制展开只改呈现、不改 draft.mode，展开后
  // 再保存动效会把 draft 里陈旧的 mode 当成"待切换模式"，把窗口拽回极简。
  const pendingAppearanceValues = { ...appearance.state.value, motion: appearanceDraft.value.motion };
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
      pushToast("核心配置保存失败：Core 未确认写入；未完成草稿已保留，请重试。", "error");
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
    if (pending.some((item) => item.key === "username" || item.key === "server" || item.key === "protocol" || item.key === "routes")) {
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
  } catch (error) {
    // T9（§2.9）：保存设置前唤醒 core 失败 → 全局「尝试重启程序 / 仅退出」模态；
    // 草稿保留（下次保存会再次尝试唤醒）。
    if (handleCoreWakeFailure(error)) return;
    const detail = error instanceof Error ? error.message : String(error);
    pushToast(completed.concat(stage + "保存失败：" + detail + "；未完成草稿已保留，请重试。").join("；"), "error");
  } finally {
    saving.value = false;
  }
}

// 分区只切换可见面板；所有字段持续挂载，沿用模块级草稿和保存逻辑。
const pageRoot = ref<HTMLElement | null>(null);
function selectSection(id: string): void {
  if (!sections.some(section => section.id === id)) return;
  activeSection.value = id;
  const content = pageRoot.value?.querySelector<HTMLElement>(".settings-page__content");
  if (content) content.scrollTop = 0;
}
onMounted(() => {
  void loadUiPreferences();
  if (!loadedOnce.value) void load();
});
onUnmounted(flushPendingAutoSave);
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
        <section v-show="activeSection === 'connection'" id="settings-connection" class="settings-section" data-testid="settings-connection">
          <header class="settings-section__header">
            <div><h2>连接</h2></div>
          </header>

          <div v-if="loading" class="settings-state">正在读取核心配置…</div>
          <div v-else-if="loadError" class="settings-state settings-state--error">
            <span>{{ loadError }}</span><button type="button" data-testid="retry-settings" @click="load">重试</button>
          </div>
          <template v-else>
            <div class="protocol-choice" data-testid="protocol-choice">
              <span class="protocol-choice__label">{{ SHARED_CORE_FIELD_COPY.protocol.label }}</span>
              <div class="protocol-segment" role="group" :aria-label="SHARED_CORE_FIELD_COPY.protocol.label">
                <button type="button" data-testid="protocol-atrust" :aria-pressed="protocol === 'atrust'" @click="onProtocolChange('atrust')">
                  <span>aTrust</span><span class="protocol-segment__new">新</span>
                </button>
                <button type="button" data-testid="protocol-cisco" :aria-pressed="protocol === 'cisco'" @click="onProtocolChange('cisco')">Cisco</button>
              </div>
            </div>
            <div v-if="protocol === 'atrust'" class="protocol-choice protocol-choice--auth" data-testid="atrust-auth-choice">
              <span class="protocol-choice__label">认证通道</span>
              <div class="protocol-segment" role="group" aria-label="认证通道">
                <!-- C1 复审 C1-1：选中态全部正向匹配草稿值——否定式派生在第三档下会双选中。 -->
                <button type="button" data-testid="atrust-auth-browser" :aria-pressed="drafts.atrust_auth_mode === 'browser'" @click="setAtrustAuthMode('browser')">系统浏览器</button>
                <button type="button" data-testid="atrust-auth-embedded" :aria-pressed="drafts.atrust_auth_mode === 'embedded'" @click="setAtrustAuthMode('embedded')">内嵌浏览器</button>
                <button type="button" data-testid="atrust-auth-headless" :aria-pressed="drafts.atrust_auth_mode === 'headless'" @click="setAtrustAuthMode('headless')">自动登录</button>
              </div>

            </div>
            <section v-for="group in connectionGroups" :key="group.id" :class="{ 'settings-traffic': group.id === 'traffic' }" :data-testid="group.id === 'traffic' ? 'settings-traffic' : undefined" :aria-labelledby="group.id === 'traffic' ? 'settings-traffic-title' : undefined">
              <header v-if="group.id === 'traffic'" class="settings-traffic__header"><h3 id="settings-traffic-title">流量控制</h3><p>路由与 DNS 设置在下次连接生效。</p></header>
              <div class="settings-fields-grid">
              <template v-for="field in group.fields" :key="field.key">
              <section v-if="field.key === 'username'" class="settings-credentials" data-testid="settings-credentials" aria-labelledby="settings-credentials-title">
                <header class="settings-credentials__header"><h3 id="settings-credentials-title">账户与密码</h3><button type="button" class="routes-edit-button" data-testid="credential-change-button" @click="openPasswordModal">修改</button></header>
                <p v-if="protocol === 'atrust'" class="protocol-choice__note" data-testid="atrust-auth-note">通过aTrust协议连接时，用户名和密码仅影响自动登录模式。</p>
                <SettingsRow label="登录账户" data-testid="setting-username"><span class="settings-readonly" data-testid="credential-account">{{ drafts.username || "未设置账户" }}</span></SettingsRow>
                <SettingsRow label="密码" data-testid="setting-credential-status"><span class="settings-readonly" data-testid="credential-status-text" :data-stored="credentialConfig.state.value.stored ? 'true' : 'false'">{{ credentialConfig.state.value.stored ? `已保存密码（账户 ${drafts.username || "—"}）` : "未设置密码" }}</span></SettingsRow>
              </section>
              <SettingsRow v-else
                :class="{ 'settings-row--dense': true, 'settings-row--wide': field.kind === 'routes' }"
                :label="field.kind === 'server' ? 'VPN 服务器' : field.label"
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
                      <option v-for="server in serverPresets" :key="server.value" :value="server.value">{{ server.value }}</option>
                      <option value="custom">自定义</option>
                    </select>
                    <input
                      v-if="serverChoice === 'custom'"
                      :data-testid="`input-${field.key}-custom`"
                      type="text"
                      autocomplete="off"
                      :value="customServerDisplay(field.key)"
                      :aria-label="`${field.label}（自定义）`"
                      @input="updateCustomServer(field.key, $event)"
                      @compositionstart="startServerComposition"
                      @compositionend="endServerComposition(field.key, $event)"
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
                  <select
                    v-else-if="field.kind === 'connection_mode'"
                    :data-testid="`input-${field.key}`"
                    :value="drafts[field.key] ?? DEFAULT_CONNECTION_MODE"
                    :aria-label="field.label"
                    @change="updateText(field.key, $event)"
                  >
                    <option v-for="mode in CONNECTION_MODE_OPTIONS" :key="mode.value" :value="mode.value">{{ mode.label }}</option>
                  </select>

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
                      :disabled="field.key === 'auto_reconnect' ? !reconnectEligible : field.key === 'auto_reconnect_backoff' ? reconnectBackoffDisabled() : undefined"
                      @change="updateBoolean(field.key, $event)"
                    >
                    <span>{{ drafts[field.key] === "true" ? "开启" : "关闭" }}</span>
                  </label>
                  <span v-if="feedback[field.key]" :data-testid="`feedback-${field.key}`" class="config-feedback">{{ feedback[field.key] }}</span>
                </div>
              </SettingsRow>
              </template>
              </div>
            </section>
          </template>

        </section>

        <section v-show="activeSection === 'service'" id="settings-service" class="settings-section" data-testid="settings-service" aria-labelledby="settings-service-title">
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

        <section v-show="activeSection === 'startup'" id="settings-startup" class="settings-section" data-testid="settings-startup">
           <header class="settings-section__header"><div><h2>应用</h2></div></header>
           <div class="settings-fields-grid settings-fields-grid--static">
            <SettingsRow class="settings-row--dense" :label="APPLICATION_FIELD_COPY.launch_at_login.label" :description="APPLICATION_FIELD_COPY.launch_at_login.description">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-launch_at_login" type="checkbox" :checked="uiPrefsDraft.launch_at_login" :disabled="autostartStateUnavailable || isUiPrefSaving('launch_at_login')" :aria-label="APPLICATION_FIELD_COPY.launch_at_login.label" @change="saveUiPreference('launch_at_login', ($event.target as HTMLInputElement).checked)">
                <span>{{ autostartStateUnavailable ? "状态未确认" : uiPrefsDraft.launch_at_login ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('launch_at_login')" class="preference-error" data-testid="ui-pref-error-launch_at_login" role="alert"><span>{{ uiPrefError('launch_at_login') }}</span><button type="button" data-testid="ui-pref-retry-launch_at_login" @click="retryUiPreference('launch_at_login')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" :label="APPLICATION_FIELD_COPY.silent_startup.label" :description="APPLICATION_FIELD_COPY.silent_startup.description">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-silent_startup" type="checkbox" :checked="uiPrefsDraft.silent_startup" :disabled="isUiPrefSaving('silent_startup')" :aria-label="APPLICATION_FIELD_COPY.silent_startup.label" @change="saveUiPreference('silent_startup', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.silent_startup ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('silent_startup')" class="preference-error" data-testid="ui-pref-error-silent_startup" role="alert"><span>{{ uiPrefError('silent_startup') }}</span><button type="button" data-testid="ui-pref-retry-silent_startup" @click="retryUiPreference('silent_startup')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" :label="APPLICATION_FIELD_COPY.minimize_to_tray_on_connect.label" :description="APPLICATION_FIELD_COPY.minimize_to_tray_on_connect.description">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-minimize_to_tray_on_connect" type="checkbox" :checked="uiPrefsDraft.minimize_to_tray_on_connect" :disabled="isUiPrefSaving('minimize_to_tray_on_connect')" :aria-label="APPLICATION_FIELD_COPY.minimize_to_tray_on_connect.label" @change="saveUiPreference('minimize_to_tray_on_connect', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.minimize_to_tray_on_connect ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('minimize_to_tray_on_connect')" class="preference-error" data-testid="ui-pref-error-minimize_to_tray_on_connect" role="alert"><span>{{ uiPrefError('minimize_to_tray_on_connect') }}</span><button type="button" data-testid="ui-pref-retry-minimize_to_tray_on_connect" @click="retryUiPreference('minimize_to_tray_on_connect')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" :label="APPLICATION_FIELD_COPY.auto_connect_on_launch.label" :description="APPLICATION_FIELD_COPY.auto_connect_on_launch.description">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-auto_connect_on_launch" type="checkbox" :checked="uiPrefsDraft.auto_connect_on_launch" :disabled="isUiPrefSaving('auto_connect_on_launch')" :aria-label="APPLICATION_FIELD_COPY.auto_connect_on_launch.label" @change="saveUiPreference('auto_connect_on_launch', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.auto_connect_on_launch ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('auto_connect_on_launch')" class="preference-error" data-testid="ui-pref-error-auto_connect_on_launch" role="alert"><span>{{ uiPrefError('auto_connect_on_launch') }}</span><button type="button" data-testid="ui-pref-retry-auto_connect_on_launch" @click="retryUiPreference('auto_connect_on_launch')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" :label="APPLICATION_FIELD_COPY.close_preference.label" :description="APPLICATION_FIELD_COPY.close_preference.description">
              <div class="preference-control"><select class="settings-close-preference" data-testid="ui-pref-close_preference" :value="uiPrefsDraft.close_preference" :disabled="isUiPrefSaving('close_preference')" :aria-label="APPLICATION_FIELD_COPY.close_preference.label" @change="saveUiPreference('close_preference', ($event.target as HTMLSelectElement).value as 'smart' | 'tray' | 'quit')">
                <option value="smart">{{ CLOSE_PREFERENCE_LABELS.smart }}</option><option value="tray">{{ CLOSE_PREFERENCE_LABELS.tray }}</option><option value="quit">{{ CLOSE_PREFERENCE_LABELS.quit }}</option>
              </select><div v-if="uiPrefError('close_preference')" class="preference-error" data-testid="ui-pref-error-close_preference" role="alert"><span>{{ uiPrefError('close_preference') }}</span><button type="button" data-testid="ui-pref-retry-close_preference" @click="retryUiPreference('close_preference')">重试</button></div></div>
            </SettingsRow>
           </div>
         </section>

        <section v-show="activeSection === 'appearance'" id="settings-appearance" class="settings-section" data-testid="settings-appearance">
          <header class="settings-section__header"><div><h2>外观</h2></div></header>
          <SettingsRow class="settings-row--dense" :label="APPEARANCE_FIELD_COPY.theme.label" :description="APPEARANCE_FIELD_COPY.theme.description">
            <TitlebarThemeModeControl class="appearance-theme" data-testid="appearance-theme" :model-value="appearanceDraft.theme" @update:model-value="appearance.setTheme($event)" />
          </SettingsRow>
          <SettingsRow class="settings-row--dense" :label="APPEARANCE_FIELD_COPY.accent.label" :description="APPEARANCE_FIELD_COPY.accent.description">
            <div class="accent-options" role="group" aria-label="强调色">
              <button v-for="accent in accents" :key="accent.id" :data-testid="`appearance-accent-${accent.id}`" class="accent-option" :style="{ '--swatch': accent.color }" :class="{ 'accent-option--active': appearanceDraft.accent === accent.id }" type="button" :aria-label="accent.label" :title="accent.label" :aria-pressed="appearanceDraft.accent === accent.id" @click="appearance.setAccent(accent.id)"><svg v-if="appearanceDraft.accent === accent.id" viewBox="0 0 16 16" aria-hidden="true"><path d="m4 8 3 3 5-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" /></svg></button>
            </div>
          </SettingsRow>
          <SettingsRow class="settings-row--dense" :label="APPEARANCE_FIELD_COPY.motion.label" :description="APPEARANCE_FIELD_COPY.motion.description">
            <select data-testid="appearance-motion" :value="appearanceDraft.motion" :aria-label="APPEARANCE_FIELD_COPY.motion.label" @change="setAppearanceDraft('motion', ($event.target as HTMLSelectElement).value as 'normal' | 'reduced')">
              <option value="normal">正常</option><option value="reduced">减少动效</option>
            </select>
          </SettingsRow>
        </section>

        <section v-show="activeSection === 'diagnostics'" id="settings-diagnostics" class="settings-section" data-testid="settings-diagnostics">
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
            <SettingsRow class="settings-row--dense" label="触发自动重连时发送通知" description="实际开始自动重连时发送系统通知。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-reconnect_notify" type="checkbox" :checked="uiPrefsDraft.reconnect_notify" :disabled="isUiPrefSaving('reconnect_notify')" aria-label="触发自动重连时发送通知" @change="saveUiPreference('reconnect_notify', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.reconnect_notify ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('reconnect_notify')" class="preference-error" data-testid="ui-pref-error-reconnect_notify" role="alert"><span>{{ uiPrefError('reconnect_notify') }}</span><button type="button" data-testid="ui-pref-retry-reconnect_notify" @click="retryUiPreference('reconnect_notify')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow label="意外断开通知" description="已连接会话意外断开时发送通知。">
              <div class="preference-control"><label class="boolean-control"><input data-testid="ui-pref-unexpected_disconnect_notify" type="checkbox" :checked="uiPrefsDraft.unexpected_disconnect_notify" :disabled="isUiPrefSaving('unexpected_disconnect_notify')" aria-label="意外断开时发送通知" @change="saveUiPreference('unexpected_disconnect_notify', ($event.target as HTMLInputElement).checked)"><span>{{ uiPrefsDraft.unexpected_disconnect_notify ? "开启" : "关闭" }}</span></label>
              <div v-if="uiPrefError('unexpected_disconnect_notify')" class="preference-error" role="alert"><span>{{ uiPrefError('unexpected_disconnect_notify') }}</span><button type="button" @click="retryUiPreference('unexpected_disconnect_notify')">重试</button></div></div>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" label="窗口在前台时不发送通知" description="主窗口可见时无需发送连接类通知；优先级最高。">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-suppress_notify_when_foreground" type="checkbox" :checked="uiPrefsDraft.suppress_notify_when_foreground" :disabled="isUiPrefSaving('suppress_notify_when_foreground')" aria-label="窗口在前台时不发送通知" @change="saveUiPreference('suppress_notify_when_foreground', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.suppress_notify_when_foreground ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('suppress_notify_when_foreground')" class="preference-error" data-testid="ui-pref-error-suppress_notify_when_foreground" role="alert"><span>{{ uiPrefError('suppress_notify_when_foreground') }}</span><button type="button" data-testid="ui-pref-retry-suppress_notify_when_foreground" @click="retryUiPreference('suppress_notify_when_foreground')">重试</button></div></div>
            </SettingsRow>
          </div>
        </section>

        <section v-show="activeSection === 'experimental'" id="settings-experimental" class="settings-section settings-experimental" data-testid="settings-experimental">
          <header class="settings-section__header"><div><h2>{{ EXPERIMENTAL_SECTION_COPY.label }}</h2><p class="settings-experimental__note" data-testid="experimental-note">{{ EXPERIMENTAL_SECTION_COPY.note }}</p></div></header>
          <div class="settings-fields-grid settings-fields-grid--static">
            <SettingsRow class="settings-row--dense" :label="EXPERIMENTAL_FIELD_COPY.mtu.label" :description="EXPERIMENTAL_FIELD_COPY.mtu.description">
              <input
                data-testid="input-mtu"
                inputmode="numeric"
                type="text"
                autocomplete="off"
                :value="experimentalDraft('mtu')"
                :aria-label="EXPERIMENTAL_FIELD_COPY.mtu.label"
                @input="updateText('mtu', $event)"
              >
              <span v-if="feedback['mtu']" class="config-feedback">{{ feedback['mtu'] }}</span>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" :label="EXPERIMENTAL_FIELD_COPY.user_agent.label" :description="EXPERIMENTAL_FIELD_COPY.user_agent.description">
              <input
                data-testid="input-user_agent"
                type="text"
                autocomplete="off"
                :value="experimentalDraft('user_agent')"
                :aria-label="EXPERIMENTAL_FIELD_COPY.user_agent.label"
                @input="updateText('user_agent', $event)"
              >
              <span v-if="feedback['user_agent']" class="config-feedback">{{ feedback['user_agent'] }}</span>
            </SettingsRow>
            <SettingsRow class="settings-row--dense" :label="EXPERIMENTAL_FIELD_COPY.show_latency.label" :description="EXPERIMENTAL_FIELD_COPY.show_latency.description">
              <div class="preference-control"><label class="boolean-control">
                <input data-testid="ui-pref-show_latency" type="checkbox" :checked="uiPrefsDraft.show_latency" :disabled="isUiPrefSaving('show_latency')" :aria-label="EXPERIMENTAL_FIELD_COPY.show_latency.label" @change="saveUiPreference('show_latency', ($event.target as HTMLInputElement).checked)">
                <span>{{ uiPrefsDraft.show_latency ? "开启" : "关闭" }}</span>
              </label><div v-if="uiPrefError('show_latency')" class="preference-error" data-testid="ui-pref-error-show_latency" role="alert"><span>{{ uiPrefError('show_latency') }}</span><button type="button" data-testid="ui-pref-retry-show_latency" @click="retryUiPreference('show_latency')">重试</button></div></div>
            </SettingsRow>
          </div>
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

    <!-- Area F：设置页统一凭据模态（M2：按用户名存在性选形态；设置保存语义）。 -->
    <CredentialRequiredModal
      :visible="credentialModalOpen"
      :mode="credentialModalMode"
      purpose="settings"
      :initial-remember="credentialConfig.state.value.remember"
      :has-saved-password="credentialConfig.state.value.stored"
      :busy="credentialModalBusy"
      :error-message="credentialModalError"
      :reset-key="credentialModalResetKey"
      :initial-username="(drafts.username ?? '').trim()"
      @close="closePasswordModal"
      @submit="submitPasswordModal"
      @clear-saved-password="submitPasswordModal({ username: $event.username, password: '', persist: false, passwordAction: 'clear' })"
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
  --settings-select-width: 140px;
  /* 顶部留白由壳层 .product-content 统一提供。 */
  margin-top: 0;
}
.settings-page__header {
  display: flex;
  flex: none;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--space-4);
  margin: 0 0 12.5px;
  padding: 0;
  /* 无圆角无缝隙：顶部完整横条，不设 border-bottom，与下方设置项容器自然衔接。 */
  background: var(--surface-canvas);
}
.settings-page__header h1 { margin: 0 0 2.5px; font-size: var(--font-size-display); line-height: 1.6; font-weight: 700; letter-spacing: -0.02em; }
.settings-section__header p { margin-bottom: 0; color: var(--text-secondary); }
.settings-page__layout { display:flex; flex:1; min-height:0; flex-direction:column; padding-bottom:var(--space-6); }
.settings-page__content { flex:1; min-height:0; overflow:auto; scrollbar-width:thin; width:100%; padding-left:6px; padding-right:8px; }
.settings-section { padding:0 18px; border:1px solid var(--border-subtle); border-radius:16px; background:var(--surface-panel); }
.settings-section__header { display:none; }
.settings-section__header h2 { margin:0; font-size: var(--font-size-heading); }
.settings-section__header p { font-size: var(--font-size-caption); }
.settings-section__lead { padding: var(--space-1) 0 0; }
.protocol-choice { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); padding: 12px 14px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); margin: 10px 0; }
.protocol-choice--auth { margin-top: -2px; flex-wrap: wrap; }
.protocol-choice__label { font-size: var(--font-size-body); font-weight: 600; }
.protocol-choice__note { flex-basis: 100%; margin: 0; color: var(--text-secondary); font-size: var(--font-size-caption); line-height: 1.4; }
.protocol-segment button:disabled { cursor: not-allowed; opacity: 0.55; }
.protocol-segment { display: inline-flex; gap: 3px; padding: 3px; border-radius: 9px; border: 1px solid var(--border-subtle); background: var(--surface-canvas); }
.protocol-segment button { position: relative; border: 0; border-radius: 6px; background: transparent; color: var(--text-secondary); padding: 7px 14px; cursor: pointer; font: inherit; }
.protocol-segment button[aria-pressed="true"] { background: var(--accent-subtle); color: var(--accent); box-shadow: none; font-weight: 600; }
.protocol-segment__new { position: absolute; top: -7px; left: -3px; min-width: 16px; padding: 1px 4px; border-radius: 7px; background: var(--accent); color: var(--accent-on); font-size: var(--font-size-caption); line-height: 1.2; font-weight: 700; }
.settings-fields-grid { display: grid; grid-template-columns: minmax(0, 1fr); }
.settings-fields-grid .settings-row--wide { grid-column: 1 / -1; }
.settings-state { margin: 0; padding: var(--space-3) 0; color: var(--text-secondary); font-size: var(--font-size-body); }
.settings-state--error { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); color: var(--state-danger); }
.accent-options, .config-control { display: flex; min-width: 0; align-items: center; justify-content: flex-end; gap: 6px; }
.accent-options { flex-wrap:wrap; gap:18px; padding:4px; }
.accent-option { display:grid; place-items:center; flex:none; width:23px; height:23px; min-width:23px; min-height:23px; padding:0; border:1px solid color-mix(in srgb,currentColor 12%,transparent); border-radius:50%; background:var(--swatch); color:white; }
.accent-option svg { width:16px; height:16px; }
.accent-option--active { outline:1.5px solid var(--swatch); outline-offset:3px; }
:global(.settings-page .appearance-theme) { padding:3px; border-radius:7px; }
:global(.settings-page .appearance-theme .titlebar-theme-mode-control__button) { min-width:58px; height:28px; font-size: var(--font-size-body); padding:0 7px; }
:global(.settings-page .appearance-theme .titlebar-theme-mode-control__button span) { display:inline; }
:global(.settings-page .appearance-theme .titlebar-theme-mode-control__icon) { width:14px; height:14px; }
.config-control > input { flex: 1 1 0; min-width: 0; width: auto; }
/* 凭据状态行的已存圆点（Area F：存在性布尔）。 */
.settings-credentials { grid-column:1 / -1; min-width:0; padding:12px 16px; border:1px solid var(--border-subtle); border-radius:var(--radius-md); background:var(--surface-panel); }
.settings-credentials__header { display:flex; align-items:center; justify-content:space-between; gap:12px; }
.settings-credentials__header h3 { margin:0; font-size:var(--font-size-body); font-weight:600; }
.credential-stored-dot { margin-right: 4px; color: var(--state-success); font-size: var(--font-size-caption); }
.settings-page select { width: var(--settings-select-width); min-width: 0; flex: 0 0 var(--settings-select-width); }
.settings-page select.settings-close-preference,
.settings-page select.settings-server-preference { width: 208px; flex: 0 0 208px; }
.server-choice { display: flex; min-width: 0; align-items: center; justify-content: flex-end; gap: 6px; }
.server-choice select { min-height: 34px; }
.server-choice input[type="text"] { flex: 1 1 0; min-width: 0; width: auto; }
.boolean-control { display: inline-flex; align-items: center; gap: 7px; min-height: 34px; color: var(--text-primary); white-space: nowrap; }
.boolean-control input { appearance: none; flex: none; width: 38px; height: 22px; margin: 0; border: 1px solid var(--border-strong); border-radius: 999px; background: radial-gradient(circle at 10px 50%, var(--surface-panel) 0 7px, transparent 7.5px), var(--border-strong); cursor: pointer; }
.boolean-control input:checked { border-color: var(--accent); background: radial-gradient(circle at 27px 50%, var(--surface-panel) 0 7px, transparent 7.5px), var(--accent); }
.boolean-control input:focus-visible { outline: 2px solid var(--focus-ring); outline-offset: 3px; }
.boolean-control input:disabled { opacity: .5; cursor: not-allowed; }
.config-feedback { flex: 0 0 auto; color: var(--state-danger); font-size: var(--font-size-caption); }
.preference-control { display: grid; min-width: 0; justify-items: end; gap: 4px; }
.preference-error { display: flex; align-items: center; justify-content: flex-end; gap: var(--space-2); color: var(--state-danger); font-size: var(--font-size-caption); line-height: 1.35; }
.preference-error button { min-height: 24px; padding: 0 var(--space-2); border-color: var(--state-danger); background: transparent; color: var(--state-danger); font-size: var(--font-size-body); }
.routes-summary { flex: none; color: var(--text-secondary); white-space: nowrap; font-size: var(--font-size-body); }
.routes-edit-button { flex: none; min-height: 32px; padding: 4px 14px; border-color: var(--accent); background: var(--accent); color: var(--accent-on); font-weight: 600; cursor: pointer; }
.routes-edit-button:hover { border-color: var(--accent-strong); background: var(--accent-strong); }
.settings-readonly { color:var(--text-secondary); overflow-wrap:anywhere; }

@media (max-width: 1000px) {
  .settings-fields-grid { grid-template-columns: 1fr; }
  .protocol-choice { align-items: flex-start; flex-direction: column; }
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

@media (max-width: 700px) {
  .settings-page__header h1 { font-size: var(--font-size-heading); }
}

.motion-reduced .settings-page *, [data-motion="reduced"] .settings-page * { scroll-behavior: auto; transition: none !important; animation: none !important; }
.settings-traffic { margin: 16px 0; padding: 14px 16px; border: 1px solid var(--border-strong); border-radius: 12px; }
.settings-traffic__header { margin-bottom: 8px; }
.settings-traffic__header h3 { margin: 0; font-size: var(--font-size-body); }
.settings-traffic__header p { margin: 5px 0 0; color: var(--text-secondary); font-size: var(--font-size-caption); }
</style>
