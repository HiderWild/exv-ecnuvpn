<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { SectionKey, SettingsSectionDescriptor } from './types'

const props = defineProps<{
  sections: SettingsSectionDescriptor[]
  activeSection: SectionKey
}>()

const emit = defineEmits<{
  select: [key: SectionKey]
}>()

const expandedSection = ref<SectionKey | null>(null)
const visualActiveSection = ref<SectionKey | null>(props.activeSection)
const activationToken = ref(0)
const axisFlight = ref<{ from: string; to: string } | null>(null)
let collapseTimer: ReturnType<typeof setTimeout> | null = null
let flightTimer: ReturnType<typeof setTimeout> | null = null

const axisFlightActive = computed(() => axisFlight.value !== null)
const axisFlightStyle = computed(() => axisFlight.value
  ? {
      '--axis-flight-from': axisFlight.value.from,
      '--axis-flight-to': axisFlight.value.to,
    }
  : {})

function clearCollapseTimer() {
  if (!collapseTimer) return
  clearTimeout(collapseTimer)
  collapseTimer = null
}

function clearFlightTimer() {
  if (!flightTimer) return
  clearTimeout(flightTimer)
  flightTimer = null
}

function nodePosition(key: SectionKey) {
  const index = props.sections.findIndex((section) => section.key === key)
  const safeIndex = index >= 0 ? index : 0
  const denominator = Math.max(props.sections.length - 1, 1)
  const ratio = safeIndex / denominator
  return `calc(24px + (100% - 48px) * ${ratio})`
}

function isActive(key: SectionKey) {
  return props.activeSection === key
}

function isHighlighted(key: SectionKey) {
  return visualActiveSection.value === key
}

function isExpanded(key: SectionKey) {
  return expandedSection.value === key
}

function expandAndScheduleCollapse(section: SectionKey, token: number) {
  if (activationToken.value !== token) return
  visualActiveSection.value = section
  expandedSection.value = section
  clearCollapseTimer()
  collapseTimer = setTimeout(() => {
    if (activationToken.value === token) {
      expandedSection.value = null
    }
  }, 2000)
}

watch(
  () => props.activeSection,
  (section, previousSection) => {
    activationToken.value += 1
    const token = activationToken.value
    clearCollapseTimer()
    clearFlightTimer()

    if (previousSection && previousSection !== section) {
      visualActiveSection.value = null
      expandedSection.value = null
      axisFlight.value = {
        from: nodePosition(previousSection),
        to: nodePosition(section),
      }
      flightTimer = setTimeout(() => {
        if (activationToken.value !== token) return
        axisFlight.value = null
        flightTimer = null
        expandAndScheduleCollapse(section, token)
      }, 200)
      return
    }

    visualActiveSection.value = section
    axisFlight.value = null
    expandAndScheduleCollapse(section, token)
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  clearCollapseTimer()
  clearFlightTimer()
})
</script>

<template>
  <nav
    class="settings-axis pointer-events-none z-30"
    aria-label="设置分区导航"
  >
    <div class="settings-axis__frame">
      <div class="settings-axis__rail" aria-hidden="true" />
      <div
        v-if="axisFlightActive"
        class="axis-flight-orb"
        :style="axisFlightStyle"
        aria-hidden="true"
      />
      <div class="settings-axis__nodes">
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
              : isHighlighted(section.key)
                ? 'w-8 rounded-full border-accent bg-accent px-0 text-white shadow-accent/20'
                : 'w-8 rounded-full border-border bg-surface px-0 text-muted hover:border-accent/50 hover:text-foreground',
          ]"
          @click="emit('select', section.key)"
        >
          <component
            :is="section.icon"
            :class="[
              'h-3.5 w-3.5 shrink-0 transition-colors',
              isHighlighted(section.key) ? 'text-white' : 'text-muted',
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
.settings-axis {
  position: fixed;
  top: 58px;
  right: 16px;
  bottom: 24px;
  width: 92px;
  display: flex;
  justify-content: flex-end;
}

.settings-axis__frame {
  position: relative;
  display: flex;
  width: 100%;
  height: 100%;
  align-items: center;
  justify-content: flex-end;
}

.settings-axis__rail {
  position: absolute;
  top: 24px;
  right: 16px;
  bottom: 24px;
  width: 1px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--color-border) 78%, transparent);
}

.settings-axis__nodes {
  position: relative;
  z-index: 2;
  display: flex;
  height: 100%;
  flex-direction: column;
  align-items: flex-end;
  justify-content: space-between;
  padding-block: 8px;
}

.axis-flight-orb {
  position: absolute;
  z-index: 3;
  top: var(--axis-flight-from);
  right: 7px;
  width: 18px;
  height: 18px;
  border-radius: 999px;
  background: var(--color-accent);
  box-shadow:
    0 0 0 4px var(--accent-soft-bg),
    0 0 24px color-mix(in srgb, var(--color-accent) 62%, transparent);
  opacity: 0;
  pointer-events: none;
  animation: axis-flight 200ms cubic-bezier(0.12, 0.82, 0.18, 1) forwards;
}

@keyframes axis-flight {
  0% {
    top: var(--axis-flight-from);
    opacity: 0.72;
    transform: translateY(-50%) scale(0.58);
  }
  62% {
    opacity: 1;
    transform: translateY(-50%) scale(1);
  }
  100% {
    top: var(--axis-flight-to);
    opacity: 0;
    transform: translateY(-50%) scale(1.65);
  }
}

@media (prefers-reduced-motion: reduce) {
  .settings-axis * {
    transition-duration: 1ms !important;
    animation-duration: 1ms !important;
  }
}
</style>
