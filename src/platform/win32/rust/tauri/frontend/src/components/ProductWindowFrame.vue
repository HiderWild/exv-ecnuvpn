<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, type PropType } from "vue";

import type {
  Appearance,
  ThemePreference,
  WindowModePreference,
} from "../product/appearance";
import type {
  NativeWindowControl,
  NativeWindowControlState,
  WindowChromePort,
} from "../product/window-chrome";
import ModeSegmentedControl from "./ModeSegmentedControl.vue";
import NativeWindowControls from "./NativeWindowControls.vue";
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
const nativeControlState = ref<NativeWindowControlState>({ control: null, pressed: false });
let unsubscribeControlState: (() => void) | null = null;
let frameUnmounted = false;

function setMode(nextMode: WindowModePreference): void {
  // 用户主动切换：提交成功才落盘；失败由 commitMode 回滚并写入 saveError。
  void props.appearance.commitMode(nextMode, (mode) => props.chrome.setMode(mode));
}

function setTheme(theme: ThemePreference): void {
  props.appearance.setTheme(theme);
}

async function activateNativeControl(control: NativeWindowControl): Promise<void> {
  try {
    await props.chrome.control(control);
  } catch (error) {
    console.error("原生窗口操作失败", error);
  }
}

onMounted(() => {
  const subscribe = props.chrome.subscribeControlState;
  if (typeof subscribe !== "function") return;

  void subscribe((state) => {
    nativeControlState.value = state;
  })
    .then((unsubscribe) => {
      if (frameUnmounted) {
        unsubscribe();
      } else {
        unsubscribeControlState = unsubscribe;
      }
    })
    .catch((error) => {
      console.error("原生窗口状态订阅失败", error);
    });
});

onUnmounted(() => {
  frameUnmounted = true;
  unsubscribeControlState?.();
  unsubscribeControlState = null;
});
</script>

<template>
  <div class="product-window" :class="`product-window--${mode}`" data-testid="product-window">
    <header
      class="product-titlebar"
      data-testid="product-titlebar"
      data-window-drag-region="true"
    >
      <div v-if="mode === 'minimal'" class="minimal-brand">
        <img src="../assets/exv-logo.svg" alt="产品 Logo" />
        <strong>EXV</strong>
      </div>

      <div class="titlebar-drag-space" data-window-drag-region="true" aria-hidden="true" />

      <div
        class="titlebar-actions"
        data-testid="titlebar-control-region"
        data-window-control-region="true"
      >
        <TitlebarThemeModeControl
          :model-value="props.appearance.state.value.theme"
          :disabled="false"
          :icon-only="mode === 'minimal'"
          @update:model-value="setTheme"
        />
        <ModeSegmentedControl
          :model-value="props.appearance.state.value.mode"
          :disabled="false"
          :icon-only="mode === 'minimal'"
          @update:model-value="setMode"
        />
        <NativeWindowControls
          :mode="mode"
          :state="nativeControlState"
          :disabled="false"
          @activate="activateNativeControl"
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
