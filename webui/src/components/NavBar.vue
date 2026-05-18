<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router'
import {
  LayoutDashboard, Key, Route, Settings, FileText, Terminal
} from 'lucide-vue-next'

const router = useRouter()
const route = useRoute()

const navItems = [
  { path: '/', name: '仪表盘', icon: LayoutDashboard },
  { path: '/auth', name: '认证', icon: Key },
  { path: '/routes', name: '路由', icon: Route },
  { path: '/service', name: '服务', icon: Terminal },
  { path: '/logs', name: '日志', icon: FileText },
  { path: '/settings', name: '设置', icon: Settings },
]

function isActive(path: string) {
  if (path === '/') return route.path === '/'
  return route.path.startsWith(path)
}
</script>

<template>
  <aside class="w-52 shrink-0 border-r border-border bg-surface/50 backdrop-blur-sm flex flex-col h-screen sticky top-0">
    <div class="px-4 h-14 flex items-center border-b border-border">
      <span class="text-foreground font-semibold text-sm">ECNU VPN</span>
    </div>
    <nav class="flex-1 px-2 py-2 space-y-0.5 overflow-y-auto">
      <button
        v-for="item in navItems"
        :key="item.path"
        :class="[
          'flex items-center gap-2 w-full px-3 py-2 rounded-md text-sm transition-colors text-left',
          isActive(item.path)
            ? 'bg-primary/40 text-foreground'
            : 'text-muted hover:text-foreground hover:bg-surface'
        ]"
        @click="router.push(item.path)"
      >
        <component :is="item.icon" class="w-4 h-4 shrink-0" />
        <span class="truncate">{{ item.name }}</span>
      </button>
    </nav>
  </aside>
</template>