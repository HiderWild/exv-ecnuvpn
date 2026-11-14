<script setup lang="ts">
import AuthPreparationModal from "./AuthPreparationModal.vue";
import { useAuthPreparation } from "../product/auth-preparation";
import { computed, inject, onMounted, onUnmounted, provide, ref, watch } from "vue";
import BrowserLoginWaitModal from "./BrowserLoginWaitModal.vue";
import ResolverConflictModal from "./ResolverConflictModal.vue";
import { onBrowserLogin, type BrowserLoginMilestone } from "../lib/ipc";
import { createBrowserLoginWait } from "../product/browser-login-wait";
import AuthenticationFailureModal from "./AuthenticationFailureModal.vue";
import CredentialRequiredModal from "./CredentialRequiredModal.vue";
import ServiceConnectFailureModal from "./ServiceConnectFailureModal.vue";
import { commandErrorMessage, isBrowserLoginCancelled } from "../lib/command-error";
import { pushToast } from "../lib/toast";
import { APPEARANCE_KEY } from "../product/appearance";
import { connectionActionFor, type ProductConnectionAction } from "../product/connection-action";
import { CONNECTION_INTERACTION_KEY } from "../product/connection-interaction";
import { CORE_CONFIG_GATEWAY_KEY, createCoreConfigGateway, credentialConfigFor, atrustAuthModeFromConfig, effectiveAtrustAuthMode, type AtrustAuthMode, type CoreConfigKey } from "../product/core-config";
import {
  installServiceOnConnect,
  setInstallServiceOnConnect,
} from "../product/install-service-preference";
import { clearedCredentialConfig, planMinimalCredentials, type MinimalCredentialDraft } from "../product/minimal-credentials";
import { PRODUCT_RUNTIME_KEY } from "../product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "../product/service-operations";
import { authModalDismissed } from "../product/ui-transient";

const emit = defineEmits<{ resolverConflictAdjust: [] }>();
const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const preparation = useAuthPreparation(() => finishLoginCancellation());
const preparationEvent = preparation.event;
const preparationBusy = preparation.busy;
const preparationError = preparation.error;
const appearance = inject(APPEARANCE_KEY, null);
const configGateway = inject(CORE_CONFIG_GATEWAY_KEY, null) ?? createCoreConfigGateway();
const state = computed(() => runtime?.state.value ?? null);
const actionBusy = ref(false);
const webLoginPending = ref(false);
// R3：与快速入门/连接页共用同一纯前端偏好键（默认 true）；用户切换经 setter 立即写回。
const autoInstallService = computed({
  get: () => installServiceOnConnect.value,
  set: (value: boolean) => setInstallServiceOnConnect(value),
});
const serviceModalVisible = ref(false);
const serviceModalError = ref("");
const serviceUpdateRequired = ref(false);
const credentialModalVisible = ref(false);
const schoolCredentialModal = ref(false);
const credentialModalError = ref("");
const credentialSubmitting = ref(false);
const credentialCancelling = ref(false);
const credentialModalResetKey = ref(0);
/** 统一凭据模态形态（Area F）：credentials = 账户密码双字段；password = 账户只读。 */
const credentialModalMode = ref<"credentials" | "password">("credentials");
/** 学校认证需要的完整账户密码说明。 */
const credentialModalNotice = ref("");
/** 只读账户投影来源（S1：表单/配置用户名，非会话投影）。 */
const credentialModalInitialUsername = ref("");
/**
 * 主动弹出标记（Area F 取消语义二分）：连接前预检打开的模态尚未发出连接，
 * 取消 = 直接关闭（不派 stop）；一旦提交发出连接或由错误驱动打开则置 false，
 * 取消回到既有「派 stop 等待 idle」语义。
 */
const credentialProactive = ref(false);
const userAuthenticationActive = ref(false);
/**
 * S3 终局穿透令牌：本地取消登录后，本次取消对应代次的终局事件允许穿透
 * retire 过滤进入收口（finishLoginCancellation 幂等）；新连接尝试
 * （connectForAuthentication）与新代活动事件都会清空该令牌，旧代终局
 * 不得反杀新会话。
 */
let loginCancelPierceGeneration: number | null = null;
const minimal = computed(() => appearance?.state.value.mode === "minimal");
const atrustAuthenticationFailure = computed(() => ["ERROR_CODE_ATRUST_SESSION_INVALIDATED", "ERROR_CODE_ATRUST_AUTH_REJECTED"].includes(state.value?.errorCode ?? "")
  || (["DARWIN_ATRUST_SESSION_INVALIDATED", "darwin/atrust_session_invalidated"].includes(state.value?.errorReason ?? "") || (state.value?.errorReason?.startsWith("DARWIN_ATRUST_AUTH_STAGE_") ?? false)));
let attemptAuthentication: { protocol: string; server: string; mode: AtrustAuthMode; autoLogin: boolean } | null = null;
async function authenticationConfig(): Promise<NonNullable<typeof attemptAuthentication>> {
  const items = await configGateway.configGet();
  const value = (key: string) => items.find((item) => item.key === key)?.value;
  const autoLogin = atrustAuthModeFromConfig(value("atrust_auth_mode")) === "headless";
  return { protocol: value("protocol") ?? "cisco", server: value("atrust_server") ?? "", mode: effectiveAtrustAuthMode(atrustAuthModeFromConfig(value("atrust_auth_mode")), autoLogin), autoLogin };
}
const authenticationFailure = computed(() => userAuthenticationActive.value && (state.value?.errorCode === "ERROR_CODE_UNAUTHORIZED" || atrustAuthenticationFailure.value)
  // 新认证尚未返回时，Core仍可能投影上次失败；不能用旧模态遮住本次等待入口。
  && !actionBusy.value && !webLoginPending.value
  && !authModalDismissed.value && !credentialModalVisible.value && !serviceModalVisible.value);
const serviceOperations = inject(
  SERVICE_OPERATION_KEY,
  runtime ? createServiceOperationCoordinator((action) => runtime.serviceControl(action)) : null,
);

/** 已保存密码存在状态独立于「记住」开关；各模式共享同一配置网关。 */
const credentialConfig = credentialConfigFor(configGateway);
const storedRememberPassword = computed(() => credentialConfig.state.value.remember);
const hasStoredPassword = computed(() => credentialConfig.state.value.stored);
let pendingForget: { operationId: string; revision: number } | null = null;
let connectAttempt = 0;
let loginCancellationNotified = false;
let pendingConnect: number | null = null;
let credentialBrowserOwner: { attempt: number; generation: number } | null = null;
let credentialAuthenticatedAttempt: number | null = null;
async function connectForAuthentication(...args: Parameters<NonNullable<typeof runtime>["connect"]>) {
  waitEvents.retire();
  ticketRefreshReason.value = undefined;
  loginCancelPierceGeneration = null;
  const attempt = connectAttempt;
  pendingConnect = attempt;
  try { return await runtime!.connect(...args); }
  finally { if (pendingConnect === attempt) pendingConnect = null; }
}
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

interface CredentialModalRequest {
  school?: boolean;
  /** credentials = 账户密码均缺；password = 仅缺密码（账户只读投影）。 */
  mode?: "credentials" | "password";
  /** 本次业务说明。 */
  notice?: string;
  /** 账户初值/只读投影来源；缺省回退会话投影 → 配置用户名。 */
  initialUsername?: string;
  /** 主动弹出（连接前预检）：取消直接关闭，不派 stop。 */
  proactive?: boolean;
}

function openCredentialModal(request: CredentialModalRequest = {}): void {
  userAuthenticationActive.value = true;
  schoolCredentialModal.value = request.school ?? false;
  credentialCancelling.value = false;
  credentialModalError.value = "";
  credentialModalMode.value = request.mode ?? "credentials";
  credentialModalNotice.value = request.notice ?? "";
  credentialModalInitialUsername.value = request.initialUsername
    ?? (state.value?.connectionInfo.account ?? credentialConfig.state.value.username ?? "");
  credentialProactive.value = request.proactive ?? false;
  credentialModalResetKey.value += 1;
  credentialModalVisible.value = true;
}

function finishCredentialCancellation(): void {
  userAuthenticationActive.value = false;
  credentialCancelling.value = false;
  credentialModalError.value = "";
  credentialModalResetKey.value += 1;
  credentialModalVisible.value = false;
}

/** 用户动作：极简表单带草稿，完整模式按钮不带参数。 */
async function runAction(draft?: MinimalCredentialDraft): Promise<void> {
  if (runtime === null || state.value === null || busy.value) return;
  const next = connectionActionFor(state.value);
  if (!next.enabled) return;
  const cancellingLogin = next.kind === "stop" && userAuthenticationActive.value && webLoginPending.value;
  actionBusy.value = true;
  pendingForget = null;
  connectAttempt += 1;
  const attempt = connectAttempt;
  waitEvents.retire();
  ticketRefreshReason.value = undefined;
  loginCancelPierceGeneration = null;
  if (next.kind === "connect") loginCancellationNotified = false;
  credentialBrowserOwner = null;
  credentialAuthenticatedAttempt = null;
  authModalDismissed.value = false;
  try {
    if (next.kind === "connect") {
      userAuthenticationActive.value = true;
      if (autoInstallService.value &&
        (state.value.service.status.kind === "not_installed" || runtime.serviceStatusReadError?.value)) {
        const current = await serviceOperations!.run("query");
        if (!current.ok) throw new Error(current.message || "服务状态读取失败，请重试。");
        if (current.service_status == null) throw new Error("服务状态回复缺少状态数据，请重试读取。");
        if (!current.service_status.installed) {
          const reply = await serviceOperations!.run("install");
          if (!reply.ok) throw new Error(reply.message || "服务安装失败，无法继续连接。");
        }
      }
      await connectWithDraft(draft);
    } else if (next.kind === "stop") {
      userAuthenticationActive.value = false;
      // S3 取消即决：取消登录等待不等停止回执，先本地收口（关模态、退休事件流）；
      // 停止命令转 best-effort，失败不把等待 UI 挂回去。
      if (cancellingLogin) finishLoginCancellation();
      await runtime.stop();
    }
  } catch (error) {
    if (attempt === connectAttempt) {
      if (next.kind === "stop") {
        // 停止失败不悬挂 UI：仅一次性提示实际错误（取消登录场景已先行收口）。
        pushToast(cancellingLogin ? "停止连接未完成：" + messageFor(error) : messageFor(error), "error");
      }
      else if (userAuthenticationActive.value) handleActionError(error, draft);
    }
  } finally {
    if (attempt === connectAttempt) actionBusy.value = false;
  }
}

/**
 * 极简三分支（T4）：勾选+密码 → persist=true；未勾选+密码 → persist=false 且成功后
 * 清除已保存密码；密码留空 → 不传凭据由 host 回落已保存密文。完整模式不带草稿，沿用
 * 原来的无参 connect。
 */
async function connectWithDraft(draft?: MinimalCredentialDraft): Promise<void> {
  if (runtime === null) return;
  attemptAuthentication = credentialConfig.authentication.value;
  if (attemptAuthentication?.protocol === "atrust" || draft === undefined) {
    await connectForAuthentication();
    return;
  }
  const plan = planMinimalCredentials(draft, { hasStoredPassword: hasStoredPassword.value, storedUsername: credentialConfig.state.value.username });
  if (plan.blocked) return;
  const attempt = connectAttempt;
  const revision = credentialConfig.savedRevision.value;
  if (plan.credentials?.persist) credentialConfig.markSaved();
  const operationId = plan.credentials?.persist
    ? await credentialConfig.write(() => connectForAuthentication(plan.credentials!))
    : await connectForAuthentication(plan.credentials ?? undefined);
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

function finishLoginCancellation(): void {
  userAuthenticationActive.value = false;
  webLoginPending.value = false;
  waitEvents.retire();
  ticketRefreshReason.value = undefined;
  // S3 终局穿透令牌：本次取消代次的终局事件可穿透 retire 进入收口。
  loginCancelPierceGeneration = waitEvents.generation >= 0 ? waitEvents.generation : null;
  authModalDismissed.value = true;
  credentialModalVisible.value = false;
  credentialModalError.value = "";
  credentialSubmitting.value = false;
  credentialModalResetKey.value += 1;
  if (!loginCancellationNotified) pushToast("您已取消登录", "info");
  loginCancellationNotified = true;
}

function handleActionError(error: unknown, draft?: MinimalCredentialDraft): void {
  if (isBrowserLoginCancelled(error)) { finishLoginCancellation(); return; }
  if (isCredentialRequiredError(error)) {
    if (attemptAuthentication?.protocol === "atrust" || credentialConfig.protocol.value === "atrust") {
      openCredentialModal({ school: true, mode: "credentials", initialUsername: "", notice: "本次学校认证需要完整账户和密码。" });
      return;
    }
    // 极简与完整模式统一弹凭据模态（Area F：极简不再有内联密码框）。
    // 账户取本次草稿/配置：有账户 → 仅密码形态；无账户 → 合并输入形态，
    // 合并形态预填会话投影的账户（既有「缺什么补什么」语义）。
    const draftUsername = draft?.username ?? credentialConfig.state.value.username;
    openCredentialModal({
      mode: draftUsername.trim() ? "password" : "credentials",
      initialUsername: draftUsername.trim() || (state.value?.connectionInfo.account ?? ""),
    });
    return;
  }
  if (isServiceRecoveryError(error)) {
    serviceUpdateRequired.value = typeof (error as { message?: unknown }).message === "string" && (error as { message: string }).message.startsWith("EXV_AUTH_RECEIVER_UPDATE_REQUIRED");
    serviceModalError.value = serviceUpdateRequired.value ? (error as { message: string }).message.replace(/^EXV_AUTH_RECEIVER_UPDATE_REQUIRED[：:]?/, "") : messageFor(error);
    serviceModalVisible.value = true;
    return;
  }
  pushToast(messageFor(error), "error");
}

/** 密码只在模态草稿与本次 runtime.connect 参数中存在；提交即一次性传输。 */
async function submitCredentials(credentials: { username: string; password: string; persist: boolean }): Promise<void> {
  if (!runtime || busy.value) return;
  userAuthenticationActive.value = true;
  credentialSubmitting.value = true;
  credentialModalError.value = "";
  // 提交即发出连接：取消语义从「主动弹出直接关」切换为「错误驱动」（派 stop 等 idle）。
  credentialProactive.value = false;
  connectAttempt += 1;
  const attempt = connectAttempt;
  loginCancellationNotified = false;
  credentialBrowserOwner = null;
  credentialAuthenticatedAttempt = null;
  const revision = credentialConfig.savedRevision.value;
  try {
    if (schoolCredentialModal.value) {
      // 学校显式凭据及persist交A2既有连接入口，不追加前端成功后密码清除。
      await connectForAuthentication(credentials);
    } else if (credentials.persist) {
      credentialConfig.markSaved();
      await credentialConfig.write(() => connectForAuthentication(credentials));
    } else {
      // D2（darwin 语义，M6 对应落点）：未勾选「记住密码」→ 本次一次性凭据，
      // 连接成功后清除已存密码（失败保留便于重试，经 pendingForget 收口）。
      const operationId = await connectForAuthentication(credentials);
      if (operationId && attempt === connectAttempt) {
        pendingForget = { operationId, revision };
        settlePendingForget();
      }
    }
    if (attempt !== connectAttempt) return;
    credentialModalResetKey.value += 1;
    credentialModalVisible.value = false;
  } catch (error) {
    if (attempt !== connectAttempt) return;
    if (isBrowserLoginCancelled(error)) { finishLoginCancellation(); return; }
    if (credentialAuthenticatedAttempt === attempt) handleActionError(error);
    else credentialModalError.value = messageFor(error);
  } finally {
    if (attempt === connectAttempt) { credentialBrowserOwner = null; credentialSubmitting.value = false; }
  }
}

async function clearSavedPassword(request: { username: string }): Promise<void> {
  if (credentialSubmitting.value || credentialCancelling.value) return;
  credentialSubmitting.value = true;
  credentialModalError.value = "";
  try {
    const updates: { key: CoreConfigKey; value: string }[] = [...clearedCredentialConfig()];
    if (request.username.trim() !== credentialConfig.state.value.username.trim()) updates.unshift({ key: "username", value: request.username.trim() });
    credentialConfig.markSaved();
    const saved = await credentialConfig.write(() => configGateway.configSet(updates));
    if (!saved) throw new Error("密码清除保存失败，请重试。");
    await credentialConfig.refresh();
    userAuthenticationActive.value = false;
    credentialModalVisible.value = false;
    credentialModalResetKey.value += 1;
    pushToast("已清除保存的密码，自动重连已关闭。", "success");
  } catch (error) { credentialModalError.value = messageFor(error); }
  finally { credentialSubmitting.value = false; }
}

function closeCredentialModal(): void {
  if (!credentialModalVisible.value || credentialSubmitting.value || credentialCancelling.value) return;
  userAuthenticationActive.value = false;
  // 主动弹出（连接前预检）：连接尚未发出，取消 = 直接关闭，不派 stop、不写配置。
  if (credentialProactive.value) {
    finishCredentialCancellation();
    return;
  }
  // 已无活动会话则无需再派 stop，避免等待不存在的终态事件。
  if (state.value?.status === "idle") {
    finishCredentialCancellation();
    return;
  }
  if (!runtime || state.value === null) {
    credentialModalError.value = "无法确认连接状态，暂不能取消。";
    return;
  }

  // S3 取消即决：立即关闭模态并收口等待状态；停止命令转 best-effort 事后动作，
  // 失败仅一次性提示实际错误，不再把失败挂回模态（取消不悬挂 UI）。
  finishCredentialCancellation();
  credentialCancelling.value = true;
  void runtime.stop()
    .catch((error: unknown) => {
      pushToast("已取消输入；停止连接未完成：" + messageFor(error), "error");
    })
    .finally(() => {
      credentialCancelling.value = false;
    });
}

/**
 * 认证失败模态「重新输入凭据」（G 闭环）：完整与极简统一打开凭据模态——
 * 重新输入入口沿用本次凭据模态。
 */
async function reenterCredentials(): Promise<void> {
  if (busy.value) return;
  userAuthenticationActive.value = true;
  if (atrustAuthenticationFailure.value) {
    const attempt = ++connectAttempt;
    loginCancellationNotified = false;
    credentialBrowserOwner = null;
    credentialAuthenticatedAttempt = null;
    actionBusy.value = true;
    authModalDismissed.value = true;
    try {
      const current = await authenticationConfig();
      if (attempt !== connectAttempt) return;
      if (!attemptAuthentication || attemptAuthentication.protocol !== "atrust" || current.protocol !== attemptAuthentication.protocol || current.server !== attemptAuthentication.server || current.mode !== attemptAuthentication.mode || current.autoLogin !== attemptAuthentication.autoLogin) {
        throw new Error("连接协议、网关或认证通道已变化，或连接来自上次启动。请关闭旧提示并重新连接。");
      }
      await connectForAuthentication(undefined, attemptAuthentication.server);
    } catch (error) {
      if (attempt !== connectAttempt) return;
      if (isBrowserLoginCancelled(error)) finishLoginCancellation();
      else if (isCredentialRequiredError(error)) handleActionError(error);
      else pushToast(messageFor(error), "error");
    }
    finally { if (attempt === connectAttempt) actionBusy.value = false; }
    return;
  }
  authModalDismissed.value = true;
  const username = (state.value?.connectionInfo.account ?? credentialConfig.state.value.username ?? "").trim();
  openCredentialModal({
    mode: username ? "password" : "credentials",
  });
}

function recoveryCredentialsRequired(): void {
  serviceModalVisible.value = false;
  if (attemptAuthentication?.protocol === "atrust" || credentialConfig.protocol.value === "atrust") {
    handleActionError({ kind: "credential_required" });
    return;
  }
  const username = credentialConfig.state.value.username.trim();
  openCredentialModal({ mode: username ? "password" : "credentials", initialUsername: username });
}

/** 极简连接前预检（M1）：缺凭据时主动弹出统一模态；取消 = 直接关闭，不派 stop。 */
function requestCredentialsFromMinimal(request: { missingUsername: boolean; username: string }): void {
  openCredentialModal({
    mode: request.missingUsername ? "credentials" : "password",
    initialUsername: request.username.trim() || credentialConfig.state.value.username,
    proactive: true,
  });
}

const webLoginMode = ref<AtrustAuthMode>("embedded");
const webLoginAutoLogin = ref<boolean | undefined>();
const webLoginMilestone = ref<BrowserLoginMilestone | undefined>();
const webLoginCancelling = ref(false);
const ticketRefreshReason = ref<"missing" | "expired">();
const waitEvents = createBrowserLoginWait();
let unlistenBrowser: (() => void | Promise<void>) | null = null;
let unmounted = false;
onMounted(async () => {
  try {
    const unlisten = await onBrowserLogin((event) => {
      const previousGeneration = waitEvents.generation;
      if (unmounted) return;
      // S3 终局事件穿透：未被 retire 的新鲜终局照常放行；本地取消后的本次代次
      // 终局穿透 retire 用于收口（finishLoginCancellation 幂等）。已被 retire 的
      // 旧代终局一律拒收，不得反杀新认证会话；活动事件仍需认证交互权限。
      if (!event.active && event.outcome !== undefined) {
        const fresh = event.generation > waitEvents.retired;
        const pierce = !fresh && pendingConnect === null && !userAuthenticationActive.value
          && loginCancelPierceGeneration !== null && event.generation === loginCancelPierceGeneration;
        if (!fresh && !pierce) return;
      } else if (!userAuthenticationActive.value) {
        return;
      }
      if (!waitEvents.accept(event)) return;
      if (event.active) loginCancelPierceGeneration = null;
      if (!event.active && event.outcome === "cancelled") { finishLoginCancellation(); return; }
      if (event.active && previousGeneration !== event.generation) {
        loginCancellationNotified = false;
        webLoginAutoLogin.value = waitEvents.recovery?.auto_login;
        webLoginMode.value = waitEvents.recovery?.mode ?? event.mode;
      } else if (event.active && event.channel_override) {
        webLoginAutoLogin.value = waitEvents.recovery?.auto_login;
        webLoginMode.value = waitEvents.recovery?.mode ?? event.mode;
      } else if (!waitEvents.recovery) {
        webLoginMode.value = event.mode;
      }
      if (event.active && credentialSubmitting.value && credentialModalVisible.value
        && pendingConnect === connectAttempt && credentialBrowserOwner === null) {
        credentialBrowserOwner = { attempt: connectAttempt, generation: event.generation };
      }
      if (!event.active && event.outcome === "authenticated" && credentialSubmitting.value
        && credentialBrowserOwner?.attempt === connectAttempt
        && credentialBrowserOwner.generation === event.generation && pendingConnect === connectAttempt) {
        credentialAuthenticatedAttempt = connectAttempt;
        credentialModalVisible.value = false;
        credentialModalError.value = "";
        credentialModalResetKey.value += 1;
        credentialBrowserOwner = null;
      }
      webLoginPending.value = event.active;
      webLoginMilestone.value = waitEvents.milestone;
      ticketRefreshReason.value = waitEvents.ticketRefreshReason;
    });
    if (unmounted) void unlisten(); else unlistenBrowser = unlisten;
  } catch { /* 预览不提供原生事件。 */ }
});
onUnmounted(() => { unmounted = true; void unlistenBrowser?.(); });
async function cancelWebLogin(): Promise<void> {
  if (!runtime || webLoginCancelling.value) return;
  webLoginCancelling.value = true;
  const attempt = ++connectAttempt;
  credentialBrowserOwner = null;
  pendingConnect = null;
  // S3 取消本地即决：立即收口本次登录等待（关模态、退休事件流），不等停止回执；
  // 停止命令转 best-effort 事后动作，失败仅一次性提示实际错误，不再悬挂模态。
  finishLoginCancellation();
  try {
    await runtime.stop();
  } catch (error) {
    if (attempt === connectAttempt) pushToast("停止连接未完成：" + messageFor(error), "error");
  } finally {
    webLoginCancelling.value = false;
    if (attempt === connectAttempt) actionBusy.value = false;
  }
}
const resolverConflictVisible = ref(false);
let dismissedResolverOperation: string | null | undefined;
watch(() => [state.value?.status, state.value?.operationId, state.value?.errorReason] as const,
  ([status, operationId, reason]) => {
    if (status !== "failed" || reason !== "DARWIN_ATRUST_RESOLVER_CONFLICT") { resolverConflictVisible.value = false; return; }
    if (operationId !== dismissedResolverOperation) resolverConflictVisible.value = true;
  });
function dismissResolverConflict(): void {
  dismissedResolverOperation = state.value?.operationId;
  resolverConflictVisible.value = false;
}
watch(() => state.value?.status, status => {
  if (status !== "connected" && status !== "stopping") return;
  if (!userAuthenticationActive.value) return;
  userAuthenticationActive.value = false;
  credentialBrowserOwner = null;
  credentialAuthenticatedAttempt = null;
  credentialSubmitting.value = false;
  credentialModalVisible.value = false;
  credentialModalError.value = "";
  credentialModalResetKey.value += 1;
  // Connected 退休认证交互权限；同次 RPC 返回仍负责按 operation/revision 清除一次性凭据。
  // Stop 继续替换操作代次，使旧连接结果不能影响后续会话。
  if (status === "stopping") connectAttempt += 1;
  actionBusy.value = false;
  pendingConnect = null;
  webLoginPending.value = false;
  waitEvents.retire();
  ticketRefreshReason.value = undefined;
}, { flush: "sync" });

provide(CONNECTION_INTERACTION_KEY, { state, action, busy, autoInstallService, runAction });
</script>

<template>
  <AuthPreparationModal :event="preparationEvent" :pending-action="preparation.pendingAction.value" :busy="preparationBusy" :error="preparationError" @action="preparation.respond" />
  <slot
    :state="state"
    :action="action"
    :busy="busy"
    :run="runAction"
    :minimal="minimal"
    :initial-username="state?.connectionInfo.protocol ? (state.connectionInfo.account || '') : credentialConfig.state.value.username"
    :initial-server="credentialConfig.state.value.server"
    :remember-password="storedRememberPassword"
    :has-stored-password="hasStoredPassword"
    :request-credentials="requestCredentialsFromMinimal"
    :protocol="credentialConfig.protocol.value"
  />
  <ServiceConnectFailureModal
    :visible="!preparationEvent && (serviceModalVisible)"
    :compact="minimal"
    :error-message="serviceModalError"
    :update-required="serviceUpdateRequired"
    @close="serviceModalVisible = false"
    @recovered="serviceModalVisible = false"
    @credential-required="recoveryCredentialsRequired"
  />
  <AuthenticationFailureModal
    :visible="!preparationEvent && (authenticationFailure)"
    :web-login="atrustAuthenticationFailure"
    :session-invalidated="state?.errorReason === 'DARWIN_ATRUST_SESSION_INVALIDATED'"
    :compact="minimal"
    @close="authModalDismissed = true"
    @reenter="reenterCredentials"
  />
  <CredentialRequiredModal
    :visible="!preparationEvent && (credentialModalVisible)"
    :compact="minimal"
    :busy="credentialSubmitting || credentialCancelling"
    :cancelling="credentialCancelling"
    :error-message="credentialModalError"
    :reset-key="credentialModalResetKey"
    :mode="credentialModalMode"
    :notice="credentialModalNotice"
    :has-saved-password="!schoolCredentialModal && hasStoredPassword"
    :initial-remember="!schoolCredentialModal && storedRememberPassword"
    :initial-username="credentialModalInitialUsername"
    @close="closeCredentialModal"
    @submit="submitCredentials"
    @clear-saved-password="clearSavedPassword"
  />
  <ResolverConflictModal :visible="!preparationEvent && (resolverConflictVisible)" :compact="minimal" :conflict="null"
    @close="dismissResolverConflict" @adjust="dismissResolverConflict(); emit('resolverConflictAdjust')" />
  <BrowserLoginWaitModal :visible="!preparationEvent && (userAuthenticationActive && webLoginPending)" :compact="minimal" :mode="webLoginMode" :auto-login="webLoginAutoLogin" :milestone="webLoginMilestone" :ticket-refresh-reason="ticketRefreshReason" :cancelling="webLoginCancelling"
    @cancel="cancelWebLogin" />
</template>
