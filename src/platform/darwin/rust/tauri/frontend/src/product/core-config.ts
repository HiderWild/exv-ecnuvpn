import { ref, type InjectionKey } from "vue";

import { kernel } from "../lib/ipc";

export const CORE_CONFIG_KEYS = [
  "server",
  "username",
  "password",
  "remember_password",
  "routes",
  "user_agent",
  "mtu",
  "protocol",
  "auto_reconnect",
  "auto_reconnect_max_attempts",
  "auto_reconnect_backoff",
] as const;

export type CoreConfigKey = (typeof CORE_CONFIG_KEYS)[number];

/** 连接协议取值；与 Core `Protocol` 枚举的稳定字面量逐字一致（小写、大小写敏感）。 */
export const CORE_PROTOCOLS = ["cisco", "atrust"] as const;
export type CoreProtocol = (typeof CORE_PROTOCOLS)[number];

/** 协议选项文案：aTrust 走外部浏览器统一认证、EXV 不接收学校密码，标签必须把这点说清。 */
export const PROTOCOL_OPTIONS: readonly { value: CoreProtocol; label: string }[] = [
  { value: "cisco", label: "Cisco AnyConnect（用户名密码）" },
  { value: "atrust", label: "aTrust（浏览器登录/统一认证）" },
];

export interface CoreConfigItem {
  key: string;
  value: string;
}

export interface CoreConfigGateway {
  configGet(): Promise<ReadonlyArray<CoreConfigItem>>;
  configSet(items: ReadonlyArray<{ key: CoreConfigKey; value: string }>): Promise<boolean>;
  savedPassword?(username: string, server: string): Promise<string | null>;
}

/** 同一配置网关共享非秘密凭据状态；保存成功与切换模式均刷新，不缓存明文。 */
const credentialStates = new WeakMap<CoreConfigGateway, ReturnType<typeof createCredentialState>>();
function createCredentialState(gateway: CoreConfigGateway) {
  const state = ref({ username: "", server: "", remember: false, stored: false });
  const savedRevision = ref(0);
  let writeTail: Promise<void> = Promise.resolve();
  /** 同一网关的配置与凭据写入串行；失败不阻塞后续显式保存。 */
  function write<T>(operation: () => Promise<T>): Promise<T> {
    const pending = writeTail.then(operation);
    writeTail = pending.then(() => undefined, () => undefined);
    return pending;
  }
  let revision = 0;
  function apply(items: ReadonlyArray<CoreConfigItem>): void {
    revision += 1;
    const value = (key: string) => items.find((item) => item.key === key)?.value;
    state.value = { username: value("username") ?? "", server: value("server") ?? "", remember: value("remember_password") === "true", stored: value("has_stored_password") === "true" };
  }
  async function refresh(): Promise<void> {
    const current = ++revision;
    try {
      const items = await gateway.configGet();
      if (current === revision) apply(items);
    } catch {
      if (current === revision) state.value = { username: "", server: "", remember: false, stored: false };
    }
  }
  return { state, apply, refresh, write, savedRevision, markSaved: () => { savedRevision.value += 1; } };
}
export function credentialConfigFor(gateway: CoreConfigGateway) {
  let value = credentialStates.get(gateway);
  if (!value) { value = createCredentialState(gateway); credentialStates.set(gateway, value); }
  return value;
}
export function revealSavedPassword(gateway: CoreConfigGateway, username: string, server: string): Promise<string | null> {
  return gateway.savedPassword ? gateway.savedPassword(username, server) : kernel.savedPassword(username, server);
}

/** 应用入口在预览模式下可注入内存网关；生产页面仍默认连接真实 core。 */
export const CORE_CONFIG_GATEWAY_KEY: InjectionKey<CoreConfigGateway> = Symbol(
  "exv.product.core-config-gateway",
);

export interface CoreConfigField {
  key: CoreConfigKey;
  label: string;
  description: string;
  kind: "text" | "password" | "boolean" | "routes" | "mtu" | "number" | "server" | "protocol";
}

export const CORE_CONFIG_FIELDS: readonly CoreConfigField[] = [
  { key: "server", label: "VPN 服务器", description: "用于建立 VPN 连接的服务地址。", kind: "server" },
  { key: "username", label: "登录账户", description: "连接时使用的账户名。", kind: "text" },
  { key: "password", label: "密码", description: "留空表示保持已保存密码。", kind: "password" },
  { key: "remember_password", label: "记住密码", description: "勾选后密码随配置保存；关闭则连接时需手动输入。", kind: "boolean" },
  { key: "routes", label: "路由", description: "由EXV处理的流量的目标地址范围", kind: "routes" },
  { key: "user_agent", label: "User-Agent", description: "连接请求使用的客户端标识。", kind: "text" },
  { key: "mtu", label: "MTU", description: "网络接口的正整数 MTU。", kind: "mtu" },
  { key: "protocol", label: "协议", description: "决定登录方式：Cisco 使用下方登录账户与密码直连；aTrust 通过外部浏览器完成学校统一认证，EXV 不接收学校密码。", kind: "protocol" },
  { key: "auto_reconnect", label: "自动重连", description: "连接意外断开时自动重新连接（下次连接生效）。", kind: "boolean" },
  { key: "auto_reconnect_max_attempts", label: "自动重连次数", description: "仅在开启自动重连后可设置；0 表示无限重连；达到次数后不再重连。", kind: "number" },
  { key: "auto_reconnect_backoff", label: "自动重连退避", description: "仅在开启自动重连后可设置；开启后掉线按 2s→4s→8s→16s→30s 退避重连，成功连接后重置。", kind: "boolean" },
];

/** VPN 服务器预设。每条预设自带协议：统一认证入口只服务 aTrust，CSTP 网关只服务 Cisco。 */
export interface VpnServerPreset {
  label: string;
  value: string;
  protocol: CoreProtocol;
}

/** VPN 服务器预设地址（与旧 C++ distribution/ecnu.json 对齐；默认 vpn-cn）。 */
export const VPN_SERVERS: readonly VpnServerPreset[] = [
  { label: "ECNU CN", value: "vpn-cn.ecnu.edu.cn", protocol: "cisco" },
  { label: "ECNU CT", value: "vpn-ct.ecnu.edu.cn", protocol: "cisco" },
  { label: "ECNU LT", value: "vpn-lt.ecnu.edu.cn", protocol: "cisco" },
  { label: "ECNU aTrust", value: "https://vpn.ecnu.edu.cn", protocol: "atrust" },
];

/** 归一化 VPN 服务器地址：去除协议前缀、尾部斜杠并统一小写。 */
export function normalizeServerValue(raw: string): string {
  return raw.trim().replace(/^https?:\/\//i, "").replace(/\/+$/, "").toLowerCase();
}

/** 命中的预设项（按归一形比较，带 `https://` 的 aTrust 入口同样命中）；未命中返回 undefined。 */
export function vpnServerPreset(value: string): VpnServerPreset | undefined {
  const normalized = normalizeServerValue(value);
  return VPN_SERVERS.find((server) => normalizeServerValue(server.value) === normalized);
}

export function isVpnServerPreset(value: string): boolean {
  return vpnServerPreset(value) !== undefined;
}

export type NormalizedCoreConfigValue =
  | { ok: true; value: string }
  | { ok: false; message: string };

export function isCoreConfigKey(key: string): key is CoreConfigKey {
  return (CORE_CONFIG_KEYS as readonly string[]).includes(key);
}

export function normalizeCoreConfigValue(key: CoreConfigKey, raw: string): NormalizedCoreConfigValue {
  if (key === "routes") {
    return {
      ok: true,
      value: raw
        .split(",")
        .map((item) => item.trim())
        .filter(Boolean)
        .join(","),
    };
  }

  if (key === "mtu") {
    const value = raw.trim();
    return /^[1-9]\d*$/.test(value)
      ? { ok: true, value }
      : { ok: false, message: "MTU 必须是正整数。" };
  }

  if (key === "auto_reconnect") {
    return raw === "true" || raw === "false"
      ? { ok: true, value: raw }
      : { ok: false, message: "自动重连只能是 true 或 false。" };
  }

  if (key === "auto_reconnect_max_attempts") {
    const value = raw.trim();
    return /^(0|[1-9]\d*)$/.test(value) && Number(value) <= 1024
      ? { ok: true, value }
      : { ok: false, message: "自动重连次数必须是 0-1024 的整数。" };
  }

  if (key === "auto_reconnect_backoff") {
    return raw === "true" || raw === "false"
      ? { ok: true, value: raw }
      : { ok: false, message: "自动重连退避只能是 true 或 false。" };
  }

  if (key === "remember_password") {
    return raw === "true" || raw === "false"
      ? { ok: true, value: raw }
      : { ok: false, message: "记住密码只能是 true 或 false。" };
  }

  // 协议取值必须原样送达 Core 的严格写路径：非法值在提交前以中文报错，
  // 绝不静默改成 cisco（那会让用户以为选中的协议生效了）。
  if (key === "protocol") {
    const value = raw.trim();
    return (CORE_PROTOCOLS as readonly string[]).includes(value)
      ? { ok: true, value }
      : { ok: false, message: "协议只能是 cisco 或 atrust。" };
  }

  if (key === "server") {
    return { ok: true, value: normalizeServerValue(raw) };
  }

  return { ok: true, value: raw };
}

/** 生产页面唯一可用的 core 配置出口。 */
export function createCoreConfigGateway(): CoreConfigGateway {
  return {
    async configGet() {
      const payload = await kernel.configGet();
      return payload.items;
    },
    configSet(items) {
      return kernel.configSet([...items]);
    },
    savedPassword: (username, server) => kernel.savedPassword(username, server),
  };
}
