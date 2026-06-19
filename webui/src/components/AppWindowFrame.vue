<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { Minus, X } from 'lucide-vue-next'
import appIconUrl from '../assets/app-icon.svg'

type WindowMode = 'advanced' | 'minimal'
type TransitionPhase = 'idle' | 'native-resize-before-animation' | 'preview-animating' | 'native-resize-after-animation' | 'settling'

const MODE_TRANSITION_MS = 300
const POST_RESIZE_SETTLE_MS = 50
const HOST_RESIZE_TIMEOUT_MS = 1200

const props = defineProps<{
  mode: WindowMode
}>()

const appliedMode = ref<WindowMode>(props.mode)
const visualMode = ref<WindowMode>(props.mode)
const transitionPhase = ref<TransitionPhase>('idle')
const previewAnimating = ref(false)
let windowModeRequest = 0

const isMac = computed(() => {
  if (typeof navigator === 'undefined') return false
  return navigator.platform.toLowerCase().includes('mac')
})

const isWindows = computed(() => !isMac.value)
const transitionActive = computed(() => transitionPhase.value !== 'idle')
const titlebarTitle = computed(() => visualMode.value === 'minimal' ? 'EXV for ECNU' : 'ECNU VPN')
const frameClass = computed(() => [
  'app-window-frame',
  `app-window-frame--${visualMode.value}`,
  isMac.value ? 'app-window-frame--mac' : 'app-window-frame--windows',
  transitionActive.value ? 'app-window-frame--transitioning' : '',
  previewAnimating.value ? 'app-window-frame--preview-animating' : '',
])

function wait(ms: number) {
  return new Promise<void>((resolve) => window.setTimeout(resolve, ms))
}

function afterNextPaint() {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

async function waitForPreviewAnimation() {
  previewAnimating.value = true
  await afterNextPaint()
  await wait(MODE_TRANSITION_MS)
  previewAnimating.value = false
}

async function resizeNativeWindow(mode: WindowMode, request: number) {
  const resize = window.ecnuVpn?.window?.resizeForMode ?? window.ecnuVpn?.window?.setMode
  if (!resize) return
  await Promise.race([
    resize(mode, request),
    wait(HOST_RESIZE_TIMEOUT_MS).then(() => {
      throw new Error('window resize timed out')
    }),
  ])
}

async function runModeTransition(nextMode: WindowMode) {
  const request = ++windowModeRequest
  if (appliedMode.value === nextMode) {
    visualMode.value = nextMode
    transitionPhase.value = 'idle'
    return
  }

  try {
    if (appliedMode.value === 'minimal' && nextMode === 'advanced') {
      transitionPhase.value = 'native-resize-before-animation'
      visualMode.value = 'minimal'
      await resizeNativeWindow(nextMode, request)
      if (request !== windowModeRequest) return
      transitionPhase.value = 'preview-animating'
      visualMode.value = 'advanced'
      await waitForPreviewAnimation()
    } else if (appliedMode.value === 'advanced' && nextMode === 'minimal') {
      transitionPhase.value = 'preview-animating'
      visualMode.value = 'minimal'
      await waitForPreviewAnimation()
      if (request !== windowModeRequest) return
      transitionPhase.value = 'native-resize-after-animation'
      await resizeNativeWindow(nextMode, request)
    } else {
      transitionPhase.value = 'native-resize-before-animation'
      visualMode.value = nextMode
      await resizeNativeWindow(nextMode, request)
    }

    if (request !== windowModeRequest) return
    transitionPhase.value = 'settling'
    await wait(POST_RESIZE_SETTLE_MS)
    if (request !== windowModeRequest) return
    appliedMode.value = nextMode
    visualMode.value = nextMode
  } catch (error) {
    console.error('[window] mode transition failed:', error)
    visualMode.value = appliedMode.value
  } finally {
    if (request === windowModeRequest) {
      previewAnimating.value = false
      transitionPhase.value = 'idle'
    }
  }
}

async function minimizeWindow() {
  if (transitionActive.value) return
  await window.ecnuVpn?.window?.minimize()
}

async function requestWindowClose() {
  if (transitionActive.value) return
  await window.ecnuVpn?.window?.requestClose()
}

function startWindowDrag(event: PointerEvent) {
  if (event.button !== 0) return
  const target = event.target instanceof Element ? event.target : null
  if (target?.closest('[data-window-control-region="true"], button, a, input, textarea, select')) {
    return
  }
  event.preventDefault()
  void window.ecnuVpn?.window?.startDrag?.()
}

watch(
  () => props.mode,
  (nextMode) => {
    void runModeTransition(nextMode)
  },
)

onMounted(() => {
  appliedMode.value = props.mode
  visualMode.value = props.mode
})
</script>

<template>
  <div :class="frameClass">
    <div class="app-window-transparent-host">
      <div class="mode-transition-surface">
        <header
          class="app-window-titlebar"
          data-window-drag-region="true"
          @pointerdown="startWindowDrag"
        >
          <div
            v-if="visualMode === 'minimal'"
            class="app-window-titlebar__identity"
          >
            <img class="app-window-titlebar__icon" :src="appIconUrl" alt="" />
            <span class="app-window-titlebar__title">{{ titlebarTitle }}</span>
          </div>
          <div
            v-if="isWindows"
            class="app-window-titlebar__controls"
            data-window-control-region="true"
          >
            <button
              type="button"
              class="app-window-titlebar__button"
              aria-label="最小化"
              :disabled="transitionActive"
              @click="minimizeWindow"
            >
              <Minus class="h-3.5 w-3.5" />
            </button>
            <button
              type="button"
              class="app-window-titlebar__button app-window-titlebar__button--close"
              aria-label="关闭"
              :disabled="transitionActive"
              @click="requestWindowClose"
            >
              <X class="h-3.5 w-3.5" />
            </button>
          </div>
        </header>

        <section class="app-window-content-shell">
          <slot />
        </section>

        <div
          v-if="transitionActive"
          class="mode-transition-overlay"
          aria-hidden="true"
        >
          <img class="mode-transition-icon" :src="appIconUrl" alt="" />
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.app-window-frame {
  --titlebar-height: 34px;
  --advanced-width: 972px;
  --advanced-height: 563px;
  --minimal-width: 302px;
  --minimal-height: 118px;
  --mac-traffic-light-inset: 78px;
  --window-radius: 16px;
  min-height: 100vh;
  overflow: hidden;
  background: transparent;
  color: inherit;
}

.app-window-transparent-host {
  min-height: 100vh;
  overflow: hidden;
  background: transparent;
}

.mode-transition-surface {
  position: relative;
  overflow: hidden;
  width: 100vw;
  height: 100vh;
  border-radius: var(--window-radius);
  background: transparent;
  transform-origin: top left;
  transition:
    width 300ms cubic-bezier(0.16, 1, 0.3, 1),
    height 300ms cubic-bezier(0.16, 1, 0.3, 1);
}

.app-window-frame--minimal.app-window-frame--transitioning .mode-transition-surface {
  width: var(--minimal-width);
  height: var(--minimal-height);
}

.app-window-frame--advanced.app-window-frame--transitioning .mode-transition-surface {
  width: var(--advanced-width);
  height: var(--advanced-height);
}

.app-window-titlebar {
  position: absolute;
  z-index: 60;
  top: 0;
  right: 0;
  left: 0;
  display: flex;
  height: var(--titlebar-height);
  align-items: center;
  justify-content: space-between;
  padding: 4px 4px 4px 10px;
  user-select: none;
  background: transparent;
  color: #f8fafc;
}

.app-window-frame--advanced .app-window-titlebar {
  left: 0;
  right: 0;
  justify-content: flex-end;
  padding: 0 4px 0 0;
}

.app-window-frame--advanced .app-window-titlebar__identity {
  display: none;
}

.app-window-frame--minimal .app-window-titlebar {
  position: relative;
  top: auto;
  right: auto;
  left: auto;
  padding: 0 0 0 10px;
  border-bottom: 0;
  background: rgba(9, 16, 30, 0.92);
}

.app-window-frame--mac .app-window-titlebar {
  padding-left: var(--mac-traffic-light-inset);
}

.app-window-titlebar__identity {
  display: inline-flex;
  min-width: 0;
  height: 100%;
  align-items: center;
  gap: 8px;
}

.app-window-titlebar__icon {
  height: 18px;
  width: 18px;
}

.app-window-titlebar__title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0;
}

.app-window-titlebar__controls {
  display: flex;
  height: 100%;
  align-items: stretch;
  overflow: hidden;
}

.app-window-titlebar__button {
  display: grid;
  width: 44px;
  place-items: center;
  color: #cbd5e1;
  transition: background-color 120ms ease, color 120ms ease;
}

.app-window-titlebar__button:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.12);
  color: white;
}

.app-window-titlebar__button--close:hover:not(:disabled) {
  background: #dc2626;
  color: white;
}

.app-window-titlebar__button:disabled {
  cursor: default;
  opacity: 0.45;
}

.app-window-content-shell {
  height: 100%;
  overflow: hidden;
  background: transparent;
}

.app-window-frame--advanced .app-window-content-shell {
  height: 100%;
}

.app-window-frame--minimal .app-window-content-shell {
  height: calc(100% - var(--titlebar-height));
}

.mode-transition-overlay {
  position: absolute;
  inset: 0;
  z-index: 80;
  display: grid;
  place-items: center;
  border-radius: inherit;
  background: #0a1223;
  backdrop-filter: blur(18px) saturate(1.1);
  pointer-events: auto;
}

.mode-transition-icon {
  width: 3.4rem;
  height: 3.4rem;
  display: block;
  filter: drop-shadow(0 0.45rem 0.9rem rgba(0, 0, 0, 0.24));
}
</style>
