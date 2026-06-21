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
import { useConfigStore } from '../stores/config'
import { useVpnStore } from '../stores/vpn'

defineProps<{
  coreCrashed: boolean
  coreCrashInfo: CoreCrashedEvent | null
}>()

const emit = defineEmits<{
  restartCore: []
  quitCore: []
}>()

const config = useConfigStore()
const vpn = useVpnStore()
const closePromptVisible = ref(false)
const closePromptBusy = ref(false)
type CloseChoice = 'smart' | 'tray' | 'quit'
const closeChoice = ref<CloseChoice>('smart')
const rememberCloseChoice = ref(true)
let closeEventUnsubscribe: (() => void) | null = null

const serviceOverlayMessage = computed(() => {
  if (vpn.serviceOverlayOperation === 'install') return '正在安装服务'
  if (vpn.serviceOverlayOperation === 'uninstall') return '正在卸载服务'
  if (vpn.serviceOverlayOperation === 'repair') return '正在修复服务'
  return ''
})
const compactModals = computed(() => config.settings.minimal_mode)

onMounted(() => {
  closeEventUnsubscribe = window.exv?.events.subscribe((event) => {
    if (event.type !== 'close-request') return
    closeChoice.value = 'smart'
    rememberCloseChoice.value = true
    closePromptVisible.value = true
  }) ?? null
})

onUnmounted(() => {
  closeEventUnsubscribe?.()
  closeEventUnsubscribe = null
})

async function resolveClosePrompt(result: 'cancel' | { action: CloseChoice; remember: boolean }) {
  if (closePromptBusy.value) return
  closePromptBusy.value = true
  closePromptVisible.value = false
  try {
    await window.exv?.window.resolveClosePrompt(result)
  } finally {
    closePromptBusy.value = false
  }
}

function confirmClosePrompt(action?: CloseChoice) {
  if (action) {
    void resolveClosePrompt({
      action,
      remember: true,
    })
    return
  }
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
    :description="compactModals ? '' : '选择关闭窗口后的处理方式。'"
    :close-on-scrim="!closePromptBusy"
    :compact="compactModals"
    size="md"
    @close="resolveClosePrompt('cancel')"
  >
    <template #icon>
      <span class="text-sm font-semibold text-warning">!</span>
    </template>

    <p v-if="compactModals" class="modal-compact-message">
      智能处理：有连接时缩小到托盘，无连接时直接退出。
    </p>

    <template v-else>
    <fieldset class="mt-4 space-y-2">
      <label
        class="flex cursor-pointer items-start gap-3 rounded-lg border px-3 py-2.5 transition-colors"
        :class="closeChoice === 'smart' ? 'border-accent/60 bg-accent/10' : 'border-border hover:border-accent/40'"
      >
        <input
          v-model="closeChoice"
          type="radio"
          value="smart"
          class="mt-1 h-4 w-4 shrink-0 accent-accent"
        />
        <span class="min-w-0">
          <span class="block text-sm font-medium text-foreground">智能处理</span>
          <span class="mt-0.5 block text-xs leading-5 text-muted">有连接时缩小到托盘，无连接时直接退出。</span>
        </span>
      </label>

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
    </template>

    <template #actions>
      <template v-if="compactModals">
        <button
          type="button"
          :disabled="closePromptBusy"
          class="border border-border text-muted hover:bg-surface/80 disabled:opacity-50"
          @click="resolveClosePrompt('cancel')"
        >
          取消
        </button>
        <button
          type="button"
          :disabled="closePromptBusy"
          class="border border-border text-muted hover:bg-surface/80 disabled:opacity-50"
          @click="confirmClosePrompt('tray')"
        >
          后台
        </button>
        <button
          type="button"
          :disabled="closePromptBusy"
          class="border border-destructive/50 text-destructive hover:bg-destructive/10 disabled:opacity-50"
          @click="confirmClosePrompt('quit')"
        >
          退出
        </button>
        <button
          type="button"
          :disabled="closePromptBusy"
          class="bg-accent text-white hover:bg-accent/90 disabled:opacity-50"
          @click="confirmClosePrompt('smart')"
        >
          智能
        </button>
      </template>
      <template v-else>
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
        @click="confirmClosePrompt()"
      >
        确认
      </button>
      </template>
    </template>
  </ModalShell>

  <ServiceInstallLoadingOverlay
    v-if="vpn.serviceOverlayOperation"
    :message="serviceOverlayMessage"
    :compact="compactModals"
  />
  <CoreCrashed
    v-if="coreCrashed"
    :exit-code="coreCrashInfo?.exitCode ?? null"
    :compact="compactModals"
    @restart="emit('restartCore')"
    @quit="emit('quitCore')"
  />
  <ErrorDialog :compact="compactModals" />
  <ConfirmDialog :compact="compactModals" />
  <AuthContinuationDialog :compact="compactModals" />
  <CredentialPromptDialog :compact="compactModals" />
  <QuickStartDialog />
  <PasswordPromptDialog :compact="compactModals" />
  <ToastStack />
</template>
