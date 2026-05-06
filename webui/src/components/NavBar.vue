<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router'
import {
  LayoutDashboard, Key, Route, Settings, FileText, Terminal
} from 'lucide-vue-next'

const router = useRouter()
const route = useRoute()

const navItems = [
  { path: '/', name: 'Dashboard', icon: LayoutDashboard },
  { path: '/auth', name: 'Auth', icon: Key },
  { path: '/routes', name: 'Routes', icon: Route },
  { path: '/service', name: 'Service', icon: Terminal },
  { path: '/logs', name: 'Logs', icon: FileText },
  { path: '/settings', name: 'Settings', icon: Settings },
]

function isActive(path: string) {
  if (path === '/') return route.path === '/'
  return route.path.startsWith(path)
}
</script>

<template>
  <nav class="border-b border-border bg-surface/50 backdrop-blur-sm sticky top-0 z-40">
    <div class="max-w-5xl mx-auto px-4 flex items-center h-14 gap-1">
      <span class="text-foreground font-semibold text-sm mr-4 shrink-0">ECNU VPN</span>
      <div class="flex items-center gap-1 overflow-x-auto">
        <button
          v-for="item in navItems"
          :key="item.path"
          :class="[
            'flex items-center gap-1.5 px-3 py-1.5 rounded-md text-sm whitespace-nowrap transition-colors',
            isActive(item.path)
              ? 'bg-primary/40 text-foreground'
              : 'text-muted hover:text-foreground hover:bg-surface'
          ]"
          @click="router.push(item.path)"
        >
          <component :is="item.icon" class="w-4 h-4" />
          {{ item.name }}
        </button>
      </div>
      <div class="flex-1" />
    </div>
  </nav>
</template>
