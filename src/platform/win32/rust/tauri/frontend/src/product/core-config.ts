import { ref, type InjectionKey } from "vue";

import { kernel } from "../lib/ipc";

export const CORE_CONFIG_KEYS = [
  "protocol",
  "server",
  "cisco_server",
  "atrust_server",
  "atrust_auth_mode",
  "username",
  "password",
  "remember_password",
  "connection_mode",
  "routes",
  "campus_dns_domains",
  "user_agent",
  "mtu",
  "auto_reconnect",
  "auto_reconnect_max_attempts",
  "auto_reconnect_backoff",
  "reconnect_on_oneshot_engine_crash",
  "reconnect_notify",
] as const;

export type CoreConfigKey = (typeof CORE_CONFIG_KEYS)[number];

/** VPN 协议选择。Cisco 与 aTrust 分别保留自己的服务器地址。 */
export type Protocol = "cisco" | "atrust";

export const DEFAULT_PROTOCOL: Protocol = "atrust";
export const PROTOCOL_OPTIONS: readonly { label: string; value: Protocol }[] = [
  { label: "aTrust", value: "atrust" },
  { label: "Cisco AnyConnect", value: "cisco" },
];

/** 读取未知或缺失值时保守使用 Cisco（旧配置缺 protocol 键即 Cisco）；写入仍由补丁校验拒绝未知值。 */
export function protocolFromConfig(raw: string | undefined): Protocol {
  return raw === "atrust" ? "atrust" : "cisco";
}

export type ConnectionMode = "standard" | "compatibility";

export const DEFAULT_CONNECTION_MODE: ConnectionMode = "standard";
export const CONNECTION_MODE_OPTIONS: readonly { label: string; value: ConnectionMode }[] = [
  { label: "标准模式", value: "standard" },
  { label: "兼容模式", value: "compatibility" },
];

/** 读取未知或缺失值时保守使用标准模式；写入仍由补丁校验拒绝未知值。 */
export function connectionModeFromConfig(raw: string | undefined): ConnectionMode {
  return raw === "compatibility" ? "compatibility" : DEFAULT_CONNECTION_MODE;
}

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
  const state = ref({ username: "", server: "", protocol: "cisco" as Protocol, remember: false, stored: false });
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
    state.value = { username: value("username") ?? "", server: value("server") ?? "", protocol: protocolFromConfig(value("protocol")), remember: value("remember_password") === "true", stored: value("has_stored_password") === "true" };
  }
  async function refresh(): Promise<void> {
    const current = ++revision;
    try {
      const items = await gateway.configGet();
      if (current === revision) apply(items);
    } catch {
      if (current === revision) state.value = { username: "", server: "", protocol: "cisco", remember: false, stored: false };
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
  kind: "text" | "password" | "boolean" | "routes" | "mtu" | "number" | "server" | "connection_mode" | "protocol" | "auth_mode";
}

export const CORE_CONFIG_FIELDS: readonly CoreConfigField[] = [
  { key: "protocol", label: "VPN 协议", description: "选择要使用的 VPN 协议。每种协议分别保存服务器地址。", kind: "protocol" },
  { key: "server", label: "VPN 服务器", description: "用于建立当前协议 VPN 连接的服务地址。", kind: "server" },
  { key: "cisco_server", label: "Cisco 服务器", description: "Cisco 专用服务器地址。", kind: "server" },
  { key: "atrust_server", label: "aTrust 服务器", description: "aTrust 专用服务器地址。", kind: "server" },
  { key: "atrust_auth_mode", label: "aTrust 认证方式", description: "浏览器登录会打开系统浏览器；内嵌登录会在学校网页中完成认证。", kind: "auth_mode" },
  { key: "username", label: "登录账户", description: "连接时使用的账户名。", kind: "text" },
  { key: "password", label: "密码", description: "留空表示保持已保存密码。", kind: "password" },
  { key: "remember_password", label: "记住密码", description: "勾选后密码随配置保存；关闭则连接时需手动输入。", kind: "boolean" },
  { key: "connection_mode", label: "连接模式", description: "标准模式优先直连；特定网络环境下连接不稳定时可尝试兼容模式。更改在下次手动连接时生效。", kind: "connection_mode" },
  { key: "routes", label: "路由", description: "由EXV处理的流量的目标地址范围", kind: "routes" },
  { key: "campus_dns_domains", label: "校园 DNS 域名", description: "多个域名用逗号分隔，包含其子域；使用 VPN 下发的 DNS。留空关闭。服务端未提供 DNS 时，请清空此项后重试连接。", kind: "text" },
  { key: "user_agent", label: "User-Agent", description: "连接请求使用的客户端标识。", kind: "text" },
  { key: "mtu", label: "MTU", description: "网络接口的正整数 MTU。", kind: "mtu" },
  { key: "auto_reconnect", label: "自动重连", description: "连接意外断开时自动重新连接（下次连接生效）。", kind: "boolean" },
  { key: "auto_reconnect_max_attempts", label: "自动重连次数", description: "仅在开启自动重连后可设置；0 表示无限重连；达到次数后不再重连。", kind: "number" },
  { key: "auto_reconnect_backoff", label: "自动重连退避", description: "仅在开启自动重连后可设置；开启后掉线按 2s→4s→8s→16s→30s 退避重连，成功连接后重置。", kind: "boolean" },
  // T4 新设置项（计划 §2.8；位置紧邻自动重连主开关）。可见性在 SettingsPage 按
  // 服务安装态与主开关门控（description = 问号帮助弹层文案，D13 裁定版 + D15
  // 灰度声明句：T10-oneshot 拉起底质落地前 keep 在 win32 oneshot 形态不可用——
  // 一次性形态下 core 崩溃即断开，恢复走重新提权；底质立项后解除）。
  { key: "reconnect_on_oneshot_engine_crash", label: "一次性 engine 崩溃后仍自动重连", description: "当前未安装服务，连接时建立的是一次性 engine，该进程在崩溃后丧失管理员权限并需要重新提权。这会重新触发 UAC 弹窗。一次性形态下 core 崩溃会断开连接，恢复将重新提权。", kind: "boolean" },
  { key: "reconnect_notify", label: "自动重连时发送通知", description: "自动重连触发时通过系统托盘气泡发送通知；仅在开启自动重连后生效。", kind: "boolean" },
];

/**
 * VPN 服务器预设地址（与旧 C++ distribution/ecnu.json 对齐；Cisco 默认 vpn-cn）。
 *
 * 预设携带协议：aTrust 门户与 Cisco 三地网关是不同地址，同一域名不通过名字猜协议。
 * 因此下拉只展示当前协议的预设，协议切换不得据此丢弃用户自定义地址。
 */
export const VPN_SERVERS: readonly { label: string; value: string; protocol: Protocol }[] = [
  { label: "ECNU CN", value: "vpn-cn.ecnu.edu.cn", protocol: "cisco" },
  { label: "ECNU CT", value: "vpn-ct.ecnu.edu.cn", protocol: "cisco" },
  { label: "ECNU LT", value: "vpn-lt.ecnu.edu.cn", protocol: "cisco" },
  { label: "ECNU 统一认证门户", value: "https://vpn.ecnu.edu.cn", protocol: "atrust" },
];

/** 指定协议的服务器预设；该协议没有预设时返回空列表。 */
export function vpnServersFor(protocol: Protocol): ReadonlyArray<{ label: string; value: string; protocol: Protocol }> {
  return VPN_SERVERS.filter((server) => server.protocol === protocol);
}

/** 指定协议的推荐地址（该协议首个预设）；没有预设时返回空串。 */
export function defaultServerFor(protocol: Protocol): string {
  return vpnServersFor(protocol)[0]?.value ?? "";
}

/**
 * 归一化 VPN 服务器地址：去尾部斜杠并统一小写。
 *
 * Cisco 存裸域名（去协议前缀，与旧 C++ 一致）；aTrust 门户地址本身是带协议的 URL，
 * 缺协议前缀时补 `https://`，否则地址会失去协议信息且不再命中预设。
 */
export function normalizeServerValue(raw: string, protocol: Protocol = DEFAULT_PROTOCOL): string {
  const trimmed = raw.trim().replace(/\/+$/, "");
  if (!trimmed) return "";
  return protocol === "atrust"
    ? (/^https?:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`).toLowerCase()
    : trimmed.replace(/^https?:\/\//i, "").toLowerCase();
}

/** 地址是否属于指定协议的预设；协议决定候选集合（Cisco 三地与 aTrust 门户互不通用）。 */
export function isVpnServerPreset(value: string, protocol: Protocol = DEFAULT_PROTOCOL): boolean {
  return vpnServersFor(protocol).some((server) => server.value === value);
}

/**
 * 协议切换后的服务器地址：当前地址属于原协议预设时改用新协议的首个预设；
 * 自定义地址原样保留——不能因为不在新协议的预设里被静默改写。
 */
export function serverAfterProtocolChange(current: string, from: Protocol, to: Protocol): string {
  if (from === to) return current;
  const normalized = normalizeServerValue(current, from);
  if (!normalized || !isVpnServerPreset(normalized, from)) return current;
  return defaultServerFor(to) || current;
}

export type NormalizedCoreConfigValue =
  | { ok: true; value: string }
  | { ok: false; message: string };

export function isCoreConfigKey(key: string): key is CoreConfigKey {
  return (CORE_CONFIG_KEYS as readonly string[]).includes(key);
}

export interface CoreConfigNormalizeOptions {
  /** 服务器地址按协议归一化：Cisco 存裸域名，aTrust 存完整 HTTPS 门户地址。 */
  protocol?: Protocol;
}

export function normalizeCoreConfigValue(
  key: CoreConfigKey,
  raw: string,
  options: CoreConfigNormalizeOptions = {},
): NormalizedCoreConfigValue {
  if (key === "protocol") {
    return raw === "cisco" || raw === "atrust"
      ? { ok: true, value: raw }
      : { ok: false, message: "协议只能是 cisco 或 atrust。" };
  }

  if (key === "atrust_auth_mode") {
    return raw === "browser" || raw === "embedded"
      ? { ok: true, value: raw }
      : { ok: false, message: "aTrust 认证方式只能是 browser 或 embedded。" };
  }

  if (key === "connection_mode") {
    return raw === "standard" || raw === "compatibility"
      ? { ok: true, value: raw }
      : { ok: false, message: "连接模式只能是 standard 或 compatibility。" };
  }

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

  if (key === "campus_dns_domains") {
    if (!raw.trim()) return { ok: true, value: "" };
    const domains: string[] = [];
    const seen = new Set<string>();
    for (const item of raw.split(",")) {
      const domain = item.trim().replace(/\.+$/, "").toLowerCase();
      if (!domain) continue;
      const labels = domain.split(".");
      const valid = domain.length <= 253
        && !/^\d+(?:\.\d+)*$/.test(domain)
        && /^[a-z0-9.-]+$/.test(domain)
        && labels.every((label) =>
          label.length >= 1
          && label.length <= 63
          && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label),
        );
      if (!valid) {
        return { ok: false, message: "校园 DNS 域名必须是裸域名，不能填写 IP、URL 或通配符。" };
      }
      if (!seen.has(domain)) {
        domains.push(domain);
        seen.add(domain);
      }
    }
    return { ok: true, value: domains.join(",") };
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

  if (key === "reconnect_on_oneshot_engine_crash") {
    return raw === "true" || raw === "false"
      ? { ok: true, value: raw }
      : { ok: false, message: "一次性 engine 崩溃重连只能是 true 或 false。" };
  }

  if (key === "reconnect_notify") {
    return raw === "true" || raw === "false"
      ? { ok: true, value: raw }
      : { ok: false, message: "自动重连通知只能是 true 或 false。" };
  }

  if (key === "remember_password") {
    return raw === "true" || raw === "false"
      ? { ok: true, value: raw }
      : { ok: false, message: "记住密码只能是 true 或 false。" };
  }

  if (key === "server" || key === "cisco_server" || key === "atrust_server") {
    const protocol = key === "cisco_server" ? "cisco" : key === "atrust_server" ? "atrust" : options.protocol ?? DEFAULT_PROTOCOL;
    return { ok: true, value: normalizeServerValue(raw, protocol) };
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
