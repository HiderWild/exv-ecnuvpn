<script setup lang="ts">
defineProps<{
  status: 'connected' | 'disconnected' | 'connecting' | 'disconnecting' | 'error' | 'installed' | 'running' | 'stopped' | 'cleanup_pending' | 'elevated'
}>()

const statusLabels: Record<string, string> = {
  connected: '已连接',
  disconnected: '未连接',
  connecting: '连接中',
  disconnecting: '断开中',
  error: '错误',
  installed: '已安装',
  running: '运行中',
  stopped: '已停止',
  cleanup_pending: '清理中',
  elevated: '临时连接',
}
</script>

<template>
  <span
    :class="[
      'inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium',
      status === 'connected' || status === 'running' ? 'bg-accent/20 text-accent' :
      status === 'disconnected' || status === 'stopped' ? 'bg-muted/20 text-muted' :
      status === 'connecting' || status === 'disconnecting' ? 'bg-warning/20 text-warning' :
      status === 'cleanup_pending' ? 'bg-warning/20 text-warning' :
      status === 'elevated' ? 'bg-blue-500/20 text-blue-400' :
      status === 'error' ? 'bg-destructive/20 text-destructive' :
      'bg-primary/20 text-foreground'
    ]"
  >
    <span
      :class="[
        'h-1.5 w-1.5 rounded-full',
        status === 'connected' || status === 'running' ? 'bg-accent' :
        status === 'disconnected' || status === 'stopped' ? 'bg-muted' :
        status === 'connecting' || status === 'disconnecting' ? 'bg-warning animate-pulse' :
        status === 'cleanup_pending' ? 'bg-warning animate-pulse' :
        status === 'elevated' ? 'bg-blue-400' :
        status === 'error' ? 'bg-destructive' :
        'bg-foreground'
      ]"
    />
    {{ statusLabels[status] || status }}
  </span>
</template>