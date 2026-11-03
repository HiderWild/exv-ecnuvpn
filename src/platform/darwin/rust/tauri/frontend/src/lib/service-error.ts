/**
 * 服务控制命令的 `internal` 诊断已由 host 清理，可在对应的服务操作界面 中显示。
 * 这条窄路径不能进入通用格式化器，连接页和凭据模态必须继续只获得稳定中文文案。
 */
export function serviceInternalDiagnostic(error: unknown): string | null {
  if (typeof error !== "object" || error === null) return null;
  const detail = error as { kind?: unknown; code?: unknown; message?: unknown };
  if (typeof detail.kind !== "string" || detail.kind.trim() !== "internal") return null;
  if (typeof detail.message !== "string" || !detail.message.trim()) return null;
  const code = typeof detail.code === "string" ? detail.code.trim() : "";
  return `${detail.message.trim()}${code ? `（错误码：${code}）` : ""}`;
}
