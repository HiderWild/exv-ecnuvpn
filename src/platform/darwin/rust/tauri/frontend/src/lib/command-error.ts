/**
 * 命令拒绝归一为人类可读文本。
 *
 * Tauri 命令的拒绝来自 serde 标记枚举 `AppError`，形状为普通对象 `{ kind, message }`
 * 而非 JS `Error`；纯浏览器预览的 `invokeTauri` 也抛 `{ kind, message }`。
 * 已知后端错误按 `kind` 翻译为可行动的中文；未知类型保留 kind、code 和原文，避免
 * 前端臆测新的后端失败。`credential_required` 的交互分流由连接页按字段完成，本文案
 * 仅用于凭据模态内的局部诊断。
 *
 * 立场更新（S2，2026-10-26 验收修复计划）：darwin engine 恢复连接门预检后，
 * `DARWIN_CORE_PREFLIGHT_BLOCKED|<reason_ids>`（resolver/路由域清理未收敛）与
 * `DARWIN_CORE_ATRUST_PROBE_UNAVAILABLE`（探测网络层失败，与凭据缺失分离）两个
 * 稳定码经壳层 Internal kind 的 message 透传到达本文件（与 ENGINE_ATTEMPT_ACTIVE
 * 同构），按稳定码路由可行动文案；win32 同语义 preflight_blocked 走 kind 路由。
 */

/** core 稳定码（2026-09-30 修复 M3）：engine 会话占用——壳层 message 已带稳定码
 * 打头的中文行动指引，原样透出。 */
const ENGINE_ATTEMPT_ACTIVE_CODE = "DARWIN_ENGINE_ATTEMPT_ACTIVE";

/** S2（2026-10-26 验收修复计划）：engine 连接门预检阻断稳定码前缀。message 形态
 * `DARWIN_CORE_PREFLIGHT_BLOCKED|<reason_ids>|<摘要>`（壳层 Internal kind 透传；
 * reason id 逗号分隔，resolver/route 的 cleanup_failed/cleanup_unresolved 为
 * 清理未收敛类）。win32 同语义错误为 kind=preflight_blocked。 */
const PREFLIGHT_BLOCKED_CODE = "DARWIN_CORE_PREFLIGHT_BLOCKED";
const PREFLIGHT_CLEANUP_REASON_IDS = [
  "resolver_cleanup_failed",
  "resolver_cleanup_unresolved",
  "route_cleanup_failed",
  "route_cleanup_unresolved",
];

/** S2：core 缓存探测网络层失败独立码（与凭据缺失分离——不再弹凭据模态）。 */
const ATRUST_PROBE_UNAVAILABLE_CODE = "DARWIN_CORE_ATRUST_PROBE_UNAVAILABLE";

/** 预检阻断文案：直陈事实（残留清不掉、本次连接已停止）+ 可执行动作。 */
function preflightBlockedMessage(message: string, codeSuffix: string): string {
  const reasonIds = (message.split("|")[1] ?? "").split(",").map(id => id.trim());
  const cleanup = reasonIds.some(id => PREFLIGHT_CLEANUP_REASON_IDS.includes(id));
  if (cleanup) {
    return `上次断开留下的网络残留未能自动清理，本次连接已停止。请重启电脑后再试；仍失败请导出日志反馈，勿手动删除系统网络配置文件。${codeSuffix}`;
  }
  return `连接前的系统检查未通过，本次连接已停止。请查看日志中的具体原因后重试。${codeSuffix}`;
}

export function commandErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  if (typeof error !== "object" || error === null) return fallback;

  const detail = error as { kind?: unknown; code?: unknown; message?: unknown };
  const kind = typeof detail.kind === "string" ? detail.kind.trim() : "";
  const code = typeof detail.code === "string" ? detail.code.trim() : "";
  const message = typeof detail.message === "string" ? detail.message.trim() : "";
  const codeSuffix = code ? `（错误码：${code}）` : "";

  // 服务原操作已提供本次阶段与原因，沿原kind保留恢复动作分流。
  if (["service_not_running", "service_connect_failed"].includes(kind) && message) {
    return `${message}${codeSuffix}。请启动或修复服务后重试。`;
  }

  if (message.includes("DARWIN_CORE_ATRUST_GATEWAY_INVALID")) return message;
  if (kind === "internal" && message.startsWith("DARWIN_CORE_SERVICE_ELEVATION_FAILED:")) return `${message}${codeSuffix}`;
  if (["authentication_context_changed", "browser_login_failed"].includes(kind) && message) return message;
  // M3（2026-09-30）：engine 会话占用稳定码同构透传（壳层已带可行动中文指引）。
  if (message.includes(ENGINE_ATTEMPT_ACTIVE_CODE)) return message;
  // S2（2026-10-26）：连接门预检阻断——按 reason id 路由可行动文案（壳层 Internal
  // kind 携带稳定码 message 透传，与 ENGINE_ATTEMPT_ACTIVE 同构）。
  if (message.includes(PREFLIGHT_BLOCKED_CODE)) {
    return preflightBlockedMessage(message, codeSuffix);
  }
  // S2：校园网探测超时=网络层未知结果（非「未登录」）——直陈残留成因与出路，
  // 不再引导重填密码。
  if (message.includes(ATRUST_PROBE_UNAVAILABLE_CODE)) {
    return `校园网探测超时，通常是上次意外断开留下的网络残留；请重试，若持续出现请重启电脑后再试。${codeSuffix}`;
  }

  const knownMessages: Record<string, string> = {
    credential_required: "需要填写账户和密码后才能连接。",
    failed_precondition: "当前操作的前置条件未满足，请检查服务状态后重试。",
    service_not_running: "VPN 服务尚未运行，请启动或修复服务后重试。",
    service_not_installed: "服务未安装。可到服务面板安装常驻服务；未安装时点击连接将请求管理员密码以一次性运行引擎（不安装系统服务）。",
    service_connect_failed: "无法连接 VPN 服务，请修复服务后重试。",
    protocol_unavailable: "当前版本尚未接入 aTrust 后端；请在设置中改用 Cisco，或更新到支持 aTrust 的版本。",
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


/** 沿用宿主类型及旧版固定取消值；超时、传输关闭和页面故障继续走错误呈现。 */
export function isBrowserLoginCancelled(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const detail = error as { kind?: unknown; code?: unknown; message?: unknown };
  if (detail.kind === "connection_cancelled") return true;
  return detail.kind === "browser_login_failed" && !detail.code
    && (detail.message === "登录已取消" || detail.message === "浏览器登录已取消");
}
