<script setup lang="ts">
import { computed, inject, ref } from "vue";
import { useDialogFocus } from "./use-dialog-focus";

import { commandErrorMessage } from "../lib/command-error";
import {
  initialSteps,
  markCurrent,
  markDone,
  markFailed,
  serviceFailurePhrase,
  stepMarker,
  type RecoveryAction,
  type RecoveryStep,
} from "../product/recovery-steps";
import { PRODUCT_RUNTIME_KEY, type ProductRuntime } from "../product/runtime";
import { createServiceOperationCoordinator, SERVICE_OPERATION_KEY } from "../product/service-operations";

/**
 * R5 服务连接失败恢复 modal（P0-2 扩展，计划 v6.1 第二批 / 2026-09-28 裁定）。
 *
 * 服务模式连接/安装失败（`service_not_running` / `service_connect_failed`——含 engine
 * 批量 Verify 快速失败透传的 `service_not_running|<exit_code>|<短语>` 稳定前缀）时由
 * 连接页挂起。**三选恢复**（降级永不自动执行——2026-09-28 用户裁定，用户未选择前
 * 不发生任何形态切换）：
 *   * 重试服务安装：serviceControl("repair"/install)（Core 侧 retire→install 序 +
 *     自动 start + readiness）→ connect()（路由回 service）；失败原因短语随错误框
 *     展示（`serviceFailurePhrase` 解析稳定前缀）；
 *   * 本次尝试一次性连接：serviceControl("uninstall") → connect()（路由回 oneshot）；
 *     **会重新弹 UAC**（D13 句式，无 UAC 等级建议句——若弹窗打扰请改用安装服务）；
 *   * 取消：仅关闭 modal，回到未连接态（页面错误文案保留）。
 * 任一步失败 → 错误显示在 modal 内（不关闭、不丢失恢复入口）。
 */
const props = defineProps<{
  visible: boolean;
  /** 触发 modal 的 connect 失败文案（保留在页面上；取消时不清除）。 */
  errorMessage: string;
  /** 极简窗内紧凑渲染变体；不改变恢复序列语义。 */
  compact?: boolean;
}>();

type CredentialRequiredFailure = {
  kind: "credential_required";
  code?: string;
  message?: string;
};

const emit = defineEmits<{
  /** 取消：关闭 modal，页面错误文案保留。 */
  close: [];
  /** 恢复序列成功派发连接：关闭 modal 并清除页面错误文案。 */
  recovered: [];
  /** 恢复连接需要用户输入凭据；由页面复用凭据模态，不能当成本地恢复错误。 */
  credentialRequired: [error: CredentialRequiredFailure];
}>();

const runtime = inject(PRODUCT_RUNTIME_KEY, null) as ProductRuntime | null;
const serviceOperations = inject(
  SERVICE_OPERATION_KEY,
  runtime ? createServiceOperationCoordinator((action) => runtime.serviceControl(action)) : null,
);

const busyAction = ref<"clean" | "reinstall" | null>(null);
const actionError = ref<string | null>(null);
/** 当前恢复序列的步骤列表（pending/current/done/failed；分步透明）。 */
const steps = ref<RecoveryStep[]>([]);

/** P0-2：失败原因短语（`service_not_running|<code>|<短语>` 稳定前缀解析；未命中时
 *  原样展示错误文案）。 */
const failurePhrase = computed(() => serviceFailurePhrase(props.errorMessage));

function isCredentialRequiredFailure(error: unknown): error is CredentialRequiredFailure {
  return typeof error === "object"
    && error !== null
    && (error as { kind?: unknown }).kind === "credential_required";
}

/** 执行一步：置 current → await → 校验（serviceControl 返回 {ok,message}）→ done；失败置 failed 并抛出。 */
async function runStep(id: string, op: () => Promise<unknown>): Promise<void> {
  steps.value = markCurrent(steps.value, id);
  try {
    const result = await op();
    if (result && typeof result === "object" && "ok" in result) {
      const reply = result as { ok: boolean; message?: string };
      if (!reply.ok) {
        steps.value = markFailed(steps.value, id);
        throw new Error(reply.message || "操作失败。");
      }
    }
    steps.value = markDone(steps.value, id);
  } catch (error) {
    steps.value = markFailed(steps.value, id);
    throw error;
  }
}

async function runSequence(action: RecoveryAction): Promise<void> {
  if (runtime === null || serviceOperations === null || busyAction.value !== null || serviceOperations.busy.value) return;
  busyAction.value = action;
  actionError.value = null;
  steps.value = initialSteps(action);
  try {
    if (action === "clean") {
      // 清理后连接 = 卸载服务 → 一次性连接（uninstall 是独立请求，不并入 connect）。
      await runStep("uninstall", () => serviceOperations.run("uninstall"));
    } else {
      // 重装后连接 = 安装服务（Core 自动停止运行中服务/安装/启动就绪）→ 服务连接。
      // 不拆显式 stop：架构约定服务常驻、不保留停止逻辑；install 内部 REPAIR 已处理
      // 运行中服务，拆 stop 既违背该约定又徒增一次 UAC。
      await runStep("install", () => serviceOperations.run("repair"));
    }
    // 两个路径都以 connect 收尾；host 按当前服务状态自动路由（oneshot / service）。
    await runStep("connect", () => runtime.connect());
    emit("recovered");
  } catch (error) {
    if (isCredentialRequiredFailure(error)) {
      emit("credentialRequired", error);
      return;
    }
    actionError.value = commandErrorMessage(error, "恢复操作失败，请稍后重试。");
  } finally {
    busyAction.value = null;
  }
}

function cancel(): void {
  if (busyAction.value !== null) return;
  emit("close");
}
const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, cancel);
</script>

<template>
  <div
    v-if="visible"
    class="modal-overlay exv-dialog"
    data-testid="service-failure-modal"
    :data-compact="compact ? 'true' : 'false'"
    role="dialog"
    aria-modal="true"
    aria-labelledby="service-failure-modal-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
  >
    <div class="modal-card exv-dialog-card">
      <h2 v-if="compact" id="service-failure-modal-title">服务连接失败</h2>
      <div class="modal-body" :tabindex="compact ? 0 : undefined" :role="compact ? 'region' : undefined" :aria-label="compact ? '详细说明' : undefined">
        <h2 v-if="!compact" id="service-failure-modal-title">服务连接失败</h2>
        <p class="modal-error" data-testid="service-failure-error">{{ failurePhrase }}</p>
        <p class="modal-hint">服务引擎未能就绪，请选择恢复方式（不会自动降级）：</p>

        <div
          v-if="actionError"
          class="modal-action-error"
          role="alert"
          data-testid="service-failure-action-error"
        >
          {{ actionError }}
        </div>
        <div
          v-if="busyAction"
          class="recovery-steps"
          role="status"
          data-testid="recovery-steps"
        >
          <div
            v-for="step in steps"
            :key="step.id"
            class="recovery-step"
            :class="`recovery-step--${step.status}`"
            :data-testid="`recovery-step-${step.id}`"
          >
            <span class="recovery-step__marker" aria-hidden="true">{{ stepMarker(step.status) }}</span>
            <span class="recovery-step__label">{{ step.label }}</span>
          </div>
        </div>
        <p v-if="compact" class="modal-footnote" data-testid="service-failure-uac-hint">一次性连接会重新弹 UAC；若弹窗打扰请改用安装服务形态。</p>
      </div>

      <div class="modal-actions">
        <button
          type="button"
          data-testid="service-failure-reinstall"
          :disabled="busyAction !== null"
          @click="runSequence('reinstall')"
        >
          重试服务安装
        </button>
        <button
          type="button"
          data-testid="service-failure-clean"
          :disabled="busyAction !== null"
          title="卸载服务并以一次性引擎连接本次"
          @click="runSequence('clean')"
        >
          本次尝试一次性连接
        </button>
        <button
          type="button"
          data-testid="service-failure-cancel"
          :disabled="busyAction !== null"
          @click="cancel"
        >
          取消
        </button>
      </div>
      <!-- D13 句式（2026-09-28 裁定）：无 UAC 等级建议句——不指导用户降低 UAC。 -->
      <p v-if="!compact" class="modal-footnote" data-testid="service-failure-uac-hint">
        一次性连接会重新弹 UAC；若弹窗打扰请改用安装服务形态。
      </p>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  z-index: 50;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--space-5);
  background: rgb(0 0 0 / 0.45);
}

.modal-card {
  max-height: calc(100dvh - 40px);
  overflow-y: auto;
  overflow-wrap: anywhere;
  width: min(100%, 440px);
  padding: var(--space-6);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: var(--surface-panel);
  box-shadow: 0 12px 32px rgb(0 0 0 / 0.28);
  color: var(--text-primary);
}

.modal-card h2 {
  margin-bottom: var(--space-2);
  font-size: var(--font-size-heading);
}

.modal-error {
  margin-bottom: var(--space-2);
  padding: var(--space-3);
  border: 1px solid var(--state-danger);
  border-radius: var(--radius-md);
  color: var(--state-danger);
  font-size: var(--font-size-caption);
}

.modal-hint {
  margin-bottom: var(--space-4);
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
}

.modal-action-error {
  margin-bottom: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--state-danger);
  border-radius: var(--radius-md);
  color: var(--state-danger);
  font-size: var(--font-size-caption);
}

.modal-pending {
  margin-bottom: var(--space-3);
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
}

.recovery-steps {
  display: grid;
  gap: var(--space-2);
  margin-bottom: var(--space-3);
}

.recovery-step {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-height: 30px;
  padding: 0 var(--space-2);
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
}

.recovery-step__marker {
  flex: none;
  width: 18px;
  text-align: center;
  font-weight: 700;
}

.recovery-step--current {
  background: color-mix(in srgb, var(--accent) 10%, var(--surface-panel));
  color: var(--text-primary);
  font-weight: 600;
}

.recovery-step--current .recovery-step__marker {
  color: var(--accent);
  animation: recovery-step-pulse 1s ease-in-out infinite;
}

.recovery-step--done {
  color: var(--text-primary);
}

.recovery-step--done .recovery-step__marker {
  color: var(--state-success);
}

.recovery-step--failed {
  color: var(--state-danger);
}

.recovery-step--failed .recovery-step__marker {
  color: var(--state-danger);
}

@keyframes recovery-step-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.4;
  }
}

.modal-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

/* D13 脚注：一次性连接的 UAC 预告（无 UAC 等级建议句）。 */
.modal-footnote {
  margin-top: var(--space-2);
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
}

.modal-actions button {
  min-height: 34px;
  padding: var(--space-1) var(--space-3);
  font-size: var(--font-size-body);
}

.modal-actions button:disabled {
  border-color: var(--border-subtle);
  background: var(--surface-subtle);
  color: var(--text-secondary);
}

/* 极简紧凑变体：面板收窄到 min(100%, 294px)，正文完整滚动，动作按钮不换行。
   覆盖本模态自己的 440px 宽 / space-6 内边距 / 18px 标题。 */
.modal-overlay[data-compact="true"] {
  padding: 8px;
}

.modal-overlay[data-compact="true"] .modal-card {
  display: flex;
  width: min(100%, 294px);
  max-height: calc(100vh - 28px);
  flex-direction: column;
  overflow: hidden;
  padding: 8px;
  border-radius: var(--radius-md);
}

.modal-overlay[data-compact="true"] .modal-card h2 {
  margin-bottom: 3px;
  font-size: var(--font-size-heading);
  line-height: 1.2;
}

.modal-overlay[data-compact="true"] .modal-error {
  margin-bottom: 3px;
  padding: 3px 6px;
  font-size: var(--font-size-caption);
  line-height: 1.25;
}

.modal-overlay[data-compact="true"] .modal-hint,
.modal-overlay[data-compact="true"] .modal-action-error,
.modal-overlay[data-compact="true"] .modal-pending,
.modal-overlay[data-compact="true"] .recovery-step {
  margin-bottom: 3px;
  font-size: var(--font-size-caption);
  line-height: 1.25;
}

.modal-overlay[data-compact="true"] .modal-footnote {
  margin-top: 3px;
  font-size: var(--font-size-caption);
  line-height: 1.25;
}

.modal-overlay[data-compact="true"] .recovery-steps {
  gap: 2px;
  margin-bottom: 3px;
}

.modal-overlay[data-compact="true"] .recovery-step {
  min-height: 20px;
  padding: 0 4px;
}

/* 正文完整滚动，动作区始终可见（对齐历史 ModalShell 的 body/actions 分工）。 */
.modal-overlay[data-compact="true"] .modal-body {
  display: grid;
  min-height: 0;
  flex: 1 1 auto;
  gap: 3px;
  overflow-y: auto;
  scrollbar-width: thin;
}

.modal-overlay[data-compact="true"] .modal-actions {
  flex: 0 0 auto;
  flex-wrap: nowrap;
  gap: 4px;
}

.modal-overlay[data-compact="true"] .modal-actions button {
  min-width: 0;
  min-height: 22px;
  padding: 2px 7px;
  font-size: var(--font-size-body);
  line-height: 1.1;
  white-space: nowrap;
}

@media (max-width: 360px), (max-height: 180px) {
  .modal-overlay[data-compact="true"] .modal-actions button {
    flex: 0 0 auto;
  }
}
</style>
