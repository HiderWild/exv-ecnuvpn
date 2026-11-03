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

  const detail = error as { kind?: unknown; code?: unknown; message?: unknown; server_detail?: unknown };
  const kind = typeof detail.kind === "string" ? detail.kind.trim() : "";
  const code = typeof detail.code === "string" ? detail.code.trim() : "";
  const message = typeof detail.message === "string" ? detail.message.trim() : "";
  const codeSuffix = code ? `（错误码：${code}）` : "";

  // 服务原操作已提供本次阶段与原因，沿原kind保留恢复动作分流。
  if (["service_not_running", "service_connect_failed"].includes(kind) && message) {
    return `${message}${codeSuffix}。请启动或修复服务后重试。`;
  }

  // 该类型的 message 已由 Rust 固定文案表归一化；其他 internal 仍保持通用提示。
  if (kind === "browser_login_failed") {
    const base = message || "浏览器登录未完成，请重试；若持续出现请反馈应用日志。";
    // 2026-10-07 终端页研究：失败/限流类可携带净化后的服务端消息摘录（独立字段），
    // toast 路径拼入括号后缀；模态路径由 BrowserAuthWaitingModal 以独立行展示。
    const serverDetail = typeof detail.server_detail === "string" && detail.server_detail.trim()
      ? detail.server_detail.trim()
      : null;
    return serverDetail ? `${base}（网关返回：${serverDetail}）` : base;
  }
  if (kind === "preflight_blocked") {
    const reasons = code.split(",").map(value => value.trim());
    const advice: string[] = [];
    // 2026-10-09 接缝终裁：旧「校园 DNS 规则冲突」指引行（nrpt_foreign_conflict）
    // 整行删除——D8 阶段2 静默精准接管后外来冲突不再进入阻断集（该 reason 不再
    // 到达前端命令拒绝通道；触发面消失）。
    //
    // 清理类（cleanup_unresolved/cleanup_failed）＝仍阻断形态（路由/代理域；NRPT
    // 域自愈按 D8 §2.2.3 只记日志零 toast，不进入本通道）：D8 §2.2.4 改写句
    // （去「重新断开」——对未连接用户该动作不存在）。**不做会话级去重**——每次
    // 被拦都应告知（2026-10-09 裁定 D9 #3：去重键按「自愈成功/仍阻断」分流，
    // 仅自愈成功引导句可按 reason+指纹会话去重；自愈成功形态已按 D8 零 toast，
    // 本通道只剩仍阻断形态，恒弹）。
    if (reasons.includes("cleanup_unresolved") || reasons.includes("cleanup_failed")) advice.push("上次断开后的网络残留未能自动清理；已自动重试仍失败。请重启电脑后再试；仍失败请导出日志反馈，勿手动删除不明网络规则。");
    return advice.length ? advice.join(" ") : `连接失败：连接前检查未通过${codeSuffix}。请查看日志中的具体原因后重试。`;
  }

  const knownMessages: Record<string, string> = {
    credential_required: "需要填写账户和密码后才能连接。",
    failed_precondition: "当前操作的前置条件未满足，请检查服务状态后重试。",
    service_not_running: "VPN 服务尚未运行，请启动或修复服务后重试。",
    service_connect_failed: "无法连接 VPN 服务，请修复服务后重试。",
    compatibility_mode_unavailable: "当前 EXV 网络服务版本不支持兼容模式，请更新或修复 EXV 后重试。",
    protocol_unavailable: "当前版本无法使用所选协议；请在设置中改用其它协议（例如 Cisco），或更新到支持该协议的 EXV 版本。",
    core_unreachable: "无法连接本地 VPN 核心，请确认服务状态后重试。",
    core_wake_failed: "无法恢复后台核心进程；请重启程序后重试。",
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

/** §7.5：页面分类稳定码；前端据 `code` 分流三选项，不解析自由文本。 */
export const BROWSER_AUTH_PAGE_CODES = [
  "browser_auth_client_install_required",
  "browser_auth_failed_page",
  "browser_auth_gateway_throttled",
  "browser_auth_stalled",
] as const;

export type BrowserAuthPageCode = (typeof BROWSER_AUTH_PAGE_CODES)[number];

export interface BrowserAuthIntervention {
  /** 稳定机器码；既有浏览器认证失败类可能没有（null）。 */
  code: string | null;
  /** Rust 已归一化的中文固定文案（前端不臆测、不改写）。 */
  message: string;
  /**
   * Rust 净化后的服务端消息摘录（2026-10-07 终端页研究：失败页 `#errorData` 文本 /
   * 限流页 `err_info` 提示）；null = 该类失败无服务端消息可携带。前端原样展示，
   * 不解析、不改写。
   */
  serverDetail: string | null;
}

/**
 * 从命令拒绝中提取浏览器认证失败信息，供三选项模态分流。
 *
 * 只按 `kind === "browser_login_failed"` 识别（含四个页面分类码与既有浏览器认证
 * 失败类）；其余错误返回 null，保持既有 toast/模态路径。
 */
export function browserAuthIntervention(error: unknown): BrowserAuthIntervention | null {
  if (typeof error !== "object" || error === null) return null;
  const detail = error as {
    kind?: unknown;
    code?: unknown;
    message?: unknown;
    server_detail?: unknown;
  };
  if (detail.kind !== "browser_login_failed") return null;
  const code = typeof detail.code === "string" && detail.code.trim() ? detail.code.trim() : null;
  const message = typeof detail.message === "string" && detail.message.trim()
    ? detail.message.trim()
    : "浏览器登录未完成，请重试；若持续出现请反馈应用日志。";
  const serverDetail = typeof detail.server_detail === "string" && detail.server_detail.trim()
    ? detail.server_detail.trim()
    : null;
  return { code, message, serverDetail };
}

/** §7.10-3 渠道边界：本次浏览器认证实际走的是哪条渠道。
 *
 * E12（阶段②附带）：增 `"headless"` 三值——headless 配置此前被 core-config 折叠进
 * "browser"→"system-browser"，无头自动登录失败后的建议句误导用户去系统浏览器找
 * 会话。三值与壳侧 `resolved_atrust_auth_mode` 的「headless+autoLogin 保持
 * headless」裁定对齐。
 */
export type BrowserAuthChannel = "embedded" | "system-browser" | "headless";

/**
 * §7.9：宿主经 `exv://browser-login` 回传的自动登录阶段 → 用户可读阶段行。
 *
 * 只有宿主才有的阶段（到达学校页/提交/网关确认）在此映射；`awaiting_manual`
 * 不映射为阶段行——它表示需要人工，前端据此切回既有等待模态（见 ConnectionInteractionHost）。
 */
export const BROWSER_AUTH_STAGE_LABELS: Readonly<Record<string, string>> = {
  opening: "正在打开认证页",
  school_login: "已到达学校登录页",
  submitting: "正在提交登录信息",
  gateway_confirming: "等待网关确认登录结果",
  recovering: "首次认证遇到异常，正在自动重试（2/2）…",
  // 前端合成的连接相位（浏览器认证完成、Core 连接受理中）。
  connecting: "正在建立连接",
};

/** 需要人工接管的阶段哨兵；不作为自动登录阶段行展示。 */
export const BROWSER_AUTH_MANUAL_STAGE = "awaiting_manual";

/** 阶段行文案；未知/缺失阶段回落首个阶段，避免出现空白。 */
export function browserAuthStageLabel(stage: string | null | undefined): string {
  return (stage ? BROWSER_AUTH_STAGE_LABELS[stage] : undefined) ?? "正在打开认证页";
}

/** 按稳定码给出错误回应的补充说明（Rust 文案之外的「可操作出路」）。
 *
 * 修复 5a（2026-10-08 UI 重试脱同步）：错误模态不再提供「手动介入」（命令已
 * reject 后保留的隐藏窗口没有监听回路，是死窗口）；两个渠道统一为
 * 「重试 / 取消」。
 *
 * P3 信号化（2026-10-11 计划；P0 审计 ⑥-5）：补充说明只陈述**错误码证明的事实**
 * 与**重试机制**，不再做渠道推断——旧版系统浏览器渠道无条件断言「你已在系统
 * 浏览器中手动认证」，但渠道值只是配置/回退推导（resolveAuthChannel），证明不了
 * 用户已在浏览器里完成认证（失败码只到停滞/未分类时更无从谈起；学校 SSO 也可能
 * 在浏览器内自动走完）。渠道仅用于描述重试形态（内嵌=等待窗口重新认证；
 * 系统浏览器=重新打开认证页；headless=先自动登录），不用于断言用户已做过什么。
 */
export function browserAuthAdvice(code: string | null, channel: BrowserAuthChannel = "embedded"): string {
  // E12：无头渠道——失败后重试将先走无头快路径，需要人工时自动弹出内嵌窗口；
  // 不引导用户去系统浏览器（与 headless 的自动化语义一致）。
  if (channel === "headless") {
    switch (code) {
      case "browser_auth_client_install_required":
        return "网关要求使用官方客户端完成认证。可以重试本次连接（将先自动登录，需要人工时弹出认证窗口），或取消本次连接。";
      case "browser_auth_failed_page":
        return "自动登录未通过。可以重试本次连接（将先自动登录，需要人工时弹出认证窗口），或取消本次连接。";
      case "browser_auth_gateway_throttled":
        return "网关提示登录受限（如短时间重复登录或并发登录冲突）。可以稍候片刻后重试本次连接，或取消本次连接。";
      case "browser_auth_stalled":
        return "自动登录长时间没有进展。可以重试本次连接（需要人工时弹出认证窗口），或取消本次连接。";
      default:
        return "可以重试本次连接（将先自动登录，需要人工时弹出认证窗口），或取消本次连接。";
    }
  }
  if (channel === "system-browser") {
    // 系统浏览器已接统一编排器，阶段里程碑由 system_auth 转发；此处只按终结
    // 错误码陈述失败事实，缺少错误码时不从阶段推测原因。
    switch (code) {
      case "browser_auth_client_install_required":
        return "网关要求使用官方客户端完成认证，本次浏览器登录未能继续。可以重试本次连接（将重新打开系统浏览器认证页），或取消本次连接。";
      case "browser_auth_failed_page":
        return "认证未通过（学校/网关已给出失败结果）。可以重试本次连接（将重新打开系统浏览器认证页），或取消本次连接。";
      case "browser_auth_gateway_throttled":
        return "网关提示登录受限（如短时间重复登录或并发登录冲突）。可以稍候片刻后重试本次连接（将重新打开系统浏览器认证页），或取消本次连接。";
      case "browser_auth_stalled":
        return "认证长时间没有进展（浏览器页面未继续推进，尚未取得认证结果）。可以重试本次连接（将重新打开系统浏览器认证页），或取消本次连接。";
      default:
        return "浏览器登录未完成。可以重试本次连接（将重新打开系统浏览器认证页），或取消本次连接。";
    }
  }
  switch (code) {
    case "browser_auth_client_install_required":
      return "网关要求使用官方客户端完成认证。可以重试本次连接（将在等待窗口中重新认证），或取消本次连接。";
    case "browser_auth_failed_page":
      return "认证未通过。可以重试本次连接（将在等待窗口中重新认证），或取消本次连接。";
    case "browser_auth_gateway_throttled":
      return "网关提示登录受限（如短时间重复登录或并发登录冲突）。可以稍候片刻后重试本次连接，或取消本次连接。";
    case "browser_auth_stalled":
      return "认证长时间没有进展。可以重试本次连接（将在等待窗口中重新认证），或取消本次连接。";
    default:
      return "可以重试本次连接（将在等待窗口中重新认证），或取消本次连接。";
  }
}


/** 沿用宿主类型及旧版固定取消值；超时、传输关闭和页面故障继续走错误呈现。 */
export function isBrowserLoginCancelled(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const detail = error as { kind?: unknown; code?: unknown; message?: unknown };
  if (detail.kind === "connection_cancelled") return true;
  return detail.kind === "browser_login_failed" && !detail.code
    && (detail.message === "登录已取消" || detail.message === "浏览器登录已取消");
}
