<script setup lang="ts">
import { useDialogFocus } from "./use-dialog-focus";

/**
 * S4（2026-10）：config.json 读失败明确报错模态（App 启动挂载）。
 *
 * 触发：启动 ConfigGet 携带 `read_failure`（core 三档自愈后仍读不了），或
 * ConfigGet 调用本身失败（`unreachable`）。与 R-D3-C「配置文件损坏，已恢复为
 * 默认设置」toast 的分界：那是**已恢复**（一次性通知），这是**读不了**（明确
 * 错误 + 出路）；文件字节未被改动（禁令：不得默认值重建/覆盖）。
 *
 * 出口：
 *   * 「以管理员身份修复权限」——仅 ACL 类（kind=acl）显示；壳 runas 重置
 *     config.json ACL（只动权限不动内容）后自动重试读取；
 *   * 「重试读取」——重新发起启动配置读取（占用类失败关闭占用程序后用）；
 *   * 「稍后再说」——关闭模态保留主界面可用（连接等不依赖 config.json 的
 *     功能不受阻；下次启动或重开设置仍会再触发读取）。
 */
const props = defineProps<{
  visible: boolean;
  /** product/config-read-failure 的渲染投影（title/message/canRepairPermissions）。 */
  view: {
    kind: "acl" | "busy" | "unreadable" | "unreachable";
    title: string;
    message: string;
    canRepairPermissions: boolean;
  } | null;
  /** 修复/重试进行中（按钮禁用 + 文案切换）。 */
  busy?: "repair" | "retry" | null;
  /** 修复失败的壳侧错误说明（如用户取消 UAC）；无则不渲染。 */
  repairError?: string | null;
  /** 极简窗内紧凑渲染变体。 */
  compact?: boolean;
}>();

const emit = defineEmits<{
  /** 用户点击「以管理员身份修复权限」。 */
  repair: [];
  /** 用户点击「重试读取」。 */
  retry: [];
  /** 用户点击「稍后再说」（保留主界面可用）。 */
  dismiss: [];
}>();

const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, () => {});
</script>

<template>
  <div
    v-if="visible && view"
    class="modal-overlay exv-dialog"
    data-testid="config-read-failure-modal"
    :data-kind="view.kind"
    :data-compact="compact ? 'true' : 'false'"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="config-read-failure-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
  >
    <div class="modal-card exv-dialog-card">
      <h2 v-if="compact" id="config-read-failure-title">{{ view.title }}</h2>
      <div class="modal-body" :tabindex="compact ? 0 : undefined" :role="compact ? 'region' : undefined" :aria-label="compact ? '详细说明' : undefined">
        <h2 v-if="!compact" id="config-read-failure-title">{{ view.title }}</h2>
        <p class="modal-error" data-testid="config-read-failure-error">{{ view.message }}</p>
        <p v-if="repairError" class="modal-detail" data-testid="config-read-failure-repair-error">{{ repairError }}</p>
        <p v-if="compact" class="modal-consequences">「以管理员身份修复权限」将请求管理员授权并只重置该文件的访问权限，不会改动配置内容；完成后会自动重试读取。</p>
      </div>

      <div class="modal-actions">
        <button
          v-if="view.canRepairPermissions"
          type="button"
          class="modal-actions__primary"
          data-testid="config-read-failure-repair"
          :disabled="busy !== null"
          @click="emit('repair')"
        >
          {{ busy === "repair" ? "正在修复…" : "以管理员身份修复权限" }}
        </button>
        <button
          type="button"
          data-testid="config-read-failure-retry"
          :disabled="busy !== null"
          @click="emit('retry')"
        >
          {{ busy === "retry" ? "正在重试…" : "重试读取" }}
        </button>
        <button
          type="button"
          data-testid="config-read-failure-dismiss"
          :disabled="busy !== null"
          @click="emit('dismiss')"
        >
          稍后再说
        </button>
      </div>
      <p v-if="!compact" class="modal-consequences">
        「以管理员身份修复权限」将请求管理员授权并只重置该文件的访问权限，不会改动配置内容；完成后会自动重试读取。
      </p>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  z-index: 70;
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

.modal-detail {
  margin-bottom: var(--space-2);
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
}

.modal-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--space-2);
}

.modal-actions button {
  min-height: 34px;
  padding: var(--space-1) var(--space-3);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  font-size: var(--font-size-body);
  cursor: pointer;
}

.modal-actions__primary {
  border-color: var(--state-danger);
  background: var(--state-danger);
  color: var(--state-danger-on);
}

.modal-actions button:disabled {
  border-color: var(--border-subtle);
  background: var(--surface-subtle);
  color: var(--text-secondary);
  cursor: default;
}

.modal-consequences {
  margin: var(--space-3) 0 0;
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
  line-height: 1.5;
}

/* 极简紧凑变体：标题与动作固定，错误与后果在正文内完整滚动。 */
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

.modal-overlay[data-compact="true"] .modal-error,
.modal-overlay[data-compact="true"] .modal-detail,
.modal-overlay[data-compact="true"] .modal-consequences {
  margin-bottom: 3px;
  font-size: var(--font-size-caption);
  line-height: 1.25;
}

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

</style>
