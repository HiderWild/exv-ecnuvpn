<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { Github, Tag, UserRound } from 'lucide-vue-next'
import appIconUrl from '../assets/app-icon.svg'
import { distributionConfig } from '../generated/distribution'
import { useConfigStore } from '../stores/config'

defineOptions({ name: 'AboutPage' })

const config = useConfigStore()

const versionLabel = computed(() => config.runtimeStatus?.version || 'dev')

async function openRepository() {
  const url = distributionConfig.repository.url
  try {
    const nativeOpen = window.exv?.shell?.openExternal?.(distributionConfig.repository.url)
    if (nativeOpen) {
      await nativeOpen
      return
    }
  } catch {
    // Fall back to browser behavior when the native shell rejects the request.
  }
  window.open(url, '_blank', 'noopener,noreferrer')
}

onMounted(() => {
  void config.fetchRuntimeStatus().catch(() => {})
})
</script>

<template>
  <div class="h-full overflow-hidden py-3">
    <div class="h-full overflow-y-auto">
      <header class="mb-4">
        <h1 class="text-3xl font-semibold text-foreground">关于</h1>
      </header>

      <section class="rounded-xl border border-border bg-surface p-5">
        <div class="flex items-center gap-4 border-b border-border pb-5">
          <div class="flex h-20 w-20 items-center justify-center rounded-2xl bg-accent/10">
            <img class="h-16 w-16" :src="appIconUrl" alt="" />
          </div>
          <div class="min-w-0">
            <p class="text-4xl font-semibold leading-10 text-foreground">{{ distributionConfig.appName }}</p>
            <p class="text-2xl font-medium leading-8 text-muted">{{ distributionConfig.brandSubtitle }}</p>
          </div>
        </div>

        <div class="grid gap-3 py-4 text-sm">
          <div class="flex items-center justify-between gap-4">
            <span class="flex items-center gap-2 text-muted">
              <Tag class="h-4 w-4" />
              版本
            </span>
            <span class="font-medium text-foreground">{{ versionLabel }}</span>
          </div>
          <div class="flex items-center justify-between gap-4">
            <span class="flex items-center gap-2 text-muted">
              <UserRound class="h-4 w-4" />
              作者
            </span>
            <span class="font-medium text-foreground">{{ distributionConfig.author }}</span>
          </div>
        </div>

        <a
          class="inline-flex items-center gap-2 rounded-lg border border-border px-3 py-2 text-sm font-medium text-foreground transition-colors hover:border-accent hover:text-accent"
          :href="distributionConfig.repository.url"
          rel="noreferrer"
          @click.prevent="openRepository"
        >
          <Github class="h-4 w-4" />
          {{ distributionConfig.repository.label }}
        </a>
      </section>
    </div>
  </div>
</template>
