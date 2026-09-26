<script setup lang="ts">
// Darwin 标题栏适配（win32 全量接入的唯一前端适配点之二）：
// 窗口配置 titleBarStyle=Overlay——无系统标题栏带，本组件的 product-titlebar
// 即真标题栏；红绿灯三键仍由系统绘制（浮于左侧 ~78px），win32 的自绘
// NativeWindowControls（最小化/最大化/关闭）不渲染、不订阅 hover/pressed；
// 主题/模式分段控件（TitlebarThemeModeControl/ModeSegmentedControl）原样
// 保留、贴标题栏最右（与物理右边缘留 8px，见 ui-first-shell.css 尾部
// darwin 专属增量）。标题栏拖动由壳侧原生拖动条承担（window_chrome.rs），
// 网页仅绘制外观。
import { computed, type PropType } from "vue";

import type {
  Appearance,
  ThemePreference,
  WindowModePreference,
} from "../product/appearance";
import type { WindowChromePort } from "../product/window-chrome";
import ModeSegmentedControl from "./ModeSegmentedControl.vue";
import ProductRail from "./ProductRail.vue";
import TitlebarThemeModeControl from "./TitlebarThemeModeControl.vue";

const props = defineProps({
  mode: { type: String as PropType<WindowModePreference>, required: true },
  chrome: { type: Object as PropType<WindowChromePort>, required: true },
  appearance: { type: Object as PropType<Appearance>, required: true },
  currentPage: { type: String, required: true },
});

const emit = defineEmits<{ navigate: [page: "connect" | "logs" | "settings" | "about"] }>();
const mode = computed(() => props.mode);

// D5（计划 §8.2）：标题栏控件改为即时提交。模式切换需要宿主 chrome.setMode 成功
// 才算生效；commitMode 失败会回滚状态并留下 saveError，由下方 alert 呈现。
async function setMode(nextMode: WindowModePreference): Promise<void> {
  await props.appearance.commitMode(nextMode, (mode) => props.chrome.setMode(mode));
}

async function setTheme(theme: ThemePreference): Promise<void> {
  await props.appearance.commitTheme(theme);
}
</script>

<template>
  <div class="product-window" :class="`product-window--${mode}`" data-testid="product-window">
    <header
      class="product-titlebar"
      data-testid="product-titlebar"
      data-window-drag-region="true"
    >
      <!-- darwin 极简不在左上放 icon+EXV（改为内容区右下角水印）：
           标题栏整条作为拖动区 + 右侧控件；左侧红绿灯由系统绘制。 -->

      <div class="titlebar-drag-space" data-window-drag-region="true" aria-hidden="true" />

      <div
        class="titlebar-actions"
        data-testid="titlebar-control-region"
        data-window-control-region="true"
      >
        <TitlebarThemeModeControl
          :model-value="props.appearance.draft.value.theme"
          :disabled="false"
          :icon-only="mode === 'minimal'"
          @update:model-value="setTheme"
        />
        <ModeSegmentedControl
          :model-value="props.appearance.draft.value.mode"
          :disabled="false"
          :icon-only="mode === 'minimal'"
          @update:model-value="setMode"
        />
      </div>
    </header>
    <p v-if="props.appearance.saveError.value" role="alert" data-testid="appearance-save-error">{{ props.appearance.saveError.value }}</p>

    <div class="product-body">
      <ProductRail
        v-if="mode === 'advanced'"
        :current-page="props.currentPage"
        @navigate="emit('navigate', $event)"
      />
      <main
        class="product-content"
        :class="{ 'product-content--scrollable': currentPage !== 'connect' }"
        data-testid="product-content"
      ><slot /></main>
    </div>
  </div>
</template>

<style scoped>
[data-testid="appearance-save-error"] { position: absolute; top: 34px; right: 8px; z-index: 20; max-width: calc(100% - 16px); margin: 0; padding: 8px; background: var(--surface-panel); color: var(--text-primary); }
</style>
