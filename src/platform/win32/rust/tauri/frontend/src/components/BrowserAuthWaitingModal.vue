<script setup lang="ts">
import { computed, inject } from "vue";
import { APPEARANCE_KEY } from "../product/appearance";

import {
  browserAuthAdvice,
  browserAuthStageLabel,
  type BrowserAuthChannel,
} from "../lib/command-error";
import type { LoginThrottleWarning } from "../lib/ipc";
import ServiceOperationOverlay from "./ServiceOperationOverlay.vue";

/**
 * 等待浏览器认证 / 自动登录进行中 / 认证失败回应的承载模态。
 *
 * §7.6：不再自带样式——渲染进 ServiceOperationOverlay（服务安装/卸载遮罩），
 * 主题 token 只有一份来源，深色/浅色自动正确。
 * §7.5：`mode="error"` 时按 Rust 稳定码给出回应选项。
 * §7.9：`mode="auto-login"` 时标题为「正在为您进行登录操作」，第二行实时显示阶段。
 * 修复 5a（2026-10-08 UI 重试脱同步）：错误模态不再提供「手动介入」——命令已
 * reject 后保留的隐藏窗口没有监听回路，用户在其中完成登录不会被观测（死窗口）；
 * 「接管」留给重试后的等待模态（那里命令在飞、系统浏览器回退有回路）。错误态
 * 两个渠道统一为「重试 / 取消」，`channel` 仅用于区分补充说明文案。
 * 验证码风险预告（2026-10-07）：`throttleWarning` 非空时追加一行固定句式提示。
 * E10（2026-10-09 区域 C 复审裁定）：预告行**所有模式都渲染，含错误态**——警示的
 * 决策价值恰在错误态达到峰值（重试正是滚动窗口内继续累计尝试、可能触发验证码的
 * 动作；此刻收回次数预告等于在最需要信息时收回信息）。与错误态的 advice 行分层
 * 并存：预告行携带窗口计数与验证码风险，advice 行是错误码对应的行动指引，互不
 * 替代。句式模板**只在本组件一处**（`throttleWarningText`），数字来自宿主
 * `exv://browser-login` 载荷（与 Rust `login_attempt_tracker.rs` 常量同源：
 * attempts = 窗口内已记录次数（>= 阈值），window_minutes = 30，threshold = 3）。
 * C4（2026-10-28 原生系统浏览器认证模式）：等待态标题按渠道区分——系统浏览器
 * 渠道是用户自己的默认浏览器（EXV 打开后零控制、等待登录票据到达），用
 * 「请在浏览器中完成登录」；内嵌渠道仍是 EXV 受控窗口，保留「弹出的浏览器」表述。
 */
const props = withDefaults(defineProps<{
  visible: boolean;
  ticketRefreshReason?: "missing" | "expired" | null;
  /** "waiting" 等待人工认证；"auto-login" 自动登录进行中；"error" 认证失败回应。 */
  mode?: "waiting" | "auto-login" | "error";
  /** 仅内嵌模式为 true（可切系统浏览器回退）；错误态不据此隐藏选项。 */
  allowSystemBrowser: boolean;
  /** §7.10-3：本次认证实际渠道；决定错误态补充说明文案（错误选项两渠道一致）。 */
  channel?: BrowserAuthChannel;
  /** §7.9：自动登录阶段（宿主回传的稳定 stage；null/缺失回落首阶段）。 */
  stage?: string | null;
  /** 验证码风险预告（本次认证开始事件携带；null = 不提示）。 */
  throttleWarning?: LoginThrottleWarning | null;
  busy: boolean;
  /** 等待态 = 回退错误；错误态 = Rust 归一化固定文案。 */
  error: string;
  /** 浏览器认证失败稳定码（页面分类码或 null）。 */
  errorCode?: string | null;
  /**
   * Rust 净化后的服务端消息摘录（2026-10-07 终端页研究：失败页 #errorData 文本 /
   * 限流页 err_info 提示）；null = 无服务端消息。以独立行展示，原样不改写。
   */
  serverDetail?: string | null;
}>(), {
  mode: "waiting",
  channel: "embedded",
  stage: null,
  throttleWarning: null,
  errorCode: null,
  serverDetail: null,
});

const emit = defineEmits<{
  cancel: [];
  "system-browser": [];
  retry: [];
}>();

const appearance = inject(APPEARANCE_KEY, null);
const minimal = computed(() => appearance?.state.value.mode === "minimal");
const isError = computed(() => props.mode === "error");
const isAutoLogin = computed(() => props.mode === "auto-login");
const advice = computed(() => browserAuthAdvice(props.errorCode, props.channel));
const stageLabel = computed(() => browserAuthStageLabel(props.stage));
const label = computed(() => {
  if (isError.value) return "认证未完成";
  if (isAutoLogin.value) return "正在为您进行登录操作";
  // C4（2026-10-28 原生系统浏览器模式）：系统浏览器是用户自己的默认浏览器
  //（EXV 打开后零控制），等待语义是「在浏览器完成登录→EXV 等到登录票据」；
  // 与内嵌模式的「弹出的浏览器」（EXV 受控窗口）区分表述。
  if (props.channel === "system-browser") return "请在浏览器中完成登录";
  return "请您在弹出的浏览器中认证";
});
/** 验证码风险预告句（模板唯一来源；数字取自宿主载荷）。 */
const throttleWarningText = computed(() => props.throttleWarning
  ? `您在${props.throttleWarning.window_minutes}分钟内已经尝试登录过${props.throttleWarning.attempts}次，可能触发验证码，请您留意`
  : null);

function requestCancel(): void {
  if (props.visible && !props.busy) emit("cancel");
}
</script>

<template>
  <ServiceOperationOverlay
    :visible="visible"
    :label="label"
    :spinner="!isError"
    label-id="auth-wait-title"
    test-id="auth-wait-overlay"
    @escape="requestCancel"
  >
    <div class="auth-wait-body" :tabindex="minimal ? 0 : undefined" :role="minimal ? 'region' : undefined" :aria-label="minimal ? '认证详细说明' : undefined">
      <p v-if="!isError && ticketRefreshReason && stage !== 'awaiting_manual'" class="auth-wait-note" data-testid="auth-wait-ticket-refresh" role="status">正在获取登录票据，本次认证时间会稍长。后续连接可复用登录信息，连接会更快。</p>
      <p v-if="!isError && (isAutoLogin || stage === 'recovering')" class="auth-wait-stage" data-testid="auth-wait-stage" aria-live="polite">{{ stageLabel }}</p>
      <p v-else-if="!isError" class="auth-wait-note">完成学校认证后，EXV 将继续建立连接。</p>
      <!-- E10：不设 isError 守卫——错误态（重试决策点）正是预告价值峰值时刻。 -->
      <p v-if="throttleWarningText" class="auth-wait-warning" data-testid="auth-wait-throttle-warning">{{ throttleWarningText }}</p>
      <p v-if="error" class="auth-wait-error" role="alert" data-testid="auth-wait-error">{{ error }}</p>
      <p v-if="isError && serverDetail" class="auth-wait-server-detail" data-testid="auth-wait-server-detail">网关返回：{{ serverDetail }}</p>
      <p v-if="isError && advice" class="auth-wait-advice" data-testid="auth-wait-advice">{{ advice }}</p>
    </div>
    <div class="auth-wait-actions">
      <template v-if="isError">
        <!-- 修复 5a：不再提供「手动介入」——命令已 reject 后保留的隐藏窗口没有
             监听回路（死窗口）；接管留给重试后的等待模态（命令在飞、有回路）。 -->
        <button type="button" data-testid="auth-wait-retry" :disabled="busy" @click="$emit('retry')">重试</button>
        <button type="button" class="auth-wait-actions__secondary" data-testid="auth-wait-cancel" :disabled="busy" @click="$emit('cancel')">取消</button>
      </template>
      <template v-else-if="isAutoLogin">
        <button type="button" class="auth-wait-actions__secondary" data-testid="auth-wait-cancel" :disabled="busy" @click="$emit('cancel')">取消</button>
      </template>
      <template v-else>
        <button type="button" class="auth-wait-actions__secondary" data-testid="auth-wait-cancel" :disabled="busy" @click="$emit('cancel')">取消</button>
        <button v-if="allowSystemBrowser" type="button" data-testid="auth-wait-fallback" :disabled="busy" @click="$emit('system-browser')">使用系统浏览器</button>
      </template>
    </div>
  </ServiceOperationOverlay>
</template>

<style scoped>
/* 只保留动作/错误区自身的排版与语义色；面板背景/边框/文字色一律由
   ServiceOperationOverlay 的 token 提供，避免第二份主题来源。 */
.auth-wait-note {
  margin: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-body);
  line-height: 1.5;
  text-align: center;
}

.auth-wait-stage {
  margin: 0;
  color: var(--text-primary);
  font-size: var(--font-size-caption);
  line-height: 1.5;
  text-align: center;
}

.auth-wait-error {
  margin: 0;
  color: var(--state-danger);
  font-size: var(--font-size-caption);
  line-height: 1.5;
  text-align: center;
}

/* 服务端消息摘录（网关返回的原文）：与固定错误文案同为错误语义色，独立成行，
   让真实原因与固定提示可区分。 */
.auth-wait-server-detail {
  margin: 0;
  color: var(--state-danger);
  font-size: var(--font-size-caption);
  line-height: 1.5;
  text-align: center;
  overflow-wrap: anywhere;
}

/* 验证码风险预告：弱警示（warning 语义），与错误色区分——这是预期提示，
   不是失败。 */
.auth-wait-warning {
  margin: 0;
  color: var(--state-warning);
  font-size: var(--font-size-caption);
  line-height: 1.5;
  text-align: center;
}

.auth-wait-advice {
  margin: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
  line-height: 1.5;
  text-align: center;
}

.auth-wait-actions {
  display: flex;
  justify-content: center;
  gap: 10px;
  flex-wrap: wrap;
}

.auth-wait-actions button {
  min-height: 38px;
  padding: 6px 16px;
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  background: var(--accent);
  color: var(--accent-on);
  font-size: var(--font-size-body);
  cursor: pointer;
}

.auth-wait-actions button:disabled {
  cursor: wait;
  opacity: 0.6;
}

.auth-wait-actions__secondary {
  border-color: var(--border-subtle) !important;
  background: var(--surface-raised) !important;
  color: var(--text-primary) !important;
}

/* 极简：动作区同步收敛，避免 328px 窗口内换行溢出。 */
:global(.service-overlay--minimal) .auth-wait-actions {
  gap: 4px;
}

:global(.service-overlay--minimal) .auth-wait-actions button {
  min-height: 24px;
  padding: 2px 8px;
  font-size: var(--font-size-body);
}
</style>
