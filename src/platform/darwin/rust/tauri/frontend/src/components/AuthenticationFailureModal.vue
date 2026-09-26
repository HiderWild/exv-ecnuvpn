<script setup lang="ts">
import { useDialogFocus } from "./use-dialog-focus";
/**
 * 认证失败（`ERROR_CODE_UNAUTHORIZED`）详细指引模态。
 *
 * 产品 UI 呈现规则：大段信息走模态弹窗（不页内插入组件）。关闭后连接按钮恢复
 * 「重试」可点（connection-action 失败态不禁用），下次连接失败会再次弹起。
 */
const props = defineProps<{
  visible: boolean;
  /** 极简窗内紧凑渲染变体；不改变步骤内容与动作语义。 */
  compact?: boolean;
}>();

const emit = defineEmits<{
  reenter: [];
  close: [];
}>();
const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, () => emit("close"));
</script>

<template>
  <div
    v-if="visible"
    class="modal-overlay"
    data-testid="auth-failure-modal"
    :data-compact="compact ? 'true' : 'false'"
    role="dialog"
    aria-modal="true"
    aria-labelledby="auth-failure-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
  >
    <div class="modal-card">
      <div class="modal-body">
        <h2 id="auth-failure-title">VPN 网关没有通过本次登录</h2>
        <p class="modal-intro">
          这不是网络通断提示，而是服务器拒绝了认证信息。请按下面步骤处理：
        </p>
        <ol class="modal-steps">
          <li>打开左侧“设置”，进入“连接与网络”。</li>
          <li>检查并保存服务器、用户名和密码；密码修改后必须点击对应的“保存”。</li>
          <li>返回连接页后再次点击“连接”。服务模式会读取同一份已保存配置。</li>
        </ol>
        <p class="modal-hint">
          如果无服务时能连接、安装服务后仍失败，请打开“设置”的“服务”面板点击“修复”，再回到连接页重试；修复会重新登记当前用户的配置目录。
        </p>
      </div>
      <div class="modal-actions">
        <button type="button" class="modal-secondary" data-testid="auth-failure-close" @click="emit('close')">
          知道了
        </button>
        <button type="button" data-testid="auth-failure-reenter" @click="emit('reenter')">
          重新输入凭据
        </button>
      </div>
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
  margin: 0 0 var(--space-2);
  color: var(--state-danger);
  font-size: 18px;
}

.modal-intro,
.modal-hint {
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1.6;
}

.modal-steps {
  margin: var(--space-2) 0 var(--space-3);
  padding-left: 1.35rem;
  display: grid;
  gap: var(--space-1);
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1.6;
}

.modal-hint {
  color: var(--text-tertiary);
}

.modal-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: var(--space-5);
}

.modal-actions button {
  min-height: 34px;
  padding: var(--space-1) var(--space-4);
  border: 1px solid var(--accent);
  border-radius: var(--radius-md);
  background: var(--accent);
  color: var(--accent-on);
  font-size: 13px;
  cursor: pointer;
}

.modal-actions .modal-secondary {
  border-color: var(--border-subtle);
  background: var(--surface-raised);
  color: var(--text-primary);
}

.modal-actions button:hover {
  border-color: var(--accent-strong);
  background: var(--accent-strong);
}

/* 极简紧凑变体：面板收窄到 min(100%, 294px)，内容区裁切，动作按钮不换行。
   覆盖本模态自己的 440px 宽 / space-6 内边距 / 18px 标题与 3 步清单。 */
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
  margin: 0 0 3px;
  font-size: 12px;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.modal-overlay[data-compact="true"] .modal-intro,
.modal-overlay[data-compact="true"] .modal-hint,
.modal-overlay[data-compact="true"] .modal-steps {
  margin: 0;
  font-size: 11px;
  line-height: 1.25;
}

.modal-overlay[data-compact="true"] .modal-steps {
  padding-left: 1rem;
  gap: 1px;
}

/* 内容区裁切，动作区始终可见（对齐历史 ModalShell 的 body/actions 分工）。 */
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
  margin-top: 6px;
}

.modal-overlay[data-compact="true"] .modal-actions button {
  min-width: 0;
  min-height: 22px;
  padding: 2px 7px;
  font-size: 11px;
  line-height: 1.1;
  white-space: nowrap;
}

@media (max-width: 360px), (max-height: 180px) {
  .modal-overlay[data-compact="true"] .modal-actions button {
    flex: 0 0 auto;
  }
}
</style>
