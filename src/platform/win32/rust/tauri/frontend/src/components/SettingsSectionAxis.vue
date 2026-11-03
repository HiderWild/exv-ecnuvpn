<script setup lang="ts">
const props = defineProps<{
  sections: readonly { id: string; label: string; icon?: string }[];
  activeSection: string;
}>();

const emit = defineEmits<{
  select: [id: string];
}>();
</script>

<template>
  <nav
    class="settings-axis"
    data-testid="settings-section-axis"
    aria-label="设置分区导航"
    :style="{ '--axis-count': props.sections.length }"
  >
    <button
      v-for="section in sections"
      :key="section.id"
      type="button"
      class="settings-axis__item"
      :class="{ 'settings-axis__item--active': section.id === activeSection }"
      :aria-current="section.id === activeSection ? 'true' : undefined"
      @click="emit('select', section.id)"
    >{{ section.label }}</button>
  </nav>
</template>

<style scoped>
.settings-axis {
  position: sticky;
  top: 0;
  z-index: 2;
  display: flex;
  flex-wrap: wrap;
  grid-column: 1;
  grid-row: 1;
  align-self: start;
  gap: 5px;
  min-width: 0;
  padding: 0;
  margin-bottom: 18px;
  background: var(--surface-canvas);
}
.settings-axis__item {
  position: relative;
  width: auto;
  min-height: auto;
  padding: 7px 12px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-caption);
  font-weight: 500;
  text-align: center;
  white-space: nowrap;
  cursor: pointer;
}
.settings-axis__item::before {
  position: absolute;
  bottom: 0;
  left: var(--space-2);
  right: var(--space-2);
  height: 2px;
  border-radius: 999px;
  background: transparent;
  content: "";
}
.settings-axis__item:not(.settings-axis__item--active):hover,
.settings-axis__item:not(.settings-axis__item--active):focus-visible { background: var(--surface-subtle); color: var(--text-primary); }
.settings-axis__item:hover,
.settings-axis__item:focus,
.settings-axis__item:active { transform: none; }
.settings-axis__item:focus-visible { outline: 2px solid var(--focus-ring); outline-offset: -2px; }
.settings-axis__item--active { background: var(--accent-subtle); color: var(--accent); font-weight: 600; }
.settings-axis__item--active::before { background: transparent; }
</style>
