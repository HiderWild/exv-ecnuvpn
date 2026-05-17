<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useRouter } from 'vue-router'
import { useVpnStore } from '../stores/vpn'
import { useSSE } from '../composables/useSSE'
import StatusBadge from '../components/StatusBadge.vue'
import {
  Plug, PlugZap, ArrowDownToLine, ArrowUpToLine, Clock, Wifi, WifiOff, Route,
  Shield, AlertTriangle, XCircle, AlertOctagon, Settings, RefreshCw, FileText
} from 'lucide-vue-next'

const vpn = useVpnStore()
const router = useRouter()
const { connect: sseConnect, disconnect: sseDisconnect } = useSSE()

const elapsed = ref(0)
let timer: ReturnType<typeof setInterval> | null = null

onMounted(() => {
  vpn.fetchAppShellState()
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

// ── Dashboard state machine helpers ──────────────────────────────────

const dashboardState = computed(() => vpn.dashboardState)

const isTransientConnected = computed(() => {
  return dashboardState.value === 'direct connected' || dashboardState.value === 'elevated connected'
})

const sessionModeLabel = computed(() => {
  const mode = vpn.currentSessionMode
  switch (mode) {
    case 'helper': return '通过服务'
    case 'elevated': return '临时提权'
    case 'direct': return '直接连接'
    default: return ''
  }
})

// Error-specific display info
const errorDisplayInfo = computed(() => {
  if (!vpn.lastErrorType) return null
  switch (vpn.lastErrorType) {
    case 'elevation_denied':
      return {
        icon: AlertOctagon,
        title: '用户拒绝了授权请求',
        description: vpn.lastError || '提权授权被用户取消，VPN 无法建立连接。',
        color: 'warning' as const,
      }
    case 'runtime_missing':
      return {
        icon: XCircle,
        title: '缺少 OpenConnect 运行时',
        description: '请重新安装桌面客户端以修复运行时组件。',
        color: 'destructive' as const,
      }
    case 'config_invalid':
      return {
        icon: AlertTriangle,
        title: '配置不完整',
        description: vpn.lastError || '请检查连接设置是否完整。',
        color: 'warning' as const,
      }
    case 'native_failure':
      return {
        icon: AlertOctagon,
        title: '操作失败',
        description: vpn.lastError || '原生操作执行失败。',
        color: 'destructive' as const,
      }
    case 'parse_failure':
      return {
        icon: AlertOctagon,
        title: '解析失败',
        description: vpn.lastError || '无法解析服务端响应。',
        color: 'destructive' as const,
      }
    default:
      return {
        icon: AlertOctagon,
        title: '发生错误',
        description: vpn.lastError || '未知错误。',
        color: 'destructive' as const,
      }
  }
})

// Primary action handler — wraps the store action to support router navigation
function handlePrimaryAction() {
  const action = vpn.dashboardPrimaryAction
  if (!action) return

  // Service-missing states navigate to /service
  const state = dashboardState.value
  if (state === 'service-missing disconnected') {
    router.push('/service')
    return
  }

  // Error recoverable: some actions need router navigation
  if (state === 'error recoverable' && vpn.lastErrorType) {
    if (vpn.lastErrorType === 'elevation_denied') {
      router.push('/service')
      return
    }
    if (vpn.lastErrorType === 'config_invalid') {
      router.push('/settings')
      return
    }
  }

  action.action()
}

// Status badge state derived from dashboard state
const badgeStatus = computed<'connected' | 'disconnected' | 'connecting' | 'error'>(() => {
  const state = dashboardState.value
  if (state === 'helper connected' || state === 'direct connected' || state === 'elevated connected') return 'connected'
  if (state === 'elevated connecting') return 'connecting'
  if (state === 'error recoverable' || state === 'error blocking') return 'error'
  return 'disconnected'
})
</script>

<template>
  <div>
    <!-- Hero card -->
    <div class="bg-surface border border-border rounded-xl p-8 mb-6">
      <div class="flex items-center justify-between mb-6">
        <div>
          <h1 class="text-xl font-semibold text-foreground mb-1">VPN 状态</h1>
          <p class="text-sm text-muted">
            {{ vpn.status?.server || '未配置' }}
          </p>
        </div>
        <StatusBadge :status="badgeStatus" />
      </div>

      <!-- ── State: elevated connecting ──────────────────────────────── -->
      <div v-if="dashboardState === 'elevated connecting'" class="flex items-center gap-4">
        <div class="flex items-center gap-2 text-warning">
          <svg class="animate-spin h-5 w-5" viewBox="0 0 24 24" fill="none">
            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z" />
          </svg>
          <span class="text-sm font-medium">等待授权...</span>
        </div>
        <p class="text-xs text-muted">请在系统弹窗中确认提权请求</p>
      </div>

      <!-- ── State: error recoverable ────────────────────────────────── -->
      <div v-else-if="dashboardState === 'error recoverable'" class="space-y-4">
        <div
          class="flex items-start gap-3 rounded-lg border px-4 py-3 text-sm"
          :class="errorDisplayInfo?.color === 'warning'
            ? 'border-warning/30 bg-warning/10 text-warning'
            : 'border-red-500/20 bg-red-500/10 text-red-300'"
        >
          <component :is="errorDisplayInfo?.icon" class="mt-0.5 h-4 w-4 shrink-0" />
          <div class="min-w-0">
            <p class="font-medium leading-5">{{ errorDisplayInfo?.title }}</p>
            <p class="mt-1 opacity-80 leading-5">{{ errorDisplayInfo?.description }}</p>
            <p v-if="vpn.lastRecommendedAction" class="mt-1 opacity-80 leading-5">{{ vpn.lastRecommendedAction }}</p>
            <router-link
              :to="{ path: '/logs', query: { from: 'dashboard' } }"
              class="inline-flex items-center gap-1 mt-1.5 text-xs opacity-70 hover:opacity-100 underline underline-offset-2"
            >
              <FileText class="w-3 h-3" />
              查看日志 →
            </router-link>
          </div>
        </div>
        <div class="flex items-center gap-3">
          <button
            class="flex items-center gap-2 bg-accent text-white rounded-lg px-6 py-3 text-sm font-medium hover:bg-accent/90 transition-colors"
            @click="handlePrimaryAction"
          >
            <RefreshCw v-if="vpn.lastErrorType === 'native_failure' || vpn.lastErrorType === 'parse_failure'" class="w-4 h-4" />
            <Shield v-else-if="vpn.lastErrorType === 'elevation_denied'" class="w-4 h-4" />
            <Settings v-else-if="vpn.lastErrorType === 'config_invalid'" class="w-4 h-4" />
            {{ vpn.dashboardPrimaryAction?.label }}
          </button>
          <button
            class="flex items-center gap-2 border border-border text-muted rounded-lg px-5 py-2.5 text-sm hover:text-foreground hover:border-accent/50 transition-colors"
            @click="vpn.clearError()"
          >
            {{ vpn.dashboardSecondaryAction?.label }}
          </button>
        </div>
      </div>

      <!-- ── State: error blocking ───────────────────────────────────── -->
      <div v-else-if="dashboardState === 'error blocking'" class="space-y-4">
        <div class="flex items-start gap-3 rounded-lg border border-red-500/20 bg-red-500/10 px-4 py-3 text-sm text-red-300">
          <component :is="errorDisplayInfo?.icon" class="mt-0.5 h-4 w-4 shrink-0" />
          <div class="min-w-0">
            <p class="font-medium leading-5">{{ errorDisplayInfo?.title }}</p>
            <p class="mt-1 opacity-80 leading-5">{{ errorDisplayInfo?.description }}</p>
            <p v-if="vpn.lastRecommendedAction" class="mt-1 opacity-80 leading-5">{{ vpn.lastRecommendedAction }}</p>
            <router-link
              :to="{ path: '/logs', query: { from: 'dashboard' } }"
              class="inline-flex items-center gap-1 mt-1.5 text-xs opacity-70 hover:opacity-100 underline underline-offset-2"
            >
              <FileText class="w-3 h-3" />
              查看日志 →
            </router-link>
          </div>
        </div>
        <button
          class="flex items-center gap-2 border border-border text-muted rounded-lg px-5 py-2.5 text-sm hover:text-foreground hover:border-accent/50 transition-colors"
          @click="vpn.clearError()"
        >
          {{ vpn.dashboardPrimaryAction?.label }}
        </button>
      </div>

      <!-- ── State: service-ready disconnected ───────────────────────── -->
      <div v-else-if="dashboardState === 'service-ready disconnected'" class="flex items-center gap-4">
        <button
          :disabled="vpn.loading"
          class="flex items-center gap-2 bg-accent text-white rounded-lg px-6 py-3 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
          @click="vpn.connect()"
        >
          <PlugZap class="w-4 h-4" />
          {{ vpn.loading ? '连接中...' : '连接' }}
        </button>
      </div>

      <!-- ── State: service-missing disconnected ─────────────────────── -->
      <div v-else-if="dashboardState === 'service-missing disconnected'" class="space-y-3">
        <button
          :disabled="vpn.loading"
          class="flex items-center gap-2 bg-accent text-white rounded-lg px-6 py-3 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
          @click="$router.push('/service')"
        >
          <Shield class="w-4 h-4" />
          安装服务（推荐）
        </button>
        <p class="text-xs text-muted">
          安装后 VPN 可开机自启、无需每次授权
        </p>
        <button
          v-if="vpn.canUseElevatedFallback"
          :disabled="vpn.loading"
          class="flex items-center gap-2 border border-border text-muted rounded-lg px-5 py-2.5 text-sm hover:text-foreground hover:border-accent/50 disabled:opacity-50 transition-colors"
          @click="vpn.connectElevated()"
        >
          <PlugZap class="w-4 h-4" />
          仅本次连接
        </button>
      </div>

      <!-- ── State: helper connected ─────────────────────────────────── -->
      <div v-else-if="dashboardState === 'helper connected'" class="flex items-center gap-4">
        <button
          :disabled="vpn.loading"
          class="flex items-center gap-2 bg-destructive text-white rounded-lg px-6 py-3 text-sm font-medium hover:bg-destructive/90 disabled:opacity-50 transition-colors"
          @click="vpn.disconnect()"
        >
          <Plug class="w-4 h-4" />
          {{ vpn.loading ? '断开中...' : '断开连接' }}
        </button>
      </div>

      <!-- ── State: direct connected / elevated connected ────────────── -->
      <div v-else-if="isTransientConnected" class="space-y-3">
        <button
          :disabled="vpn.loading"
          class="flex items-center gap-2 bg-destructive text-white rounded-lg px-6 py-3 text-sm font-medium hover:bg-destructive/90 disabled:opacity-50 transition-colors"
          @click="vpn.disconnectElevated()"
        >
          <Plug class="w-4 h-4" />
          {{ vpn.loading ? '断开中...' : '断开连接' }}
        </button>
        <button
          class="flex items-center gap-2 border border-border text-muted rounded-lg px-5 py-2.5 text-sm hover:text-foreground hover:border-accent/50 transition-colors"
          @click="$router.push('/service')"
        >
          <Shield class="w-4 h-4" />
          安装服务
        </button>
      </div>

      <!-- ── Transient mode warning banner ───────────────────────────── -->
      <div
        v-if="isTransientConnected"
        class="mt-4 flex items-start gap-3 rounded-lg border border-warning/30 bg-warning/10 px-4 py-3 text-sm text-warning"
      >
        <AlertTriangle class="mt-0.5 h-4 w-4 shrink-0" />
        <div class="min-w-0">
          <p class="leading-5">
            当前为临时连接模式，重启后将失效。建议安装服务以获得持久连接。
          </p>
        </div>
      </div>

      <!-- ── Upstream virtual adapter warning ─────────────────────────── -->
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
        <div v-if="vpn.status.connected">
          <span class="text-muted">连接模式：</span>
          <span class="text-foreground ml-2">{{ sessionModeLabel }}</span>
        </div>
      </div>
    </div>

    <!-- Empty state — context-appropriate message -->
    <div v-if="!vpn.status && !vpn.loading && !vpn.lastError" class="flex items-center justify-center py-16">
      <div class="text-center">
        <WifiOff class="w-12 h-12 text-muted mx-auto mb-4" />
        <p class="text-muted text-sm">
          {{ vpn.isDesktop ? '无法连接到本地服务' : '无法获取 VPN 状态' }}
        </p>
        <p class="text-muted text-xs mt-1">
          {{ vpn.isDesktop ? '请确保应用已正确启动' : '请确保 VPN 服务正在运行' }}
        </p>
      </div>
    </div>
  </div>
</template>
