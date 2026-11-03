import type { ConfigReadFailure, ConfigRecovery } from "../lib/ipc";
import {
  defaultServerFor,
  isCoreConfigKey,
  normalizeCoreConfigValue,
  protocolFromConfig,
  type CoreConfigItem,
  type CoreConfigKey,
  type CoreProtocol,
} from "./core-config";

/** 新装默认协议 aTrust（2026-09-30 用户裁定，翻转设计 §8 的「默认不翻转」）；
 * 与 core 侧 `DarwinStoredConfig::default()` 的 bootstrap 缺省一致；旧配置缺键仍 Cisco。 */
const RECOMMENDED_PROTOCOL: CoreProtocol = "atrust";

/**
 * Rust `ExvConfig::default()` 的前端可编辑投影。快速与高级视图共享此草稿。
 *
 * 新装默认协议 aTrust（`RECOMMENDED_PROTOCOL`，2026-09-30 裁定）；`server` 仍取 Cisco
 * 首个预设、`atrust_server` 取 aTrust 首个预设（各自独立保存，互不通用）；
 * `atrust_auth_mode` 默认 browser；`campus_dns_domains` 默认列表为 ecnu.edu.cn，
 * 与设置页共用多域规范化和保存入口。atrust_auto_login 已死代码清理（S6a）：
 * 残留键读取侧忽略，投影不再下发。
 */
const DEFAULT_QUICK_START_CORE_DRAFT: Record<CoreConfigKey, string> = {
  server: defaultServerFor("cisco"),
  atrust_server: defaultServerFor(RECOMMENDED_PROTOCOL),
  atrust_auth_mode: "browser",
  campus_dns_domains: "ecnu.edu.cn",
  username: "",
  password: "",
  remember_password: "false",
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
  user_agent: "AnyConnect Win_x86_64 4.10.05095",
  mtu: "1290",
  protocol: RECOMMENDED_PROTOCOL,
  auto_reconnect: "false",
  auto_reconnect_max_attempts: "0",
  auto_reconnect_backoff: "false",
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
  config_recovery?: ConfigRecovery | null;
  read_failure?: ConfigReadFailure | null;
}

/** 初次打开的唯一决定因素是 Core 明确返回的配置健康结论。 */
export function quickStartShouldOpen(payload: QuickStartConfigPayload): boolean {
  return !payload.read_failure && payload.requires_quick_start === true;
}

/** 由 bootstrap 后 Core 返回的已知配置覆盖默认值；未知字段不会进入 UI 草稿。 */
export function createQuickStartDraft(items: ReadonlyArray<CoreConfigItem>): QuickStartDraft {
  const draft: QuickStartDraft = { ...DEFAULT_QUICK_START_DRAFT };
  if (items.length > 0) draft.protocol = protocolFromConfig(items.find((item) => item.key === "protocol")?.value);
  for (const item of items) {
    if (!isCoreConfigKey(item.key) || item.key === "password") continue;
    // 首次配置可保留空服务器；入门采用现有预设，后续用户编辑仍正常校验。
    if (["server", "atrust_server"].includes(item.key) && !item.value.trim()) continue;
    const normalized = normalizeCoreConfigValue(item.key, item.value);
    if (normalized.ok) draft[item.key] = normalized.value;
  }
  return draft;
}

/** 仅使用保存密码的存在性和所属账户，明文始终留在 Core。 */
export interface QuickStartSavedCredential { username: string; stored: boolean }

export function validateQuickStartDraft(
  draft: QuickStartDraft,
  saved: QuickStartSavedCredential = { username: "", stored: false },
): Partial<Record<"username" | "password" | "server", string>> {
  const errors: Partial<Record<"username" | "password" | "server", string>> = {};
  if (protocolFromConfig(draft.protocol) === "atrust") {
    if (!draft.atrust_server.trim()) errors.server = "请输入 aTrust 服务器。";
    if (draft.atrust_auth_mode === "headless") {
      if (!draft.username.trim()) errors.username = "请输入登录账户。";
      if (!draft.password && !(saved.stored && draft.username.trim() === saved.username.trim())) {
        errors.password = "请输入该账户的密码。";
      }
    }
    return errors;
  }
  if (!draft.username.trim()) errors.username = "请输入登录账户。";
  if (!draft.server.trim()) errors.server = "请输入 VPN 服务器。";
  return errors;
}

/** 隐藏的凭据草稿只由 Cisco 或自动登录提交。 */
export function quickStartItems(draft: QuickStartDraft): CoreConfigItem[] {
  const credentialsVisible = protocolFromConfig(draft.protocol) === "cisco" || draft.atrust_auth_mode === "headless";
  return (Object.keys(DEFAULT_QUICK_START_CORE_DRAFT) as CoreConfigKey[])
    .filter((key) => credentialsVisible || !["username", "password", "remember_password"].includes(key))
    .filter((key) => key !== "remember_password" || draft.password.length > 0)
    .map((key) => {
      if (key === "remember_password") return { key, value: "true" };
      const normalized = normalizeCoreConfigValue(key, draft[key]);
      return { key, value: normalized.ok ? normalized.value : draft[key] };
    });
}


// ---- 「已见」标记（win32 KI-13 的 darwin 移植：有且仅有首次运行弹出） ----

/**
 * 启动判定三态：
 *   * `show`  —— 无标记且 Core 判定需要快速入门（干净安装首运行）→ 弹出；
 *   * `seed`  —— 无标记但已有合法配置（历史安装从未写过标记）→ 补写标记、不弹
 *                （升级用户二次弹出违反「绝不弹第二次」）；
 *   * `quiet` —— 已有标记（完成或跳过过）→ 永不再弹（即使 config.json 之后损坏
 *                触发 Core 修复性 requires_quick_start，也不得二次弹出）。
 */
export type QuickStartStartupDecision = "show" | "seed" | "quiet";

export function decideQuickStartOnStartup(
  payload: QuickStartConfigPayload,
  seen: boolean,
): QuickStartStartupDecision {
  if (payload.read_failure || seen) return "quiet";
  return quickStartShouldOpen(payload) ? "show" : "seed";
}

/** 「已见」标记网关：读/写宿主侧 `<config_dir>/quick-start.json`（单键
 * `quick_start_seen`；与 core config.json 同目录，卸载整树清理天然耦合）。 */
export interface QuickStartSeenGateway {
  getSeen(): Promise<boolean>;
  markSeen(): Promise<void>;
}

/** 生产网关：走壳注入的命令 adapter（`kernel/quick_start_marker.rs` 的两个命令）。 */
function createTauriQuickStartSeenGateway(): QuickStartSeenGateway {
  return {
    async getSeen() {
      const { kernel } = await import("../lib/ipc");
      return kernel.quickStartSeenGet();
    },
    async markSeen() {
      const { kernel } = await import("../lib/ipc");
      await kernel.quickStartSeenMark();
    },
  };
}

let seenGateway: QuickStartSeenGateway | null = null;

/** 注入网关（测试注入内存实现；生产保持 null 走真实 adapter 网关）。 */
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

/** 读「已见」标记；外壳外/后端不可用诚实降级为未见过（不猜测）。 */
export async function loadQuickStartSeen(): Promise<boolean> {
  try {
    return await effectiveSeenGateway().getSeen();
  } catch (error) {
    console.error("快速入门已见标记读取失败", error);
    return false;
  }
}

/** 写「已见」标记（跳过/完成/迁移播种三处时机）；尽力而为，失败不阻断。 */
export async function markQuickStartSeen(): Promise<void> {
  try {
    await effectiveSeenGateway().markSeen();
  } catch (error) {
    console.error("快速入门已见标记写入失败", error);
  }
}
