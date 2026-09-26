<script setup lang="ts">
import { computed, inject, ref, watch } from "vue";

import type { ProductConnectionAction } from "../product/connection-action";
import type { MinimalCredentialDraft } from "../product/minimal-credentials";
import { planMinimalCredentials } from "../product/minimal-credentials";
import type { ProductUiState } from "../product/types";
import { useConnectionVisual } from "../product/connection-visual";
import {
  installServiceOnConnect,
  setInstallServiceOnConnect,
} from "../product/install-service-preference";
import { uiPreferencesState } from "../product/ui-prefs";
import MinimalTrafficSummary from "./MinimalTrafficSummary.vue";
import PasswordField from "./PasswordField.vue";
import { CORE_CONFIG_GATEWAY_KEY, createCoreConfigGateway, revealSavedPassword } from "../product/core-config";

/**
 * 极简连接视图：左列状态 + 矩形主按钮，右列两行表单（用户名/密码 + 服务/记住）。
 *
 * 依据用户逐字需求与 C++ 历史实现（`MinimalModeView.vue`）的左右两列布局重做，
 * **不移植其圆形电源按钮**（规格 §6.1 禁止居中巨大圆形按钮），改用矩形按钮。
 * 内容区硬预算 102px（`ui-first-shell.css`：136px 窗口 − 34px 标题栏），
 * 底部动效条紧贴物理底边。
 */
const props = withDefaults(defineProps<{
  state: ProductUiState;
  action: ProductConnectionAction;
  /** core 配置中的已有用户名（连接身份投影）。 */
  initialUsername?: string;
  initialServer?: string;
  /** core `remember_password === "true"` 的初始勾选值。 */
  rememberPassword?: boolean;
  /** Core 验证过的已保存凭据状态，不随本次勾选变化。 */
  hasStoredPassword?: boolean;
  /** 极简内联凭据错误（credential_required / 认证失败重新输入）。 */
  credentialError?: string | null;
}>(), {
  initialUsername: "",
  initialServer: "",
  rememberPassword: false,
  hasStoredPassword: false,
  credentialError: null as string | null,
});

const emit = defineEmits<{ run: [draft: MinimalCredentialDraft] }>();
const configGateway = inject(CORE_CONFIG_GATEWAY_KEY, null) ?? createCoreConfigGateway();

const { visualState, paused } = useConnectionVisual(() => props.state);
const connected = computed(() => props.state.status === "connected");
const availableMetrics = computed(() =>
  props.state.metrics?.availability === "available" ? props.state.metrics : null,
);
const connectionInfo = computed(() => props.state.connectionInfo);
const displayMetric = (value: string | null | undefined): string => value ?? "—";
/** 连接时延显示开关（已生效偏好）：关闭时不渲染时延（默认关闭）。 */
const showLatency = computed(() => uiPreferencesState().value.show_latency);

/** 「服务」勾选只在服务明确未安装且连接空闲时显示（与 ConnectPage 同一条件）。 */
const showServiceChoice = computed(() =>
  props.state.status === "idle" && props.state.service.status.kind === "not_installed",
);
/** 与连接页共用同一纯前端偏好键；不新建第二个键。 */
const autoInstallService = computed({
  get: () => installServiceOnConnect.value,
  set: (value: boolean) => setInstallServiceOnConnect(value),
});

const username = ref(props.initialUsername);
const password = ref("");
const remember = ref(props.rememberPassword);
const usernameTouched = ref(false);
const rememberTouched = ref(false);

// 配置身份/记住密码经异步 IPC 到达时同步初值；用户已经改过就不再覆盖。
watch(() => props.initialUsername, (value) => {
  if (!usernameTouched.value) username.value = value;
});
watch(() => props.rememberPassword, (value) => {
  if (!rememberTouched.value) remember.value = value;
});

const plan = computed(() => planMinimalCredentials(
  { username: username.value, password: password.value, remember: remember.value },
  { hasStoredPassword: props.hasStoredPassword, storedUsername: props.initialUsername },
));
const actionDisabled = computed(() => !props.action.enabled || (props.action.kind === "connect" && plan.value.blocked));
const matchingStoredPassword = computed(() => props.hasStoredPassword && username.value.trim() === props.initialUsername.trim());

function requestAction(): void {
  if (actionDisabled.value) return;
  emit("run", { username: username.value, password: password.value, remember: remember.value });
}
</script>

<template>
  <section
    class="minimal-connection-view"
    aria-label="极简连接"
    :data-visual-state="visualState"
    :data-paused="paused"
  >
    <div class="minimal-connection-view__body">
      <div class="minimal-connection-view__status">
        <h1
          :data-status="state.status"
          class="minimal-connection-view__title"
          data-testid="minimal-status"
          aria-live="polite"
          aria-atomic="true"
        >{{ state.title }}</h1>
        <button
          class="minimal-connection-view__action"
          data-testid="minimal-action"
          type="button"
          :disabled="actionDisabled"
          @click="requestAction"
        >
          {{ action.label }}
        </button>
      </div>

      <div v-if="connected" class="minimal-traffic" data-testid="minimal-traffic">
        <p class="minimal-traffic__line">
          <b>账号</b> {{ connectionInfo.account ?? "—" }}
          <span aria-hidden="true">·</span>
          <b>在线</b> {{ displayMetric(state.metrics?.online) }}
        </p>
        <MinimalTrafficSummary :metrics="availableMetrics" />
        <p v-if="showLatency" class="minimal-traffic__line">
          <b>连接时延</b> {{ displayMetric(availableMetrics?.latency) }}
        </p>
      </div>

      <form v-else class="minimal-form" @submit.prevent="requestAction">
        <div class="minimal-form__row">
          <input
            v-model="username"
            class="minimal-form__input"
            data-testid="minimal-username"
            type="text"
            autocomplete="username"
            placeholder="用户名"
            aria-label="用户名"
            @input="usernameTouched = true"
          >
          <label v-if="showServiceChoice" class="minimal-form__utility" data-testid="minimal-service-label">
            <input
              v-model="autoInstallService"
              class="minimal-form__checkbox"
              data-testid="minimal-service"
              type="checkbox"
            >
            服务
          </label>
        </div>
        <div class="minimal-form__row">
          <PasswordField
            v-model="password"
            class="minimal-form__password"
            data-testid="minimal-password"
            :stored="matchingStoredPassword"
            :identity="`${initialUsername}|${initialServer}`"
            :reveal-stored="() => revealSavedPassword(configGateway, initialUsername, initialServer)"
            compact
          />
          <label class="minimal-form__utility">
            <input
              v-model="remember"
              class="minimal-form__checkbox"
              data-testid="minimal-remember"
              type="checkbox"
              @change="rememberTouched = true"
            >
            记住
          </label>
        </div>
        <p v-if="credentialError" class="minimal-form__note minimal-form__note--error" data-testid="minimal-credential-error" role="alert">
          {{ credentialError }}
        </p>
      </form>
    </div>

    <div class="minimal-connection-view__activity">
      <span class="minimal-activity-beam" data-testid="minimal-activity-beam" aria-hidden="true" />
    </div>
    <div v-if="connected" class="minimal-watermark" data-testid="minimal-watermark" aria-hidden="true">
      <img src="../assets/exv-logo.svg" alt="" />
      <strong>EXV</strong>
    </div>
  </section>
</template>

<style scoped>
.minimal-connection-view {
  position: relative;
  display: grid;
  width: 328px;
  min-width: 328px;
  height: 102px;
  min-height: 102px;
  grid-template-rows: minmax(0, 1fr) auto;
  overflow: hidden;
  /* 底部不留内边距：动效条紧贴物理底边。 */
  padding: 6px 10px 0;
  color: var(--text-primary);
}

.minimal-connection-view__body {
  display: grid;
  min-width: 0;
  min-height: 0;
  grid-template-columns: 4.6rem minmax(0, 1fr);
  align-items: start;
  align-content: center;
  gap: 8px;
}

.minimal-connection-view__status {
  display: grid;
  min-width: 0;
  justify-items: stretch;
  grid-template-rows: 32px 32px;
  gap: 6px;
}

.minimal-connection-view__title {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  margin: 0;
  font-size: 13px;
  font-weight: 680;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.minimal-connection-view__title::before {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-tertiary);
  content: "";
}

.minimal-connection-view__title[data-status="connected"]::before { background: var(--state-success); }
.minimal-connection-view__title[data-status="connecting"]::before,
.minimal-connection-view__title[data-status="awaiting"]::before { background: var(--accent); }
.minimal-connection-view__title[data-status="failed"]::before { background: var(--state-danger); }

.minimal-connection-view__action {
  min-width: 72px;
  height: 32px;
  min-height: 32px;
  padding: 4px 8px;
  border: 1px solid var(--accent);
  border-radius: 7px;
  background: var(--accent);
  color: var(--accent-on);
  font-size: 13px;
  font-weight: 650;
  line-height: 1.1;
}

.minimal-connection-view__action:hover:not(:disabled) {
  border-color: var(--accent-strong);
  background: var(--accent-strong);
}

.minimal-connection-view__action:disabled {
  border-color: var(--border-subtle);
  background: var(--surface-subtle);
  color: var(--text-secondary);
}

.minimal-connection-view[data-visual-state="connected"] .minimal-connection-view__action {
  border-color: var(--border-strong);
  background: var(--surface-panel);
  color: var(--text-primary);
}

.minimal-form {
  display: grid;
  min-width: 0;
  gap: 6px;
}

.minimal-form__row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 6px;
}

.minimal-form__input {
  min-width: 0;
  height: 32px;
  min-height: 32px;
  flex: 1 1 auto;
  padding: 0 7px;
  border: 1px solid var(--border-strong);
  border-radius: 6px;
  background: var(--surface-raised);
  color: var(--text-primary);
  font-size: 12px;
  line-height: 1.2;
}

.minimal-form__utility {
  display: inline-flex;
  height: 32px;
  flex: 0 0 auto;
  align-items: center;
  gap: 4px;
  padding: 0 6px;
  border: 1px solid var(--border-strong);
  border-radius: 6px;
  background: var(--surface-raised);
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1;
  white-space: nowrap;
}

.minimal-form__checkbox {
  width: 12px;
  height: 12px;
  margin: 0;
  accent-color: var(--accent);
}
.minimal-form__password { flex:1 1 auto; }

.minimal-form__note {
  overflow: hidden;
  margin: 0;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.minimal-form__note--error { color: var(--state-danger); }

.minimal-traffic {
  display: grid;
  min-width: 0;
  gap: 3px;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.25;
}

.minimal-traffic__line {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  margin: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.minimal-traffic__line b {
  color: var(--text-tertiary);
  font-size: 12px;
  font-weight: 500;
}

.minimal-connection-view__activity {
  position: relative;
  height: 2px;
  overflow: hidden;
}

.minimal-activity-beam {
  display: block;
  width: 100%;
  height: 100%;
  background: linear-gradient(90deg, transparent 0%, var(--accent) 50%, transparent 100%);
  opacity: 0.5;
}

/* 连接中/断开中：光带流动；已连接：低频漂移；其余状态静止。 */
.minimal-connection-view[data-visual-state="working"] .minimal-activity-beam {
  animation: minimal-activity-flow 2.4s ease-in-out infinite;
}

.minimal-connection-view[data-visual-state="connected"] .minimal-activity-beam {
  animation: minimal-activity-drift 8.5s ease-in-out infinite;
}

.minimal-connection-view[data-paused="true"] .minimal-activity-beam {
  animation-play-state: paused;
}

@keyframes minimal-activity-flow {
  0% { transform: translateX(-18%); opacity: 0.28; }
  50% { transform: translateX(0); opacity: 0.82; }
  100% { transform: translateX(18%); opacity: 0.28; }
}

@keyframes minimal-activity-drift {
  0%, 100% { opacity: 0.34; }
  50% { opacity: 0.72; }
}

/* 产品内动效设置（appearance.motion）关闭动效；系统级 reduce 同为减速信号。
   本节沿用同文件与 C++ 参考的既有契约；不扩大 2026-09-12-product-motion-setting-only 范围。 */
:global(.motion-reduced) .minimal-activity-beam { animation: none !important; }
:global([data-motion="reduced"]) .minimal-activity-beam { animation: none !important; }

@media (prefers-reduced-motion: reduce) {
  .minimal-activity-beam { animation: none !important; }
}
.minimal-watermark {
  position: absolute;
  right: 10px;
  bottom: 8px;
  display: flex;
  align-items: center;
  gap: 4px;
  opacity: 0.75;
  pointer-events: none;
  user-select: none;
}

.minimal-watermark img {
  width: 14px;
  height: 14px;
}

.minimal-watermark strong {
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.08em;
  color: var(--text-tertiary);
}
</style>
