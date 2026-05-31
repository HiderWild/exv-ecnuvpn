<script setup lang="ts">
import { ref, onActivated, onDeactivated, onMounted, onUnmounted, nextTick, computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useVpnStore, type LogEntry } from '../stores/vpn'
import { useUiStore } from '../stores/ui'
import { useSSE } from '../composables/useSSE'
import { FileText, Trash2, Download, ArrowLeft, Copy } from 'lucide-vue-next'

defineOptions({ name: 'LogsPage' })

const vpn = useVpnStore()
const ui = useUiStore()
const route = useRoute()
const router = useRouter()
const { connect: sseConnect, disconnect: sseDisconnect } = useSSE()
const autoScroll = ref(true)
const logsContainer = ref<HTMLElement | null>(null)
const savedScrollTop = ref(0)

const isContextJump = computed(() => route.query.from === 'dashboard')
const highlightCount = 10 // Number of recent entries to highlight on context jump

onMounted(async () => {
  sseConnect()
  try {
    const api = (await import('../api/desktop')).default
    const { data } = await api.get<LogEntry[]>('/logs')
    if (Array.isArray(data)) {
      vpn.setLogs(data)
    }
  } catch {}

  if (autoScroll.value) await scrollToBottom()
})

onUnmounted(() => {
  sseDisconnect()
})

async function scrollToBottom() {
  await nextTick()
  const container = logsContainer.value
  if (container) {
    container.scrollTop = container.scrollHeight
  }
}

onActivated(() => {
  void nextTick(() => {
    if (autoScroll.value) {
      void scrollToBottom()
      return
    }
    if (logsContainer.value) logsContainer.value.scrollTop = savedScrollTop.value
  })
})

onDeactivated(() => {
  savedScrollTop.value = logsContainer.value?.scrollTop ?? savedScrollTop.value
})

function clearLogs() {
  vpn.clearLogs()
}

function formatLogs() {
  return vpn.logs
    .map((e: LogEntry) => `[${e.level.toUpperCase()}] ${e.message}`)
    .join('\n')
}

function downloadLogs() {
  const text = formatLogs()
  const blob = new Blob([text], { type: 'text/plain' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `ecnu-vpn-${new Date().toISOString().slice(0, 10)}.log`
  a.click()
  URL.revokeObjectURL(url)
}

function copyTextFallback(text: string) {
  const textarea = document.createElement('textarea')
  textarea.value = text
  textarea.setAttribute('readonly', 'true')
  textarea.style.position = 'fixed'
  textarea.style.left = '-9999px'
  document.body.appendChild(textarea)
  textarea.select()
  const ok = document.execCommand('copy')
  document.body.removeChild(textarea)
  return ok
}

async function copyLogs() {
  const text = formatLogs()
  if (!text) {
    ui.addToast('暂无日志可复制', 'warning')
    return
  }
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
    } else if (!copyTextFallback(text)) {
      throw new Error('Clipboard API is unavailable.')
    }
    ui.addToast('日志已复制', 'success')
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    ui.requestError({ title: '复制日志失败', message })
  }
}

watch(
  () => vpn.logs.length,
  () => {
    if (autoScroll.value) void scrollToBottom()
  },
)

watch(autoScroll, (enabled) => {
  if (enabled) void scrollToBottom()
})

function shouldHighlight(index: number): boolean {
  if (!isContextJump.value) return false
  // Highlight the last `highlightCount` entries
  return index >= vpn.logs.length - highlightCount
}
</script>

<template>
  <div class="h-full overflow-hidden py-4">
    <div class="flex items-center justify-between mb-4">
      <div class="flex items-center gap-3">
        <button
          v-if="isContextJump"
          class="flex items-center gap-1.5 text-xs text-muted hover:text-foreground transition-colors"
          @click="router.push('/')"
        >
          <ArrowLeft class="w-3.5 h-3.5" />
          返回 Dashboard
        </button>
        <h1 class="text-xl font-semibold text-foreground flex items-center gap-2">
          <FileText class="w-5 h-5 text-accent" />
          日志
        </h1>
      </div>
      <div class="flex items-center gap-2">
        <label class="flex items-center gap-2 text-xs text-muted cursor-pointer">
          <input v-model="autoScroll" type="checkbox" class="w-3.5 h-3.5 accent-accent" />
          自动滚动
        </label>
        <button
          class="flex items-center gap-1.5 border border-border text-muted rounded-lg px-3 py-1.5 text-xs hover:text-foreground transition-colors"
          @click="copyLogs"
        >
          <Copy class="w-3.5 h-3.5" />
          复制
        </button>
        <button
          class="flex items-center gap-1.5 border border-border text-muted rounded-lg px-3 py-1.5 text-xs hover:text-foreground transition-colors"
          @click="downloadLogs"
        >
          <Download class="w-3.5 h-3.5" />
          导出
        </button>
        <button
          class="flex items-center gap-1.5 border border-border text-muted rounded-lg px-3 py-1.5 text-xs hover:text-foreground transition-colors"
          @click="clearLogs"
        >
          <Trash2 class="w-3.5 h-3.5" />
          清空
        </button>
      </div>
    </div>

    <div
      id="logs-container"
      ref="logsContainer"
      class="bg-bg border border-border rounded-xl p-4 h-[calc(100%-4rem)] overflow-y-auto font-mono text-xs leading-relaxed"
    >
      <div v-if="vpn.logs.length === 0" class="text-muted text-center py-8">
        暂无日志
      </div>
      <div
        v-for="(entry, i) in vpn.logs"
        :key="i"
        class="whitespace-pre-wrap break-all"
        :class="[
          shouldHighlight(i) ? 'log-highlight' : '',
          {
            'text-red-400': entry.level === 'error' || entry.message.includes('[ERROR]') || entry.message.includes('[error]'),
            'text-yellow-400': entry.level === 'warn' || entry.message.includes('[WARN]') || entry.message.includes('[warn]'),
            'text-foreground': entry.level === 'info' && !entry.message.includes('[ERROR]') && !entry.message.includes('[error]') && !entry.message.includes('[WARN]') && !entry.message.includes('[warn]'),
          }
        ]"
      >{{ entry.message }}</div>
    </div>
  </div>
</template>

<style scoped>
@keyframes highlight-fade {
  from { background-color: rgba(59, 130, 246, 0.2); }
  to { background-color: transparent; }
}
.log-highlight {
  animation: highlight-fade 2s ease-out forwards;
}
</style>
