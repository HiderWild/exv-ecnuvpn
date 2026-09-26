<script setup lang="ts">
import PasswordField from "./PasswordField.vue";
import { credentialConfigFor } from "../product/core-config";
import { computed, inject, ref, watch } from "vue";

import RoutesEditor from "./RoutesEditor.vue";
import { WINDOW_CHROME_PORT_KEY } from "../product/window-chrome";
import {
  kernel,
  type QuickStartApplyReply,
  type QuickStartApplyRequest,
} from "../lib/ipc";
import { APPEARANCE_KEY } from "../product/appearance";
import { CORE_CONFIG_GATEWAY_KEY, createCoreConfigGateway } from "../product/core-config";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "../product/service-operations";
import {
  createQuickStartDraft,
  quickStartItems,
  quickStartCredentialsMatch,
  validateQuickStartDraft,
  type QuickStartDraft,
} from "../product/quick-start";
import {
  editUiPreference,
  uiPrefsDraft,
  uiPrefErrors,
  updateUiPreferences,
  type ClosePreference,
} from "../product/ui-prefs";

const props = withDefaults(defineProps<{
  open: boolean;
  /** 默认接真实 Tauri command；测试可注入确定性实现。 */
  apply?: (request: QuickStartApplyRequest) => Promise<QuickStartApplyReply>;
  /** 应用根层在核心配置落盘后刷新运行时投影；resolve 前本对话框保持打开。 */
  afterCoreSaved?: () => Promise<void>;
}>(), {
  apply: (request: QuickStartApplyRequest) => kernel.quickStartApply(request),
});

const emit = defineEmits<{
  complete: [];
  skip: [];
}>();

const appearance = inject(APPEARANCE_KEY, null);
const chrome = inject(WINDOW_CHROME_PORT_KEY, null);
const configGateway = inject(CORE_CONFIG_GATEWAY_KEY, null) ?? createCoreConfigGateway();
const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const serviceOperations = inject(
  SERVICE_OPERATION_KEY,
  runtime ? createServiceOperationCoordinator((action) => runtime.serviceControl(action)) : null,
);
const advanced = ref(false);
const submitting = ref(false);
const submittedInstallService = ref(false);
// 配置及安全回读已确认后，仅重试展示刷新，不再次发送密码或 ConfigSet。
const savedAwaitingRefresh = ref(false);
// 核心配置（含秘密）已经保存后，服务安装失败只重试本阶段，绝不重放配置提交。
const savedAwaitingService = ref(false);
const formFrozen = computed(() => savedAwaitingService.value || savedAwaitingRefresh.value);
const submitError = ref<string | null>(null);
const fieldErrors = ref<Partial<Record<"username" | "password" | "server", string>>>({});
const draft = ref<QuickStartDraft>(createQuickStartDraft([]));

const routes = computed(() => draft.value.routes.split(",").map((item) => item.trim()).filter(Boolean));
const routesSummary = computed(() => `${routes.value.length} 条路由`);
// 主题属"立即生效 + 自动持久化"的个性化项：直接读写已生效状态，不进草稿、不等提交。
const selectedTheme = computed({
  get: () => appearance?.state.value.theme ?? "system",
  set: (theme: "system" | "light" | "dark") => appearance?.setTheme(theme),
});
const submitLabel = computed(() => {
  if (savedAwaitingService.value) return submitting.value ? "正在安装服务…" : "重试安装服务";
  if (savedAwaitingRefresh.value) return submitting.value ? "正在刷新…" : "重试刷新";
  if (!submitting.value) return "完成设置";
  return submittedInstallService.value ? "正在保存配置并安装服务…" : "正在保存配置…";
});

/** 该组件仅在本次打开周期保留草稿；Quick ↔ Advanced 切换绝不重置。 */
watch(
  () => props.open,
  (open, wasOpen) => {
    if (!open || wasOpen) return;
    advanced.value = false;
    submitting.value = false;
    submittedInstallService.value = false;
    savedAwaitingRefresh.value = false;
    savedAwaitingService.value = false;
    submitError.value = null;
    fieldErrors.value = {};
    draft.value = {
      ...createQuickStartDraft([]),
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
  draft.value[key] = (event.target as HTMLInputElement).value;
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

function saveAdvancedUiPreferences(pending: QuickStartDraft): Promise<boolean> {
  editUiPreference("launch_at_login", pending.launch_at_login === "true");
  editUiPreference("auto_connect_on_launch", pending.auto_connect_on_launch === "true");
  editUiPreference("minimize_to_tray_on_connect", pending.minimize_to_tray_on_connect === "true");
  editUiPreference("close_preference", pending.close_preference as ClosePreference);
  return updateUiPreferences(["launch_at_login", "auto_connect_on_launch", "minimize_to_tray_on_connect", "close_preference"]);
}

async function refreshSavedConfiguration(): Promise<void> {
  submitting.value = true;
  submitError.value = null;
  try {
    if (props.afterCoreSaved) {
      await props.afterCoreSaved();
    } else {
      await runtime?.configurationChanged({ strict: true });
    }
    emit("complete");
  } catch {
    submitError.value = "配置已保存，但主界面刷新失败；请重试刷新。";
  } finally {
    submitting.value = false;
  }
}

async function installSavedService(): Promise<void> {
  if (serviceOperations === null) {
    submitError.value = "配置已保存，但服务操作不可用，请重试。";
    return;
  }
  submitting.value = true;
  submitError.value = null;
  try {
    const reply = await serviceOperations.run("install");
    if (!reply.ok) {
      submitError.value = reply.message || "配置已保存，但安装服务失败，请重试安装。";
      return;
    }
    savedAwaitingService.value = false;
    savedAwaitingRefresh.value = true;
    await refreshSavedConfiguration();
  } catch {
    submitError.value = "配置已保存，但安装服务失败，请重试安装。";
  } finally {
    submitting.value = false;
  }
}

async function submit(): Promise<void> {
  if (submitting.value || appearance?.saving.value) return;
  if (savedAwaitingService.value) {
    await installSavedService();
    return;
  }
  if (savedAwaitingRefresh.value) {
    await refreshSavedConfiguration();
    return;
  }
  const errors = validateQuickStartDraft(draft.value);
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
  submittedInstallService.value = pending.install_service === "true";
  submitting.value = true;
  try {
    credentialConfigFor(configGateway).markSaved();
    const reply = await credentialConfigFor(configGateway).write(() => props.apply({
      items: quickStartItems(pending),
      // 服务操作由应用级协调器独立执行，避免后端组合命令绕开唯一 busy/overlay。
      install_service: false,
    }));
    if (!reply.ok) {
      submitError.value = reply.message ?? "快速入门提交失败，请检查设置后重试。";
      return;
    }
    if (!(await saveAdvancedUiPreferences(pending))) {
      submitError.value = "核心配置已保存，但前端偏好保存失败：" + Object.values(uiPrefErrors.value).filter(Boolean).join(" ") + "；草稿已保留，请重试。";
      return;
    }
    if (appearance?.motionDirty.value && !(await appearance.saveDraft(chrome ? (mode) => chrome.setMode(mode) : undefined, pendingAppearance))) {
      submitError.value = "核心配置已保存；" + (appearance.saveError.value ?? "外观保存失败，请重试。");
      return;
    }
    // 只用安全回读验证身份与记住标记；密码不写入错误或运行时投影。
    try {
      const items = await configGateway.configGet();
      credentialConfigFor(configGateway).apply(items);
      if (!quickStartCredentialsMatch(pending.username, String(pending.password.length > 0), items)) {
        submitError.value = "配置保存后的账户或记住密码状态未确认，请重试。";
        return;
      }
    } catch {
      submitError.value = "配置保存后回读失败，请重试。";
      return;
    }
    if (submittedInstallService.value) {
      savedAwaitingService.value = true;
      await installSavedService();
    } else {
      savedAwaitingRefresh.value = true;
      await refreshSavedConfiguration();
    }
  } catch {
    // 刻意不回显异常原文：apply 的传输错误文本可能内嵌密码（见
    // `QuickStartCredentials.test.ts` 的 `错误不携带凭据` 用例），因此这里必须保持
    // 与原因无关的固定文案。真实拒绝原因需从别处诊断。
    submitError.value = "快速入门提交失败，请重试。";
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <div
    v-if="open"
    class="quick-start-overlay"
    data-testid="quick-start-dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="quick-start-title"
  >
    <section
      class="quick-start-card"
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

      <section v-if="!advanced" :inert="formFrozen" class="quick-start-fields" data-testid="quick-start-basic-fields">
        <label>
          登录账户
          <input
            :value="draft.username"
            data-testid="quick-start-username"
            autocomplete="username"
            @input="updateText('username', $event)"
          >
          <span v-if="fieldErrors.username" class="field-error">{{ fieldErrors.username }}</span>
        </label>
        <label>
          密码
          <PasswordField
            :model-value="draft.password"
            data-testid="quick-start-password"
            @update:model-value="draft.password = $event; fieldErrors.password = undefined"
          />
          <span v-if="fieldErrors.password" class="field-error">{{ fieldErrors.password }}</span>
        </label>
        <label class="check-row">
          <input
            :checked="draft.install_service === 'true'"
            data-testid="quick-start-install-service"
            type="checkbox"
            @change="updateBoolean('install_service', $event)"
          >
          安装服务
        </label>
        <label>
          主题
          <select v-model="selectedTheme" data-testid="quick-start-theme">
            <option value="system">跟随系统</option>
            <option value="light">浅色</option>
            <option value="dark">深色</option>
          </select>
        </label>
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
          <label>
            登录账户
            <input :value="draft.username" data-testid="quick-start-username" autocomplete="username" @input="updateText('username', $event)">
            <span v-if="fieldErrors.username" class="field-error">{{ fieldErrors.username }}</span>
          </label>
          <label>
            密码
            <PasswordField :model-value="draft.password" data-testid="quick-start-password" @update:model-value="draft.password = $event; fieldErrors.password = undefined" />
            <span v-if="fieldErrors.password" class="field-error">{{ fieldErrors.password }}</span>
          </label>
          <label class="check-row"><input :checked="draft.install_service === 'true'" data-testid="quick-start-install-service" type="checkbox" @change="updateBoolean('install_service', $event)">安装服务</label>
          <label>
            主题
            <select v-model="selectedTheme" data-testid="quick-start-theme"><option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option></select>
          </label>
        </div>
        <div class="quick-start-advanced-column" data-testid="quick-start-advanced-column">
          <label>
            VPN 服务器
            <input :value="draft.server" data-testid="quick-start-server" @input="updateText('server', $event)">
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
          <RoutesEditor :routes="routes" @change="onRoutesChange" />
        </div>
      </section>

      <p v-if="submitError" class="submit-error" data-testid="quick-start-submit-error">{{ submitError }}</p>
      <footer class="quick-start-actions">
        <button type="button" data-testid="quick-start-skip" :disabled="submitting" @click="emit('skip')">暂时跳过</button>
        <button type="button" class="quick-start-submit" data-testid="quick-start-submit" :disabled="submitting" @click="submit">
          {{ submitLabel }}
        </button>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.quick-start-overlay { position: fixed; inset: 0; z-index: 60; display: grid; place-items: center; padding: max(var(--space-4), env(safe-area-inset-top)) max(var(--space-4), env(safe-area-inset-right)) max(var(--space-4), env(safe-area-inset-bottom)) max(var(--space-4), env(safe-area-inset-left)); background: rgb(0 0 0 / 0.5); }
.quick-start-card { box-sizing: border-box; width: min(100%, 620px); overflow: hidden; padding: var(--space-6); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: var(--surface-panel); color: var(--text-primary); box-shadow: 0 16px 48px rgb(0 0 0 / 0.32); transition: width 180ms ease; }
.quick-start-card--advanced { width: min(100%, 1180px); height: min(88dvh, 720px); display: flex; flex-direction: column; }
.quick-start-header { display: flex; min-width: 0; align-items: center; justify-content: space-between; gap: var(--space-4); }
.quick-start-card h2 { min-width: 0; margin: 0; overflow-wrap: anywhere; font-size: 24px; }
.quick-start-mode-control { display: inline-flex; flex: none; align-items: center; gap: 2px; padding: 2px; border: 1px solid var(--border-subtle); border-radius: 8px; background: color-mix(in srgb, var(--surface-canvas) 84%, transparent); }
.quick-start-mode-control__button { display: inline-flex; min-height: 28px; min-width: 48px; align-items: center; justify-content: center; padding: 0 var(--space-2); border: 0; border-radius: 6px; background: transparent; color: var(--text-secondary); font-size: 12px; line-height: 1; cursor: pointer; transition: background-color 120ms ease, color 120ms ease; }
.quick-start-mode-control__button:hover:not(:disabled) { color: var(--text-primary); }
.quick-start-mode-control__button--active { background: var(--accent-subtle); color: var(--accent-strong); }
.quick-start-mode-control__button:focus-visible { outline: 3px solid var(--focus-ring); outline-offset: -2px; }
.quick-start-fields, .quick-start-advanced { display: grid; gap: var(--space-3); margin-top: var(--space-5); }
.quick-start-advanced { padding-top: var(--space-4); border-top: 1px solid var(--border-subtle); }
.quick-start-advanced--three-columns { grid-template-columns: minmax(0, 1fr) minmax(0, .85fr) minmax(0, 1.15fr); column-gap: var(--space-5); flex: 1 1 auto; min-height: 0; }
.quick-start-advanced-column { display: grid; align-content: start; min-width: 0; gap: var(--space-3); }
.quick-start-route-column { min-height: 0; display: flex; flex-direction: column; }
.quick-start-route-column h3 { margin: 0; font-size: 16px; }
.quick-start-route-column > p { margin: calc(var(--space-1) * -1) 0 0; color: var(--text-secondary); font-size: 12px; line-height: 1.4; }
.quick-start-route-column :deep(.routes-editor) { flex: 1 1 auto; min-height: 0; }
.quick-start-route-column :deep(.routes-list) { max-height: none; }
label { display: grid; gap: var(--space-1); font-size: 13px; }
input, select { min-height: 36px; padding: 0 var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); background: var(--surface-subtle); color: var(--text-primary); }
.check-row { display: flex; align-items: center; gap: var(--space-2); }
.check-row input { min-height: auto; }
.routes-summary { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); }
.field-error, .submit-error { color: var(--state-danger); font-size: 12px; }
.submit-error { margin: var(--space-4) 0 0; }
.quick-start-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-5); }
.quick-start-submit { border-color: var(--accent); background: var(--accent); color: var(--accent-on); font-weight: 600; }
button:disabled { cursor: not-allowed; opacity: .65; }

@media (max-width: 760px) {
  .quick-start-card--advanced { width: min(100%, 620px); height: auto; max-height: min(88dvh, 720px); }
  .quick-start-advanced--three-columns { grid-template-columns: minmax(0, 1fr); overflow: hidden; }
  .quick-start-route-column { min-height: 280px; }
}

@media (max-width: 420px) {
  .quick-start-card { padding: var(--space-4); }
  .quick-start-header { align-items: flex-start; }
  .quick-start-mode-control__button { min-width: 42px; }
}

.motion-reduced .quick-start-card,
[data-motion="reduced"] .quick-start-card { transition: none; }
</style>
