import { assertConfigReadable } from "../lib/config-read-failure";
import { ref, type InjectionKey } from "vue";

import { SHARED_CORE_FIELD_COPY } from "../../../../../../../common/frontend/settings-fields";
import { kernel } from "../lib/ipc";

// atrust_auto_login 已死代码清理（S6a）：读取侧经 isCoreConfigKey 忽略旧配置/
// 旧载荷残留键，写入侧不再由前端投影下发。
export const CORE_CONFIG_KEYS = [
  "server",
  "atrust_server",
  "atrust_auth_mode",
  "campus_dns_domains",
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

/**
 * 协议选项文案：aTrust 走学校统一认证，登录方式由其认证设置决定。
 * 顺序即 UI 呈现顺序（2026-09-30 用户裁定：aTrust 左置且默认激活）。
 */
export const PROTOCOL_OPTIONS: readonly { value: CoreProtocol; label: string }[] = [
  { value: "atrust", label: "aTrust（浏览器登录/统一认证）" },
  { value: "cisco", label: "Cisco AnyConnect（用户名密码）" },
];

/**
 * 读取未知或缺失值时保守使用 Cisco。注意与「新装默认」区分（2026-09-30 裁定后两层
 * 默认刻意不同）：新装默认协议在 core bootstrap 层翻转为 aTrust（config.json 缺失时
 * 落盘的缺省）；这里保持 Cisco 只覆盖「旧配置缺键 / 未知取值」的读路径回退——协议键
 * 面世前的安装基数全是 Cisco 用户，升级不得隐式改协议。写入仍由补丁校验拒绝未知值。
 */
export function protocolFromConfig(raw: string | undefined): CoreProtocol {
  return raw === "atrust" ? "atrust" : "cisco";
}

/** aTrust 认证方式取值（core `atrust_auth_mode` 键；2026-10-22 双端对齐后默认 headless）。 */
export const ATRUST_AUTH_MODES = ["browser", "embedded", "headless"] as const;
export type AtrustAuthMode = (typeof ATRUST_AUTH_MODES)[number];

export function atrustAuthModeFromConfig(raw: string | undefined): AtrustAuthMode {
  return raw === "embedded" || raw === "headless" ? raw : "browser";
}

/** 系统浏览器始终手动登录；自动登录选择内嵌，显式无窗口保持无窗口。 */
export function effectiveAtrustAuthMode(mode: AtrustAuthMode, _autoLogin: boolean): AtrustAuthMode {
  return mode;
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
  const state = ref({ username: "", server: "", remember: false, stored: false });
  const savedRevision = ref(0);
  const protocol = ref<CoreProtocol>("cisco");
  const authentication = ref<{ protocol: CoreProtocol; server: string; mode: AtrustAuthMode; autoLogin: boolean } | null>(null);
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
    protocol.value = protocolFromConfig(value("protocol"));
    const autoLogin = atrustAuthModeFromConfig(value("atrust_auth_mode")) === "headless";
    const mode = effectiveAtrustAuthMode(atrustAuthModeFromConfig(value("atrust_auth_mode")), autoLogin);
    authentication.value = { protocol: protocol.value, server: value("atrust_server") ?? "", mode, autoLogin };
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
  return { state, protocol, authentication, apply, refresh, write, savedRevision, markSaved: () => { savedRevision.value += 1; } };
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
  kind: "text" | "password" | "boolean" | "routes" | "mtu" | "number" | "server" | "protocol" | "auth_mode" | "campus_domains";
}

// 跨端字段文案引自共享表（S6a）；kind/顺序为本端呈现形态。
export const CORE_CONFIG_FIELDS: readonly CoreConfigField[] = [
  { key: "server", kind: "server", ...SHARED_CORE_FIELD_COPY.server },
  { key: "atrust_server", kind: "server", ...SHARED_CORE_FIELD_COPY.atrust_server },
  { key: "atrust_auth_mode", kind: "auth_mode", ...SHARED_CORE_FIELD_COPY.atrust_auth_mode },
  { key: "username", kind: "text", ...SHARED_CORE_FIELD_COPY.username },
  // Area F（2026-10-09）：密码不设内联输入；该字段仅维持配置键与草稿置空语义，
  // 设置页呈现为「凭据状态行 + 统一模态修改」（存在性见 has_stored_password）。
  { key: "password", kind: "password", ...SHARED_CORE_FIELD_COPY.password },
  { key: "remember_password", kind: "boolean", ...SHARED_CORE_FIELD_COPY.remember_password },
  { key: "routes", kind: "routes", ...SHARED_CORE_FIELD_COPY.routes },
  { key: "campus_dns_domains", kind: "campus_domains", ...SHARED_CORE_FIELD_COPY.campus_dns_domains },
  { key: "user_agent", kind: "text", ...SHARED_CORE_FIELD_COPY.user_agent },
  { key: "mtu", kind: "mtu", ...SHARED_CORE_FIELD_COPY.mtu },
  { key: "protocol", kind: "protocol", ...SHARED_CORE_FIELD_COPY.protocol },
  { key: "auto_reconnect", kind: "boolean", ...SHARED_CORE_FIELD_COPY.auto_reconnect },
  { key: "auto_reconnect_max_attempts", kind: "number", ...SHARED_CORE_FIELD_COPY.auto_reconnect_max_attempts },
  { key: "auto_reconnect_backoff", kind: "boolean", ...SHARED_CORE_FIELD_COPY.auto_reconnect_backoff },
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
  { label: "ECNU aTrust", value: "vpn.ecnu.edu.cn", protocol: "atrust" },
];

/** 指定协议的服务器预设（Cisco 三地与 aTrust 入口互不通用）。 */
export function vpnServersFor(protocol: CoreProtocol): ReadonlyArray<VpnServerPreset> {
  return VPN_SERVERS.filter((server) => server.protocol === protocol);
}

/** 指定协议的推荐地址（该协议首个预设）；没有预设时返回空串。 */
export function defaultServerFor(protocol: CoreProtocol): string {
  return vpnServersFor(protocol)[0]?.value ?? "";
}

/** 归一化 VPN 服务器地址：去除协议前缀、尾部斜杠并统一小写（Cisco 与 aTrust 均存裸域名——core 同款规范化）。 */
export function normalizeServerValue(raw: string): string {
  return raw.trim().replace(/^https?:\/\//i, "").replace(/\/+$/, "").toLowerCase();
}

/** 命中的预设项（按归一形比较，带 `https://` 的 aTrust 入口同样命中）；未命中返回 undefined。 */
export function vpnServerPreset(value: string, protocol?: CoreProtocol): VpnServerPreset | undefined {
  const normalized = normalizeServerValue(value);
  const candidates = protocol === undefined ? VPN_SERVERS : vpnServersFor(protocol);
  return candidates.find((server) => normalizeServerValue(server.value) === normalized);
}

export function isVpnServerPreset(value: string, protocol?: CoreProtocol): boolean {
  return vpnServerPreset(value, protocol) !== undefined;
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

  // aTrust 认证方式同走严格写路径（core 写路径 must-be 检查同款）。
  if (key === "atrust_auth_mode") {
    const value = raw.trim();
    return (ATRUST_AUTH_MODES as readonly string[]).includes(value)
      ? { ok: true, value }
      : { ok: false, message: "aTrust 认证方式只能是 embedded、browser 或 headless。" };
  }

  // 校园 DNS 域名与 Core 保持列表语义，完整保存规范化后的各域。
  if (key === "campus_dns_domains") {
    const domains = campusDnsDomainList(raw);
    if (!domains.ok) {
      return { ok: false, message: "校园 DNS 域名必须是裸域名，不能填写 IP、URL 或通配符。" };
    }
    return { ok: true, value: domains.value.join(",") };
  }

  if (key === "server" || key === "atrust_server") {
    return { ok: true, value: normalizeServerValue(raw) };
  }

  return { ok: true, value: raw };
}

/**
 * 校园 DNS 域名草稿解析（与 Core `campus_dns_domains` 归一化一致）：
 * 逗号拆分 → 逐条小写/去空白/去尾点/去重；空输入得到空列表（=清空）。非法条目
 * 整批拒绝（与 core 写路径的「非法整批拒绝」对齐）。返回完整去重列表。
 */
function campusDnsDomainList(raw: string): { ok: true; value: string[] } | { ok: false } {
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
    if (!valid) return { ok: false };
    if (!seen.has(domain)) {
      domains.push(domain);
      seen.add(domain);
    }
  }
  return { ok: true, value: domains };
}

/** 生产页面唯一可用的 core 配置出口。 */
export function createCoreConfigGateway(): CoreConfigGateway {
  return {
    async configGet() {
      const payload = await kernel.configGet();
      assertConfigReadable(payload);
      return payload.items;
    },
    configSet(items) {
      return kernel.configSet([...items]);
    },
  };
}
