<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import type { SectionKey, SettingsSectionDescriptor } from './types'

const props = defineProps<{
  sections: SettingsSectionDescriptor[]
  activeSection: SectionKey
}>()

const emit = defineEmits<{
  select: [key: SectionKey]
}>()

const expandedSection = ref<SectionKey | null>(null)
const activationToken = ref(0)
let collapseTimer: ReturnType<typeof setTimeout> | null = null

function clearCollapseTimer() {
  if (!collapseTimer) return
  clearTimeout(collapseTimer)
  collapseTimer = null
}

function isActive(key: SectionKey) {
  return props.activeSection === key
}

function isExpanded(key: SectionKey) {
  return expandedSection.value === key
}

watch(
  () => props.activeSection,
  (section) => {
    expandedSection.value = section
    activationToken.value += 1
    const token = activationToken.value
    clearCollapseTimer()
    collapseTimer = setTimeout(() => {
      if (activationToken.value === token) {
        expandedSection.value = null
      }
    }, 2000)
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  clearCollapseTimer()
})
</script>

<template>
  <nav
    class="settings-axis pointer-events-none absolute bottom-4 right-2 top-4 z-30 flex w-32 justify-end"
    aria-label="设置分区导航"
  >
    <div class="relative flex h-full w-full items-center justify-end">
      <div class="absolute right-4 top-4 bottom-4 w-px bg-border" aria-hidden="true" />
      <div class="relative z-10 flex h-full flex-col justify-between py-2">
        <button
          v-for="section in sections"
          :key="section.key"
          type="button"
          :title="section.label"
          :aria-label="section.label"
          :aria-current="isActive(section.key) ? 'true' : undefined"
          :class="[
            'pointer-events-auto ml-auto flex h-8 items-center justify-center overflow-hidden border text-xs font-medium shadow-sm transition-all duration-300 cursor-pointer',
            isExpanded(section.key)
              ? 'w-auto rounded-full border-accent bg-accent px-2 text-white shadow-accent/20'
              : isActive(section.key)
                ? 'w-8 rounded-full border-accent bg-accent px-0 text-white shadow-accent/20'
                : 'w-8 rounded-full border-border bg-surface px-0 text-muted hover:border-accent/50 hover:text-foreground',
          ]"
          @click="emit('select', section.key)"
        >
          <component
            :is="section.icon"
            :class="[
              'h-3.5 w-3.5 shrink-0 transition-colors',
              isActive(section.key) ? 'text-white' : 'text-muted',
            ]"
          />
          <span
            :class="[
              'whitespace-nowrap transition-all duration-300',
              isExpanded(section.key) ? 'ml-1 max-w-none opacity-100' : 'ml-0 max-w-0 opacity-0',
            ]"
          >
            {{ section.label }}
          </span>
        </button>
      </div>
    </div>
  </nav>
</template>

<style scoped>
@media (prefers-reduced-motion: reduce) {
  .settings-axis * {
    transition-duration: 1ms !important;
  }
}
</style>
