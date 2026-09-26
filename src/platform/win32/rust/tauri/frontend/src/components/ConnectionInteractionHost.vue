<script setup lang="ts">
import { computed, inject, provide, ref, watch } from "vue";
import AuthenticationFailureModal from "./AuthenticationFailureModal.vue";
import CredentialRequiredModal from "./CredentialRequiredModal.vue";
import ServiceConnectFailureModal from "./ServiceConnectFailureModal.vue";
import { commandErrorMessage } from "../lib/command-error";
import { pushToast } from "../lib/toast";
import { APPEARANCE_KEY } from "../product/appearance";
import { connectionActionFor, type ProductConnectionAction } from "../product/connection-action";
import { CONNECTION_INTERACTION_KEY } from "../product/connection-interaction";
import { CORE_CONFIG_GATEWAY_KEY, createCoreConfigGateway, credentialConfigFor } from "../product/core-config";
import {
  installServiceOnConnect,
  setInstallServiceOnConnect,
} from "../product/install-service-preference";
import { clearedCredentialConfig, planMinimalCredentials, type MinimalCredentialDraft } from "../product/minimal-credentials";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "../product/service-operations";
import { authModalDismissed } from "../product/ui-transient";

const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const appearance = inject(APPEARANCE_KEY, null);
const configGateway = inject(CORE_CONFIG_GATEWAY_KEY, null) ?? createCoreConfigGateway();
const state = computed(() => runtime?.state.value ?? null);
const actionBusy = ref(false);
// R3：与快速入门/连接页共用同一纯前端偏好键（默认 true）；用户切换经 setter 立即写回。
const autoInstallService = computed({
  get: () => installServiceOnConnect.value,
  set: (value: boolean) => setInstallServiceOnConnect(value),
});
const serviceModalVisible = ref(false);
const serviceModalError = ref("");
const credentialModalVisible = ref(false);
const credentialModalError = ref("");
const credentialSubmitting = ref(false);
const credentialCancelling = ref(false);
const credentialStopReturned = ref(false);
const credentialModalResetKey = ref(0);
const minimal = computed(() => appearance?.state.value.mode === "minimal");
const authenticationFailure = computed(() => state.value?.errorCode === "ERROR_CODE_UNAUTHORIZED"
  && !authModalDismissed.value && !credentialModalVisible.value && !serviceModalVisible.value);
const serviceOperations = inject(
  SERVICE_OPERATION_KEY,
  runtime ? createServiceOperationCoordinator((action) => runtime.serviceControl(action)) : null,
);

/** 已保存密码存在状态独立于「记住」开关；各模式共享同一配置网关。 */
const credentialConfig = credentialConfigFor(configGateway);
const storedRememberPassword = computed(() => credentialConfig.state.value.remember);
const hasStoredPassword = computed(() => credentialConfig.state.value.stored);
const credentialError = ref<string | null>(null);
let pendingForget: { operationId: string; revision: number } | null = null;
let connectAttempt = 0;
function settlePendingForget(): void {
  const pending = pendingForget;
  if (!pending || state.value?.operationId !== pending.operationId) return;
  if (state.value.status === "connected") {
    pendingForget = null;
    if (credentialConfig.savedRevision.value === pending.revision) void clearStoredCredentials(pending.revision);
  } else if (["failed", "idle", "stopping", "reconciling"].includes(state.value.status)) {
    pendingForget = null;
  }
}
watch(() => [state.value?.status, state.value?.operationId], settlePendingForget);
watch(minimal, () => { void credentialConfig.refresh(); }, { immediate: true });
watch(() => state.value?.status, (status) => { if (status === "connected") void credentialConfig.refresh(); });

const busy = computed(() => actionBusy.value || credentialSubmitting.value || credentialCancelling.value || serviceOperations?.busy.value === true);
const action = computed<ProductConnectionAction>(() => {
  const base = state.value ? connectionActionFor(state.value) : { kind: "none" as const, label: "处理中" as const, enabled: false };
  return { ...base, enabled: base.enabled && !busy.value };
});

/** 切到极简时把已打开的凭据模态收敛为表单内联错误；服务/认证失败模态在窗内紧凑渲染。 */
watch(minimal, (isMinimal) => {
  if (!isMinimal || !credentialModalVisible.value) return;
  credentialModalVisible.value = false;
  setMinimalCredentialError("需要账户和密码，请直接在极简表单中填写。");
});

function setMinimalCredentialError(message: string): void {
  credentialError.value = message;
}

function isServiceRecoveryError(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const { kind, message } = error as { kind?: unknown; message?: unknown };
  if (kind === "service_not_running" || kind === "service_connect_failed") return true;
  return typeof message === "string"
    && (message.includes("service_not_running|") || message.includes("service_connect_failed|"));
}

function isCredentialRequiredError(error: unknown): boolean {
  return typeof error === "object" && error !== null
    && (error as { kind?: unknown }).kind === "credential_required";
}

function messageFor(error: unknown): string {
  return commandErrorMessage(error, "操作未完成，请稍后重试。");
}

function openCredentialModal(): void {
  credentialCancelling.value = false;
  credentialStopReturned.value = false;
  credentialModalError.value = "";
  credentialModalResetKey.value += 1;
  credentialModalVisible.value = true;
}

function finishCredentialCancellation(): void {
  credentialCancelling.value = false;
  credentialStopReturned.value = false;
  credentialModalError.value = "";
  credentialModalResetKey.value += 1;
  credentialModalVisible.value = false;
}

function settleCredentialCancellation(): void {
  if (credentialCancelling.value && credentialStopReturned.value && state.value?.status === "idle") {
    finishCredentialCancellation();
  }
}

watch(() => state.value?.status, settleCredentialCancellation);

/** 用户动作：极简表单带草稿，完整模式按钮不带参数。 */
async function runAction(draft?: MinimalCredentialDraft): Promise<void> {
  if (runtime === null || state.value === null || busy.value) return;
  const next = connectionActionFor(state.value);
  if (!next.enabled) return;
  actionBusy.value = true;
  pendingForget = null;
  connectAttempt += 1;
  authModalDismissed.value = false;
  credentialError.value = null;
  try {
    if (next.kind === "connect") {
      if (autoInstallService.value && state.value.service.status.kind === "not_installed") {
        const reply = await serviceOperations!.run("install");
        if (!reply.ok) throw new Error(reply.message || "服务安装失败，无法继续连接。");
      }
      await connectWithDraft(draft);
    } else if (next.kind === "stop") {
      await runtime.stop();
    }
  } catch (error) {
    handleActionError(error, draft !== undefined);
  } finally {
    actionBusy.value = false;
  }
}

/**
 * 极简三分支（T4）：勾选+密码 → persist=true；未勾选+密码 → persist=false 且成功后
 * 清除已保存密码；密码留空 → 不传凭据由 host 回落已保存密文。完整模式不带草稿，沿用
 * 原来的无参 connect。
 */
async function connectWithDraft(draft?: MinimalCredentialDraft): Promise<void> {
  if (runtime === null) return;
  if (draft === undefined) {
    await runtime.connect();
    return;
  }
  const plan = planMinimalCredentials(draft, { hasStoredPassword: hasStoredPassword.value, storedUsername: credentialConfig.state.value.username });
  if (plan.blocked) return;
  const attempt = connectAttempt;
  const revision = credentialConfig.savedRevision.value;
  if (plan.credentials?.persist) credentialConfig.markSaved();
  const operationId = plan.credentials?.persist
    ? await credentialConfig.write(() => runtime.connect(plan.credentials!))
    : await runtime.connect(plan.credentials ?? undefined);
  credentialError.value = null;
  if (plan.clearStoredAfterSuccess && operationId && attempt === connectAttempt) {
    pendingForget = { operationId, revision };
    settlePendingForget();
  }
}

/** 清除只在连接成功之后执行（不可逆操作；失败时保留已保存密码便于重试）。 */
async function clearStoredCredentials(revision: number): Promise<void> {
  if (configGateway === null) return;
  try {
    const ok = await credentialConfig.write(async () => {
      if (credentialConfig.savedRevision.value !== revision) return null;
      return configGateway.configSet([...clearedCredentialConfig()]);
    });
    if (ok === null) return;
    if (!ok) throw new Error("配置写入被拒绝");
    credentialConfig.markSaved();
    await credentialConfig.refresh();
  } catch (error) {
    // 清除失败必须可见，不能静默。
    pushToast("未能清除已保存的密码：" + messageFor(error), "error");
  }
}

function handleActionError(error: unknown, fromMinimalForm: boolean): void {
  if (isCredentialRequiredError(error)) {
    if (fromMinimalForm) credentialError.value = "需要账户和密码，请直接在极简表单中填写。";
    else openCredentialModal();
    return;
  }
  if (isServiceRecoveryError(error)) {
    serviceModalError.value = messageFor(error);
    serviceModalVisible.value = true;
    return;
  }
  pushToast(messageFor(error), "error");
}

/** 密码只在极简表单草稿与本次 runtime.connect 参数中存在。 */
async function submitCredentials(credentials: { username: string; password: string; persist: boolean }): Promise<void> {
  if (!runtime || busy.value) return;
  credentialSubmitting.value = true;
  credentialModalError.value = "";
  try {
    if (credentials.persist) {
      credentialConfig.markSaved();
      await credentialConfig.write(() => runtime.connect(credentials));
    } else {
      await runtime.connect(credentials);
    }
    credentialModalResetKey.value += 1;
    credentialModalVisible.value = false;
  } catch (error) {
    credentialModalError.value = messageFor(error);
  } finally {
    credentialSubmitting.value = false;
  }
}

function closeCredentialModal(): void {
  if (!credentialModalVisible.value || credentialSubmitting.value || credentialCancelling.value) return;
  // 已无活动会话则无需再派 stop，避免等待不存在的终态事件。
  if (state.value?.status === "idle") {
    finishCredentialCancellation();
    return;
  }
  if (!runtime || state.value === null) {
    credentialModalError.value = "无法确认连接状态，暂不能取消。";
    return;
  }

  credentialCancelling.value = true;
  credentialStopReturned.value = false;
  credentialModalError.value = "";
  void runtime.stop()
    .then(() => {
      credentialStopReturned.value = true;
      settleCredentialCancellation();
    })
    .catch((error: unknown) => {
      credentialCancelling.value = false;
      credentialStopReturned.value = false;
      credentialModalError.value = messageFor(error);
    });
}

/** 认证失败模态：极简下“重新输入”转表单内联，完整模式仍打开凭据模态。 */
function reenterCredentials(): void {
  authModalDismissed.value = true;
  if (minimal.value) {
    setMinimalCredentialError("请重新输入凭据后再连接。");
    return;
  }
  openCredentialModal();
}

function recoveryCredentialsRequired(): void {
  serviceModalVisible.value = false;
  if (minimal.value) {
    setMinimalCredentialError("恢复连接需要账户和密码，请直接在极简表单中填写。");
    return;
  }
  openCredentialModal();
}

provide(CONNECTION_INTERACTION_KEY, { state, action, busy, autoInstallService, runAction });
</script>

<template>
  <slot
    :state="state"
    :action="action"
    :busy="busy"
    :run="runAction"
    :minimal="minimal"
    :initial-username="credentialConfig.state.value.username || state?.connectionInfo.account || ''"
    :initial-server="credentialConfig.state.value.server"
    :remember-password="storedRememberPassword"
    :has-stored-password="hasStoredPassword"
    :credential-error="credentialError"
  />
  <ServiceConnectFailureModal
    :visible="serviceModalVisible"
    :compact="minimal"
    :error-message="serviceModalError"
    @close="serviceModalVisible = false"
    @recovered="serviceModalVisible = false"
    @credential-required="recoveryCredentialsRequired"
  />
  <AuthenticationFailureModal
    :visible="authenticationFailure"
    :compact="minimal"
    @close="authModalDismissed = true"
    @reenter="reenterCredentials"
  />
  <CredentialRequiredModal
    :visible="credentialModalVisible && !minimal"
    :compact="minimal"
    :busy="credentialSubmitting || credentialCancelling"
    :cancelling="credentialCancelling"
    :error-message="credentialModalError"
    :reset-key="credentialModalResetKey"
    :initial-username="state?.connectionInfo.account ?? ''"
    @close="closeCredentialModal"
    @submit="submitCredentials"
  />
</template>
