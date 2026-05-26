<script setup lang="ts">
import { useRouter, useRoute } from 'vue-router'
import {
  LayoutDashboard, Key, Route, Settings, FileText
} from 'lucide-vue-next'

const router = useRouter()
const route = useRoute()

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
</script>

<template>
  <nav class="sticky top-0 z-40 border-b border-border bg-surface/80 backdrop-blur-sm lg:fixed lg:inset-y-0 lg:left-0 lg:w-36 lg:border-b-0 lg:border-r">
    <div class="flex items-center justify-between gap-3 px-4 py-4 lg:px-3 lg:py-5">
      <div class="min-w-0">
        <button
          class="text-left text-xl font-bold leading-tight text-foreground transition-colors hover:text-accent"
          @click="router.push('/')"
        >
          EXV for ECNU
        </button>
      </div>
    </div>

    <div class="px-3 pb-3 lg:px-3 lg:pb-5">
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
  </nav>
</template>
