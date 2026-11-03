import { ref, type InjectionKey } from "vue";

import { SHARED_CORE_FIELD_COPY } from "../../../../../../../common/frontend/settings-fields";
import { kernel } from "../lib/ipc";

// atrust_auto_login 已死代码清理（S6a）：读取侧经 isCoreConfigKey 忽略旧配置/
// 旧载荷残留键，写入侧不再由前端投影下发。
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
}

/** 同一配置网关共享非秘密凭据状态；保存成功与切换模式均刷新，不缓存明文。 */
const credentialStates = new WeakMap<CoreConfigGateway, ReturnType<typeof createCredentialState>>();
function createCredentialState(gateway: CoreConfigGateway) {
  const state = ref({
    username: "",
    server: "",
    protocol: "cisco" as Protocol,
    remember: false,
    stored: false,
    // §7.10-3 渠道边界：认证渠道由「认证方式 + 是否开启自动登录」推导（S7 裁定后：
    // headless+自动登录保持无头；browser+自动登录升级内嵌）。E12（阶段②附带）：
    // authMode 三值——headless 不再折叠进 "browser"（旧折叠使无头渠道的失败文案
    // 误走系统浏览器建议）。
    authMode: "browser" as "browser" | "embedded" | "headless",
    autoLogin: false,
  });
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
    state.value = {
      username: value("username") ?? "",
      server: value("server") ?? "",
      protocol: protocolFromConfig(value("protocol")),
      remember: value("remember_password") === "true",
      stored: value("has_stored_password") === "true",
      // E12：三值渠道——embedded/headless 原样透传，其余（含缺省）回落 browser。
      authMode:
        value("atrust_auth_mode") === "embedded"
          ? "embedded"
          : value("atrust_auth_mode") === "headless"
            ? "headless"
            : "browser",
      autoLogin: value("atrust_auth_mode") === "headless",
    };
  }
  async function refresh(): Promise<void> {
    const current = ++revision;
    try {
      const items = await gateway.configGet();
      if (current === revision) apply(items);
    } catch {
      if (current === revision) state.value = { username: "", server: "", protocol: "cisco", remember: false, stored: false, authMode: "browser", autoLogin: false };
    }
  }
  return { state, apply, refresh, write, savedRevision, markSaved: () => { savedRevision.value += 1; } };
}
export function credentialConfigFor(gateway: CoreConfigGateway) {
  let value = credentialStates.get(gateway);
  if (!value) { value = createCredentialState(gateway); credentialStates.set(gateway, value); }
  return value;
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

// 跨端字段文案引自共享表（S6a）；kind/顺序与本端平台特有键（cisco_server /
// connection_mode / 重连两项）留在本表。
export const CORE_CONFIG_FIELDS: readonly CoreConfigField[] = [
  { key: "protocol", kind: "protocol", ...SHARED_CORE_FIELD_COPY.protocol },
  { key: "server", kind: "server", ...SHARED_CORE_FIELD_COPY.server },
  { key: "cisco_server", label: "Cisco 服务器", description: "Cisco 专用服务器地址。", kind: "server" },
  { key: "atrust_server", kind: "server", ...SHARED_CORE_FIELD_COPY.atrust_server },
  { key: "atrust_auth_mode", kind: "auth_mode", ...SHARED_CORE_FIELD_COPY.atrust_auth_mode },
  // T4c（2026-09-30 用户授权）：与 Cisco 完全共享的单对账密（username/password/
  // remember_password）；仅 aTrust 协议下参与一次性学校页自动填交。
  { key: "username", kind: "text", ...SHARED_CORE_FIELD_COPY.username },
  // Area F（2026-10-09）：密码不设内联输入；该字段仅维持配置键与草稿置空语义，
  // 设置页呈现为「凭据状态行 + 统一模态修改」（存在性见 has_stored_password）。
  { key: "password", kind: "password", ...SHARED_CORE_FIELD_COPY.password },
  { key: "remember_password", kind: "boolean", ...SHARED_CORE_FIELD_COPY.remember_password },
  { key: "connection_mode", label: "连接模式", description: "标准模式优先直连；特定网络环境下连接不稳定时可尝试兼容模式。更改在下次手动连接时生效。", kind: "connection_mode" },
  { key: "routes", kind: "routes", ...SHARED_CORE_FIELD_COPY.routes },
  { key: "campus_dns_domains", kind: "text", ...SHARED_CORE_FIELD_COPY.campus_dns_domains },
  { key: "user_agent", kind: "text", ...SHARED_CORE_FIELD_COPY.user_agent },
  { key: "mtu", kind: "mtu", ...SHARED_CORE_FIELD_COPY.mtu },
  { key: "auto_reconnect", kind: "boolean", ...SHARED_CORE_FIELD_COPY.auto_reconnect },
  { key: "auto_reconnect_max_attempts", kind: "number", ...SHARED_CORE_FIELD_COPY.auto_reconnect_max_attempts },
  { key: "auto_reconnect_backoff", kind: "boolean", ...SHARED_CORE_FIELD_COPY.auto_reconnect_backoff },
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
    // C1（2026-10-10）放行 headless：三值与 core config_set / 壳侧
    // configured_atrust_auth_mode 的取值集合一致。
    return raw === "browser" || raw === "embedded" || raw === "headless"
      ? { ok: true, value: raw }
      : { ok: false, message: "aTrust 认证方式只能是 browser、embedded 或 headless。" };
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
  };
}
