<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { RouterView } from 'vue-router'
import { useRoute } from 'vue-router'
import NavBar from './components/NavBar.vue'
import ConfirmDialog from './components/ConfirmDialog.vue'
import CoreCrashed from './components/CoreCrashed.vue'
import ErrorDialog from './components/ErrorDialog.vue'
import MinimalModeView from './components/MinimalModeView.vue'
import PasswordPromptDialog from './components/PasswordPromptDialog.vue'
import ServiceInstallLoadingOverlay from './components/ServiceInstallLoadingOverlay.vue'
import ToastStack from './components/Toast.vue'
import { useSSE } from './composables/useSSE'
import { useConfigStore } from './stores/config'
import { useUiStore } from './stores/ui'
import { useVpnStore } from './stores/vpn'

const config = useConfigStore()
const vpn = useVpnStore()
const ui = useUiStore()
const { connect: sseConnect, disconnect: sseDisconnect, coreCrashed, coreCrashInfo, resetCrashState } = useSSE()
const route = useRoute()

const servicePromptVisible = ref(false)
const servicePromptBusy = ref(false)
const closePromptVisible = ref(false)
const closePromptBusy = ref(false)
const closeChoice = ref<'tray' | 'quit'>('tray')
const rememberCloseChoice = ref(false)
let closeEventUnsubscribe: (() => void) | null = null
const minimalMode = computed(() => config.settings.minimal_mode)
const keptAlivePages = ['DashboardPage', 'SettingsPage', 'LogsPage']
const serviceOverlayMessage = computed(() => {
  if (vpn.serviceOverlayOperation === 'install') return '正在安装服务'
  if (vpn.serviceOverlayOperation === 'uninstall') return '正在卸载服务'
  return ''
})
const modalRoute = computed(() =>
  route.path.startsWith('/modal/') ||
  (typeof window !== 'undefined' && window.location.hash.startsWith('#/modal/')),
)

onMounted(async () => {
  if (modalRoute.value) return
  sseConnect()
  closeEventUnsubscribe = window.ecnuVpn?.events.subscribe((event) => {
    if (event.type !== 'close-request') return
    closeChoice.value = 'tray'
    rememberCloseChoice.value = false
    closePromptVisible.value = true
  }) ?? null
  await Promise.allSettled([
    config.fetchSettings(),
    config.fetchAuthConfig(),
    vpn.fetchAppShellState(),
  ])
  await window.ecnuVpn?.window?.setMode(config.settings.minimal_mode ? 'minimal' : 'advanced')
  if (!config.settings.service_install_prompt_seen && !vpn.serviceAvailable) {
    if (config.settings.minimal_mode && window.ecnuVpn?.modal) {
      const result = await window.ecnuVpn.modal.serviceInstallPrompt()
      await markServicePromptSeen()
      if (result === 'install') {
        const installed = await vpn.installService()
        if (installed) {
          ui.addToast('辅助服务已安装', 'success')
        }
      }
      return
    }
    servicePromptVisible.value = true
  }
})

onUnmounted(() => {
  sseDisconnect()
  closeEventUnsubscribe?.()
  closeEventUnsubscribe = null
})

async function markServicePromptSeen() {
  if (config.settings.service_install_prompt_seen) return
  await config.saveSettings({ service_install_prompt_seen: true })
}

async function dismissServicePrompt() {
  servicePromptBusy.value = true
  try {
    await markServicePromptSeen()
    servicePromptVisible.value = false
  } finally {
    servicePromptBusy.value = false
  }
}

async function installServiceFromPrompt() {
  servicePromptBusy.value = true
  try {
    await markServicePromptSeen()
    servicePromptVisible.value = false
    const installed = await vpn.installService()
    if (installed) {
      ui.addToast('辅助服务已安装', 'success')
    }
  } finally {
    servicePromptBusy.value = false
  }
}

async function resolveClosePrompt(result: 'cancel' | { action: 'tray' | 'quit'; remember: boolean }) {
  if (closePromptBusy.value) return
  closePromptBusy.value = true
  try {
    await window.ecnuVpn?.window.resolveClosePrompt(result)
    closePromptVisible.value = false
  } finally {
    closePromptBusy.value = false
  }
}

function confirmClosePrompt() {
  void resolveClosePrompt({
    action: closeChoice.value,
    remember: rememberCloseChoice.value,
  })
}

async function handleCoreRestart() {
  try {
    await window.ecnuVpn?.core.restart()
    resetCrashState()
    ui.addToast('内核已重启', 'success')
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    ui.addToast(`重启失败: ${message}`, 'error')
  }
}

async function handleCoreQuit() {
  try {
    await window.ecnuVpn?.core.quit()
  } catch {
    // App is quitting, errors expected
  }
}
</script>

<template>
  <RouterView v-if="modalRoute" />
  <MinimalModeView v-else-if="minimalMode" />
  <div v-else class="flex h-screen overflow-hidden bg-bg text-foreground font-sans">
    <NavBar />
    <main class="min-w-0 flex-1 overflow-hidden pl-44">
      <div class="mx-auto h-full w-full max-w-5xl overflow-hidden px-6 py-6">
        <RouterView v-slot="{ Component, route: viewRoute }">
          <KeepAlive :include="keptAlivePages">
            <component :is="Component" :key="viewRoute.name" />
          </KeepAlive>
        </RouterView>
      </div>
    </main>
  </div>

  <div
    v-if="servicePromptVisible"
    class="fixed inset-0 z-50 grid place-items-center bg-black/55 px-4 backdrop-blur-sm"
  >
    <section class="w-full max-w-sm rounded-lg border border-border bg-surface p-5 shadow-xl shadow-black/30">
      <h2 class="text-base font-semibold text-foreground">建议您安装辅助服务</h2>
      <p class="mt-2 text-sm leading-6 text-muted">
        安装服务可以免于输入密码提权，精简连接流程。需要管理员权限。
      </p>
      <div class="mt-5 flex justify-end gap-2">
        <button
          type="button"
          :disabled="servicePromptBusy"
          class="rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-accent/50 disabled:opacity-50"
          @click="dismissServicePrompt"
        >
          暂不安装
        </button>
        <button
          type="button"
          :disabled="servicePromptBusy"
          class="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-accent/90 disabled:opacity-50"
          @click="installServiceFromPrompt"
        >
          安装服务
        </button>
      </div>
    </section>
  </div>

  <div
    v-if="closePromptVisible"
    class="fixed inset-0 z-50 grid place-items-center bg-black/55 px-4 backdrop-blur-sm"
  >
    <section class="w-full max-w-md rounded-lg border border-border bg-surface p-5 shadow-xl shadow-black/30">
      <div class="flex items-start gap-2.5">
        <div class="mt-0.5 grid h-5 w-5 shrink-0 place-items-center text-warning">
          !
        </div>
        <div class="min-w-0">
          <h2 class="text-base font-semibold text-foreground">关闭 EXV for ECNU</h2>
          <p class="mt-2 text-sm leading-6 text-muted">选择关闭窗口后的处理方式。</p>
        </div>
      </div>

      <fieldset class="mt-4 space-y-2">
        <label
          class="flex cursor-pointer items-start gap-3 rounded-lg border px-3 py-2.5 transition-colors"
          :class="closeChoice === 'tray' ? 'border-accent/60 bg-accent/10' : 'border-border hover:border-accent/40'"
        >
          <input
            v-model="closeChoice"
            type="radio"
            value="tray"
            class="mt-1 h-4 w-4 shrink-0 accent-accent"
          />
          <span class="min-w-0">
            <span class="block text-sm font-medium text-foreground">保持后台运行</span>
            <span class="mt-0.5 block text-xs leading-5 text-muted">关闭窗口后保留托盘入口，VPN 状态继续保持。</span>
          </span>
        </label>

        <label
          class="flex cursor-pointer items-start gap-3 rounded-lg border px-3 py-2.5 transition-colors"
          :class="closeChoice === 'quit' ? 'border-destructive/70 bg-destructive/10' : 'border-border hover:border-destructive/40'"
        >
          <input
            v-model="closeChoice"
            type="radio"
            value="quit"
            class="mt-1 h-4 w-4 shrink-0 accent-destructive"
          />
          <span class="min-w-0">
            <span class="block text-sm font-medium text-foreground">退出并断连</span>
            <span class="mt-0.5 block text-xs leading-5 text-muted">断开 VPN 后退出 EXV。</span>
          </span>
        </label>
      </fieldset>

      <label class="mt-3 flex cursor-pointer items-center gap-2 text-xs text-muted">
        <input
          v-model="rememberCloseChoice"
          type="checkbox"
          class="h-4 w-4 rounded border-border bg-bg accent-accent"
        />
        <span>记住我的选择</span>
      </label>

      <div class="mt-5 flex justify-end gap-2">
        <button
          type="button"
          :disabled="closePromptBusy"
          class="rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-accent/50 disabled:opacity-50"
          @click="resolveClosePrompt('cancel')"
        >
          取消
        </button>
        <button
          type="button"
          :disabled="closePromptBusy"
          class="rounded-lg px-4 py-2 text-sm font-medium text-white transition-colors disabled:opacity-50"
          :class="closeChoice === 'quit' ? 'bg-destructive hover:bg-destructive/90' : 'bg-accent hover:bg-accent/90'"
          @click="confirmClosePrompt"
        >
          确认
        </button>
      </div>
    </section>
  </div>

  <ServiceInstallLoadingOverlay
    v-if="vpn.serviceOverlayOperation"
    :message="serviceOverlayMessage"
  />
  <CoreCrashed
    v-if="coreCrashed"
    :exit-code="coreCrashInfo?.exitCode ?? null"
    @restart="handleCoreRestart"
    @quit="handleCoreQuit"
  />
  <ErrorDialog />
  <ConfirmDialog />
  <PasswordPromptDialog />
  <ToastStack />
</template>
