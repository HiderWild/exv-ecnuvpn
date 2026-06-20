<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import AuthContinuationDialog from '../components/AuthContinuationDialog.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import CoreCrashed from '../components/CoreCrashed.vue'
import CredentialPromptDialog from '../components/CredentialPromptDialog.vue'
import ErrorDialog from '../components/ErrorDialog.vue'
import ModalShell from '../components/ModalShell.vue'
import PasswordPromptDialog from '../components/PasswordPromptDialog.vue'
import QuickStartDialog from '../components/QuickStartDialog.vue'
import ServiceInstallLoadingOverlay from '../components/ServiceInstallLoadingOverlay.vue'
import ToastStack from '../components/Toast.vue'
import type { CoreCrashedEvent } from '../composables/useSSE'
import { useVpnStore } from '../stores/vpn'

defineProps<{
  coreCrashed: boolean
  coreCrashInfo: CoreCrashedEvent | null
}>()

const emit = defineEmits<{
  restartCore: []
  quitCore: []
}>()

const vpn = useVpnStore()
const closePromptVisible = ref(false)
const closePromptBusy = ref(false)
const closeChoice = ref<'tray' | 'quit'>('tray')
const rememberCloseChoice = ref(false)
let closeEventUnsubscribe: (() => void) | null = null

const serviceOverlayMessage = computed(() => {
  if (vpn.serviceOverlayOperation === 'install') return '正在安装服务'
  if (vpn.serviceOverlayOperation === 'uninstall') return '正在卸载服务'
  return ''
})

onMounted(() => {
  closeEventUnsubscribe = window.exv?.events.subscribe((event) => {
    if (event.type !== 'close-request') return
    closeChoice.value = 'tray'
    rememberCloseChoice.value = false
    closePromptVisible.value = true
  }) ?? null
})

onUnmounted(() => {
  closeEventUnsubscribe?.()
  closeEventUnsubscribe = null
})

async function resolveClosePrompt(result: 'cancel' | { action: 'tray' | 'quit'; remember: boolean }) {
  if (closePromptBusy.value) return
  closePromptBusy.value = true
  closePromptVisible.value = false
  try {
    await window.exv?.window.resolveClosePrompt(result)
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
</script>

<template>
  <ModalShell
    :open="closePromptVisible"
    title="关闭 EXV"
    description="选择关闭窗口后的处理方式。"
    :close-on-scrim="!closePromptBusy"
    size="md"
    @close="resolveClosePrompt('cancel')"
  >
    <template #icon>
      <span class="text-sm font-semibold text-warning">!</span>
    </template>
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

    <template #actions>
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
    </template>
  </ModalShell>

  <ServiceInstallLoadingOverlay
    v-if="vpn.serviceOverlayOperation"
    :message="serviceOverlayMessage"
  />
  <CoreCrashed
    v-if="coreCrashed"
    :exit-code="coreCrashInfo?.exitCode ?? null"
    @restart="emit('restartCore')"
    @quit="emit('quitCore')"
  />
  <ErrorDialog />
  <ConfirmDialog />
  <AuthContinuationDialog />
  <CredentialPromptDialog />
  <QuickStartDialog />
  <PasswordPromptDialog />
  <ToastStack />
</template>
