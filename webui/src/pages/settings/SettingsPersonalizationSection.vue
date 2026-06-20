<script setup lang="ts">
import { computed, onMounted, type Component } from 'vue'
import { Monitor, Moon, Palette, Sun } from 'lucide-vue-next'
import {
  themeAccentPalettes,
  useThemeStore,
  type ThemeAccentTheme,
  type ThemeMode,
} from '../../stores/theme'

const theme = useThemeStore()

const themeOptions: Array<{
  value: ThemeMode
  label: string
  description: string
  icon: Component
}> = [
  {
    value: 'light',
    label: '浅色主题',
    description: '白色背景与清晰强调色。',
    icon: Sun,
  },
  {
    value: 'dark',
    label: '深色主题',
    description: '深色背景与夜间强调色。',
    icon: Moon,
  },
  {
    value: 'system',
    label: '跟随系统',
    description: '根据系统外观自动切换。',
    icon: Monitor,
  },
]

const activeAccentTheme = computed<ThemeAccentTheme>(() => {
  if (theme.mode === 'system') return theme.systemDark ? 'dark' : 'light'
  return theme.mode
})
const activeAccentOptions = computed(() => themeAccentPalettes[activeAccentTheme.value])
const activeAccentDescription = computed(() =>
  activeAccentTheme.value === 'light'
    ? '当前应用处于浅色主题，仅显示浅色候选。'
    : '当前应用处于深色主题，仅显示深色候选。',
)

function accentSelected(accentKey: string) {
  return activeAccentTheme.value === 'light'
    ? theme.lightAccent === accentKey
    : theme.darkAccent === accentKey
}

function selectThemeMode(mode: ThemeMode) {
  theme.setThemeMode(mode)
}

function selectAccent(accentKey: string) {
  theme.setAccent(activeAccentTheme.value, accentKey)
}

onMounted(() => {
  theme.initialize()
})
</script>

<template>
  <section class="rounded-xl border border-border bg-surface p-5">
    <h2 class="mb-4 flex items-center gap-2 text-base font-semibold text-foreground">
      <Palette class="h-5 w-5 text-accent" />
      个性化
    </h2>

    <div class="space-y-4">
      <div>
        <p class="text-sm font-medium text-foreground">主题</p>
        <p class="mt-1 text-xs text-muted">此设置仅保存在本机前端，不写入 VPN 配置。</p>
      </div>

      <div class="grid gap-3 md:grid-cols-3">
        <button
          v-for="option in themeOptions"
          :key="option.value"
          type="button"
          :class="[
            'min-h-[96px] rounded-lg border px-4 py-3 text-left transition-colors',
            theme.mode === option.value
              ? 'border-accent bg-accent/10 text-foreground'
              : 'border-border bg-bg/40 text-muted hover:border-accent/50 hover:text-foreground',
          ]"
          @click="selectThemeMode(option.value)"
        >
          <component
            :is="option.icon"
            :class="[
              'mb-3 h-5 w-5',
              theme.mode === option.value ? 'text-accent' : 'text-muted',
            ]"
          />
          <span class="block text-sm font-medium">{{ option.label }}</span>
          <span class="mt-1 block text-xs leading-5 text-muted">{{ option.description }}</span>
        </button>
      </div>

      <div class="border-t border-border pt-4">
        <div class="mb-3">
          <p class="text-sm font-medium text-foreground">强调色</p>
          <p class="mt-1 text-xs text-muted">{{ activeAccentDescription }}</p>
        </div>

        <div class="grid gap-2 md:grid-cols-3">
          <button
            v-for="option in activeAccentOptions"
            :key="option.key"
            type="button"
            :class="[
              'flex min-h-[72px] items-center gap-3 rounded-lg border px-3 py-2 text-left transition-colors',
              accentSelected(option.key)
                ? 'border-accent bg-accent/10 text-foreground'
                : 'border-border bg-bg/40 text-muted hover:border-accent/50 hover:text-foreground',
            ]"
            @click="selectAccent(option.key)"
          >
            <span
              class="h-5 w-5 shrink-0 rounded-full border border-border shadow-sm"
              :style="{ backgroundColor: option.color }"
              aria-hidden="true"
            />
            <span class="min-w-0">
              <span class="block text-sm font-medium">{{ option.label }}</span>
              <span class="mt-0.5 block text-xs leading-4 text-muted">{{ option.description }}</span>
            </span>
          </button>
        </div>
      </div>
    </div>
  </section>
</template>
