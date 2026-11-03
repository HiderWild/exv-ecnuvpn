<script setup lang="ts">
import { inject, onMounted, ref } from "vue";
import { PRODUCT_LICENSE_NOTICES } from "../product/licenses";

const licensesOpen = ref(false);

import {
  CORE_CONFIG_GATEWAY_KEY,
  createCoreConfigGateway,
  isCoreConfigKey,
  type CoreConfigGateway,
} from "../product/core-config";
import { PRODUCT_VERSION } from "../product/version";
import { CHANGELOG_ENTRIES } from "../product/releases";

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



// 最新发布默认展开，所有条目复用同一披露状态与操作。
const latestVersion = CHANGELOG_ENTRIES[0]?.version;
const expandedVersions = ref(new Set<string>(latestVersion ? [latestVersion] : []));

function isVersionExpanded(version: string): boolean {
  return expandedVersions.value.has(version);
}

function toggleVersion(version: string): void {
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
      <div class="about-hero__text"><h1 id="about-title">EXV</h1><p class="about-subtitle">for ECNU</p></div>
    </header>
    <div class="about-body" data-testid="about-scroll-body">
      <section class="about-metadata" aria-label="版本与仓库">
        <div class="about-metadata__row"><span>当前版本</span><span><strong class="about-version" data-testid="about-version">{{ PRODUCT_VERSION }}</strong><span class="about-maintainer"> · 由 <strong data-testid="about-author">{{ AUTHOR }}</strong> 维护</span></span></div>
        <div class="about-metadata__row"><span>GitHub 仓库</span>
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
        </div>
      </section>
      <section class="about-licenses-card" aria-labelledby="about-licenses-title">
        <div class="about-licenses-row">
          <h2 id="about-licenses-title">开源许可</h2>
          <button type="button" data-testid="about-licenses-toggle" :aria-expanded="licensesOpen" aria-controls="about-license-notices" @click="licensesOpen = !licensesOpen">{{ licensesOpen ? "收起资料" : "查看许可" }}</button>
        </div>
        <div v-if="licensesOpen" id="about-license-notices" data-testid="about-licenses" class="about-license-notices">
          <article v-for="notice in PRODUCT_LICENSE_NOTICES" :key="notice.source">
            <h3>{{ notice.name }} · {{ notice.license }}</h3>
            <p class="about-license-source">来源：{{ notice.source }}</p>
            <pre>{{ notice.text }}</pre>
          </article>
        </div>
      </section>
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
            <button
              type="button"
              class="about-changelog__entry-toggle"
              :data-testid="`changelog-toggle-${entry.version}`"
              :aria-expanded="isVersionExpanded(entry.version)"
              :aria-controls="`changelog-highlights-${entry.version}`"
              @click="toggleVersion(entry.version)"
            >
              <span class="about-changelog__entry-head">
                <span class="about-changelog__entry-version">v{{ entry.version }}</span>
                <span class="about-changelog__entry-date">{{ entry.dateLabel }}</span>
                <span v-if="index === 0" class="about-changelog__latest">最新发布记录</span>
                <span v-if="entry.version === PRODUCT_VERSION" class="about-changelog__current">当前版本</span>
              </span>
              <svg class="about-changelog__chevron" viewBox="0 0 12 12" fill="currentColor" aria-hidden="true">
                <path d="M4 2 9 6 4 10Z" />
              </svg>
            </button>
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
  padding: 0;
}

.about-body {
  flex: 1 1 0;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
}

.about-hero { display:flex; flex:none; align-items:center; justify-content:center; gap:12px; padding:5px 0 25px; }
.about-logo { width:48px; height:56px; object-fit:contain; flex:none; }
.about-hero__text { min-width:0; }
.about-hero__text h1 { margin:0; font-size: var(--font-size-display); line-height:36px; letter-spacing:0; }
.about-subtitle { margin:0; color:var(--text-secondary); font-size: var(--font-size-caption); line-height:18px; }
.about-version { font-size: var(--font-size-caption); font-weight:650; }
.about-maintainer { font-size: var(--font-size-caption); color:var(--text-secondary); }
.about-metadata { padding:0 18px; border:1px solid var(--border-subtle); border-radius:14px; background:var(--surface-panel); }
.about-metadata__row { display:flex; justify-content:space-between; align-items:center; gap:12px; min-height:40px; color:var(--text-secondary); font-size: var(--font-size-caption); }
.about-metadata__row + .about-metadata__row { border-top:1px solid var(--border-subtle); }
.about-metadata__row > span:last-child { text-align:right; }
.about-repo {
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  min-width: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
  font-weight:500;
  text-decoration: none;
  transition: color var(--motion-fast) var(--motion-ease);
}
.about-repo:hover { color: var(--accent); text-decoration: none; }
.about-repo__icon { display:none; }
.about-repo__label { min-width: 0; overflow-wrap: anywhere; }
.about-changelog { margin-top:18px; }
.about-changelog__header { display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-3); padding: 0 0 var(--space-2); }
.about-changelog__header h2 { margin: 0; font-size: var(--font-size-heading); letter-spacing: -0.01em; }
.about-changelog__count { color: var(--text-secondary); font-size: var(--font-size-caption); font-weight: 400; }
.about-changelog__list { display: grid; gap: 10px; }
.about-changelog__entry { border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; background: var(--surface-panel); }
.about-changelog__entry--latest { border-color: color-mix(in srgb, var(--accent) 32%, var(--border-subtle)); }
.about-changelog__current { color: var(--text-secondary); font-size: var(--font-size-caption); font-weight: 500; }
.about-changelog__entry-toggle {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 16px;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  min-height: 44px;
  padding: 13px 16px;
  border: 0;
  border-radius: 0;
  font-size: var(--font-size-caption);
  background: transparent;
  color: var(--text-primary);
  text-align: left;
  cursor: pointer;
}
.about-changelog__entry-toggle:hover { background: var(--surface-subtle); }
.about-changelog__entry-toggle:focus-visible { position: relative; z-index: 1; outline: 2px solid var(--focus-ring); outline-offset: -2px; }
.about-changelog__entry-head { display: flex; flex-wrap:wrap; align-items:baseline; gap: var(--space-2); min-width: 0; }
.about-changelog__entry-version { font-family: var(--font-mono); font-size: var(--font-size-caption); font-weight: 600; white-space: nowrap; }
.about-changelog__latest { color: var(--accent); font-size: var(--font-size-caption); font-weight: 650; white-space: nowrap; }
.about-changelog__entry-date { color: var(--text-secondary); font-size: var(--font-size-caption); white-space: nowrap; }
.about-changelog__chevron { width: 12px; height: 12px; color: var(--text-secondary); transition: transform var(--motion-fast) var(--motion-ease); }
.about-changelog__entry-toggle[aria-expanded="true"] .about-changelog__chevron { transform: rotate(90deg); }
.about-changelog__content { padding:0 16px 15px; font-size: var(--font-size-caption); line-height:1.9; }
.about-changelog__section h3 { margin:14px 0 5px; color: var(--text-primary); font-size: var(--font-size-heading); font-weight: 600; line-height:1.9; }
.about-changelog__highlights { margin:5px 0 14px; padding-left:18px; color: var(--text-secondary); font-size: var(--font-size-caption); line-height: 1.9; }
.about-changelog__highlights li { margin:0; padding:0; }
.about-advanced { margin-top: var(--space-5); }
.about-advanced h2 { margin: 0 0 var(--space-2); font-size: var(--font-size-heading); }
.readonly-list { display: grid; gap: 0; }
.readonly-row { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3); min-height: 34px; border-top: 1px solid var(--border-subtle); color: var(--text-secondary); font-size: var(--font-size-caption); }
.readonly-row span:last-child { color: var(--text-primary); font-weight: 600; text-align: right; }
.about-state { margin: 0; color: var(--text-secondary); font-size: var(--font-size-caption); }

.about-licenses-card { margin-top:8px; padding:0 3px; }
.about-licenses-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.about-licenses-row h2 { margin:0; color:var(--text-secondary); font-size: var(--font-size-heading); font-weight:500; }
.about-licenses-row p, .about-license-source { margin: 4px 0 0; color: var(--text-secondary); font-size: var(--font-size-caption); overflow-wrap: anywhere; }
.about-licenses-row button { flex:none; min-height:24px; padding:2px 8px; font-size: var(--font-size-body); }
.about-license-notices article { border-top: 1px solid var(--border-subtle); margin-top: 16px; padding-top: 14px; }
.about-license-notices h3 { margin: 0; font-size: var(--font-size-heading); }
.about-license-notices pre { margin: 12px 0 0; white-space: pre-wrap; overflow-wrap: anywhere; font: var(--font-size-caption)/1.7 var(--font-mono); color: var(--text-secondary); }
@media (max-width: 700px) { .about-changelog__entry-date { white-space:normal; } }
</style>
