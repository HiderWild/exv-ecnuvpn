<script setup lang="ts">
// 预卸载（darwin 单侧）：**全过程在一个模态窗内完成**。
//
// 交互契约（用户 2026-09-13 明确）：
//  1. 卸载过程用一个模态弹窗完成，**不在设置页原地摊开**；
//  2. 确认阶段用一张规整表格交代"要做什么"；
//  3. 结果阶段也在**模态内**用表格列出（不在设置页里堆结果）；
//  4. 每项只显示**一个**状态（状态列 + 说明列，说明列不重复状态语义）；
//  5. 没有任何产物时显示"未扫描到"。
//
// 安全契约（计划 §5.1/§5.3）：
//  * 连接必须已停；壳层检测到活动连接会先停并等权威 Idle，停不下来则中止卸载；
//  * 提权段可能被用户取消——那时如实标注"系统级残留仍在"，不呈现为完成；
//  * **结果先呈现，用户关闭后再退出**（不先退出再渲染）。
import { computed, ref } from "vue";

import {
  PRE_UNINSTALL_STATUS_LABELS,
  quitAfterPreUninstall,
  runPreUninstall,
  type PreUninstallItem,
  type PreUninstallReply,
} from "../product/pre-uninstall";

const emit = defineEmits<{
  /** 用户已查看结果并请求退出（由本组件调用壳侧退出命令）。 */
  closeApp: [];
}>();

type Phase = "closed" | "confirm" | "running" | "done";

const phase = ref<Phase>("closed");
const errorMessage = ref("");
const reply = ref<PreUninstallReply | null>(null);

/** 确认表格：从"连接"到"应用本体"的完整动作清单（顺序即执行顺序）。 */
const plan: ReadonlyArray<{ action: string; target: string }> = [
  { action: "断开连接", target: "当前 VPN 连接（若有）" },
  { action: "卸载系统组件", target: "服务、历史遗留守护、/Library 应用支持目录" },
  { action: "删除运行时残留", target: "/private/tmp 下标属于 EXV 的目录" },
  { action: "删除配置与凭据", target: "~/.exv（含已保存密码与密钥）" },
  { action: "删除前端偏好", target: "~/Library/Application Support/EXV" },
  { action: "删除登录自启动项", target: "~/Library/LaunchAgents 下的 EXV plist" },
  { action: "删除网页缓存", target: "WebKit / Caches / HTTPStorages / Saved Application State" },
  { action: "删除应用本体", target: "EXV.app（从只读安装镜像运行时需手动删除）" },
];

const resultItems = computed<ReadonlyArray<PreUninstallItem>>(() => reply.value?.items ?? []);
const requiresManual = computed(() => (reply.value?.manual_actions.length ?? 0) > 0);

function openConfirm(): void {
  errorMessage.value = "";
  reply.value = null;
  phase.value = "confirm";
}

function dismiss(): void {
  if (phase.value === "running") return;
  phase.value = "closed";
}

async function confirmUninstall(): Promise<void> {
  if (phase.value === "running") return;
  phase.value = "running";
  errorMessage.value = "";
  try {
    reply.value = await runPreUninstall();
    phase.value = "done";
  } catch (error) {
    errorMessage.value = error instanceof Error ? error.message : String(error);
    phase.value = "confirm";
  }
}

/** 结果确认后的退出：走壳侧固定退出命令（不受 close_preference 影响）。 */
async function quitApp(): Promise<void> {
  try {
    await quitAfterPreUninstall();
    emit("closeApp");
  } catch (error) {
    errorMessage.value = error instanceof Error ? error.message : String(error);
  }
}
</script>

<template>
  <div class="settings-danger" data-testid="settings-danger">
    <header class="settings-danger__header">
      <div>
        <h3>卸载</h3>
        <p>删除 EXV 在本机的全部数据与服务组件。此操作不可恢复。</p>
      </div>
    </header>

    <button
      type="button"
      class="danger-action"
      data-testid="pre-uninstall-open"
      @click="openConfirm"
    >
      卸载 EXV（清除全部本地数据）
    </button>

    <!-- 全过程模态：确认 / 执行中 / 结果 都在这里，设置页不摊开任何内容 -->
    <div
      v-if="phase !== 'closed'"
      class="danger-modal"
      data-testid="pre-uninstall-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="pre-uninstall-modal-title"
    >
      <div class="danger-modal__card">
        <!-- 阶段一：确认（规整表格交代要做什么） -->
        <template v-if="phase === 'confirm'">
          <h3 id="pre-uninstall-modal-title">确认卸载 EXV？</h3>
          <p class="danger-modal__warning">
            此操作不可恢复：已保存的凭据与密钥将被删除。完成后应用将退出。
          </p>
          <table class="danger-table" data-testid="pre-uninstall-plan">
            <thead>
              <tr><th>将执行</th><th>对象</th></tr>
            </thead>
            <tbody>
              <tr v-for="row in plan" :key="row.action">
                <td>{{ row.action }}</td>
                <td>{{ row.target }}</td>
              </tr>
            </tbody>
          </table>
          <p v-if="errorMessage" class="danger-modal__error" role="alert" data-testid="pre-uninstall-error">
            {{ errorMessage }}
          </p>
          <div class="danger-modal__actions">
            <button type="button" data-testid="pre-uninstall-cancel" @click="dismiss">取消</button>
            <button
              type="button"
              class="danger-action"
              data-testid="pre-uninstall-confirm-submit"
              @click="confirmUninstall"
            >
              确认卸载
            </button>
          </div>
        </template>

        <!-- 阶段二：执行中 -->
        <template v-else-if="phase === 'running'">
          <h3 id="pre-uninstall-modal-title">正在卸载…</h3>
          <p class="danger-modal__note">
            正在断开连接并清理本机数据。若弹出系统授权窗口，请由你本人输入管理员密码。
          </p>
        </template>

        <!-- 阶段三：结果（表格；每项只有一个状态） -->
        <template v-else>
          <h3 id="pre-uninstall-modal-title">卸载结果</h3>
          <p v-if="reply?.aborted" class="danger-modal__warning" data-testid="pre-uninstall-aborted">
            已中止卸载：当前连接未能停止，未删除任何数据。请先在连接页断开并确认回到「未连接」后重试。
          </p>
          <table class="danger-table" data-testid="pre-uninstall-result">
            <thead>
              <tr><th>项目</th><th class="danger-table__status">结果</th><th>说明</th></tr>
            </thead>
            <tbody>
              <tr v-for="item in resultItems" :key="item.label">
                <td>{{ item.label }}</td>
                <td class="danger-table__status" :data-testid="`pre-uninstall-status-${item.status}`">
                  {{ PRE_UNINSTALL_STATUS_LABELS[item.status] }}
                </td>
                <td class="danger-table__detail">{{ item.detail }}</td>
              </tr>
            </tbody>
          </table>
          <p
            v-if="reply?.elevation_skipped"
            class="danger-modal__note"
            data-testid="pre-uninstall-elevation-skipped"
          >
            未完成管理员授权：系统级组件（服务、/Library 产物）仍在本机，需以管理员身份重新执行或手动清理。
          </p>
          <div v-if="requiresManual" class="danger-modal__manual" data-testid="pre-uninstall-manual">
            <p>需要你手动处理：</p>
            <ul>
              <li v-for="action in reply!.manual_actions" :key="action">{{ action }}</li>
            </ul>
          </div>
          <p v-if="errorMessage" class="danger-modal__error" role="alert" data-testid="pre-uninstall-error">
            {{ errorMessage }}
          </p>
          <div class="danger-modal__actions">
            <button
              type="button"
              class="danger-action"
              data-testid="pre-uninstall-quit"
              @click="quitApp"
            >
              关闭并退出 EXV
            </button>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 并入「实验性功能」分区后不再自成一节：用一条分隔线把卸载与上方实验性设置项分开，
   取消原 `.settings-section` 的整段内边距。 */
.settings-danger {
  margin-top: var(--space-4);
  padding-top: var(--space-4);
  border-top: 1px solid var(--border-subtle);
}
.settings-danger__header {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-3);
  margin-bottom: var(--space-2);
}
.settings-danger__header h3 { margin: 0 0 2px; font-size: 15px; }
.settings-danger__header p { margin: 0; color: var(--text-secondary); font-size: 12px; }
.danger-action {
  margin-top: var(--space-2);
  padding: 6px 14px;
  border: 1px solid var(--status-error, #c0392b);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--status-error, #c0392b);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}
.danger-action:disabled { opacity: 0.6; cursor: default; }
.danger-modal {
  position: fixed;
  inset: 0;
  z-index: 40;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
}
.danger-modal__card {
  width: min(640px, calc(100vw - 48px));
  max-height: calc(100vh - 96px);
  overflow: auto;
  padding: var(--space-4);
  border-radius: var(--radius-lg, 10px);
  background: var(--surface-panel, #fff);
  color: var(--text-primary);
}
.danger-modal__card h3 { margin: 0 0 var(--space-2); font-size: 15px; }
.danger-modal__warning { color: var(--status-error, #c0392b); font-size: 13px; }
.danger-modal__note { color: var(--text-secondary); font-size: 12px; }
.danger-modal__error { color: var(--status-error, #c0392b); font-size: 12px; }
.danger-table {
  width: 100%;
  margin-top: var(--space-2);
  border-collapse: collapse;
  font-size: 12px;
}
.danger-table th,
.danger-table td {
  padding: 5px 8px;
  border-bottom: 1px solid var(--border-subtle);
  text-align: left;
  vertical-align: top;
}
.danger-table th { color: var(--text-secondary); font-weight: 600; }
.danger-table__status { width: 84px; white-space: nowrap; }
.danger-table__detail { color: var(--text-secondary); }
.danger-modal__manual { margin-top: var(--space-2); font-size: 12px; }
.danger-modal__manual ul { padding-left: 18px; }
.danger-modal__actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  margin-top: var(--space-3);
}
</style>
