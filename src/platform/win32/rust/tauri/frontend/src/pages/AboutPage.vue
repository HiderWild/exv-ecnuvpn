<script setup lang="ts">
import { inject, onMounted, ref } from "vue";

import {
  CORE_CONFIG_GATEWAY_KEY,
  createCoreConfigGateway,
  isCoreConfigKey,
  type CoreConfigGateway,
} from "../product/core-config";
import { PRODUCT_VERSION } from "../product/version";

/** 关于页（左侧栏独立入口；与设置/日志并列）。品牌展示（图标/应用名/副标题/版本/作者/仓库）
 * + 更新日志（最新发布记录常显）+ 未知核心配置只读。 */

const props = defineProps<{ gateway?: CoreConfigGateway }>();

const injectedGateway = inject(CORE_CONFIG_GATEWAY_KEY, null);
const gateway = props.gateway ?? injectedGateway ?? createCoreConfigGateway();
const otherItems = ref<ReadonlyArray<{ key: string; value: string }>>([]);
const loadError = ref<string | null>(null);

onMounted(async () => {
  try {
    const items = await gateway.configGet();
    otherItems.value = items.filter((item) => !isCoreConfigKey(item.key));
  } catch {
    loadError.value = "暂时无法读取配置。";
  }
});

/** 产品作者与项目仓库（继承 C++ 产品线 distribution/ecnu.json 的身份信息）。 */
const AUTHOR = "HiderWild";
const REPOSITORY_LABEL = "HiderWild/exv-ecnuvpn";
const REPOSITORY_URL = "https://github.com/HiderWild/exv-ecnuvpn/";

/**
 * 打开外部浏览器：优先走 Tauri `open_external` 命令（WebView2 会拦截 window.open）；
 * 命令不可用时回退到 window.open。
 */
async function openRepository() {
  const url = REPOSITORY_URL;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("open_external", { url });
    return;
  } catch {
    // 命令不可用（独立预览/测试环境）时回退到浏览器窗口打开。
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

/** 版本更新日志（继承 C++ 产品线 webui/src/data/changelog.ts 的历史条目，最前补当前版本）。 */
export interface ChangelogEntry {
  version: string;
  dateLabel: string;
  highlights: string[];
  sections?: { title: string; highlights: string[] }[];
}

const CHANGELOG_ENTRIES: ChangelogEntry[] = [
  {
    version: "4.0.0",
    dateLabel: "2026-08-25 · 更新至 2026-09-13",
    highlights: [
      "4.0.0 汇总 3.3.7 之后的持续进展：使用 Rust 完全重构，重组业务流和架构设计，并将 macOS 的连接、服务、设置与桌面体验接入新的产品链路。",
    ],
    sections: [
      {
        title: "原生连接与网络基础",
        highlights: [
          "重新实现从网关解析、登录认证、CSTP/TLS 协商到隧道和路由应用的连接流程，连接页按阶段显示进展。",
          "macOS 使用原生 utun 隧道承载校园网流量；修正接口地址、/32 掩码和路由网关配置，解决连接后的源地址选择及 ping 失败问题。",
          "VPN 网关连接优先使用物理网络出口，结合独立解析与 fake-ip 地址过滤，减少其他代理 TUN 干扰造成的连接失败。",
          "校园网分流路由与网关直连路由分别管理；断开连接时清理 EXV 创建的隧道地址和路由。",
        ],
      },
      {
        title: "连接恢复与生命周期",
        highlights: [
          "接通数据面掉线后的自动重连，支持重试次数设置及可选退避等待，并显示重连进展。",
          "完善 CSTP 保活、网关探测应答、Core 与 Engine 心跳和退出看护，及时发现断线与失去控制的会话。",
          "统一主动断开、进程退出和通信中断的资源清理，修复旧连接残留及跨次连接状态影响下一次连接的问题。",
          "连接与断开请求及时反馈受理状态，改善按钮、当前连接步骤和最终状态之间的响应一致性。",
        ],
      },
      {
        title: "macOS 服务与按需授权",
        highlights: [
          "支持在应用内安装、卸载和按需启动连接服务，通过系统管理员授权执行需要权限的操作。",
          "增加未安装服务时的一次性提权连接路径；在同一次 Core 会话中复用已启动的引擎，减少重复授权。",
          "已安装的服务由 launchd 保持运行；断开连接回到空闲，不必每次重新启动服务引擎。",
          "区分未安装、可用和不可用的服务状态，保留刷新过程中的已知状态，避免界面短暂跳成未知。",
          "服务操作失败时保留原因和恢复入口，并将安装后的连接继续操作接入同一用户流程。",
        ],
      },
      {
        title: "首次使用、账户与配置",
        highlights: [
          "首次使用或配置缺失、损坏时进入快速入门；有效配置不会仅因账户为空而重复触发引导。",
          "提供服务器预设与自定义地址、记住密码、校园路由和自动重连设置，快速入门可选择安装服务。",
          "连接时按缺失项补全凭据并预填已有用户名，允许只为本次连接提供密码；选择保存时再持久化。",
          "修复修改密码后无法重试及弹窗凭据被旧配置覆盖的问题；保存的密码使用加密存储，不在设置页回显原密码。",
          "设置草稿和分区位置在切页后保留；路由通过独立编辑窗口增删，字段校验和保存结果就近反馈。",
          "主题、强调色与用户主动切换的窗口模式即时保存；内部临时展开不改变下次启动模式。",
        ],
      },
      {
        title: "桌面界面与 macOS 操作",
        highlights: [
          "重新设计连接、设置、日志和关于页面，统一完整与极简窗口的连接操作及待处理交互。",
          "新增空闲、连接中、已连接的三态场景与过渡动效；减少动效时使用静态场景，按产品动效设置生效。",
          "连接运行信息集中展示账户、服务器、校园地址、在线时长、上传下载速率和累计流量。",
          "设置按连接、应用、外观、通知和高级参数组织，支持主题、强调色、静默启动、自动连接及按连接事件发送通知。",
          "适配 macOS 标题栏与原生窗口拖动，修复拖动闪退；应用重复启动时激活已有实例。",
          "应用包统一显示 EXV 名称和产品图标，关于页项目链接可从 macOS 原生外部浏览器打开。",
        ],
      },
      {
        title: "日志与故障定位",
        highlights: [
          "历史日志与新增日志按游标增量加载，修复边界漏条；去重并过滤无助于用户排障的周期噪声。",
          "支持按等级筛选、暂停跟随和回到最新，后台刷新保留正在阅读的日志内容。",
          "补充断线原因、运行统计及连接恢复相关诊断，在线时长随当前会话正确更新。",
        ],
      },
      {
        title: "窗口、日志与安装体验",
        highlights: [
          "日志新增复制筛选结果，包含完整时间、事件代码和结构化字段；可展开查看详细信息，复制成功或失败均有反馈。",
          "日志切页后保留筛选、跟随和阅读位置；读取或清空失败时保留已有记录并提供重试。",
          "关于页完整整理 4.0.0 的分领域更新，最新发布记录保持展开，历史版本按需展开；当前安装版本独立标识。",
          "MTU、User-Agent 与连接时延显示集中在金黄色边框的实验性功能区；最新发布记录保持强调色，不再与历史条目使用相同的悬停背景。",
          "macOS 设置提供卸载 EXV 入口，确认、执行和分项结果统一在模态中展示；必须先停止连接，停止失败则中止卸载。",
          "智能关闭根据连接状态决定：连接期间保持后台运行，空闲时退出；也可选择始终后台运行或直接退出。",
          "macOS 从系统睡眠唤醒后，等待主网络接口恢复再恢复原连接；遵循自动重连设置，用户主动停止后不自动重新连接。",
          "系统代理状态通过 macOS 原生接口读取，明确显示开启或未开启；外部 TUN 状态动态更新，并排除 EXV 自己的隧道接口。",
          "macOS 安装磁盘命名为“安装EXV”，增加小蓝箱标识，重新设计背景与图标布局，使拖入应用程序的安装步骤更清楚。",
          "安装盘精简说明，并明确修复工具仅在系统提示应用“已损坏”时使用，避免让用户误以为应用已经出错。",
        ],
      },
      {
        title: "Windows 平台补充",
        highlights: [
          "Windows 同期接入 Rust 连接核心、Wintun 数据面与新的桌面页面，提供安装程序和便携包。",
          "服务管理提供安装、卸载、修复和重置服务密钥，改进按需授权、服务就绪判断与优雅停机。",
          "系统代理修改增加恢复记录与启动回放，减少异常退出后代理配置遗留；新增代理 TUN 诊断和服务自愈进展展示。",
        ],
      },
      {
        title: "实验性功能与使用边界",
        highlights: [
          "macOS 连接时延显示仍列为实验性功能：显示已收到的采样，暂无采样时显示横杠，目前不支持手动刷新。",
          "睡眠恢复时若未保存密码，或一次性连接需要重新进行管理员授权，请回到应用中重试连接。",
          "代理共存已有物理出口、网关解析及分流改进；校园域名仍可能受第三方代理的 fake-ip 配置影响，不能等同于已兼容所有代理软件和模式。",
        ],
      },
    ],
  },
  {
    version: "3.3.7",
    dateLabel: "2026-07-06",
    highlights: [
      "连接成功后如果 service 或 oneshot helper 被终止，核心会回收连接 attempt 与 active tunnel guard，避免下一次连接被误判为仍在进行。",
      "连接创建期间 helper 意外退出时，vpn.connect 会识别本进程的 PreparingHelper 残留并自动重试，避免继续报 Helper connection could not be established。",
      "重连恢复期间即使 helper 状态仍显示 connected，只要存在旧 session 或 CoreLease 残留，也会清理同进程连接守卫并允许用户重试。",
      "已连接后 helper 控制管道断开时只标记服务不可用，不再把 Helper control pipe disconnected during VPN session 当作阻塞错误弹窗。",
      "连接过程中 CoreLease 或 helper 控制面先降级、但 native 数据面随后成功时，核心会清理控制面旧错误，不再在已连接状态弹出 Helper control pipe disconnected during VPN session。",
      "修复旧 helper 重连失败晚到时覆盖新连接成功状态的问题，已连接后会清除过期错误模态。",
      "重连启动前会向当前 helper 验证旧 CoreLease，helper 已重启时会重新获取租约，避免连接已恢复却弹出 empty session_id 错误。",
      "核心状态汇聚层会在 controller 已连接时清理或抑制迟到的连接失败，并等待已接受启动的 helper service 真正可用后再判定结果。",
      "vpn.connect 遇到同进程 stale guard 时会先核对本地 runtime 终止态，清理成功后自动重试一次。",
      "修正启动重置发布空闲状态时误释放 active tunnel guard 的竞态，避免 oneshot 重连前的资源守卫被提前清掉。",
      "修复 helper 被结束后旧重连 controller 与用户重试并行的竞态，避免重试已成功却因核心 RPC transport 关闭继续弹错。",
      "补充 helper 生命周期 reconcile 与连接 attempt retry 日志，便于定位服务被结束、管道断开和重连被阻塞的原因。",
    ],
  },
  {
    version: "3.3.6",
    dateLabel: "2026-06-28",
    highlights: [
      "完善 oneshot 与持久 helper 的断开清理，释放连接事务和 CoreLease，避免快速重连时复用到旧状态。",
      "优化服务安装、卸载和修复路径，正在连接时先给出确认并统一走断开流程。",
      "修复 A/B 类私有路由开关未勾选时仍可能应用宽网段路由的问题。",
      "加速空闲态服务维护路径，减少不必要的连接准备等待。",
    ],
  },
  {
    version: "3.3.5",
    dateLabel: "2026-06-28",
    highlights: [
      "刷新主仪表盘布局和浅色主题默认强调色，提升状态扫描和视觉层次。",
      "增加托盘状态快照、静默启动和后台断开能力，窗口不再被无谓唤起。",
      "强化 helper service / oneshot 生命周期守卫，改进健康检查和意外断开恢复。",
      "统一密码显示、保存密码覆盖提示和快速设置连接前应用顺序。",
      "补齐 Windows helper 稳定安装目录中的 Wintun 运行时文件。",
    ],
  },
  {
    version: "3.3.4",
    dateLabel: "2026-06-26",
    highlights: [
      "引入 helper 单实例保护和固定 oneshot endpoint，降低多实例漂移风险。",
      "拆分 helper 控制、隧道和维护 lane，让高权限操作和连接事务互不阻塞。",
      "把服务安装、卸载和修复改为 runas 启动路径，减少 daemon 内部特权操作。",
      "清理旧的 session handoff 链路，为后续 helper 重用和退出清理打基础。",
    ],
  },
  {
    version: "3.3.3",
    dateLabel: "2026-06-23",
    highlights: [
      "新增标准 macOS EXV.app 包结构，可拖入 Applications 后直接启动。",
      "让 macOS 包内资源使用相对路径解析，减少安装路径和工作目录差异带来的问题。",
      "完善 macOS 打包校验、Info.plist 版本信息和构建文档。",
    ],
  },
  {
    version: "3.3.2",
    dateLabel: "2026-06-21",
    highlights: [
      "提供 Windows x64 installer 和 portable zip，并加入发布打包脚本。",
      "改进 helper service 重新安装、卸载、修复和快捷方式清理流程。",
      "增强配置导入导出、Quick Start 和 minimal mode 对话框体验。",
      "禁用 WebView 缩放手势，减少误触导致的界面比例变化。",
    ],
  },
  {
    version: "3.3.1",
    dateLabel: "2026-06-19",
    highlights: [
      "拆分连接意图、原生握手、认证交互和 packet attach 阶段，让连接建立流程更清晰可恢复。",
      "引入固定 RPC lane 调度和异步 host message 处理，避免 UI 与核心动作互相阻塞。",
      "修复真实 VPN 连接链路中的认证提示、状态探测和 Windows core IPC 探测问题。",
      "加强连接 pipeline 的回归测试和平台 readiness 快照。",
    ],
  },
  {
    version: "3.3.0",
    dateLabel: "2026-06-16",
    highlights: [
      "完成从命令行版本到带 UI 桌面版本的主要转向，建立原生 WebView shell 与前端渲染输出。",
      "实现 Windows WebView2、macOS WKWebView 和 Linux WebKitGTK shell 基础设施。",
      "建立 UI shell 与 core RPC 的消息桥、版本身份和生命周期注册机制。",
      "退役 Electron 生产打包路径，统一走原生 WebView 包结构。",
    ],
  },
];

// 最新发布记录常显；只保存历史条目的展开状态。
const latestVersion = CHANGELOG_ENTRIES[0]?.version;
const expandedVersions = ref(new Set<string>());

function isVersionExpanded(version: string): boolean {
  return version === latestVersion || expandedVersions.value.has(version);
}

function toggleVersion(version: string): void {
  if (version === latestVersion) return;
  const next = new Set(expandedVersions.value);
  if (next.has(version)) next.delete(version);
  else next.add(version);
  expandedVersions.value = next;
}
</script>

<template>
  <section class="about-page" aria-labelledby="about-title">
    <header class="about-hero">
      <img class="about-logo" src="../assets/exv-logo.svg" alt="EXV 产品 Logo" />
      <div class="about-hero__text">
        <p class="about-subtitle">for ECNU</p>
        <h1 id="about-title">EXV</h1>
        <div class="about-release">
          <span>版本</span>
          <strong class="about-version" data-testid="about-version">{{ PRODUCT_VERSION }}</strong>
          <span class="about-release__divider" aria-hidden="true"></span>
          <span>由 <strong data-testid="about-author">{{ AUTHOR }}</strong> 维护</span>
        </div>
      </div>
      <a
        class="about-repo"
        data-testid="about-repository"
        :href="REPOSITORY_URL"
        rel="noopener noreferrer"
        @click.prevent="openRepository"
      >
        <svg class="about-repo__icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"/></svg>
        <span class="about-repo__label">{{ REPOSITORY_LABEL }}</span>
      </a>
    </header>

    <div class="about-body" data-testid="about-scroll-body">
      <section class="about-changelog" data-testid="about-changelog" aria-labelledby="changelog-title">
        <header class="about-changelog__header">
          <h2 id="changelog-title">更新日志</h2>
          <span class="about-changelog__count">{{ CHANGELOG_ENTRIES.length }} 个版本</span>
        </header>
        <div class="about-changelog__list">
          <article
            v-for="(entry, index) in CHANGELOG_ENTRIES"
            :key="entry.version"
            class="about-changelog__entry"
            :class="{ 'about-changelog__entry--latest': index === 0 }"
          >
            <component
              :is="index === 0 ? 'div' : 'button'"
              :type="index === 0 ? undefined : 'button'"
              class="about-changelog__entry-toggle"
              :data-testid="`changelog-toggle-${entry.version}`"
              :aria-expanded="isVersionExpanded(entry.version)"
              :aria-controls="`changelog-highlights-${entry.version}`"
              @click="toggleVersion(entry.version)"
            >
              <span class="about-changelog__entry-head">
                <span class="about-changelog__entry-version">v{{ entry.version }}</span>
                <span v-if="index === 0" class="about-changelog__latest">最新发布记录</span>
                <span v-if="entry.version === PRODUCT_VERSION" class="about-changelog__current">当前版本</span>
              </span>
              <span class="about-changelog__entry-date">{{ entry.dateLabel }}</span>
              <svg v-if="index !== 0" class="about-changelog__chevron" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <path d="m4 6 4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </component>
            <div
              v-if="isVersionExpanded(entry.version)"
              :id="`changelog-highlights-${entry.version}`"
              class="about-changelog__content"
              :data-testid="`changelog-highlights-${entry.version}`"
            >
              <ul class="about-changelog__highlights">
                <li v-for="highlight in entry.highlights" :key="highlight">{{ highlight }}</li>
              </ul>
              <section v-for="section in entry.sections" :key="section.title" class="about-changelog__section">
                <h3>{{ section.title }}</h3>
                <ul class="about-changelog__highlights">
                  <li v-for="highlight in section.highlights" :key="highlight">{{ highlight }}</li>
                </ul>
              </section>
            </div>
          </article>
        </div>
      </section>
  
      <section v-if="otherItems.length" class="about-advanced" data-testid="about-advanced">
        <h2>其它配置（只读）</h2>
        <div class="readonly-list">
          <div v-for="item in otherItems" :key="item.key" class="readonly-row">
            <span>{{ item.key }}</span><span>{{ item.value }}</span>
          </div>
        </div>
      </section>
      <p v-else-if="loadError" class="about-state">{{ loadError }}</p>
    </div>
  </section>
</template>

<style scoped>
.about-page {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  height: 100%;
  overflow: hidden;
  padding: var(--product-page-top-gap) 0 var(--space-6);
}

.about-body {
  flex: 1 1 0;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
}

.about-hero {
  flex: none;
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  align-items: center;
  gap: var(--space-4);
  padding: 0 0 var(--space-5);
  border-bottom: 1px solid var(--border-subtle);
}

.about-logo { width: 68px; height: 68px; flex: none; }
.about-hero__text { min-width: 0; }
.about-hero__text h1 { margin: 0; font-size: clamp(28px, 3vw, 38px); line-height: 1; letter-spacing: -0.035em; }
.about-subtitle { margin: 0 0 var(--space-1); color: var(--accent); font-size: 12px; font-weight: 650; letter-spacing: 0.08em; text-transform: uppercase; }
.about-release { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-1) var(--space-2); margin-top: var(--space-2); color: var(--text-secondary); font-size: 12px; }
.about-release strong { color: var(--text-primary); font-weight: 650; }
.about-version { font-family: var(--font-mono); font-size: 14px; }
.about-release__divider { width: 1px; height: 12px; background: var(--border-strong); }
.about-repo {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 550;
  text-decoration: none;
  transition: color var(--motion-fast) var(--motion-ease);
}
.about-repo:hover { color: var(--accent); text-decoration: none; }
.about-repo__icon { width: 16px; height: 16px; flex: none; }
.about-repo__label { min-width: 0; overflow-wrap: anywhere; }
.about-changelog { margin-top: var(--space-5); }
.about-changelog__header { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-3); padding: 0 0 var(--space-2); }
.about-changelog__header h2 { margin: 0; font-size: 16px; letter-spacing: -0.01em; }
.about-changelog__count { color: var(--text-secondary); font-size: 12px; font-weight: 400; }
.about-changelog__list { border-top: 1px solid var(--border-subtle); }
.about-changelog__entry { border-bottom: 1px solid var(--border-subtle); }
.about-changelog__entry--latest { background: var(--accent-subtle); }
.about-changelog__entry--latest .about-changelog__entry-toggle { cursor: default; }
.about-changelog__current { color: var(--text-secondary); font-size: 11px; font-weight: 500; }
.about-changelog__entry-toggle {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto 16px;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  min-height: 44px;
  padding: var(--space-2) var(--space-3);
  border: 0;
  border-radius: 0;
  background: transparent;
  color: var(--text-primary);
  text-align: left;
  cursor: pointer;
}
.about-changelog__entry:not(.about-changelog__entry--latest) .about-changelog__entry-toggle:hover { background: var(--surface-subtle); }
.about-changelog__entry-toggle:focus-visible { position: relative; z-index: 1; outline: 2px solid var(--focus-ring); outline-offset: -2px; }
.about-changelog__entry-head { display: flex; align-items: center; gap: var(--space-2); min-width: 0; }
.about-changelog__entry-version { font-family: var(--font-mono); font-size: 13px; font-weight: 650; white-space: nowrap; }
.about-changelog__latest { color: var(--accent); font-size: 11px; font-weight: 650; white-space: nowrap; }
.about-changelog__entry-date { color: var(--text-secondary); font-size: 12px; white-space: nowrap; }
.about-changelog__chevron { width: 16px; height: 16px; color: var(--text-secondary); transition: transform var(--motion-fast) var(--motion-ease); }
.about-changelog__entry-toggle[aria-expanded="true"] .about-changelog__chevron { transform: rotate(180deg); }
.about-changelog__section h3 { margin: var(--space-3) var(--space-3) var(--space-1); color: var(--text-primary); font-size: 13px; font-weight: 650; }
.about-changelog__highlights { margin: 0; padding: var(--space-1) var(--space-5) var(--space-3) 34px; color: var(--text-secondary); font-size: 13px; line-height: 1.55; }
.about-changelog__highlights li { margin: var(--space-1) 0; padding-left: var(--space-1); }
.about-advanced { margin-top: var(--space-5); }
.about-advanced h2 { margin: 0 0 var(--space-2); font-size: 14px; }
.readonly-list { display: grid; gap: 0; }
.readonly-row { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); min-height: 34px; border-top: 1px solid var(--border-subtle); color: var(--text-secondary); font-size: 12px; }
.readonly-row span:last-child { color: var(--text-primary); font-weight: 600; text-align: right; }
.about-state { margin: 0; color: var(--text-secondary); font-size: 13px; }

@media (max-width: 700px) {
  .about-hero { grid-template-columns: auto minmax(0, 1fr); }
  .about-logo { width: 56px; height: 56px; }
  .about-repo { grid-column: 2; }
}
</style>
