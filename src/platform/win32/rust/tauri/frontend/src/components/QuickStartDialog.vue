<script setup lang="ts">
import { useDialogFocus } from "./use-dialog-focus";
import CredentialFields from "./CredentialFields.vue";
import { pushToast } from "../lib/toast";
import { credentialConfigFor } from "../product/core-config";
import { handleCoreWakeFailure, isCoreUnreachable } from "../product/core-crash";
import { computed, inject, ref, watch } from "vue";

import RoutesEditor from "./RoutesEditor.vue";
import { WINDOW_CHROME_PORT_KEY } from "../product/window-chrome";
import {
  kernel,
  type QuickStartApplyReply,
  type QuickStartApplyRequest,
} from "../lib/ipc";
import { commandErrorMessage } from "../lib/command-error";
import { APPEARANCE_KEY, type AppearanceState } from "../product/appearance";
import {
  CORE_CONFIG_GATEWAY_KEY,
  PROTOCOL_OPTIONS,
  createCoreConfigGateway,
  defaultServerFor,
  normalizeServerValue,
  sanitizeCustomServerInput,
  protocolFromConfig,
  type CoreConfigItem,
} from "../product/core-config";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "../product/service-operations";
import {
  createQuickStartDraft,
  quickStartCertificateGateway,
  type QuickStartCertificateGateway,
  quickStartItems,
  validateQuickStartDraft,
  type QuickStartDraft,
} from "../product/quick-start";
import {
  editUiPreference,
  uiPrefsDraft,
  updateUiPreferences,
  type ClosePreference,
} from "../product/ui-prefs";

const props = withDefaults(defineProps<{
  open: boolean;
  certificateGateway?: QuickStartCertificateGateway;
  initialItems?: ReadonlyArray<CoreConfigItem>;
  /** 默认接真实 Tauri command；测试可注入确定性实现。 */
  apply?: (request: QuickStartApplyRequest) => Promise<QuickStartApplyReply>;
  /** 独立刷新主界面；读取失败不改变已完成的保存或安装结果。 */
  afterCoreSaved?: () => Promise<void>;
}>(), {
  certificateGateway: () => quickStartCertificateGateway,
  apply: (request: QuickStartApplyRequest) => kernel.quickStartApply(request),
});

const emit = defineEmits<{
  complete: [];
  skip: [];
}>();

const appearance = inject(APPEARANCE_KEY, null);
const compact = computed(() => appearance?.state.value.mode === "minimal");
const chrome = inject(WINDOW_CHROME_PORT_KEY, null);
const configGateway = inject(CORE_CONFIG_GATEWAY_KEY, null) ?? createCoreConfigGateway();
const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const serviceOperations = inject(
  SERVICE_OPERATION_KEY,
  runtime ? createServiceOperationCoordinator((action) => runtime.serviceControl(action)) : null,
);
const advanced = ref(false);
const submitting = ref(false);
function requestSkip(): void {
  if (!submitting.value && !appearance?.saving.value) {
    draft.value.password = "";
    emit("skip");
  }
}
const { dialog, onDialogKeydown } = useDialogFocus(() => props.open, requestSkip);
const submittedInstallService = ref(false);
// 本次打开周期只完成一次；独立显示读取不重新提交保存或安装。
const setupCompleted = ref(false);
// 核心配置（含秘密）已经保存后，服务安装失败只重试本阶段，绝不重放配置提交。
const savedAwaitingService = ref(false);
const savedAwaitingCertificate = ref(false);
const installCertificate = ref(true);
const needsCertificate = computed(() => draft.value.protocol === "atrust" && draft.value.atrust_auth_mode === "browser");
const authOptions = [
  { value: "browser", label: "系统浏览器" },
  { value: "embedded", label: "内嵌浏览器" },
  { value: "headless", label: "自动登录" },
] as const;
const themeOptions = [
  { value: "system", label: "跟随系统" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
] as const;
const formFrozen = computed(() => savedAwaitingService.value || savedAwaitingCertificate.value || setupCompleted.value);
const submitError = ref<string | null>(null);
// T4（F8）：偏好/外观收尾腿的非阻断提示条——失败不换按钮相位、不阻断完成，
// 文案语义为「将在下次启动重试」；与 submitError（阻断行）分开展示。
const preferenceNotice = ref<string | null>(null);
// 首次提交的草稿/外观快照，供安装腿成功后的偏好收尾腿（含安装失败重试路径）使用。
const pendingPreferences = ref<{ draft: QuickStartDraft; appearance: AppearanceState | undefined } | null>(null);
const fieldErrors = ref<Partial<Record<"username" | "password" | "server", string>>>({});
const draft = ref<QuickStartDraft>(createQuickStartDraft([]));
const savedCredential = ref({ username: "", stored: false });
const clearedPassword = ref(false);
const usesCredentials = computed(() => draft.value.protocol === "cisco" || draft.value.atrust_auth_mode === "headless");
const canKeepPassword = computed(() => savedCredential.value.stored && !clearedPassword.value
  && draft.value.username.trim() === savedCredential.value.username.trim());
const authDescription = computed(() => draft.value.atrust_auth_mode === "headless"
  ? "账户与密码保存到本机，用于后续自动登录。"
  : draft.value.atrust_auth_mode === "embedded"
    ? "在 EXV 内嵌浏览器中完成学校统一认证。"
    : "使用系统默认浏览器完成学校统一认证。连接时将检查 aTrust 服务；需要停用时会提示您，停用持续到您在设置中主动恢复。");
function clearPasswordDraft(): void {
  draft.value.password = "";
  clearedPassword.value = true;
  fieldErrors.value.password = undefined;
}


const routes = computed(() => draft.value.routes.split(",").map((item) => item.trim()).filter(Boolean));
const routesSummary = computed(() => `${routes.value.length} 条路由`);
// 主题属"立即生效 + 自动持久化"的个性化项：直接读写已生效状态，不进草稿、不等提交。
const selectedTheme = computed({
  get: () => appearance?.state.value.theme ?? "system",
  set: (theme: "system" | "light" | "dark") => appearance?.setTheme(theme),
});
const submitLabel = computed(() => {
  if (savedAwaitingCertificate.value) return submitting.value ? "正在准备证书…" : "重试准备证书";
  if (savedAwaitingService.value) return submitting.value ? "正在安装服务…" : "重试安装服务";
  if (!submitting.value) return "完成设置";
  return submittedInstallService.value ? "正在保存配置并安装服务…" : "正在保存配置…";
});

/** 该组件仅在本次打开周期保留草稿；Quick ↔ Advanced 切换绝不重置。 */
watch(
  () => props.open,
  (open, wasOpen) => {
    if (!open) { draft.value.password = ""; return; }
    if (wasOpen) return;
    advanced.value = false;
    submitting.value = false;
    submittedInstallService.value = false;
    setupCompleted.value = false;
    savedAwaitingService.value = false;
    savedAwaitingCertificate.value = false;
    installCertificate.value = true;
    submitError.value = null;
    preferenceNotice.value = null;
    pendingPreferences.value = null;
    fieldErrors.value = {};
    clearedPassword.value = false;
    const initial = props.initialItems ?? [];
    savedCredential.value = {
      username: initial.find(item => item.key === "username")?.value ?? "",
      stored: initial.some(item => item.key === "has_saved_credential_pair" && item.value === "true")
        && initial.some(item => item.key === "remember_password" && item.value === "true"),
    };
    draft.value = {
      ...createQuickStartDraft(props.initialItems ?? []),
      // 「安装服务」是本次快速入门的清单项，作用域只到提交那一刻；不是持久化设置项，
      // 与连接页「安装服务后连接」的纯前端偏好无关（各自独立，不共享键）。
      launch_at_login: String(uiPrefsDraft.value.launch_at_login),
      auto_connect_on_launch: String(uiPrefsDraft.value.auto_connect_on_launch),
      minimize_to_tray_on_connect: String(uiPrefsDraft.value.minimize_to_tray_on_connect),
      close_preference: uiPrefsDraft.value.close_preference,
    };
  },
  { immediate: true },
);

function updateText(key: keyof QuickStartDraft, event: Event): void {
  const input = event.target as HTMLInputElement;
  if ((event as InputEvent).isComposing) return;
  // 编辑投影沿现有裸域名归一化去除 URL 外壳，持久值仍由 quickStartItems 按协议生成。
  const value = key === "server" || key === "atrust_server"
    ? sanitizeCustomServerInput(normalizeServerValue(input.value, "cisco")) : input.value;
  input.value = value;
  draft.value[key] = value;
  if (key === "server") draft.value[draft.value.protocol === "atrust" ? "atrust_server" : "cisco_server"] = draft.value.server;
  if (key === "username" || key === "password" || key === "server") {
    fieldErrors.value = { ...fieldErrors.value, [key]: undefined };
  }
}

function updateBoolean(key: "auto_reconnect" | "install_service", event: Event): void {
  // 只改本周期草稿；提交时按该值决定是否安装服务，提交后勾选随草稿一起丢弃。
  draft.value[key] = (event.target as HTMLInputElement).checked ? "true" : "false";
}

function onRoutesChange(nextRoutes: string[]): void {
  draft.value.routes = nextRoutes.join(",");
}

/** 协议切换时保存当前侧的服务器，再恢复另一协议各自保存的服务器。 */
function onProtocolChange(event: Event): void {
  const next = protocolFromConfig((event.target as HTMLSelectElement).value);
  const previous = protocolFromConfig(draft.value.protocol);
  draft.value[previous === "atrust" ? "atrust_server" : "cisco_server"] = draft.value.server;
  draft.value.protocol = next;
  draft.value.server = draft.value[next === "atrust" ? "atrust_server" : "cisco_server"] || defaultServerFor(next);
}

function saveAdvancedUiPreferences(pending: QuickStartDraft): Promise<boolean> {
  editUiPreference("launch_at_login", pending.launch_at_login === "true");
  editUiPreference("auto_connect_on_launch", pending.auto_connect_on_launch === "true");
  editUiPreference("minimize_to_tray_on_connect", pending.minimize_to_tray_on_connect === "true");
  editUiPreference("close_preference", pending.close_preference as ClosePreference);
  return updateUiPreferences(["launch_at_login", "auto_connect_on_launch", "minimize_to_tray_on_connect", "close_preference"]);
}

function finishSavedSetup(): void {
  if (setupCompleted.value) return;
  setupCompleted.value = true;
  const displayRefresh = Promise.resolve().then(() => props.afterCoreSaved
    ? props.afterCoreSaved()
    : runtime?.configurationChanged({ strict: true }));
  emit("complete");
  void displayRefresh.catch((error: unknown) => {
    const diagnostic = typeof error === "object" && error !== null
      ? error as { kind?: unknown; code?: unknown }
      : {};
    const code = [diagnostic.kind, diagnostic.code]
      .filter((value) => typeof value === "string" || typeof value === "number")
      .join(" / ");
    pushToast("配置已保存，主界面信息暂未更新" + (code ? `（${code}）` : ""), "warning");
  });
}

/**
 * ④ 偏好/外观收尾腿（T4/F8：改到安装腿之后执行）：失败只置非阻断提示条——
 * 不换按钮相位、不阻断刷新与完成；两项保存器各自内部吞错返回布尔，这里的
 * catch 只是兜底保险，保证本腿任何异常都不改变主链路。
 */
async function finishPreferenceTail(): Promise<void> {
  const pending = pendingPreferences.value;
  if (!pending) return;
  const failed: string[] = [];
  try {
    if (!(await saveAdvancedUiPreferences(pending.draft))) failed.push("界面偏好");
    if (appearance?.motionDirty.value && !(await appearance.saveDraft(
      chrome ? (mode) => chrome.setMode(mode) : undefined,
      pending.appearance,
    ))) {
      failed.push("外观动效");
    }
  } catch {
    failed.push("界面偏好/外观动效");
  }
  if (failed.length > 0) {
    preferenceNotice.value = `${failed.join("、")}保存失败，将在下次启动重试。`;
  }
  pendingPreferences.value = null;
}

async function installSavedService(): Promise<void> {
  if (serviceOperations === null) {
    submitError.value = "配置已保存，但服务操作不可用，请重试。";
    return;
  }
  submitting.value = true;
  submitError.value = null;
  let installed = false;
  try {
    const reply = await serviceOperations.run("install");
    if (!reply.ok) {
      submitError.value = reply.message || "配置已保存，但安装服务失败，请重试安装。";
      return;
    }
    installed = true;
  } catch (error) {
    // T4（F8 后半）：kind 透传——安装腿异常按 AppError kind 经 command-error
    // 词表归一为可行动文案，不再吞成与原因无关的固定文案（恢复入口 R5 另行裁定）。
    submitError.value = "配置已保存，但安装服务失败：" + commandErrorMessage(error, "请重试安装。");
  } finally {
    if (!installed) submitting.value = false;
  }
  if (!installed) return;
  savedAwaitingService.value = false;
  try {
    await finishPreferenceTail();
    finishSavedSetup();
  } finally {
    submitting.value = false;
  }
}

async function prepareSavedCertificate(): Promise<void> {
  submitting.value = true;
  submitError.value = null;
  try {
    const status = await props.certificateGateway.inspect();
    if (!status.trusted) await props.certificateGateway.install();
  } catch (error) {
    const message = error instanceof Error ? error.message
      : typeof error === "object" && error !== null && "message" in error ? String(error.message) : String(error);
    submitError.value = `配置已保存，但证书准备失败：${message}。请重试准备证书。`;
    submitting.value = false;
    return;
  }
  savedAwaitingCertificate.value = false;
  try {
    if (submittedInstallService.value) {
      savedAwaitingService.value = true;
      await installSavedService();
    } else {
      await finishPreferenceTail();
      finishSavedSetup();
    }
  } finally {
    submitting.value = false;
  }
}

async function submit(): Promise<void> {
  if (setupCompleted.value || submitting.value || appearance?.saving.value) return;
  if (savedAwaitingCertificate.value) { await prepareSavedCertificate(); return; }
  if (savedAwaitingService.value) {
    await installSavedService();
    return;
  }
  const errors = validateQuickStartDraft(draft.value, { ...savedCredential.value, stored: canKeepPassword.value });
  fieldErrors.value = errors;
  submitError.value = null;
  if (Object.keys(errors).length > 0) return;

  const pending = { ...draft.value };
  // 外观侧仍有待保存语义的只剩动效；主题/强调色/模式已在用户操作时即时提交。
  // 提交时必须给出完整 AppearanceState（未变的键取当前已生效值），否则 saveDraft
  // 会把整体快照写入存储，清掉其它外观项。
  const pendingAppearance = appearance
    ? { ...appearance.state.value, motion: appearance.draft.value.motion }
    : undefined;
  const prepareCertificate = needsCertificate.value && installCertificate.value;
  submittedInstallService.value = pending.install_service === "true";
  submitting.value = true;
  try {
    // ① ConfigSet：服务操作由应用级协调器独立执行，后端组合安装腿已删除（T4）。
    credentialConfigFor(configGateway).markSaved();
    const reply = await credentialConfigFor(configGateway).write(() => props.apply({
      items: quickStartItems(pending),
    }));
    if (!reply.ok) {
      submitError.value = reply.message ?? "快速入门提交失败，请检查设置后重试。";
      return;
    }
    // ③ 勾选安装则立即进入服务安装相位；④ 偏好/外观改到安装腿之后执行
    // （T4/F8：偏好失败不再阻断安装腿，只置非阻断提示条）。
    draft.value.password = "";
    pending.password = "";
    pendingPreferences.value = { draft: pending, appearance: pendingAppearance };
    if (prepareCertificate) {
      savedAwaitingCertificate.value = true;
      await prepareSavedCertificate();
    } else if (submittedInstallService.value) {
      savedAwaitingService.value = true;
      await installSavedService();
    } else {
      await finishPreferenceTail();
      finishSavedSetup();
    }
  } catch (error) {
    // T9（§2.9）：快速入门提交前唤醒 core 失败 → 全局「尝试重启程序 / 仅退出」模态。
    // 唤醒失败错误由壳层生成（不含凭据），可安全展示；其余异常保持与原因无关的
    // 固定文案（见 `QuickStartCredentials.test.ts` 的 `错误不携带凭据` 用例）。
    if (handleCoreWakeFailure(error)) return;
    // A1-2（2026-10-09 区域 A）：壳层 ConfigSet 重试环（A1-1）耗尽后仍 core 不可达
    // → 分流为「后台服务未就绪」的可重试文案（固定文案、不含凭据）。与
    // `command-error.ts` 的全局 core_unreachable 映射并存：全局版服务 toast 路径，
    // 对话框版更具体地指向「稍候重试」动作，两者各自固定、不互相改写。
    if (isCoreUnreachable(error)) {
      submitError.value = "配置保存失败：后台服务未就绪，请稍候重试；若反复失败请重启程序。";
      return;
    }
    submitError.value = "快速入门提交失败，请重试。";
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <div
    v-if="open"
    class="quick-start-overlay exv-dialog"
    :data-compact="compact ? 'true' : 'false'"
    data-testid="quick-start-dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="quick-start-title"
    ref="dialog"
    tabindex="-1"
    :aria-busy="submitting"
    @keydown="onDialogKeydown"
  >
    <section
      class="quick-start-card exv-dialog-card"
      :class="{ 'quick-start-card--advanced': advanced }"
      data-testid="quick-start-dialog-card"
    >
      <header class="quick-start-header">
        <h2 id="quick-start-title">快速入门</h2>
        <div class="quick-start-mode-control" data-testid="quick-start-mode-control" role="group" aria-label="快速入门模式">
          <button
            type="button"
            class="quick-start-mode-control__button"
            :class="{ 'quick-start-mode-control__button--active': !advanced }"
            data-testid="quick-start-mode-basic"
            :aria-pressed="!advanced"
            :disabled="submitting"
            @click="advanced = false"
          >
            快速
          </button>
          <button
            type="button"
            class="quick-start-mode-control__button"
            :class="{ 'quick-start-mode-control__button--active': advanced }"
            data-testid="quick-start-mode-advanced"
            :aria-pressed="advanced"
            :disabled="submitting"
            @click="advanced = true"
          >
            高级
          </button>
        </div>
      </header>

      <div class="quick-start-body" tabindex="0" role="region" aria-label="快速入门设置与提示">
      <section v-if="!advanced" :inert="formFrozen" class="quick-start-fields" data-testid="quick-start-basic-fields">
        <div v-if="draft.protocol === 'atrust'" class="quick-start-choice-field">
          <span>认证通道</span>
          <div class="quick-start-choice" role="group" aria-label="认证通道" data-testid="quick-start-atrust-auth-mode">
            <button v-for="option in authOptions" :key="option.value" type="button" :data-value="option.value"
              :aria-pressed="draft.atrust_auth_mode === option.value" :disabled="submitting || formFrozen"
              @click="draft.atrust_auth_mode = option.value; fieldErrors = {}">{{ option.label }}</button>
          </div>
        </div>
        <p v-if="draft.protocol === 'atrust'" class="protocol-note" data-testid="quick-start-atrust-note">{{ authDescription }}</p>
        <label v-if="needsCertificate" class="check-row">
          <input v-model="installCertificate" data-testid="quick-start-certificate" type="checkbox" :disabled="submitting || formFrozen">
          安装系统浏览器登录证书
        </label>
        <CredentialFields v-if="usesCredentials" v-model:username="draft.username" v-model:password="draft.password" test-id-prefix="quick-start" :has-saved-password="canKeepPassword" :busy="submitting || formFrozen" :compact="compact" :show-clear="draft.protocol === 'atrust'" :username-error="fieldErrors.username" :password-error="fieldErrors.password" @update:username="fieldErrors.username = undefined" @update:password="fieldErrors.password = undefined" @clear-password="clearPasswordDraft" />
        <label class="check-row">
          <input
            :checked="draft.install_service === 'true'"
            data-testid="quick-start-install-service"
            type="checkbox"
            @change="updateBoolean('install_service', $event)"
          >
          安装服务
        </label>
        <div class="quick-start-choice-field">
          <span>主题</span>
          <div class="quick-start-choice" role="group" aria-label="主题" data-testid="quick-start-theme">
            <button v-for="option in themeOptions" :key="option.value" type="button" :data-value="option.value"
              :aria-pressed="selectedTheme === option.value" :disabled="submitting || formFrozen || appearance?.saving.value"
              @click="selectedTheme = option.value">{{ option.label }}</button>
          </div>
        </div>
        <div class="routes-summary">
          <span data-testid="quick-start-routes-summary">{{ routesSummary }}</span>
        </div>
      </section>

      <section
        v-else
        :inert="formFrozen"
        class="quick-start-advanced quick-start-advanced--three-columns"
        data-testid="quick-start-advanced-fields"
      >
        <div class="quick-start-advanced-column" data-testid="quick-start-advanced-column">
          <div v-if="draft.protocol === 'atrust'" class="quick-start-choice-field">
          <span>认证通道</span>
          <div class="quick-start-choice" role="group" aria-label="认证通道" data-testid="quick-start-atrust-auth-mode">
            <button v-for="option in authOptions" :key="option.value" type="button" :data-value="option.value"
              :aria-pressed="draft.atrust_auth_mode === option.value" :disabled="submitting || formFrozen"
              @click="draft.atrust_auth_mode = option.value; fieldErrors = {}">{{ option.label }}</button>
          </div>
        </div>
          <p v-if="draft.protocol === 'atrust'" class="protocol-note" data-testid="quick-start-atrust-note">{{ authDescription }}</p>
        <label v-if="needsCertificate" class="check-row">
          <input v-model="installCertificate" data-testid="quick-start-certificate" type="checkbox" :disabled="submitting || formFrozen">
          安装系统浏览器登录证书
        </label>
          <CredentialFields v-if="usesCredentials" v-model:username="draft.username" v-model:password="draft.password" test-id-prefix="quick-start" :has-saved-password="canKeepPassword" :busy="submitting || formFrozen" :compact="compact" :show-clear="draft.protocol === 'atrust'" :username-error="fieldErrors.username" :password-error="fieldErrors.password" @update:username="fieldErrors.username = undefined" @update:password="fieldErrors.password = undefined" @clear-password="clearPasswordDraft" />
          <label class="check-row"><input :checked="draft.install_service === 'true'" data-testid="quick-start-install-service" type="checkbox" @change="updateBoolean('install_service', $event)">安装服务</label>
          <div class="quick-start-choice-field">
          <span>主题</span>
          <div class="quick-start-choice" role="group" aria-label="主题" data-testid="quick-start-theme">
            <button v-for="option in themeOptions" :key="option.value" type="button" :data-value="option.value"
              :aria-pressed="selectedTheme === option.value" :disabled="submitting || formFrozen || appearance?.saving.value"
              @click="selectedTheme = option.value">{{ option.label }}</button>
          </div>
        </div>
        </div>
        <div class="quick-start-advanced-column" data-testid="quick-start-advanced-column">
          <label>
            VPN 协议
            <select :value="draft.protocol" data-testid="quick-start-protocol" @change="onProtocolChange($event)">
              <option v-for="option in PROTOCOL_OPTIONS" :key="option.value" :value="option.value">{{ option.label }}</option>
            </select>
          </label>
          <label>
            VPN 服务器
            <input :value="normalizeServerValue(draft.server, 'cisco')" data-testid="quick-start-server" @input="updateText('server', $event)" @compositionend="updateText('server', $event)">
            <span v-if="fieldErrors.server" class="field-error">{{ fieldErrors.server }}</span>
          </label>
          <label class="check-row"><input :checked="draft.auto_reconnect === 'true'" data-testid="quick-start-auto-reconnect" type="checkbox" @change="updateBoolean('auto_reconnect', $event)">自动重连</label>
          <label>
            自动重连次数
            <input v-model="draft.auto_reconnect_max_attempts" data-testid="quick-start-auto-reconnect-attempts" :disabled="draft.auto_reconnect !== 'true'" inputmode="numeric">
          </label>
          <label class="check-row"><input v-model="draft.launch_at_login" true-value="true" false-value="false" data-testid="quick-start-launch-at-login" type="checkbox">开机自启</label>
          <label class="check-row"><input v-model="draft.auto_connect_on_launch" true-value="true" false-value="false" data-testid="quick-start-auto-connect" type="checkbox">启动后自动连接</label>
          <label class="check-row"><input v-model="draft.minimize_to_tray_on_connect" true-value="true" false-value="false" data-testid="quick-start-minimize-on-connect" type="checkbox">连接后最小化到托盘</label>
          <label>
            关闭窗口时
            <select v-model="draft.close_preference" data-testid="quick-start-close-preference"><option value="smart">智能判断最小化与退出</option><option value="tray">最小化到托盘</option><option value="quit">直接退出</option></select>
          </label>
        </div>
        <div class="quick-start-advanced-column quick-start-route-column" data-testid="quick-start-advanced-column">
          <h3 data-testid="quick-start-route-column">路由设置</h3>
          <p>经过 VPN 的目标网段；支持 CIDR 或单个 IP。</p>
          <RoutesEditor :routes="routes" :compact="compact" @change="onRoutesChange" />
        </div>
      </section>

      <p v-if="submitError" class="submit-error" data-testid="quick-start-submit-error">{{ submitError }}</p>
      <p v-if="preferenceNotice" class="preference-notice" data-testid="quick-start-preference-notice">{{ preferenceNotice }}</p>
      </div>
      <footer class="quick-start-actions">
        <button type="button" data-testid="quick-start-skip" :disabled="submitting" @click="requestSkip">暂时跳过</button>
        <button type="button" class="quick-start-submit" data-testid="quick-start-submit" :disabled="submitting" @click="submit">
          {{ submitLabel }}
        </button>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.quick-start-overlay { position: fixed; inset: 0; z-index: 60; display: grid; place-items: center; padding: max(var(--space-4), env(safe-area-inset-top)) max(var(--space-4), env(safe-area-inset-right)) max(var(--space-4), env(safe-area-inset-bottom)) max(var(--space-4), env(safe-area-inset-left)); background: rgb(0 0 0 / 0.5); }
.quick-start-card { max-height: calc(100dvh - 32px); display: flex; flex-direction: column; box-sizing: border-box; width: min(100%, 680px); max-height: calc(100dvh - 2 * var(--space-4)); display: flex; flex-direction: column; overflow: hidden; padding: var(--space-6); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: var(--surface-panel); color: var(--text-primary); box-shadow: 0 16px 48px rgb(0 0 0 / 0.32); transition: width 180ms ease; }
.quick-start-card--advanced { width: min(100%, 1180px); height: min(88dvh, 720px); display: flex; flex-direction: column; }
.quick-start-body { min-height: 0; overflow-y: auto; }
.quick-start-header { display: flex; min-width: 0; align-items: center; justify-content: space-between; gap: var(--space-4); }
.quick-start-card h2 { min-width: 0; margin: 0; overflow-wrap: anywhere; font-size: var(--font-size-heading); }
.quick-start-mode-control { display: inline-flex; flex: none; align-items: center; gap: 2px; padding: 2px; border: 1px solid var(--border-subtle); border-radius: 8px; background: color-mix(in srgb, var(--surface-canvas) 84%, transparent); }
.quick-start-mode-control__button { display: inline-flex; min-height: 28px; min-width: 48px; align-items: center; justify-content: center; padding: 0 var(--space-2); border: 0; border-radius: 6px; background: transparent; color: var(--text-secondary); font-size: var(--font-size-body); line-height: 1; cursor: pointer; transition: background-color 120ms ease, color 120ms ease; }
.quick-start-mode-control__button:hover:not(:disabled) { color: var(--text-primary); }
.quick-start-mode-control__button--active { background: var(--accent-subtle); color: var(--accent-strong); }
.quick-start-mode-control__button:focus-visible { outline: 3px solid var(--focus-ring); outline-offset: -2px; }
.quick-start-fields, .quick-start-advanced { display: grid; gap: var(--space-3); margin-top: var(--space-5); }
.quick-start-advanced { padding-top: var(--space-4); border-top: 1px solid var(--border-subtle); }
.quick-start-advanced--three-columns { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(0, 1.15fr); column-gap: var(--space-5); flex: 1 1 auto; min-height: 0; }
.quick-start-advanced-column { display: grid; align-content: start; min-width: 0; gap: var(--space-3); }
.quick-start-route-column { min-height: 0; display: flex; flex-direction: column; }
.quick-start-route-column h3 { margin: 0; font-size: var(--font-size-heading); }
.quick-start-route-column > p { margin: calc(var(--space-1) * -1) 0 0; color: var(--text-secondary); font-size: var(--font-size-caption); line-height: 1.4; }
.quick-start-route-column :deep(.routes-editor) { flex: 1 1 auto; min-height: 0; }
.quick-start-route-column :deep(.routes-list) { max-height: none; }
label { min-width: 0; overflow-wrap: anywhere; display: grid; gap: var(--space-1); font-size: var(--font-size-body); }
input, select { box-sizing: border-box; min-width: 0; max-width: 100%; width: 100%; min-height: 36px; padding: 0 var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); background: var(--surface-subtle); color: var(--text-primary); }
.check-row { display: flex; align-items: center; gap: var(--space-2); }
.check-row input { width: auto; flex: none; min-height: auto; }
.routes-summary { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); }
.protocol-note { overflow-wrap: anywhere; margin: 0; color: var(--text-secondary); font-size: var(--font-size-caption); line-height: 1.5; }
.field-error, .submit-error { color: var(--state-danger); font-size: var(--font-size-caption); }
.submit-error { margin: var(--space-4) 0 0; }
/* T4：偏好/外观收尾失败的非阻断提示条（warning 色，区别于 submit-error 的阻断红）。 */
.preference-notice { margin: var(--space-3) 0 0; color: var(--state-warning); font-size: var(--font-size-caption); }
.quick-start-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-5); }
.quick-start-submit { border-color: var(--accent); background: var(--accent); color: var(--accent-on); font-weight: 600; }
button:disabled { cursor: not-allowed; opacity: .65; }

@media (max-width: 1000px) {
  .quick-start-card--advanced { width: min(100%, 620px); height: auto; max-height: min(88dvh, 720px); }
  .quick-start-advanced--three-columns { grid-template-columns: minmax(0, 1fr); }
  .quick-start-route-column { min-height: 280px; }
}

@media (max-width: 420px) {
  .quick-start-card { padding: var(--space-4); }
  .quick-start-header { align-items: flex-start; }
  .quick-start-mode-control__button { min-width: 42px; }
}

.motion-reduced .quick-start-card,
[data-motion="reduced"] .quick-start-card { transition: none; }
/* 与设置页多档按钮一致：当前项可见，键盘焦点清晰，窄列允许完整换行。 */
.quick-start-choice-field { display: grid; min-width: 0; gap: var(--space-1); font-size: var(--font-size-body); }
.quick-start-choice { display: flex; flex-wrap: wrap; min-width: 0; gap: 2px; padding: 2px; border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); background: var(--surface-subtle); }
.quick-start-choice button { flex: 1 1 auto; min-width: 0; min-height: 36px; padding: var(--space-1) var(--space-2); border: 0; border-radius: var(--radius-sm); background: transparent; color: var(--text-secondary); white-space: normal; overflow-wrap: anywhere; }
.quick-start-choice button[aria-pressed="true"] { background: var(--accent-subtle); color: var(--accent-strong); }
.quick-start-choice button:focus-visible, .quick-start-body:focus-visible { outline: 3px solid var(--focus-ring); outline-offset: -2px; }
.quick-start-header { flex-wrap: wrap; }
.submit-error { overflow-wrap: anywhere; }
</style>
