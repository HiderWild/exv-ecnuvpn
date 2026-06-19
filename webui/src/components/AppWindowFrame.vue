<script setup lang="ts">
import { computed } from 'vue'
import { Minus, X } from 'lucide-vue-next'

type WindowMode = 'advanced' | 'minimal'

const props = defineProps<{
  mode: WindowMode
}>()

const isMac = computed(() => {
  if (typeof navigator === 'undefined') return false
  return navigator.platform.toLowerCase().includes('mac')
})

const isWindows = computed(() => !isMac.value)
const frameClass = computed(() => [
  'app-window-frame',
  `app-window-frame--${props.mode}`,
  isMac.value ? 'app-window-frame--mac' : 'app-window-frame--windows',
])

async function minimizeWindow() {
  await window.ecnuVpn?.window?.minimize()
}

async function requestWindowClose() {
  await window.ecnuVpn?.window?.requestClose()
}
</script>

<template>
  <div :class="frameClass">
    <header class="app-window-titlebar" data-window-drag-region="true">
      <div class="app-window-titlebar__identity">
        <img class="app-window-titlebar__icon" src="/favicon.svg" alt="" />
        <span class="app-window-titlebar__title">ECNU VPN</span>
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
          @click="minimizeWindow"
        >
          <Minus class="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          class="app-window-titlebar__button app-window-titlebar__button--close"
          aria-label="关闭"
          @click="requestWindowClose"
        >
          <X class="h-3.5 w-3.5" />
        </button>
      </div>
    </header>

    <section class="app-window-content-shell">
      <slot />
    </section>
  </div>
</template>

<style scoped>
.app-window-frame {
  --titlebar-height: 34px;
  --mac-traffic-light-inset: 78px;
  min-height: 100vh;
  overflow: hidden;
  background: transparent;
  color: inherit;
}

.app-window-titlebar {
  display: flex;
  height: var(--titlebar-height);
  align-items: center;
  justify-content: space-between;
  padding: 0 0 0 10px;
  user-select: none;
  background: rgba(9, 16, 30, 0.92);
  color: #f8fafc;
}

.app-window-frame--mac .app-window-titlebar {
  padding-left: var(--mac-traffic-light-inset);
}

.app-window-titlebar__identity {
  display: inline-flex;
  min-width: 0;
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
}

.app-window-titlebar__button {
  display: grid;
  width: 44px;
  place-items: center;
  color: #cbd5e1;
  transition: background-color 120ms ease, color 120ms ease;
}

.app-window-titlebar__button:hover {
  background: rgba(255, 255, 255, 0.12);
  color: white;
}

.app-window-titlebar__button--close:hover {
  background: #dc2626;
  color: white;
}

.app-window-content-shell {
  min-height: calc(100vh - var(--titlebar-height));
  overflow: hidden;
  background: transparent;
}
</style>
