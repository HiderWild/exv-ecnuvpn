import { kernel } from "../lib/ipc";
import {
  DEFAULT_CONNECTION_MODE,
  defaultServerFor,
  isCoreConfigKey,
  normalizeCoreConfigValue,
  protocolFromConfig,
  type CoreConfigItem,
  type CoreConfigKey,
  type Protocol,
} from "./core-config";

/** 新安装默认选择 aTrust；新配置的服务器地址与该协议的门户地址配套。 */
const RECOMMENDED_PROTOCOL: Protocol = "atrust";

/** Rust `ExvConfig::default()` 的前端可编辑投影。快速与高级视图共享此草稿。 */
const DEFAULT_QUICK_START_CORE_DRAFT: Record<CoreConfigKey, string> = {
  protocol: RECOMMENDED_PROTOCOL,
  server: defaultServerFor(RECOMMENDED_PROTOCOL),
  cisco_server: defaultServerFor("cisco"),
  atrust_server: defaultServerFor("atrust"),
  // 2026-10-22 双端对齐裁定：新装默认系统浏览器（browser），
  // 与后端 ExvConfig::default 一致。atrust_auto_login 已死代码清理（S6a）：
  // 残留键读取侧忽略，投影不再下发。
  atrust_auth_mode: "browser",
  username: "",
  password: "",
  remember_password: "false",
  connection_mode: DEFAULT_CONNECTION_MODE,
  routes: [
    "49.52.4.0/25",
    "59.78.176.0/20",
    "59.78.199.0/21",
    "58.198.176.128/25",
    "59.78.189.128/25",
    "219.228.63.0/21",
    "202.120.80.0/20",
    "219.228.144.0/22",
  ].join(","),
  atrust_dns_enabled: "false",
  campus_dns_domains: "ecnu.edu.cn",
  user_agent: "AnyConnect Win_x86_64 4.10.05095",
  mtu: "1290",
  auto_reconnect: "false",
  auto_reconnect_max_attempts: "0",
  auto_reconnect_backoff: "false",
  // T4 重连两设置项的默认草稿（§2.8：oneshot 崩溃重连默认关；重连通知默认开）。
  reconnect_on_oneshot_engine_crash: "false",
  reconnect_notify: "true",
};

type QuickStartUiKey =
  | "install_service"
  | "launch_at_login"
  | "auto_connect_on_launch"
  | "minimize_to_tray_on_connect"
  | "close_preference";

/** Core 默认值加上当前快速入门独有的前端/宿主偏好草稿。 */
export const DEFAULT_QUICK_START_DRAFT: Record<CoreConfigKey | QuickStartUiKey, string> = {
  ...DEFAULT_QUICK_START_CORE_DRAFT,
  install_service: "true",
  launch_at_login: "false",
  auto_connect_on_launch: "false",
  minimize_to_tray_on_connect: "false",
  close_preference: "smart",
};

export type QuickStartDraft = Record<CoreConfigKey | QuickStartUiKey, string>;

export interface QuickStartConfigPayload {
  items: ReadonlyArray<CoreConfigItem>;
  requires_quick_start: boolean;
  /** R-D3-C：当次启动损坏修复的一次性报告；缺失/合法启动为 null/缺省。 */
  config_recovery?: { corrupted: boolean; backup_file: string } | null;
}

/** 初次打开的唯一决定因素是 Core 明确返回的配置健康结论。 */
export function quickStartShouldOpen(payload: QuickStartConfigPayload): boolean {
  return payload.requires_quick_start === true;
}

/**
 * R-D3-C（裁定实施）：损坏修复的一次性通知文案。仅 quiet 分支消费（seen=true
 * 的老用户不重弹快速入门，改收本通知）；show 路径走快速入门引导，**不得**双提示。
 * 非损坏（无报告 / corrupted=false）返回 null——不通知。
 * 一次性语义天然成立：信息源是当次损坏分支，用户下次启动 config 已合法不再触发。
 */
export function configRecoveryNotice(payload: QuickStartConfigPayload): string | null {
  const recovery = payload.config_recovery;
  if (!recovery || recovery.corrupted !== true) return null;
  const backup = recovery.backup_file.trim();
  const backupHint = backup
    ? `原配置已备份为 ${backup}（配置目录内）`
    : "原配置已备份（配置目录内）";
  return `配置文件损坏，已恢复为默认设置。${backupHint}；请到设置重新填写账户，或从备份找回原配置。`;
}

// ---- 「已见」标记（KI-13：有且仅有首次运行弹出） ----

/**
 * 启动判定三态：
 *   * `show`  —— 无标记且 Core 判定需要快速入门（干净安装首运行）→ 弹出；
 *   * `seed`  —— 无标记但已有合法配置（≤4.1.0 升级/历史安装从未写过标记）→
 *                补写标记、不弹（升级用户二次弹出违反「绝不弹第二次」）；
 *   * `quiet` —— 已有标记（完成或跳过过）→ 永不再弹（即使 config.json 之后
 *                损坏触发 Core 修复性 requires_quick_start，也不得二次弹出）。
 */
export type QuickStartStartupDecision = "show" | "seed" | "quiet";

export function decideQuickStartOnStartup(
  payload: QuickStartConfigPayload,
  seen: boolean,
): QuickStartStartupDecision {
  if (seen) return "quiet";
  return quickStartShouldOpen(payload) ? "show" : "seed";
}

/** 「已见」标记网关：读/写宿主侧 `<config_dir>/quick-start.json`（单键
 * `quick_start_seen`；与 core config.json 同目录，卸载整树清理天然耦合）。 */
export interface QuickStartSeenGateway {
  getSeen(): Promise<boolean>;
  markSeen(): Promise<void>;
}

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function invokeCommand<T>(cmd: string): Promise<T> {
  if (!isTauri()) {
    throw new Error("quick start 标记仅在 Tauri 外壳内可用");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd);
}

/** 生产网关：走 Tauri Command（app/src/quick_start_marker.rs）。 */
export function createTauriQuickStartSeenGateway(): QuickStartSeenGateway {
  return {
    getSeen() {
      return invokeCommand<boolean>("quick_start_seen_get");
    },
    markSeen() {
      return invokeCommand<void>("quick_start_seen_mark");
    },
  };
}

let seenGateway: QuickStartSeenGateway | null = null;

/** 注入网关（测试注入内存实现；生产保持 null 走真实 Tauri 网关）。 */
export function provideQuickStartSeenGateway(impl: QuickStartSeenGateway): void {
  seenGateway = impl;
}

/** 测试重置（生产路径不调用）。 */
export function resetQuickStartSeenGateway(): void {
  seenGateway = null;
}

function effectiveSeenGateway(): QuickStartSeenGateway {
  return seenGateway ?? createTauriQuickStartSeenGateway();
}

/**
 * 读「已见」标记。外壳之外（浏览器/测试预览）没有标记存储，静默保持「未见过」
 * 默认（不视为错误，与 ui-prefs 的降级口径一致）；真实网关失败也诚实降级为
 * false，交由 {@link decideQuickStartOnStartup} 结合 Core 结论决定，不阻断启动。
 */
export async function loadQuickStartSeen(): Promise<boolean> {
  if (!seenGateway && !isTauri()) return false;
  try {
    return await effectiveSeenGateway().getSeen();
  } catch {
    return false;
  }
}

/**
 * 写「已见」标记（跳过/完成/迁移播种三处时机）。尽力而为：失败不抛出、
 * 不阻断界面——后续启动的 `seed` 分支会重试补写。
 */
export async function markQuickStartSeen(): Promise<void> {
  if (!seenGateway && !isTauri()) return;
  try {
    await effectiveSeenGateway().markSeen();
  } catch (error) {
    console.error("快速入门已见标记写入失败", error);
  }
}

/**
 * Core 返回的已知配置覆盖默认值；未知字段不会进入 UI 草稿。旧配置仍按 Cisco 解释，
 * 并把旧 `server` 仅迁入当前协议对应的独立服务器值。
 */
export function createQuickStartDraft(items: ReadonlyArray<CoreConfigItem>): QuickStartDraft {
  const draft: QuickStartDraft = { ...DEFAULT_QUICK_START_DRAFT };
  const protocol: Protocol = items.length > 0
    ? protocolFromConfig(items.find((item) => item.key === "protocol")?.value)
    : RECOMMENDED_PROTOCOL;
  draft.protocol = protocol;
  if (!items.some((item) => item.key === "server" && item.value.trim())) draft.server = defaultServerFor(protocol);
  for (const item of items) {
    if (!isCoreConfigKey(item.key) || item.key === "password") continue;
    // 首次配置可保留空服务器；入门采用现有预设，后续用户编辑仍正常校验。
    if (["server", "atrust_server", "cisco_server"].includes(item.key) && !item.value.trim()) continue;
    const fieldProtocol = item.key === "cisco_server" ? "cisco" : item.key === "atrust_server" ? "atrust" : protocol;
    const normalized = normalizeCoreConfigValue(item.key, item.value, { protocol: fieldProtocol });
    if (normalized.ok) draft[item.key] = normalized.value;
  }
  // 配置迁移兼容：旧 Core 若只回传活动 server，把它仅归入旧协议一侧。
  if (!items.some((item) => item.key === "cisco_server") && protocol === "cisco" && items.some((item) => item.key === "server")) draft.cisco_server = draft.server;
  if (!items.some((item) => item.key === "atrust_server") && protocol === "atrust" && items.some((item) => item.key === "server")) draft.atrust_server = draft.server;
  return draft;
}

/** 自动登录可复用当前账户已有密码；浏览器通道仅校验服务器。 */
export function validateQuickStartDraft(
  draft: QuickStartDraft,
  saved: { username: string; stored: boolean } = { username: "", stored: false },
): Partial<Record<"username" | "password" | "server", string>> {
  const errors: Partial<Record<"username" | "password" | "server", string>> = {};
  const usesCredentials = protocolFromConfig(draft.protocol) === "cisco" || draft.atrust_auth_mode === "headless";
  if (usesCredentials && !draft.username.trim()) errors.username = "请输入登录账户。";
  if (!draft.server.trim()) errors.server = "请输入 VPN 服务器。";
  if (protocolFromConfig(draft.protocol) === "atrust" && draft.atrust_auth_mode === "headless"
    && !draft.password && !(saved.stored && draft.username.trim() === saved.username.trim())) {
    errors.password = "请输入自动登录密码。";
  }
  return errors;
}

/** 快速入门提交包含同一组已知 Core 配置键；密码留空时省略派生键。 */
export function quickStartItems(draft: QuickStartDraft): CoreConfigItem[] {
  // 服务器地址的归一化跟随草稿协议；协议本身在下面单独收口。
  const protocol = protocolFromConfig(draft.protocol);
  return (Object.keys(DEFAULT_QUICK_START_CORE_DRAFT) as CoreConfigKey[])
    // S5a（2026-10-26）：密码留空时省略 remember_password 键——core 对缺键保持
    // 原值，已保存密文与 owner 不被"派生 false"误清除（"留空保留原密码"承诺在
    // 快速入门同样成立）；非空密码仍派生 true 一并落盘。Rust 白名单只拒未知键，
    // 少发安全。
    .filter((key) => {
      if (protocol === "atrust" && draft.atrust_auth_mode !== "headless"
        && ["username", "password", "remember_password"].includes(key)) return false;
      return key !== "remember_password" || draft.password.length > 0;
    })
    .map((key) => {
      if (key === "remember_password") return { key, value: String(draft.password.length > 0) };
      // 协议必须落在后端接受的枚举内：草稿只来自水合或下拉，仍在此收口，
      // 否则一个未知取值会让后端整批拒绝（Core 日志里看不到具体是哪一项）。
      if (key === "protocol") return { key, value: protocol };
      const fieldProtocol = key === "cisco_server" ? "cisco" : key === "atrust_server" ? "atrust" : protocol;
      const normalized = normalizeCoreConfigValue(key, draft[key], { protocol: fieldProtocol });
      return { key, value: normalized.ok ? normalized.value : draft[key] };
    });
}

/** 证书选择仅属于本次向导，信任状态由宿主现有证书入口提供。 */
export interface QuickStartCertificateGateway {
  inspect(): Promise<{ trusted: boolean }>;
  install(): Promise<void>;
}
export const quickStartCertificateGateway: QuickStartCertificateGateway = {
  inspect: () => kernel.quickStartCertificateInspect(),
  install: () => kernel.quickStartCertificateInstall(),
};
