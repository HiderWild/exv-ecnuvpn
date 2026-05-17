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
  <nav class="sticky top-0 z-40 border-b border-border bg-surface/80 backdrop-blur-sm lg:fixed lg:inset-y-0 lg:left-0 lg:w-72 lg:border-b-0 lg:border-r">
    <div class="flex items-center justify-between gap-3 px-4 py-4 lg:flex-col lg:items-start lg:px-5 lg:py-6">
      <div class="min-w-0">
        <p class="text-xs uppercase tracking-[0.24em] text-muted">Desktop</p>
        <button
          class="mt-1 text-left text-base font-semibold text-foreground transition-colors hover:text-accent"
          @click="router.push('/')"
        >
          ECNU VPN
        </button>
        <p class="mt-1 text-xs text-muted lg:max-w-48">
          左侧导航切换页面，右侧内容保持当前工作上下文。
        </p>
      </div>
      <div class="hidden text-[11px] text-muted lg:block">
        图形界面优先
      </div>
    </div>

    <div class="px-3 pb-3 lg:px-4 lg:pb-5">
      <div class="flex items-center gap-1 overflow-x-auto lg:flex-col lg:items-stretch lg:gap-1.5 lg:overflow-y-auto">
        <button
          v-for="item in navItems"
          :key="item.path"
          :class="[
            'flex items-center gap-2 rounded-xl px-3 py-2 text-sm whitespace-nowrap transition-colors lg:w-full',
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
