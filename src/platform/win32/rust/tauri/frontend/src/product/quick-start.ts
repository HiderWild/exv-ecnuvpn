import {
  DEFAULT_CONNECTION_MODE,
  isCoreConfigKey,
  normalizeCoreConfigValue,
  type CoreConfigItem,
  type CoreConfigKey,
} from "./core-config";

/** Rust `ExvConfig::default()` 的前端可编辑投影。快速与高级视图共享此草稿。 */
const DEFAULT_QUICK_START_CORE_DRAFT: Record<CoreConfigKey, string> = {
  server: "vpn-cn.ecnu.edu.cn",
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
  user_agent: "AnyConnect Win_x86_64 4.10.05095",
  mtu: "1290",
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
}

/** 初次打开的唯一决定因素是 Core 明确返回的配置健康结论。 */
export function quickStartShouldOpen(payload: QuickStartConfigPayload): boolean {
  return payload.requires_quick_start === true;
}

/** 由 bootstrap 后 Core 返回的已知配置覆盖默认值；未知字段不会进入 UI 草稿。 */
export function createQuickStartDraft(items: ReadonlyArray<CoreConfigItem>): QuickStartDraft {
  const draft: QuickStartDraft = { ...DEFAULT_QUICK_START_DRAFT };
  for (const item of items) {
    if (!isCoreConfigKey(item.key)) continue;
    const normalized = normalizeCoreConfigValue(item.key, item.value);
    if (normalized.ok) draft[item.key] = normalized.value;
  }
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
  return (Object.keys(DEFAULT_QUICK_START_CORE_DRAFT) as CoreConfigKey[]).map((key) => {
    // 快速入门按实际密码输入派生记住选择，不沿用 bootstrap 或历史草稿标记。
    if (key === "remember_password") return { key, value: String(draft.password.length > 0) };
    const normalized = normalizeCoreConfigValue(key, draft[key]);
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
