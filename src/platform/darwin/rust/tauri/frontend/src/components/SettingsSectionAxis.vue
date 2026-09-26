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
  display: grid;
  grid-template-columns: repeat(var(--axis-count), minmax(0, 1fr));
  grid-column: 1;
  grid-row: 1;
  align-self: start;
  gap: 2px;
  min-width: 0;
  padding: var(--space-2) 0;
  border-bottom: 1px solid var(--border-subtle);
  background: var(--surface-canvas);
}
.settings-axis__item {
  position: relative;
  width: 100%;
  min-height: 34px;
  padding: 6px var(--space-3);
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-size: 13px;
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
.settings-axis__item:hover, .settings-axis__item:focus-visible { background: var(--surface-subtle); color: var(--text-primary); }
.settings-axis__item:focus-visible { outline: 2px solid var(--focus-ring); outline-offset: -2px; }
.settings-axis__item--active { background: var(--accent-subtle); color: var(--text-primary); font-weight: 650; }
.settings-axis__item--active::before { background: var(--accent); }
</style>
