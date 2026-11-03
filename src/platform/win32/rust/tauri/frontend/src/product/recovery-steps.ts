// 服务恢复操作的步骤序列 + 状态机（纯函数，可测）。
//
// 需求（用户明确）：点「清理服务后连接 / 重装服务后连接」时，模态内容替换为步骤清单，
// 每步显示 待执行 / 执行中 / 已完成 / 失败——提高信息透明度。
//
// 架构约定（用户明确）：engine 作为服务常驻待命，core 不保留「停止服务」逻辑——服务
// 操作不拆显式 stop。重装的 install 内部已处理运行中服务（`--service-install` REPAIR
// 路径：stop_service_instance + 改配置），所以重装 = 安装并启动服务 → 建立连接（2 步，
// 不额外 stop，也避免多一次 UAC）。

export type RecoveryStepStatus = "pending" | "current" | "done" | "failed";

export interface RecoveryStepDef {
  id: string;
  label: string;
}

export interface RecoveryStep extends RecoveryStepDef {
  status: RecoveryStepStatus;
}

export type RecoveryAction = "clean" | "reinstall";

/** 恢复动作 → 步骤定义（顺序即执行顺序）。 */
export const RECOVERY_STEP_DEFS: Record<RecoveryAction, readonly RecoveryStepDef[]> = {
  clean: [
    { id: "uninstall", label: "卸载服务（切换一次性引擎）" },
    { id: "connect", label: "建立连接" },
  ],
  reinstall: [
    { id: "install", label: "安装并启动服务" },
    { id: "connect", label: "建立连接" },
  ],
};

/**
 * P0-2（计划 v6.1 第二批，2026-09-28 根因 B）：从服务失败消息中提取可读短语。
 *
 * host/engine 侧稳定前缀 `service_not_running|<exit_code>|<短语>`（engine 批量
 * Verify 快速失败产出；可能嵌在 `…; repair batch failed: <前缀消息>` 等复合串中）。
 * 命中 → 返回短语（模态「重试服务安装」选项携带失败原因）；未命中 → 原样返回
 * （诚实透传，不伪造归因）。
 */
export function serviceFailurePhrase(message: string): string {
  const match = message.match(/service_not_running\|\d+\|([^;]+)/);
  const phrase = match?.[1]?.trim();
  return phrase ? phrase : message;
}

/** 初始步骤列表（全部 pending）。 */
export function initialSteps(action: RecoveryAction): RecoveryStep[] {
  return RECOVERY_STEP_DEFS[action].map((step) => ({ ...step, status: "pending" }));
}

/** 把指定 step 置为 current（已 done 的保持 done，其余 pending）。 */
export function markCurrent(steps: RecoveryStep[], id: string): RecoveryStep[] {
  return steps.map((step) =>
    step.id === id
      ? { ...step, status: "current" }
      : step.status === "done"
        ? step
        : { ...step, status: "pending" },
  );
}

/** 把指定 step 置为 done。 */
export function markDone(steps: RecoveryStep[], id: string): RecoveryStep[] {
  return steps.map((step) => (step.id === id ? { ...step, status: "done" } : step));
}

/** 把指定 step 置为 failed（其余保持）。 */
export function markFailed(steps: RecoveryStep[], id: string): RecoveryStep[] {
  return steps.map((step) => (step.id === id ? { ...step, status: "failed" } : step));
}

/** 步骤标记符号（展示用）：pending ○ / current ● / done ✓ / failed ✗。 */
export function stepMarker(status: RecoveryStepStatus): string {
  switch (status) {
    case "current":
      return "●";
    case "done":
      return "✓";
    case "failed":
      return "✗";
    default:
      return "○";
  }
}
