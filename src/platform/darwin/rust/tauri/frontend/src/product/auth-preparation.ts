import { computed, inject, onBeforeUnmount, onMounted, ref, type InjectionKey } from "vue";
import { pushToast } from "../lib/toast";
import { kernel, onAuthPreparation } from "../lib/ipc";

export type AuthPreparationAction = "retry" | "disable_official_and_retry" | "use_embedded" | "try_saved_login" | "approve_certificate" | "cancel";
export interface AuthReceiverConfig {
  windows_start_type?: number | null;
  windows_delayed_auto_start?: boolean | null;
  launchd_targets?: { domain: string; label: string; disabled: boolean }[];
}
export interface AuthReceiverStatus {
  running: boolean;
  can_start: boolean;
  recovery_available: boolean;
  source: string;
  original_config: AuthReceiverConfig | null;
  current_config: AuthReceiverConfig | null;
}
export interface AuthReceiverReply {
  receiver: AuthReceiverStatus | null;
  completed_steps: string[];
  error: { reason?: string; message?: string; code?: string | number; stage?: string | number; native?: { code: number } | null } | null;
}
export interface AuthPreparationEvent {
  generation: number;
  kind: "exclusive" | "certificate" | "closed";
  stage?: "receiver" | "certificate" | "agent_start" | "complete";
  busy: boolean;
  message: string;
  saved_login_available: boolean;
  certificate_approval_required?: boolean;
  receiver: AuthReceiverStatus | null;
  completed_steps: string[] | null;
  native_code: number | null;
}
export interface AuthPreparationGateway {
  listen(callback: (event: AuthPreparationEvent) => void): Promise<() => void | Promise<void>>;
  respond(generation: number, action: AuthPreparationAction): Promise<void>;
  status(): Promise<AuthReceiverReply>;
  restore(): Promise<AuthReceiverReply>;
}
export const AUTH_PREPARATION_GATEWAY_KEY = "auth-preparation-gateway" as unknown as InjectionKey<AuthPreparationGateway>;
export function useAuthPreparationGateway(): AuthPreparationGateway {
  return inject(AUTH_PREPARATION_GATEWAY_KEY, {
    listen: onAuthPreparation,
    respond: (generation, action) => kernel.authPreparationRespond(generation, action),
    status: () => kernel.authReceiverStatus(),
    restore: () => kernel.restoreOfficialAuthReceiver(),
  });
}
/** 本次准备动作的拒绝已经是实际阶段与错误，保留它供同一模态重试。 */
export function authPreparationError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error && typeof error === "object") {
    const detail = error as NonNullable<AuthReceiverReply["error"]>;
    const native = detail.native?.code;
    const message = detail.message || "认证接收端操作失败，请重试。";
    return native == null ? message : `${message}（系统错误码：${native}）`;
  }
  return "认证接收端操作失败，请重试。";
}
export function receiverConfigLabel(config: AuthReceiverConfig | null): string {
  if (!config) return "暂无记录";
  if (config.windows_start_type != null) {
    switch (config.windows_start_type) {
      case 2: return config.windows_delayed_auto_start ? "自动（延迟启动）" : "自动启动";
      case 3: return "手动启动";
      case 4: return "已禁用";
      case 0: return "引导时启动";
      case 1: return "系统启动时启动";
      default: return `启动类型 ${config.windows_start_type}`;
    }
  }
  const targets = config.launchd_targets ?? [];
  if (!targets.length) return "无已登记的启动项";
  const disabled = targets.filter(t => t.disabled).length;
  return targets.length === disabled ? `已禁用（${disabled} 项）` : disabled === 0 ? `允许启动（${targets.length} 项）` : `允许启动 ${targets.length - disabled} 项，已禁用 ${disabled} 项`;
}
export function receiverStepLabel(step: string): string {
  const labels: Record<string, string> = {
    save_original_config: "已保存原启动设置", save_original_launchd_state: "已保存原启动设置",
    disable_service: "已禁用 aTrust 服务启动", stop_service: "已停止 aTrust 服务",
    cleanup_processes: "已结束残留的 aTrust 认证进程", wait_launchd_stopped: "aTrust 后台服务已停止",
    cleanup_verified_official_residuals: "已结束残留的 aTrust 认证进程",
    restore_start_type: "已恢复原启动类型", restore_delayed_auto_start: "已恢复原延迟启动设置",
    clear_recovery: "原启动设置已恢复，恢复记录已清除", remove_recovery_after_restore: "原启动设置已恢复，恢复记录已清除",
  };
  if (step.startsWith("disable:")) return "已禁用 aTrust 后台启动项";
  if (step.startsWith("bootout:")) return "已停止 aTrust 后台启动项";
  if (step.startsWith("restore:")) return "已恢复 aTrust 后台启动项的原设置";
  return labels[step] ?? step;
}
/** 同阶段的逐项成功记录汇总显示；原始步骤及目标标识继续由调用方保留。 */
export function receiverStepSummaries(steps: readonly string[]): string[] {
  const summaries: string[] = [];
  for (let index = 0; index < steps.length;) {
    const step = steps[index]!;
    const prefix = ["disable:", "bootout:", "restore:"].find(value => step.startsWith(value));
    if (!prefix) {
      summaries.push(receiverStepLabel(step));
      index += 1;
      continue;
    }
    let end = index + 1;
    while (end < steps.length && steps[end]!.startsWith(prefix)) end += 1;
    summaries.push(`${receiverStepLabel(step)}（${end - index} 项）`);
    index = end;
  }
  return summaries;
}
/** 只拥有当前准备模态；连接及认证动作的真实权限仍由 Rust 请求持有。 */
export function useAuthPreparation(onCancel: () => void) {
  const gateway = useAuthPreparationGateway();
  const event = ref<AuthPreparationEvent | null>(null);
  const submitting = ref(false);
  const pendingAction = ref<AuthPreparationAction | null>(null);
  const error = ref("");
  let latest = -1;
  let retired = -1;
  let actionSequence = 0;
  let disposed = false;
  let unlisten: (() => void | Promise<void>) | undefined;
  onMounted(async () => {
    try {
      const stop = await gateway.listen(value => {
        if (disposed || value.generation < latest || value.generation <= retired) return;
        const keepAction = value.generation === latest && value.busy;
        latest = value.generation;
        actionSequence += 1;
        submitting.value = false;
        if (!keepAction) pendingAction.value = null;
        error.value = "";
        if (value.kind === "closed") { event.value = null; if (value.message) pushToast(value.message, "warning"); }
        else event.value = value;
      });
      if (disposed) void stop(); else unlisten = stop;
    } catch { /* 预览没有宿主事件。 */ }
  });
  onBeforeUnmount(() => { disposed = true; actionSequence += 1; void unlisten?.(); });
  async function respond(action: AuthPreparationAction): Promise<void> {
    const current = event.value;
    if (!current || submitting.value || current.busy) return;
    if (action === "try_saved_login" && !current.saved_login_available) return;
    const sequence = ++actionSequence;
    submitting.value = true;
    pendingAction.value = action;
    error.value = "";
    try {
      await gateway.respond(current.generation, action);
      if (action === "cancel" && latest === current.generation) {
        retired = current.generation;
        event.value = null;
        submitting.value = false;
        pendingAction.value = null;
        onCancel();
      }
      // 命令只确认动作已入队；closed 或下一次准备事件决定模态的后续呈现。
    } catch (cause) {
      if (sequence === actionSequence && event.value?.generation === current.generation) {
        error.value = authPreparationError(cause);
        submitting.value = false;
        pendingAction.value = null;
      }
    }
  }
  return { event, error, pendingAction, busy: computed(() => submitting.value || event.value?.busy === true), respond };
}
