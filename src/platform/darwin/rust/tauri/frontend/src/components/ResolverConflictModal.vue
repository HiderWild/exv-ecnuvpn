<script setup lang="ts">
import { computed, ref } from "vue";

import { useDialogFocus } from "./use-dialog-focus";
import { manualCleanupSteps, type ResolverConflictFact } from "../product/resolver-conflict";

/**
 * M7：aTrust 校园 resolver 外来同名冲突三选模态（win32 无 NRPT 冲突三选形态，
 * 本组件为 darwin 新建的简单模态；样式与 `AuthenticationFailureModal` 同源）。
 *
 * 触发证据（`ConnectionInteractionHost` 判定）：当前失败快照携带 engine 稳定 reason `DARWIN_ATRUST_RESOLVER_CONFLICT`。
 * 三选：
 *   ① 手动清理——展开终端步骤（EXV 绝不代替用户删除外来文件）；
 *   ② 调整校园域配置——跳转设置页「连接」分区改 campus_dns_domains；
 *   ③ 放弃连接——关闭模态。
 */
const props = defineProps<{
  visible: boolean;
  conflict: ResolverConflictFact | null;
  /** 极简窗内紧凑渲染变体；不改变选项语义。 */
  compact?: boolean;
}>();

const emit = defineEmits<{
  close: [];
  adjust: [];
}>();

const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, () => emit("close"));
const showCleanup = ref(false);
const domain = computed(() => props.conflict?.domain ?? null);
const cleanupSteps = computed(() => manualCleanupSteps(domain.value));

function toggleCleanup(): void {
  showCleanup.value = !showCleanup.value;
}
</script>

<template>
  <div
    v-if="visible"
    class="modal-overlay exv-dialog"
    data-testid="resolver-conflict-modal"
    :data-compact="compact ? 'true' : 'false'"
    role="dialog"
    aria-modal="true"
    aria-labelledby="resolver-conflict-title"
    ref="dialog"
    tabindex="-1"
    @keydown="onDialogKeydown"
  >
    <div class="modal-card exv-dialog-card">
      <h2 id="resolver-conflict-title">校园 DNS 配置文件冲突</h2>
      <div class="modal-body" :tabindex="compact ? 0 : undefined" :role="compact ? 'region' : undefined" :aria-label="compact ? '详细说明' : undefined">
        <p class="modal-intro">
          连接 aTrust 需要写入系统分域解析文件
          <code v-if="domain">/etc/resolver/{{ domain }}</code><code v-else>/etc/resolver/&lt;校园域名&gt;</code>，
          但该位置已存在 EXV 之外创建的同名文件。EXV 不会覆盖它。请选择处理方式：
        </p>
        <ol v-if="showCleanup" class="modal-steps" data-testid="resolver-conflict-steps">
          <li v-for="step in cleanupSteps" :key="step">{{ step }}</li>
        </ol>
        <p class="modal-hint">
          冲突详情：{{ conflict?.message ?? "（暂无）" }}
        </p>
      </div>
      <div class="modal-actions">
        <button type="button" class="modal-secondary" data-testid="resolver-conflict-abandon" @click="emit('close')">
          放弃连接
        </button>
        <button type="button" class="modal-secondary" data-testid="resolver-conflict-adjust" @click="emit('adjust')">
          调整校园域配置
        </button>
        <button type="button" data-testid="resolver-conflict-cleanup" @click="toggleCleanup">
          {{ showCleanup ? "收起手动清理步骤" : "查看手动清理步骤" }}
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
  width: min(100%, 460px);
  padding: var(--space-6);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: var(--surface-panel);
  box-shadow: 0 12px 32px rgb(0 0 0 / 0.28);
  color: var(--text-primary);
}
.modal-card h2 { margin: 0 0 var(--space-2); color: var(--state-danger); font-size: var(--font-size-heading); }
.modal-intro, .modal-hint { color: var(--text-secondary); font-size: var(--font-size-caption); line-height: 1.6; }
.modal-card code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: var(--font-size-caption); color: var(--text-primary); }
.modal-steps {
  margin: var(--space-2) 0 var(--space-3);
  padding-left: 1.35rem;
  display: grid;
  gap: var(--space-1);
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
  line-height: 1.6;
}
.modal-steps code { font-size: var(--font-size-caption); }
.modal-hint { color: var(--text-tertiary); }
.modal-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-5); }
.modal-actions button {
  min-height: 34px;
  padding: var(--space-1) var(--space-4);
  border: 1px solid var(--accent);
  border-radius: var(--radius-md);
  background: var(--accent);
  color: var(--accent-on);
  font-size: var(--font-size-body);
  cursor: pointer;
}
.modal-actions .modal-secondary { border-color: var(--border-subtle); background: var(--surface-raised); color: var(--text-primary); }
.modal-actions button:hover { border-color: var(--accent-strong); background: var(--accent-strong); }

/* 标题与动作固定，长说明独立滚动；不占用 Darwin 原生标题栏预算。 */
.modal-overlay.exv-dialog .modal-card { display: flex; flex-direction: column; overflow: hidden; min-height: 0; }
.modal-overlay.exv-dialog h2 { flex: none; }
.modal-overlay.exv-dialog .modal-body { min-height: 1.3em; min-width: 0; overflow-y: auto; overflow-wrap: anywhere; overscroll-behavior: contain; padding: 3px; margin: 0 -3px; }
.modal-overlay.exv-dialog :is(.modal-actions, .credential-modal-actions) { flex: none; padding: 5px 5px 8px; margin-left: -5px; margin-right: -5px; }
.modal-overlay[data-compact="true"] .modal-card { max-height: calc(100dvh - 16px); }
.modal-overlay[data-compact="true"] .modal-steps { padding-left: 15px; }
.modal-overlay[data-compact="true"] code { font-size: var(--font-size-caption); }
</style>
