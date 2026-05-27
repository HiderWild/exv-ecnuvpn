<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import {
  LayoutDashboard, Key, Route, Settings, FileText
} from 'lucide-vue-next'
import { useVpnStore } from '../stores/vpn'

const router = useRouter()
const route = useRoute()
const vpn = useVpnStore()

const navItems = [
  { path: '/', name: '主面板', icon: LayoutDashboard },
  { path: '/auth', name: '认证', icon: Key },
  { path: '/routes', name: '路由', icon: Route },
  { path: '/logs', name: '日志', icon: FileText },
  { path: '/settings', name: '设置', icon: Settings },
]

function isActive(path: string) {
  if (path === '/') return route.path === '/'
  return route.path.startsWith(path)
}

onMounted(() => {
  if (vpn.isDesktop) void vpn.fetchStatus()
})

const uptimeFormatted = computed(() => {
  const total = vpn.displayUptimeSeconds
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
})

const proxyTunLabel = computed(() => {
  const adapters = vpn.status?.upstream_virtual_adapters || []
  if (adapters.length > 0) {
    return adapters.map((adapter) => adapter.name).filter(Boolean).join('、') || '已检测到'
  }
  return vpn.status?.upstream_virtual_message || '--'
})

const connectionState = computed(() => {
  if (vpn.disconnectInFlight) return { label: '正在断开', tone: 'warning' }
  if (vpn.connectInFlight || vpn.serviceBusy) return { label: '连接中', tone: 'warning' }
  if (vpn.lastError) return { label: '需要处理', tone: 'warning' }
  if (!vpn.status?.connected) return { label: '未连接', tone: 'muted' }

  switch (vpn.currentSessionMode) {
    case 'helper': return { label: '通过服务', tone: 'accent' }
    case 'elevated': return { label: '临时提权', tone: 'accent' }
    case 'direct': return { label: '直接连接', tone: 'accent' }
    default: return { label: '已连接', tone: 'accent' }
  }
})

const sidebarStatusItems = computed(() => [
  { label: '用户', value: vpn.status?.username || '--' },
  { label: '运行时长', value: vpn.status?.connected ? uptimeFormatted.value : '--' },
  { label: '代理 TUN', value: proxyTunLabel.value },
  { label: '内网地址', value: vpn.status?.internal_ip || '--' },
  { label: 'VPN 服务器', value: vpn.status?.server || '--' },
])
</script>

<template>
  <nav class="sticky top-0 z-40 border-b border-border bg-surface/80 backdrop-blur-sm lg:fixed lg:inset-y-0 lg:left-0 lg:flex lg:w-44 lg:flex-col lg:border-b-0 lg:border-r">
    <div class="flex items-center justify-between gap-3 px-4 py-4 lg:px-3 lg:py-5">
      <div class="min-w-0">
        <button
          class="flex min-w-0 items-center gap-2.5 text-left transition-colors hover:text-accent"
          @click="router.push('/')"
        >
          <img src="/favicon.svg" alt="" class="h-9 w-9 shrink-0" />
          <span class="min-w-0 leading-tight">
            <span class="block text-xl font-bold text-foreground">EXV</span>
            <span class="block text-sm font-semibold text-muted">for ECNU</span>
          </span>
        </button>
      </div>
    </div>

    <div class="px-3 pb-3 lg:min-h-0 lg:flex-1 lg:px-3 lg:pb-5">
      <div class="flex items-center gap-1 overflow-x-auto lg:flex-col lg:items-stretch lg:gap-1.5 lg:overflow-y-auto">
        <button
          v-for="item in navItems"
          :key="item.path"
          :class="[
            'flex items-center gap-2 rounded-lg px-3 py-2 text-sm whitespace-nowrap transition-colors lg:w-full',
            isActive(item.path)
              ? 'bg-primary/50 text-foreground shadow-[inset_0_0_0_1px_rgba(255,255,255,0.06)]'
              : 'text-muted hover:bg-bg/70 hover:text-foreground'
          ]"
          @click="router.push(item.path)"
        >
          <component :is="item.icon" class="h-4 w-4 shrink-0" />
          <span class="flex-1 text-left">{{ item.name }}</span>
        </button>
      </div>
    </div>

    <div class="hidden px-3 pb-4 lg:block">
      <div class="space-y-3 border-t border-border pt-3">
        <div class="space-y-2">
          <div
            v-for="item in sidebarStatusItems"
            :key="item.label"
            class="min-w-0"
          >
            <p class="text-[0.66rem] leading-4 text-muted">{{ item.label }}</p>
            <p class="truncate text-xs font-semibold leading-4 text-foreground" :title="item.value">
              {{ item.value }}
            </p>
          </div>
        </div>

        <div class="flex items-center gap-2 rounded-full border border-border bg-bg/40 px-2.5 py-1.5 text-xs">
          <span
            :class="[
              'h-2 w-2 rounded-full',
              connectionState.tone === 'accent' ? 'bg-accent' : connectionState.tone === 'warning' ? 'bg-warning' : 'bg-muted',
            ]"
          />
          <span class="truncate text-muted">{{ connectionState.label }}</span>
        </div>
      </div>
    </div>
  </nav>
</template>
