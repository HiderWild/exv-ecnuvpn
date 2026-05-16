<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useVpnStore } from '../stores/vpn'
import { useSSE } from '../composables/useSSE'
import StatusBadge from '../components/StatusBadge.vue'
import {
  Plug, PlugZap, ArrowDownToLine, ArrowUpToLine, Clock, Wifi, WifiOff, Route
} from 'lucide-vue-next'

const vpn = useVpnStore()
const { connect: sseConnect, disconnect: sseDisconnect } = useSSE()

const elapsed = ref(0)
let timer: ReturnType<typeof setInterval> | null = null

onMounted(() => {
  vpn.fetchStatus()
  sseConnect()
  timer = setInterval(() => {
    if (vpn.status?.connected) elapsed.value++
  }, 1000)
})

onUnmounted(() => {
  if (timer) clearInterval(timer)
  sseDisconnect()
})

const uptimeFormatted = computed(() => {
  const h = Math.floor(elapsed.value / 3600)
  const m = Math.floor((elapsed.value % 3600) / 60)
  const s = elapsed.value % 60
  return `${h}h ${m}m ${s}s`
})

const bytesFormatted = (bytes: number) => {
  if (!bytes) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  let i = 0
  let val = bytes
  while (val >= 1024 && i < units.length - 1) { val /= 1024; i++ }
  return `${val.toFixed(1)} ${units[i]}`
}

const upstreamVirtualNames = computed(() => {
  const adapters = vpn.status?.upstream_virtual_adapters || []
  return adapters.map((adapter) => adapter.name).filter(Boolean).join('、')
})
</script>

<template>
  <div class="py-8">
    <!-- Hero card -->
    <div class="bg-surface border border-border rounded-xl p-8 mb-6">
      <div class="flex items-center justify-between mb-6">
        <div>
          <h1 class="text-xl font-semibold text-foreground mb-1">VPN 状态</h1>
          <p class="text-sm text-muted">
            {{ vpn.status?.server || '未配置' }}
          </p>
        </div>
        <StatusBadge
          :status="vpn.status?.connected ? 'connected' : 'disconnected'"
        />
      </div>

      <div class="flex items-center gap-4">
        <button
          v-if="!vpn.status?.connected"
          :disabled="vpn.loading"
          class="flex items-center gap-2 bg-accent text-white rounded-lg px-6 py-3 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
          @click="vpn.connect()"
        >
          <PlugZap class="w-4 h-4" />
          {{ vpn.loading ? '连接中...' : '连接' }}
        </button>
        <button
          v-else
          :disabled="vpn.loading"
          class="flex items-center gap-2 bg-destructive text-white rounded-lg px-6 py-3 text-sm font-medium hover:bg-destructive/90 disabled:opacity-50 transition-colors"
          @click="vpn.disconnect()"
        >
          <Plug class="w-4 h-4" />
          {{ vpn.loading ? '断开中...' : '断开连接' }}
        </button>
      </div>

      <div
        v-if="vpn.lastError"
        class="mt-4 rounded-lg border border-red-500/20 bg-red-500/10 px-4 py-3 text-sm text-red-300"
      >
        {{ vpn.lastError }}
      </div>

      <div
        v-if="vpn.status?.upstream_virtual_detected"
        class="mt-4 flex items-start gap-3 rounded-lg border border-warning/30 bg-warning/10 px-4 py-3 text-sm text-warning"
      >
        <Route class="mt-0.5 h-4 w-4 shrink-0" />
        <div class="min-w-0">
          <p class="leading-5">
            {{ vpn.status.upstream_virtual_message || '发现其他虚拟网卡，正在把 EXV 串联到它们前面提前路由校园流量。' }}
          </p>
          <p v-if="upstreamVirtualNames" class="mt-1 text-xs opacity-80 break-words">
            {{ upstreamVirtualNames }}
          </p>
        </div>
      </div>
    </div>

    <!-- Info cards -->
    <div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
      <div class="bg-surface border border-border rounded-xl p-4">
        <div class="flex items-center gap-2 text-muted text-xs mb-2">
          <Clock class="w-3.5 h-3.5" />
          运行时长
        </div>
        <p class="text-lg font-mono text-foreground">
          {{ vpn.status?.connected ? uptimeFormatted : '--' }}
        </p>
      </div>

      <div class="bg-surface border border-border rounded-xl p-4">
        <div class="flex items-center gap-2 text-muted text-xs mb-2">
          <ArrowDownToLine class="w-3.5 h-3.5" />
          已接收
        </div>
        <p class="text-lg font-mono text-foreground">
          {{ vpn.status ? bytesFormatted(vpn.status.rx_bytes) : '--' }}
        </p>
      </div>

      <div class="bg-surface border border-border rounded-xl p-4">
        <div class="flex items-center gap-2 text-muted text-xs mb-2">
          <ArrowUpToLine class="w-3.5 h-3.5" />
          已发送
        </div>
        <p class="text-lg font-mono text-foreground">
          {{ vpn.status ? bytesFormatted(vpn.status.tx_bytes) : '--' }}
        </p>
      </div>

      <div class="bg-surface border border-border rounded-xl p-4">
        <div class="flex items-center gap-2 text-muted text-xs mb-2">
          <Wifi class="w-3.5 h-3.5" />
          MTU
        </div>
        <p class="text-lg font-mono text-foreground">
          {{ vpn.status?.mtu || '--' }}
        </p>
      </div>
    </div>

    <!-- Status details -->
    <div v-if="vpn.status" class="bg-surface border border-border rounded-xl p-6">
      <h2 class="text-sm font-medium text-foreground mb-4">连接详情</h2>
      <div class="grid grid-cols-2 gap-3 text-sm">
        <div>
          <span class="text-muted">服务器：</span>
          <span class="text-foreground ml-2">{{ vpn.status.server }}</span>
        </div>
        <div>
          <span class="text-muted">用户名：</span>
          <span class="text-foreground ml-2">{{ vpn.status.username }}</span>
        </div>
        <div v-if="vpn.status.internal_ip">
          <span class="text-muted">内网 IP：</span>
          <span class="text-foreground ml-2">{{ vpn.status.internal_ip }}</span>
        </div>
        <div v-if="vpn.status.interface">
          <span class="text-muted">接口：</span>
          <span class="text-foreground ml-2">{{ vpn.status.interface }}</span>
        </div>
        <div v-if="vpn.status.pid > 0">
          <span class="text-muted">PID：</span>
          <span class="text-foreground ml-2">{{ vpn.status.pid }}</span>
        </div>
        <div>
          <span class="text-muted">网络就绪：</span>
          <span class="text-foreground ml-2">{{ vpn.status.network_ready ? '是' : '否' }}</span>
        </div>
      </div>
    </div>

    <div v-if="!vpn.status" class="flex items-center justify-center py-16">
      <div class="text-center">
        <WifiOff class="w-12 h-12 text-muted mx-auto mb-4" />
        <p class="text-muted text-sm">无法获取 VPN 状态</p>
        <p class="text-muted text-xs mt-1">请确保 VPN 服务正在运行</p>
      </div>
    </div>
  </div>
</template>
