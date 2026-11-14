<script setup lang="ts">
import AuthPreparationModal from "./AuthPreparationModal.vue";
import { useAuthPreparation } from "../product/auth-preparation";
import { computed, inject, onBeforeUnmount, provide, ref, watch } from "vue";
import AuthenticationFailureModal from "./AuthenticationFailureModal.vue";
import BrowserAuthWaitingModal from "./BrowserAuthWaitingModal.vue";
import CredentialRequiredModal from "./CredentialRequiredModal.vue";
import ServiceConnectFailureModal from "./ServiceConnectFailureModal.vue";
import {
  BROWSER_AUTH_MANUAL_STAGE,
  browserAuthIntervention,
  commandErrorMessage,
  isBrowserLoginCancelled,
  type BrowserAuthChannel,
  type BrowserAuthIntervention,
} from "../lib/command-error";
import { pushToast } from "../lib/toast";
import { APPEARANCE_KEY } from "../product/appearance";
import { connectionActionFor, type ProductConnectionAction } from "../product/connection-action";
import { CONNECTION_INTERACTION_KEY } from "../product/connection-interaction";
import { handleCoreWakeFailure } from "../product/core-crash";
import { CORE_CONFIG_GATEWAY_KEY, createCoreConfigGateway, credentialConfigFor, type CoreConfigKey } from "../product/core-config";
import {
  installServiceOnConnect,
  setInstallServiceOnConnect,
} from "../product/install-service-preference";
import { clearedCredentialConfig, planMinimalCredentials, type MinimalCredentialDraft } from "../product/minimal-credentials";
import {
  PENDING_ACTION_TIMEOUT_ONESHOT_MS,
  PENDING_ACTION_TIMEOUT_SERVICE_MS,
  PRODUCT_RUNTIME_KEY,
} from "../product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "../product/service-operations";
import { authModalDismissed } from "../product/ui-transient";

const runtime = inject(PRODUCT_RUNTIME_KEY, null);
const preparation = useAuthPreparation(() => finishLoginCancellation());
const preparationEvent = preparation.event;
const preparationBusy = preparation.busy;
const preparationError = preparation.error;
const appearance = inject(APPEARANCE_KEY, null);
const configGateway = inject(CORE_CONFIG_GATEWAY_KEY, null) ?? createCoreConfigGateway();
const state = computed(() => runtime?.state.value ?? null);
const actionBusy = ref<"connect" | "stop" | null>(null);
let actionSequence = 0;
/**
 * 修复 3b 兜底（2026-10-08 UI 重试脱同步）：`actionBusy` 看门狗——命令 Promise 因
 * IPC 异常悬挂时，busy 最坏 30/90s 自愈（档位与 runtime 的
 * `PENDING_ACTION_TIMEOUT_*` 同源：服务已装/在服务模式走 90s，其余 30s）。迟到的
 * 命令拒绝仍按 sequence 守卫处理（不清除后续操作的 busy）；正常路径 finally 先
 * 解除。状态侧不受影响——呈现的「连接中/等待浏览器登录」由 runtime 的命令在飞
 * 时钟负责（修复 3a），本看门狗只兜按钮互斥。
 */
let actionBusyWatchdog: ReturnType<typeof setTimeout> | null = null;
function armActionBusyWatchdog(kind: "connect" | "stop"): void {
  disarmActionBusyWatchdog();
  const servicePath = kind === "connect"
    && state.value?.service.status.kind === "installed";
  const timeoutMs = servicePath ? PENDING_ACTION_TIMEOUT_SERVICE_MS : PENDING_ACTION_TIMEOUT_ONESHOT_MS;
  actionBusyWatchdog = setTimeout(() => {
    actionBusyWatchdog = null;
    if (actionBusy.value === null) return;
    actionBusy.value = null;
  }, timeoutMs);
}
function disarmActionBusyWatchdog(): void {
  if (actionBusyWatchdog === null) return;
  clearTimeout(actionBusyWatchdog);
  actionBusyWatchdog = null;
}
onBeforeUnmount(disarmActionBusyWatchdog);
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
const credentialModalResetKey = ref(0);
/** 统一凭据模态形态（Area F）：credentials = 账户密码双字段；password = 账户只读。 */
const credentialModalMode = ref<"credentials" | "password">("credentials");
/** 只读账户投影来源（S1：表单/配置用户名，非会话投影）。 */
const credentialModalInitialUsername = ref("");
/**
 * 主动弹出标记（Area F 取消语义二分）：连接前预检/设置入口打开的模态尚未发出连接，
 * 取消 = 直接关闭（不派 stop）；一旦提交发出连接或由错误驱动打开则置 false，
 * 取消回到既有「派 stop 等待 idle」语义。
 */
const credentialProactive = ref(false);
const userAuthenticationActive = ref(false);
const minimal = computed(() => appearance?.state.value.mode === "minimal");
const authenticationFailure = computed(() => userAuthenticationActive.value && state.value?.errorCode === "ERROR_CODE_UNAUTHORIZED"
  && !authModalDismissed.value && !credentialModalVisible.value && !serviceModalVisible.value && authFailure.value === null);
const serviceOperations = inject(
  SERVICE_OPERATION_KEY,
  runtime ? createServiceOperationCoordinator((action) => runtime.serviceControl(action)) : null,
);

/** 已保存密码存在状态独立于「记住」开关；各模式共享同一配置网关。 */
const credentialConfig = credentialConfigFor(configGateway);
const storedRememberPassword = computed(() => credentialConfig.state.value.remember);
const hasStoredPassword = computed(() => credentialConfig.state.value.stored);
/**
 * 分支 2「成功后清除」（2026-10-26 验收计划 S5a 已裁定项 2，与 darwin 同构）：
 * 未勾选记住 + 本次密码连接时武装本次操作的待清除凭据；仅在本次连接到达
 * connected 时执行清除，失败/取消/新操作丢弃——尝试失败期间保留已保存密码，
 * 失败重试更友好（推翻原 D2「尝试连接时立即清除」）。
 */
let pendingForget: { operationId: string; revision: number } | null = null;
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
/**
 * bug3（EXV_UNFREEZE 2026-10-04）：会话活动时，极简页的协议/认证 UI 与表单初值改用
 * 会话身份投影（受理时冻结），不再随设置页保存而改写；非活动相位回落当前配置。
 * `connectionInfo.protocol` 只在会话活动时由 runtime 投影写入，因此它就是活动判别。
 */
const sessionProtocol = computed(() => state.value?.connectionInfo.protocol ?? null);
const minimalProtocol = computed(() => sessionProtocol.value ?? credentialConfig.state.value.protocol);
const initialUsername = computed(() => sessionProtocol.value
  ? (state.value?.connectionInfo.account ?? "")
  : (credentialConfig.state.value.username || state.value?.connectionInfo.account || ""));
const initialServer = computed(() => sessionProtocol.value
  ? (state.value?.connectionInfo.vpnServer ?? "")
  : credentialConfig.state.value.server);
/** §7.5：浏览器认证失败（三个页面分类码或既有认证失败类）→ 三选项模态。 */
const authFailure = ref<BrowserAuthIntervention | null>(null);
const loginCancelled = ref(false);
/** 重试需要重新发起「本次连接」，因此保留最后一次极简草稿（完整模式为 undefined）。 */
let lastConnectDraft: MinimalCredentialDraft | undefined;
watch(minimal, () => { void credentialConfig.refresh(); }, { immediate: true });
watch(() => state.value?.status, (status) => { if (status === "connected") void credentialConfig.refresh(); });

/**
 * §7.10-3 渠道边界：本次认证实际走的渠道（内嵌 / 系统浏览器 / 无头 / 内嵌回退到
 * 系统浏览器）。依据运行时可得状态推导：配置的认证方式 + 是否开启自动登录
 * （非 headless 时开启即被 Rust 强制内嵌）+ 本次是否发生过「使用系统浏览器」回退。
 * 不新增状态机。
 *
 * E12（阶段②附带）三值渠道：headless 配置如实返回 "headless"——与壳侧
 * `resolved_atrust_auth_mode` 的「headless + autoLogin 保持 headless」裁定对齐
 * （自动登录分支仅在非 headless 时强制内嵌）。
 */
const authChannel = ref<BrowserAuthChannel>("embedded");
function resolveAuthChannel(): BrowserAuthChannel {
  const cfg = credentialConfig.state.value;
  // headless 渠道优先如实上报（含开启自动登录的形态——无头即本次连接的实际渠道）。
  if (cfg.authMode === "headless") return "headless";
  // 勾选「尝试自动登录」⇒ Rust 强制内嵌（connect.atrust_auth_mode_forced），前端同判据。
  return cfg.authMode === "browser" ? "system-browser" : "embedded";
}

/**
 * §7.9 自动登录专用模态：已开启自动登录且本次连接确实由浏览器认证驱动时，
 * 用「正在为您进行登录操作 + 实时阶段」模态；需要人工时由 awaiting_manual 阶段切回
 * 既有等待模态（不再新增状态机，仅据既有信号与宿主 stage 推导）。
 */
const autoLoginConfigured = computed(() => (runtime?.browserLoginRecovery?.value?.mode ?? credentialConfig.state.value.authMode) === "headless");
const autoLoginStarted = ref(false);
let credentialAttempt = 0;
let credentialBrowserOwner: { attempt: number; generation: number } | null = null;
let credentialAuthenticatedAttempt: number | null = null;
watch(() => runtime?.browserLoginGeneration?.value, (generation, previous) => {
  if (generation != null && generation !== previous) {
    loginCancelled.value = false;
  }
  if (generation != null && credentialSubmitting.value && credentialModalVisible.value && credentialBrowserOwner === null) {
    credentialBrowserOwner = { attempt: credentialAttempt, generation };
  }
}, { flush: "sync" });
watch(() => runtime?.browserAuthenticated?.value, success => {
  if (!success || !credentialSubmitting.value || !credentialModalVisible.value
    || credentialBrowserOwner?.attempt !== credentialAttempt || credentialBrowserOwner.generation !== success.generation) return;
  credentialAuthenticatedAttempt = credentialAttempt;
  credentialBrowserOwner = null;
  credentialModalVisible.value = false;
  credentialModalError.value = "";
  credentialModalResetKey.value += 1;
}, { flush: "sync" });
watch(() => runtime?.browserLoginCancelled?.value, cancelled => {
  if (cancelled && userAuthenticationActive.value) finishLoginCancellation();
}, { flush: "sync" });
const autoLoginManual = computed(() => runtime?.browserLoginStage?.value === BROWSER_AUTH_MANUAL_STAGE);
watch(() => state.value?.status, (status) => {
  if (status === "connected" || status === "idle" || status === "stopping" || status === "failed") {
    autoLoginStarted.value = false;
  }
});

const fallbackAvailable = ref(false);
const fallbackBusy = ref(false);
const fallbackError = ref("");
const browserLoginActive = computed(() => userAuthenticationActive.value && runtime?.browserLoginActive?.value === true
  && state.value?.status === "connecting");
let fallbackQuery = 0;
watch([browserLoginActive, () => runtime?.browserLoginGeneration?.value, () => runtime?.browserLoginRecovery?.value?.mode, () => runtime?.browserLoginRecovery?.value?.auto_login], async ([active]) => {
  const query = ++fallbackQuery;
  fallbackAvailable.value = false;
  fallbackError.value = "";
  if (!active) return;
  // 后台认证使用原请求冻结渠道；普通 Connect 沿原配置判定，并登记本次运行。
  // 配置可能仍在异步加载（autoLoginConfigured 稍后为真）；此处只登记「本次由浏览器认证驱动」，
  // 是否显示自动登录模态再由 autoLoginConfigured 门控，避免漏判。
  const recovery = runtime?.browserLoginRecovery?.value;
  authChannel.value = recovery
    ? (recovery.mode === "browser" ? "system-browser" : recovery.mode)
    : resolveAuthChannel();
  autoLoginStarted.value = true;
  if (!runtime?.atrustBrowserFallbackAvailable) return;
  try {
    const available = await runtime.atrustBrowserFallbackAvailable();
    if (query === fallbackQuery) fallbackAvailable.value = browserLoginActive.value && available;
  } catch {
    if (query === fallbackQuery) fallbackAvailable.value = false;
  }
}, { immediate: true });

async function useSystemBrowserFallback(): Promise<void> {
  if (!fallbackAvailable.value || fallbackBusy.value || !browserLoginActive.value
    || !runtime?.atrustBrowserFallback) return;
  fallbackBusy.value = true;
  fallbackError.value = "";
  try {
    await runtime.atrustBrowserFallback();
    fallbackAvailable.value = false;
    // 回退到系统浏览器后即纯手动渠道：不再显示自动登录阶段模态，错误态也不再给「手动介入」。
    authChannel.value = "system-browser";
    autoLoginStarted.value = false;
  } catch {
    fallbackError.value = "无法切换到系统浏览器，请取消本次登录后重试。";
  } finally {
    fallbackBusy.value = false;
  }
}

/** §7.9：自动登录阶段——认证中取宿主 stage，认证完成进入连接相位即「正在建立连接」。 */
const autoLoginStage = computed<string | null>(() =>
  browserLoginActive.value ? (runtime?.browserLoginStage?.value ?? null) : "connecting");

/** 自动登录模态可见：已开启、本次确实启动、未转人工、未失败，且仍在认证/连接相位。 */
const autoLoginVisible = computed(() => autoLoginConfigured.value
  && autoLoginStarted.value
  && !autoLoginManual.value
  && authFailure.value === null
  && browserLoginActive.value);

/** §7.5：等待认证与认证失败三选项共用同一模态（两种连接模式同源）。 */
const authWaitVisible = computed(() => !loginCancelled.value && (browserLoginActive.value || authFailure.value !== null || autoLoginVisible.value));
/** 验证码风险预告：随本次认证开始事件到达 runtime 的频次数字（认证结束/新连接清空）；
 * 模态可见期间持续渲染（标志存续于整个认证等待期，不依赖事件与可见性的先后）。 */
const loginThrottleWarning = computed(() => runtime?.loginThrottleWarning?.value ?? null);
const authModalMode = computed<"auto-login" | "waiting" | "error">(() => {
  if (authFailure.value) return "error";
  if (autoLoginVisible.value) return "auto-login";
  return "waiting";
});
const authWaitBusy = computed(() => authFailure.value !== null
  ? (actionBusy.value !== null || fallbackBusy.value)
  : (fallbackBusy.value || actionBusy.value === "stop"));

/** 重试 = 重新发起本次连接（沿用最后一次极简草稿）。 */
function retryAuthIntervention(): void {
  if (actionBusy.value !== null) return;
  const draft = lastConnectDraft;
  authFailure.value = null;
  void runAction(draft);
}

/** 取消 = 中止本次连接（等同 Stop；取消后不自行连接）。 */
function cancelAuthIntervention(): void {
  authFailure.value = null;
  if (state.value && connectionActionFor(state.value).kind === "stop") void runAction();
  else userAuthenticationActive.value = false;
}

/**
 * 修复 3b（2026-10-08 UI 重试脱同步）：主按钮的唯一决策入口。
 *
 * 连接命令在飞期间（actionBusy === "connect"），**任意相位**都呈现并派发「取消」
 * （Stop）——Rust stop 先 `cancel_with_completion` 取消在飞认证并等待收口
 * （client.rs，含 5s 超时保护），被取消的旧 connect 拒绝由 runAction 的 sequence
 * 守卫吞掉，不弹错误模态。idle/failed 相位由此获得取消出口，「未连接+灰按钮」
 * 的死锁形态（根因 B）不再存在；状态侧的「连接中/等待浏览器登录」强制项由
 * runtime 的命令在飞时钟保证（修复 3a）。
 */
const effectiveAction = computed<ProductConnectionAction>(() => {
  const base = state.value
    ? connectionActionFor(state.value)
    : { kind: "none" as const, label: "处理中" as const, enabled: false };
  if (actionBusy.value === "connect" && base.kind !== "stop") {
    return { kind: "stop", label: "取消", enabled: true };
  }
  return base;
});

const busy = computed(() => {
  // 正在等待 Connect 返回时，取消（Stop）是唯一合法动作，任意相位都必须放行
  // （修复 3b）；其他操作（凭据提交、安装服务、已发出的 Stop）仍保持互斥。
  const canCancelConnect = actionBusy.value === "connect"
    && effectiveAction.value.kind === "stop";
  return (actionBusy.value !== null && !canCancelConnect)
    || credentialSubmitting.value || credentialCancelling.value || serviceOperations?.busy.value === true;
});
const action = computed<ProductConnectionAction>(() => ({
  ...effectiveAction.value,
  enabled: effectiveAction.value.enabled && !busy.value,
}));

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
  /** credentials = 账户密码均缺；password = 仅缺密码（账户只读投影）。 */
  mode?: "credentials" | "password";
  /** 本次业务说明。 */
  /** 账户初值/只读投影来源；缺省回退会话投影 → 配置用户名。 */
  initialUsername?: string;
  /** 主动弹出（连接前预检）：取消直接关闭，不派 stop。 */
  proactive?: boolean;
}

function openCredentialModal(request: CredentialModalRequest = {}): void {
  userAuthenticationActive.value = true;
  credentialCancelling.value = false;
  credentialModalError.value = "";
  credentialModalMode.value = request.mode ?? "credentials";
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

/** 用户动作：极简表单带草稿，完整模式按钮不带参数。点击的语义以 effectiveAction
 *  为准（修复 3b：连接在飞期间呈现「取消」即派发 Stop，不再按快照相位重算）。 */
async function runAction(draft?: MinimalCredentialDraft): Promise<void> {
  if (runtime === null || state.value === null || busy.value) return;
  const next = effectiveAction.value;
  if (!next.enabled || next.kind === "none") return;
  const cancellingLogin = next.kind === "stop" && browserLoginActive.value;
  const sequence = ++actionSequence;
  actionBusy.value = next.kind;
  armActionBusyWatchdog(next.kind);
  // 新用户操作丢弃上一轮武装的「成功后清除」（与 darwin 同构）。
  pendingForget = null;
  authModalDismissed.value = false;
  try {
    if (next.kind === "connect") {
      userAuthenticationActive.value = true;
      loginCancelled.value = false;
      lastConnectDraft = draft;
      // §7.10-3：连接发起即按当时配置判定渠道；认证失败早于 browser-login 事件时仍有正确渠道。
      authChannel.value = resolveAuthChannel();
      autoLoginStarted.value = false;
      if (autoInstallService.value && state.value.service.status.kind === "not_installed") {
        const reply = await serviceOperations!.run("install");
        if (!reply.ok) throw new Error(reply.message || "服务安装失败，无法继续连接。");
      }
      await connectWithDraft(draft);
    } else if (next.kind === "stop") {
      userAuthenticationActive.value = false;
      // S3 取消即决：取消登录等待不等停止回执，先本地收口（关模态、退休事件流）；
      // 停止命令转 best-effort，失败不把等待 UI 挂回去（与 darwin 同构）。
      if (cancellingLogin) finishLoginCancellation();
      await runtime.stop();
    }
  } catch (error) {
    // 用户已取消或开始新操作后，旧连接的拒绝不得重新打开认证/恢复模态。
    if (sequence === actionSequence) {
      if (next.kind === "stop") {
        // 停止失败不悬挂 UI：仅一次性提示实际错误（取消登录场景已先行收口）。
        pushToast(cancellingLogin ? "停止连接未完成：" + messageFor(error) : messageFor(error), "error");
      }
      else if (userAuthenticationActive.value) handleActionError(error, draft !== undefined);
    }
  } finally {
    // 旧 Connect 的 finally 不得清除随后发出的 Stop 或新连接的忙碌状态。
    if (sequence === actionSequence) {
      actionBusy.value = null;
      disarmActionBusyWatchdog();
    }
  }
}

/**
 * 极简分支（D1/D3 + S5a 分支 2 统一）：勾选+密码 → persist=true；未勾选+密码 →
 * persist=false 且**连接成功后**清除已保存密码（失败保留便于重试，经
 * pendingForget 收口）；勾选+留空+有已保存 → 不传凭据由 host 回落已保存密文。
 * 完整模式不带草稿，沿用原来的无参 connect。
 */
async function connectWithDraft(draft?: MinimalCredentialDraft): Promise<void> {
  if (runtime === null) return;
  if (draft === undefined) {
    await runtime.connect();
    return;
  }
  const plan = planMinimalCredentials(draft, { hasStoredPassword: hasStoredPassword.value, storedUsername: credentialConfig.state.value.username });
  if (plan.blocked) return;
  const revision = credentialConfig.savedRevision.value;
  const sequence = actionSequence;
  // C3：极简路径出现同型需求（勾选+密码+persist）时与凭据模态复用同一 aTrust 落盘
  // helper；Cisco 维持原 core 通道。
  if (plan.credentials?.persist && credentialConfig.state.value.protocol === "atrust") {
    await saveAtrustCredentialsThenConnect(plan.credentials);
    return;
  }
  if (plan.credentials?.persist) credentialConfig.markSaved();
  const operationId = await (plan.credentials?.persist
    ? credentialConfig.write(() => runtime.connect(plan.credentials!))
    : runtime.connect(plan.credentials ?? undefined));
  // 分支 2（成功后清除）：本次操作到达 connected 才执行；失败/新操作丢弃。
  if (plan.clearStoredAfterSuccess && operationId && sequence === actionSequence) {
    pendingForget = { operationId, revision };
    settlePendingForget();
  }
}

/** 分支 2 的清除收口：只在连接成功之后执行（不可逆操作；失败时保留已保存密码便于重试）。 */
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

/**
 * C3（aTrust「记住密码」经 configSet 落盘）：aTrust 的连接请求在壳层剥除账密
 * （既有安全边界），core 的 Cisco persist 通道对 aTrust 不可达——勾选记住时改走
 * 与设置页 `submitPasswordModal` 逐字同构的 `config_set` 载荷（`password` +
 * `remember_password:"true"`，账户与已存值不同时补 `username`），经同一
 * `credentialConfig.write` 串行队列保存；保存被拒绝时不发出连接，错误沿用调用方
 * 既有显示（模态内错误/极简 toast）。保存成功后刷新一次配置投影，使凭据状态行
 * 即时更新（复审 MINOR），再以本次内存凭据发出连接。Cisco 分支不经此路径。
 */
async function saveAtrustCredentialsThenConnect(credentials: { username: string; password: string; persist: boolean }): Promise<void> {
  if (runtime === null) return;
  const items: { key: CoreConfigKey; value: string }[] = [
    { key: "password", value: credentials.password },
    { key: "remember_password", value: "true" },
  ];
  // 与设置页同构的条件 username：password 形态的只读投影=已存账户（不写）；
  // credentials 形态/极简新输入时与已存值不同（写入）。
  if (credentials.username !== credentialConfig.state.value.username) {
    items.unshift({ key: "username", value: credentials.username });
  }
  credentialConfig.markSaved();
  const saved = await credentialConfig.write(() => configGateway.configSet(items));
  if (!saved) throw new Error("Core 未确认写入");
  await credentialConfig.refresh();
  await runtime.connect(credentials);
}

function finishLoginCancellation(): void {
  userAuthenticationActive.value = false;
  authFailure.value = null;
  autoLoginStarted.value = false;
  authModalDismissed.value = true;
  credentialModalVisible.value = false;
  credentialModalError.value = "";
  credentialSubmitting.value = false;
  credentialModalResetKey.value += 1;
  if (!loginCancelled.value) pushToast("您已取消登录", "info");
  loginCancelled.value = true;
}

function handleActionError(error: unknown, fromMinimalForm: boolean): void {
  if (isBrowserLoginCancelled(error)) { finishLoginCancellation(); return; }
  if (isCredentialRequiredError(error)) {
    // 极简与完整模式统一弹凭据模态（Area F：极简不再有内联密码框）。
    // 账户取本次草稿/配置：有账户 → 仅密码形态；无账户 → 合并输入形态，
    // 合并形态预填会话投影的账户（既有「缺什么补什么」语义）。
    const draftUsername = (fromMinimalForm ? lastConnectDraft?.username : undefined)
      ?? credentialConfig.state.value.username;
    openCredentialModal({
      mode: draftUsername.trim() ? "password" : "credentials",
      initialUsername: draftUsername.trim() || (state.value?.connectionInfo.account ?? ""),
    });
    return;
  }
  // §7.5：浏览器认证失败（三个页面分类码或既有认证失败类）→ 如实通知 + 三选项。
  const authIntervention = browserAuthIntervention(error);
  if (authIntervention) {
    authFailure.value = authIntervention;
    return;
  }
  if (isServiceRecoveryError(error)) {
    serviceModalError.value = messageFor(error);
    serviceModalVisible.value = true;
    return;
  }
  // T9（§2.9）：连接前唤醒 core 失败 → 全局「尝试重启程序 / 仅退出」强报错模态。
  if (handleCoreWakeFailure(error)) return;
  pushToast(messageFor(error), "error");
}

/** 密码只在模态草稿与本次 runtime.connect 参数中存在；提交即一次性传输。 */
async function submitCredentials(credentials: { username: string; password: string; persist: boolean }): Promise<void> {
  if (!runtime || busy.value) return;
  userAuthenticationActive.value = true;
  loginCancelled.value = false;
  credentialAttempt += 1;
  const attempt = credentialAttempt;
  const sequence = actionSequence;
  credentialBrowserOwner = null;
  credentialAuthenticatedAttempt = null;
  credentialSubmitting.value = true;
  credentialModalError.value = "";
  // §7.10-3：凭据模态提交同样先判定渠道。
  authChannel.value = resolveAuthChannel();
  autoLoginStarted.value = false;
  // 提交即发出连接：取消语义从「主动弹出直接关」切换为「错误驱动」（派 stop 等 idle）。
  credentialProactive.value = false;
  try {
    if (credentials.persist) {
      // C3：aTrust 剥凭据上 wire（连接请求不携带密码），core 的 Cisco persist 通道
      // 对 aTrust 不可达——改经 config_set 落盘后再连接；Cisco 分支原样（core
      // x-exv-persist-credentials 通道）。
      if (credentialConfig.state.value.protocol === "atrust") {
        await saveAtrustCredentialsThenConnect(credentials);
      } else {
        credentialConfig.markSaved();
        await credentialConfig.write(() => runtime.connect(credentials));
      }
    } else {
      // 分支 2（S5a 统一，M6 对应落点）：未勾选「记住密码」→ 本次一次性凭据，
      // 连接成功后清除已存密码（失败保留便于重试，经 pendingForget 收口）。
      const revision = credentialConfig.savedRevision.value;
      const operationId = await runtime.connect(credentials);
      if (operationId && attempt === credentialAttempt && sequence === actionSequence) {
        pendingForget = { operationId, revision };
        settlePendingForget();
      }
    }
    if (attempt !== credentialAttempt || sequence !== actionSequence) return;
    credentialModalResetKey.value += 1;
    credentialModalVisible.value = false;
  } catch (error) {
    if (attempt !== credentialAttempt || sequence !== actionSequence) return;
    if (isBrowserLoginCancelled(error)) { finishLoginCancellation(); return; }
    // T9：凭据提交的连接前唤醒失败同样走全局强报错模态（不困在凭据模态内）。
    if (handleCoreWakeFailure(error)) {
      credentialModalVisible.value = false;
      return;
    }
    if (credentialAuthenticatedAttempt === attempt) handleActionError(error, false);
    else credentialModalError.value = messageFor(error);
  } finally {
    if (attempt === credentialAttempt && sequence === actionSequence) {
      credentialBrowserOwner = null;
      credentialSubmitting.value = false;
    }
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
  // 失败仅一次性提示实际错误，不再把失败挂回模态（取消不悬挂 UI，与 darwin 同构）。
  finishCredentialCancellation();
  credentialBrowserOwner = null;
  credentialAttempt += 1;
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
function reenterCredentials(): void {
  authModalDismissed.value = true;
  const username = (state.value?.connectionInfo.account ?? credentialConfig.state.value.username ?? "").trim();
  openCredentialModal({
    mode: username ? "password" : "credentials",
  });
}

function recoveryCredentialsRequired(): void {
  serviceModalVisible.value = false;
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
  actionSequence += 1;
  credentialAttempt += 1;
  actionBusy.value = null;
  disarmActionBusyWatchdog();
  authFailure.value = null;
  autoLoginStarted.value = false;
}, { flush: "sync" });

provide(CONNECTION_INTERACTION_KEY, { state, action, busy, autoInstallService, runAction, fallbackAvailable: computed(() => fallbackAvailable.value), fallbackBusy: computed(() => fallbackBusy.value), fallbackError: computed(() => fallbackError.value), useSystemBrowserFallback });
</script>

<template>
  <AuthPreparationModal :event="preparationEvent" :pending-action="preparation.pendingAction.value" :busy="preparationBusy" :error="preparationError" @action="preparation.respond" />
  <slot
    :state="state"
    :action="action"
    :busy="busy"
    :run="runAction"
    :minimal="minimal"
    :protocol="minimalProtocol"
    :initial-username="initialUsername"
    :initial-server="initialServer"
    :remember-password="storedRememberPassword"
    :has-stored-password="hasStoredPassword"
    :request-credentials="requestCredentialsFromMinimal"
  />
  <BrowserAuthWaitingModal
    :visible="!preparationEvent && (authWaitVisible)"
    :mode="authModalMode"
    :allow-system-browser="fallbackAvailable"
    :channel="authChannel"
    :stage="autoLoginStage"
    :ticket-refresh-reason="runtime?.ticketRefreshReason?.value ?? null"
    :throttle-warning="loginThrottleWarning"
    :busy="authWaitBusy"
    :error="authFailure ? authFailure.message : fallbackError"
    :error-code="authFailure ? authFailure.code : null"
    :server-detail="authFailure ? authFailure.serverDetail : null"
    @cancel="authFailure ? cancelAuthIntervention() : runAction()"
    @system-browser="useSystemBrowserFallback"
    @retry="retryAuthIntervention"
  />
  <ServiceConnectFailureModal
    :visible="!preparationEvent && (serviceModalVisible)"
    :compact="minimal"
    :error-message="serviceModalError"
    @close="serviceModalVisible = false"
    @recovered="serviceModalVisible = false"
    @credential-required="recoveryCredentialsRequired"
  />
  <AuthenticationFailureModal
    :visible="!preparationEvent && (authenticationFailure)"
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
    :has-saved-password="hasStoredPassword"
    :initial-remember="storedRememberPassword"
    :initial-username="credentialModalInitialUsername"
    @close="closeCredentialModal"
    @submit="submitCredentials"
    @clear-saved-password="clearSavedPassword"
  />
</template>
