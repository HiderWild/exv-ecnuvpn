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
}

/** 初次打开的唯一决定因素是 Core 明确返回的配置健康结论。 */
export function quickStartShouldOpen(payload: QuickStartConfigPayload): boolean {
  return payload.requires_quick_start === true;
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
  if (!items.some((item) => item.key === "server")) draft.server = defaultServerFor(protocol);
  for (const item of items) {
    if (!isCoreConfigKey(item.key)) continue;
    const fieldProtocol = item.key === "cisco_server" ? "cisco" : item.key === "atrust_server" ? "atrust" : protocol;
    const normalized = normalizeCoreConfigValue(item.key, item.value, { protocol: fieldProtocol });
    if (normalized.ok) draft[item.key] = normalized.value;
  }
  // 配置迁移兼容：旧 Core 若只回传活动 server，把它仅归入旧协议一侧。
  if (!items.some((item) => item.key === "cisco_server") && protocol === "cisco" && items.some((item) => item.key === "server")) draft.cisco_server = draft.server;
  if (!items.some((item) => item.key === "atrust_server") && protocol === "atrust" && items.some((item) => item.key === "server")) draft.atrust_server = draft.server;
  return draft;
}

/** 快速入门仅强制账号和服务器必填；密码留空表示不保存。 */
export function validateQuickStartDraft(draft: QuickStartDraft): Partial<Record<"username" | "password" | "server", string>> {
  const errors: Partial<Record<"username" | "password" | "server", string>> = {};
  if (!draft.username.trim()) errors.username = "请输入登录账户。";
  if (!draft.server.trim()) errors.server = "请输入 VPN 服务器。";
  return errors;
}

/** 快速入门提交始终包含同一组已知 Core 配置键。 */
export function quickStartItems(draft: QuickStartDraft): CoreConfigItem[] {
  // 服务器地址的归一化跟随草稿协议；协议本身在下面单独收口。
  const protocol = protocolFromConfig(draft.protocol);
  return (Object.keys(DEFAULT_QUICK_START_CORE_DRAFT) as CoreConfigKey[]).map((key) => {
    // 快速入门按实际密码输入派生记住选择，不沿用 bootstrap 或历史草稿标记。
    if (key === "remember_password") return { key, value: String(draft.password.length > 0) };
    // 协议必须落在后端接受的枚举内：草稿只来自水合或下拉，仍在此收口，
    // 否则一个未知取值会让后端整批拒绝（Core 日志里看不到具体是哪一项）。
    if (key === "protocol") return { key, value: protocol };
    const fieldProtocol = key === "cisco_server" ? "cisco" : key === "atrust_server" ? "atrust" : protocol;
    const normalized = normalizeCoreConfigValue(key, draft[key], { protocol: fieldProtocol });
    return { key, value: normalized.ok ? normalized.value : draft[key] };
  });
}

/** 成功回读只确认非秘密身份字段；绝不把密码显示值引入草稿或错误状态。 */
export function quickStartCredentialsMatch(
  username: string,
  rememberPassword: string,
  items: ReadonlyArray<CoreConfigItem>,
): boolean {
  return !items.some((item) => item.key === "password" && item.value !== "") &&
    items.some((item) => item.key === "username" && item.value === username) &&
    items.some((item) => item.key === "remember_password" && item.value === rememberPassword);
}
