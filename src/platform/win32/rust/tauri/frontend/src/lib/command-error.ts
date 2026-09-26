/**
 * 命令拒绝归一为人类可读文本。
 *
 * Tauri 命令的拒绝来自 serde 标记枚举 `AppError`，形状为普通对象 `{ kind, message }`
 * 而非 JS `Error`；纯浏览器预览的 `invokeTauri` 也抛 `{ kind, message }`。
 * 已知后端错误按 `kind` 翻译为可行动的中文；未知类型保留 kind、code 和原文，避免
 * 前端臆测新的后端失败。`credential_required` 的交互分流由连接页按字段完成，本文案
 * 仅用于凭据模态内的局部诊断。
 */
export function commandErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  if (typeof error !== "object" || error === null) return fallback;

  const detail = error as { kind?: unknown; code?: unknown; message?: unknown };
  const kind = typeof detail.kind === "string" ? detail.kind.trim() : "";
  const code = typeof detail.code === "string" ? detail.code.trim() : "";
  const message = typeof detail.message === "string" ? detail.message.trim() : "";
  const codeSuffix = code ? `（错误码：${code}）` : "";

  const knownMessages: Record<string, string> = {
    credential_required: "需要填写账户和密码后才能连接。",
    failed_precondition: "当前操作的前置条件未满足，请检查服务状态后重试。",
    service_not_running: "VPN 服务尚未运行，请启动或修复服务后重试。",
    service_connect_failed: "无法连接 VPN 服务，请修复服务后重试。",
    compatibility_mode_unavailable: "当前 EXV 网络服务版本不支持兼容模式，请更新或修复 EXV 后重试。",
    core_unreachable: "无法连接本地 VPN 核心，请确认服务状态后重试。",
    not_wired: "当前运行在预览环境，无法连接 VPN；请在 EXV 桌面应用中运行。",
    internal: "应用内部处理未完成，请重试；若持续出现请提交错误码。",
  };
  if (kind && knownMessages[kind]) return `${knownMessages[kind]}${codeSuffix}`;

  if (kind) {
    const diagnostics = [
      `类型：${kind}`,
      ...(code ? [`错误码：${code}`] : []),
    ].join("；");
    return message ? `未知错误（${diagnostics}）：${message}` : `未知错误（${diagnostics}）。`;
  }
  return message || fallback;
}
