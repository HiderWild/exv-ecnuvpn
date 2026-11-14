<script setup lang="ts">
import { computed } from "vue";
import type { BrowserLoginMilestone } from "../lib/ipc";
import { useDialogFocus } from "./use-dialog-focus";
const props = defineProps<{
  visible: boolean;
  ticketRefreshReason?: "missing" | "expired";
  compact: boolean;
  mode: "embedded" | "browser" | "headless";
  /** 后台认证使用本次冻结意图；普通连接沿用原等待文案。 */
  autoLogin?: boolean;
  cancelling: boolean;
  /** 只呈现宿主实际报告的里程碑；缺失时保留既有等待文案。 */
  milestone?: BrowserLoginMilestone;
}>();
const emit = defineEmits<{ cancel: [] }>();
const instruction = computed(() => props.mode === "browser"
  ? "请在浏览器中完成学校认证。"
  : props.mode === "headless"
    ? "自动登录正在等待学校响应；需要交互时可打开内嵌浏览器。"
    : props.autoLogin === true
      ? "正在内嵌浏览器自动完成学校认证。"
      : "请在内嵌浏览器完成学校认证。");
const milestoneMessage = computed(() => {
  switch (props.milestone) {
    case "recovering": return "首次认证遇到异常，正在自动重试（2/2）…";
    case "opening": return "正在打开学校认证页面，请稍候。";
    case "school_login": return props.mode === "browser"
      ? "请在浏览器中完成学校认证。" : "正在等待学校登录页面处理本次认证。";
    case "submitting": return props.mode === "browser"
      ? "学校登录页面正在提交认证信息，请等待页面响应。" : "正在提交本次学校认证请求，请稍候。";
    case "gateway_confirming": return "正在等待 VPN 网关确认本次认证结果。";
    case "awaiting_manual": return props.mode === "browser"
      ? "请在浏览器中完成剩余确认。" : "本次认证需要人工确认，请打开登录窗口继续。";
    case "ticket_invalid": return "登录票据已失效，请重新在浏览器中登录。";
    default: return null;
  }
});
function cancel() { if (!props.cancelling) emit("cancel"); }
const { dialog, onDialogKeydown } = useDialogFocus(() => props.visible, cancel);
</script>
<template>
  <div v-if="visible" ref="dialog" class="browser-login-wait exv-dialog" :data-compact="compact" role="dialog" aria-modal="true" aria-labelledby="browser-login-title" data-testid="web-login-waiting" tabindex="-1" :aria-busy="cancelling" @keydown="onDialogKeydown">
    <section class="browser-login-card exv-dialog-card">
      <h2 id="browser-login-title">等待学校网页登录</h2>
      <div class="browser-login-body" role="region" aria-label="网页登录说明" tabindex="0">
        <p>{{ instruction }}</p>
        <p v-if="ticketRefreshReason && milestone !== 'awaiting_manual'" data-testid="web-login-ticket-refresh" role="status">正在获取登录票据，本次认证时间会稍长。后续连接可复用登录信息，连接会更快。</p>
        <p v-if="mode === 'browser'" data-testid="web-login-manual-browser">登录由你在浏览器中手动完成，EXV 不会自动填写或提交账户密码；连接建立后浏览器页面如提示异常，可直接关闭，不影响 EXV 连接。</p>
        <p v-if="milestoneMessage" role="status" aria-live="polite" data-testid="web-login-milestone">{{ milestoneMessage }}</p>
      </div>
      <div class="modal-actions">
        <button type="button" data-testid="web-login-cancel" :disabled="cancelling" @click="cancel">{{ cancelling ? '正在取消…' : '取消登录' }}</button>
      </div>
    </section>
  </div>
</template>
<style scoped>
.browser-login-wait { position:fixed; inset:0; z-index:45; display:flex; align-items:center; justify-content:center; }
.browser-login-card { display:flex; max-height:calc(100dvh - 36px); flex-direction:column; min-height:0; }
.browser-login-card h2 { flex:none; }
.browser-login-body { min-height:0; min-width:0; overflow-y:auto; overscroll-behavior:contain; overflow-wrap:anywhere; font-size: var(--font-size-caption); line-height:1.7; }
.browser-login-body p { margin:0 0 8px; }
.modal-actions { display:flex; flex-wrap:wrap; gap:8px; }
.modal-actions { flex:none; padding:5px 5px 8px; margin:8px -5px -8px; }
[data-compact="true"].browser-login-wait { padding:8px; }
[data-compact="true"] .browser-login-card { padding:8px; max-height:calc(100dvh - 16px); }
[data-compact="true"] .browser-login-card h2 { margin:0 0 4px; font-size: var(--font-size-heading); line-height:1.3; }
[data-compact="true"] .browser-login-body { flex:0 1 auto; min-height:1.3em; font-size: var(--font-size-caption); line-height:1.3; }
[data-compact="true"] .modal-actions { margin-top:4px; gap:4px; }
[data-compact="true"] .modal-actions button { min-height:24px; padding:2px 7px; font-size: var(--font-size-body); line-height:1.3; }
</style>
