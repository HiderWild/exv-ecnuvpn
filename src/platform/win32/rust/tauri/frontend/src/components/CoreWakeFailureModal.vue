<script setup lang="ts">
import { ref } from "vue";

import { useDialogFocus } from "./use-dialog-focus";

/**
 * T9（§2.9）core 唤醒失败强报错模态。
 *
 * 触发：关键业务动作（保存设置/建立连接/快速入门）前自动唤醒 core 失败（壳层以
 * `kind = "core_wake_failed"` 拒绝命令）。两条出路均写明后果：
 *   * 尝试重启程序——relaunch 整个 EXV：当前界面状态丢失；若连接进行中会中断；
 *     UI 退出经 O3 有序停机（core/engine 一并终止），新 UI 启动重新拉起 core。
 *   * 仅退出——退出 EXV：后台不残留任何 EXV 进程；下次手动启动恢复。
 * 无「关闭」出口（强报错语义）：动作失败时保留模态并以 toast 提示手动处理。
 */
const props = defineProps<{
  visible: boolean;
  /** 壳侧稳定错误说明（可选；不含凭据/栈）。 */
  message?: string | null;
  /** 极简窗内紧凑渲染变体。 */
  compact?: boolean;
  /** 动作执行器（默认接 App 壳的 core-crash 绑定；测试可注入）。 */
  onRestartApp?: () => Promise<void>;
  onExitApp?: () => Promise<void>;
}>();

const emit = defineEmits<{
  /** 用户选择「尝试重启程序」。 */
  restartApp: [];
  /** 用户选择「仅退出」。 */
  exitApp: [];
}>();

const busyAction = ref<"restart" | "exit" | null>(null);

async function runRestart(): Promise<void> {
  if (busyAction.value !== null) return;
  busyAction.value = "restart";
  try {
    if (props.onRestartApp) {
      await props.onRestartApp();
    } else {
      emit("restartApp");
    }
  } finally {
    busyAction.value = null;
  }
}

async function runExit(): Promise<void> {
  if (busyAction.value !== null) return;
  busyAction.value = "exit";
  try {
    if (props.onExitApp) {
      await props.onExitApp();
    } else {
      emit("exitApp");
    }
  } finally {
    busyAction.value = null;
  }
}

// 强报错模态无关闭出口；焦点管理仍复用对话框约定（Escape 不关闭，仅循环焦点）。
const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, () => {});
</script>

<template>
  <div
    v-if="visible"
    class="modal-overlay"
    data-testid="core-wake-failure-modal"
    :data-compact="compact ? 'true' : 'false'"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="core-wake-failure-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
  >
    <div class="modal-card">
      <div class="modal-body">
        <h2 id="core-wake-failure-title">无法恢复后台服务</h2>
        <p class="modal-error" data-testid="core-wake-failure-error">
          后台核心进程已停止，自动重启未能成功；本次操作未完成。
        </p>
        <p v-if="message" class="modal-detail" data-testid="core-wake-failure-detail">{{ message }}</p>
        <p class="modal-hint">请选择以下处理方式：</p>
      </div>

      <div class="modal-actions">
        <button
          type="button"
          class="modal-actions__primary"
          data-testid="core-wake-failure-restart-app"
          :disabled="busyAction !== null"
          @click="runRestart"
        >
          {{ busyAction === "restart" ? "正在重启…" : "尝试重启程序" }}
        </button>
        <button
          type="button"
          data-testid="core-wake-failure-exit-app"
          :disabled="busyAction !== null"
          @click="runExit"
        >
          {{ busyAction === "exit" ? "正在退出…" : "仅退出" }}
        </button>
      </div>
      <p class="modal-consequences">
        「尝试重启程序」将关闭并重新启动 EXV：进行中的连接会中断，未保存的界面状态会丢失，重启后后台服务会重新拉起。
        「仅退出」将退出 EXV，后台不会继续运行任何服务；下次启动时恢复。
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
  font-size: 18px;
}

.modal-error {
  margin-bottom: var(--space-2);
  padding: var(--space-3);
  border: 1px solid var(--state-danger);
  border-radius: var(--radius-md);
  color: var(--state-danger);
  font-size: 13px;
}

.modal-detail {
  margin-bottom: var(--space-2);
  color: var(--text-secondary);
  font-size: 12px;
}

.modal-hint {
  margin-bottom: var(--space-4);
  color: var(--text-secondary);
  font-size: 13px;
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
  font-size: 13px;
  cursor: pointer;
}

.modal-actions__primary {
  border-color: var(--state-danger);
  background: var(--state-danger);
  color: #fff;
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
  font-size: 12px;
  line-height: 1.5;
}

/* 极简紧凑变体：对齐既有模态的紧凑规则（面板收窄、正文裁切、按钮不换行）。 */
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
  overflow: hidden;
  margin-bottom: 3px;
  font-size: 12px;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.modal-overlay[data-compact="true"] .modal-error,
.modal-overlay[data-compact="true"] .modal-detail,
.modal-overlay[data-compact="true"] .modal-hint,
.modal-overlay[data-compact="true"] .modal-consequences {
  margin-bottom: 3px;
  font-size: 11px;
  line-height: 1.25;
}

.modal-overlay[data-compact="true"] .modal-body {
  display: grid;
  min-height: 0;
  flex: 1 1 auto;
  gap: 3px;
  overflow: hidden;
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
  font-size: 11px;
  line-height: 1.1;
  white-space: nowrap;
}

.modal-overlay[data-compact="true"] .modal-consequences {
  overflow: hidden;
  max-height: 3.9em;
}
</style>
