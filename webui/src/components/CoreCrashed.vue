<script setup lang="ts">
import { AlertTriangle } from 'lucide-vue-next'
import ModalShell from './ModalShell.vue'

defineProps<{ exitCode: number | null }>()
defineEmits<{ restart: []; quit: [] }>()
</script>

<template>
  <ModalShell
    open
    title="核心进程已退出"
    description="VPN 核心进程已终止，VPN 连接可能已中断。"
    :close-on-scrim="false"
    size="sm"
  >
    <template #icon>
      <AlertTriangle class="h-4 w-4 text-warning" />
    </template>

    <p v-if="exitCode !== null" class="rounded-lg border border-border bg-bg px-3 py-2 font-mono text-xs text-destructive">
      退出码: {{ exitCode }}
    </p>

    <template #actions>
      <button
        type="button"
        class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80"
        @click="$emit('quit')"
      >
        退出程序
      </button>
      <button
        type="button"
        class="rounded-lg bg-primary px-3 py-2 text-sm text-white hover:bg-primary/90"
        @click="$emit('restart')"
      >
        重启内核
      </button>
    </template>
  </ModalShell>
</template>
