<script setup lang="ts">
import { ref, computed } from 'vue'
import { useVpnStore } from '../stores/vpn'
import { useSSE } from '../composables/useSSE'
import { Trash2, Pause, Play, ArrowDown } from 'lucide-vue-next'

const vpn = useVpnStore()

const filter = ref('')
const paused = ref(false)
const autoScroll = ref(true)
const logContainer = ref<HTMLElement | null>(null)

useSSE()

const filteredLogs = computed(() => {
  if (!filter.value) return vpn.logs
  const q = filter.value.toLowerCase()
  return vpn.logs.filter(
    (l) => l.message.toLowerCase().includes(q) || l.level.toLowerCase().includes(q),
  )
})

function scrollToBottom() {
  if (logContainer.value) {
    logContainer.value.scrollTop = logContainer.value.scrollHeight
  }
}

function levelColor(level: string) {
  switch (level) {
    case 'error':
      return 'text-destructive'
    case 'warn':
      return 'text-warning'
    case 'debug':
      return 'text-muted'
    default:
      return 'text-foreground'
  }
}

function levelBg(level: string) {
  switch (level) {
    case 'error':
      return 'bg-destructive/10'
    case 'warn':
      return 'bg-warning/10'
    case 'debug':
      return 'bg-surface'
    default:
      return 'bg-transparent'
  }
}
</script>

<template>
  <div class="py-8">
    <div class="flex items-center justify-between mb-4">
      <h1 class="text-xl font-semibold text-foreground">Logs</h1>
      <div class="flex items-center gap-2">
        <button
          :class="[
            'p-2 rounded-lg transition-colors',
            paused ? 'text-warning bg-warning/10' : 'text-muted hover:text-foreground'
          ]"
          @click="paused = !paused"
        >
          <Play v-if="paused" class="w-4 h-4" />
          <Pause v-else class="w-4 h-4" />
        </button>
        <button
          class="p-2 rounded-lg text-muted hover:text-destructive transition-colors"
          @click="vpn.clearLogs()"
        >
          <Trash2 class="w-4 h-4" />
        </button>
      </div>
    </div>

    <!-- Search filter -->
    <div class="mb-3 flex items-center gap-3">
      <input
        v-model="filter"
        type="text"
        placeholder="Filter logs..."
        class="flex-1 bg-surface border border-border rounded-lg px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
      />
      <label class="flex items-center gap-1.5 text-xs text-muted cursor-pointer select-none">
        <input v-model="autoScroll" type="checkbox" class="rounded" />
        Auto-scroll
      </label>
    </div>

    <!-- Log container -->
    <div
      ref="logContainer"
      class="bg-bg border border-border rounded-xl overflow-auto font-mono text-xs leading-relaxed max-h-[60vh]"
    >
      <div v-if="!filteredLogs.length" class="flex items-center justify-center py-16 text-muted">
        No logs yet
      </div>
      <div
        v-for="(log, i) in filteredLogs"
        :key="i"
        :class="['flex gap-3 px-4 py-1.5 border-b border-border/50', levelBg(log.level)]"
      >
        <span class="text-muted shrink-0 w-20">{{ log.timestamp }}</span>
        <span :class="['shrink-0 w-12 font-medium uppercase', levelColor(log.level)]">
          {{ log.level }}
        </span>
        <span :class="['flex-1', levelColor(log.level)]">{{ log.message }}</span>
      </div>
    </div>

    <!-- Scroll to bottom button -->
    <button
      v-if="!autoScroll"
      class="mt-3 flex items-center gap-1.5 mx-auto text-xs text-muted hover:text-foreground transition-colors"
      @click="scrollToBottom"
    >
      <ArrowDown class="w-3.5 h-3.5" />
      Scroll to bottom
    </button>

    <p class="text-xs text-muted mt-2">
      {{ filteredLogs.length }} log{{ filteredLogs.length !== 1 ? 's' : '' }}
      <span v-if="paused" class="text-warning ml-2">(paused)</span>
    </p>
  </div>
</template>