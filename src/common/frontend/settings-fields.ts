/**
 * 双端（darwin / win32）设置页共享字段文案表（沿双端共享 CSS 机制引入）。
 *
 * 只承载跨端必须逐字一致的用户可见文案：连接卡与实验性卡的 core 配置键、应用卡
 * 与外观卡的前端偏好行，以及实验性分区的标签与提示行。字段顺序、控件形态与平台
 * 特有键（win32 连接模式 / 一次性 engine 崩溃重连、darwin 卸载区）仍由各端
 * core-config.ts 与 SettingsPage.vue 决定。本表不得出现平台专名（macOS / Windows /
 * UAC 等）；文案直接陈述业务事实，遵守「产品表达禁止防御性语句」治理。
 */

/** 单个设置字段的跨端共享文案。 */
export interface SharedSettingFieldCopy {
  label: string;
  description: string;
}

/** 连接卡与实验性卡共用的 core 配置键文案（双端 CORE_CONFIG_FIELDS 逐字引用）。 */
export const SHARED_CORE_FIELD_COPY: Readonly<Record<string, SharedSettingFieldCopy>> = {
  protocol: {
    label: "VPN 协议",
    description: "选择 VPN 协议与登录方式：Cisco 使用登录账户与密码直连；aTrust 使用学校统一认证。每种协议分别保存服务器地址。",
  },
  server: {
    label: "VPN 服务器",
    description: "用于建立当前协议 VPN 连接的服务地址。",
  },
  atrust_server: {
    label: "aTrust 服务器",
    description: "aTrust 网关地址（Cisco 服务器单独保存）。",
  },
  atrust_auth_mode: {
    label: "aTrust 认证方式",
    description: "浏览器登录打开系统浏览器手动认证，不填充账户和密码；网页内嵌在学校网页中完成认证；无窗口优先用无头会话自动登录，需要时静默回退内嵌窗口。",
  },
  username: {
    label: "登录账户",
    description: "连接时使用的账户名。",
  },
  password: {
    label: "密码",
    description: "以加密形式保存；设置页显示凭据状态，修改需通过「修改密码」输入完整新密码。",
  },
  remember_password: {
    label: "记住密码",
    description: "勾选后密码随配置保存；关闭则连接时需手动输入。",
  },
  routes: {
    label: "路由",
    description: "由EXV处理的流量的目标地址范围",
  },
  campus_dns_domains: {
    label: "校园 DNS 域名",
    description: "Cisco 与 aTrust 均生效，所填域名及其子域通过 VPN 下发的 DNS 解析。多个域名用英文逗号分隔；留空关闭。服务端未提供 DNS 时，请清空此项后重试连接。",
  },
  user_agent: {
    label: "User-Agent",
    description: "连接请求使用的客户端标识。",
  },
  mtu: {
    label: "MTU",
    description: "网络接口的正整数 MTU。",
  },
  auto_reconnect: {
    label: "自动重连",
    description: "连接意外断开时自动重新连接（下次连接生效）。",
  },
  auto_reconnect_max_attempts: {
    label: "自动重连次数",
    description: "仅在开启自动重连后可设置；0 表示无限重连；达到次数后不再重连。",
  },
  auto_reconnect_backoff: {
    label: "自动重连退避",
    description: "仅在开启自动重连后可设置；开启后掉线按 2s→4s→8s→16s→30s 退避重连，成功连接后重置。",
  },
};

/** 实验性分区：导航/卡片标题与标题下常显提示（双端逐字一致）。 */
export const EXPERIMENTAL_SECTION_COPY = {
  label: "实验性",
  note: "以下选项可能影响连接兼容性，建议保持默认。",
} as const;

/** 实验性卡三项共享字段（win32 的显示连接时延自应用卡迁入；darwin 卸载区位于三项之后）。 */
export const EXPERIMENTAL_FIELD_COPY: Readonly<Record<string, SharedSettingFieldCopy>> = {
  mtu: {
    label: "MTU",
    description: "网络接口 MTU。默认值已适配校园网；非专业场景建议保持默认。",
  },
  user_agent: {
    label: "User-Agent",
    description: "连接请求的客户端标识。修改可能导致服务端拒绝连接；建议保持默认。",
  },
  show_latency: {
    label: "显示连接时延",
    description: "连接后显示时延采样（毫秒）。默认关闭；无采样时显示横杠。",
  },
};

/** 应用卡跨端共享行（纯前端/宿主偏好；平台特有行为行不进本表）。 */
export const APPLICATION_FIELD_COPY: Readonly<Record<string, SharedSettingFieldCopy>> = {
  launch_at_login: {
    label: "开机自动运行",
    description: "登录系统后自动启动 EXV。",
  },
  silent_startup: {
    label: "静默启动",
    description: "启动时不弹出主窗口，仅托盘驻留，可从托盘唤出。",
  },
  minimize_to_tray_on_connect: {
    label: "连接后最小化到托盘",
    description: "连接成功后自动把主窗口最小化到托盘。",
  },
  auto_connect_on_launch: {
    label: "启动时自动连接",
    description: "应用启动且空闲时自动发起连接。",
  },
  close_preference: {
    label: "关闭按钮行为",
    description: "智能判断会在已连接或正在连接、重连时收起到托盘；没有活动连接时退出。也可固定选择托盘或直接退出。",
  },
};

/** 外观卡跨端共享行（控件形态各端自定，文案逐字一致）。 */
export const APPEARANCE_FIELD_COPY: Readonly<Record<string, SharedSettingFieldCopy>> = {
  theme: {
    label: "主题",
    description: "跟随系统或固定为浅色、深色；修改后立即生效并保存。",
  },
  accent: {
    label: "强调色",
    description: "用于主操作与当前状态，不改变语义颜色；修改后立即生效并保存。",
  },
  motion: {
    label: "动效",
    description: "减少动效以静态场景图显示连接状态，不加载三维组件；修改后停顿片刻自动保存。",
  },
};
