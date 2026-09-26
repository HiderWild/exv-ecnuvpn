<script setup lang="ts">
import { inject, onMounted, ref } from "vue";

import {
  CORE_CONFIG_GATEWAY_KEY,
  createCoreConfigGateway,
  isCoreConfigKey,
  type CoreConfigGateway,
} from "../product/core-config";
import { darwinCommandAdapter } from "../product/command-adapter-global";
import { CHANGELOG_ENTRIES } from "../product/releases";
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
    // 本区本意是展示「前端不认识的未来新增配置键」；has_stored_password 是 core
    // ConfigGet 契约里的派生只读标志（设置页凭据状态的正当消费方），不在此展示。
    otherItems.value = items.filter(
      (item) => !isCoreConfigKey(item.key) && item.key !== "has_stored_password",
    );
  } catch {
    loadError.value = "暂时无法读取配置。";
  }
});

/** 产品作者与项目仓库（继承 C++ 产品线 distribution/ecnu.json 的身份信息）。 */
const AUTHOR = "HiderWild";
const REPOSITORY_LABEL = "HiderWild/exv-ecnuvpn";
const REPOSITORY_URL = "https://github.com/HiderWild/exv-ecnuvpn/";

/**
 * 打开外部浏览器：优先走 `open_external` 命令（Darwin 经壳注入的全局 adapter 单接缝）；
 * 命令不可用/拒绝时回退到 window.open。
 */
async function openRepository() {
  const url = REPOSITORY_URL;
  const adapter = darwinCommandAdapter();
  if (adapter !== null) {
    try {
      await adapter.call("open_external", { url });
      return;
    } catch {
      // 命令不可用（独立预览/测试环境/平台不支持）时回退到浏览器窗口打开。
    }
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

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
          <span>当前版本</span>
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
