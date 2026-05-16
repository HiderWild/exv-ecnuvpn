<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useVpnStore, type LogEntry } from '../stores/vpn'
import { useSSE } from '../composables/useSSE'
import { FileText, Trash2, Download } from 'lucide-vue-next'

const vpn = useVpnStore()
const { connect: sseConnect, disconnect: sseDisconnect } = useSSE()
const autoScroll = ref(true)

onMounted(async () => {
  sseConnect()
  try {
    const api = (await import('../api/desktop')).default
    const { data } = await api.get<LogEntry[]>('/logs')
    if (Array.isArray(data)) {
      vpn.setLogs(data)
    }
  } catch {}
})

onUnmounted(() => {
  sseDisconnect()
})

function clearLogs() {
  vpn.clearLogs()
}

function downloadLogs() {
  const text = vpn.logs
    .map((e: LogEntry) => `[${e.level.toUpperCase()}] ${e.message}`)
    .join('\n')
  const blob = new Blob([text], { type: 'text/plain' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `ecnu-vpn-${new Date().toISOString().slice(0, 10)}.log`
  a.click()
  URL.revokeObjectURL(url)
}
</script>

<template>
  <div class="py-8">
    <div class="flex items-center justify-between mb-4">
      <h1 class="text-xl font-semibold text-foreground flex items-center gap-2">
        <FileText class="w-5 h-5 text-accent" />
        日志
      </h1>
      <div class="flex items-center gap-2">
        <label class="flex items-center gap-2 text-xs text-muted cursor-pointer">
          <input v-model="autoScroll" type="checkbox" class="w-3.5 h-3.5 accent-accent" />
          自动滚动
        </label>
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
      class="bg-bg border border-border rounded-xl p-4 h-[calc(100vh-200px)] overflow-y-auto font-mono text-xs leading-relaxed"
    >
      <div v-if="vpn.logs.length === 0" class="text-muted text-center py-8">
        暂无日志
      </div>
      <div
        v-for="(entry, i) in vpn.logs"
        :key="i"
        class="whitespace-pre-wrap break-all"
        :class="{
          'text-red-400': entry.level === 'error' || entry.message.includes('[ERROR]') || entry.message.includes('[error]'),
          'text-yellow-400': entry.level === 'warn' || entry.message.includes('[WARN]') || entry.message.includes('[warn]'),
          'text-foreground': entry.level === 'info' && !entry.message.includes('[ERROR]') && !entry.message.includes('[error]') && !entry.message.includes('[WARN]') && !entry.message.includes('[warn]'),
        }"
      >{{ entry.message }}</div>
    </div>
  </div>
</template>