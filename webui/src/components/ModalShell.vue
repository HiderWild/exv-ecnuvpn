<script setup lang="ts">
const props = withDefaults(defineProps<{
  open: boolean
  title?: string
  description?: string
  size?: 'sm' | 'md' | 'lg'
  closeOnScrim?: boolean
}>(), {
  title: '',
  description: '',
  size: 'md',
  closeOnScrim: true,
})

const emit = defineEmits<{
  close: []
}>()

function onScrimClick() {
  if (props.closeOnScrim) emit('close')
}
</script>

<template>
  <div
    v-if="open"
    class="modal-shell__scrim"
    role="presentation"
    @click.self="onScrimClick"
  >
    <section
      class="modal-shell__panel"
      :class="`modal-shell__panel--${size}`"
      role="dialog"
      aria-modal="true"
      :aria-label="title || undefined"
    >
      <header v-if="title || description || $slots.icon" class="modal-shell__header">
        <div v-if="$slots.icon" class="modal-shell__icon">
          <slot name="icon" />
        </div>
        <div class="modal-shell__heading">
          <h2 v-if="title" class="modal-shell__title">{{ title }}</h2>
          <p v-if="description" class="modal-shell__description">{{ description }}</p>
        </div>
      </header>

      <div class="modal-shell__body">
        <slot />
      </div>

      <footer v-if="$slots.actions" class="modal-shell__actions">
        <slot name="actions" />
      </footer>
    </section>
  </div>
</template>

<style>
.modal-shell__scrim {
  position: absolute;
  inset: 0;
  z-index: 120;
  display: grid;
  place-items: center;
  padding: 14px;
  background: rgba(2, 6, 23, 0.56);
  backdrop-filter: blur(10px) saturate(1.05);
}

.modal-shell__panel {
  width: min(100%, 400px);
  max-height: min(82vh, 520px);
  overflow: auto;
  border: 1px solid rgba(148, 163, 184, 0.24);
  border-radius: 8px;
  background: #111827;
  color: #f8fafc;
  box-shadow: 0 18px 40px rgba(0, 0, 0, 0.36);
  padding: 18px;
}

.modal-shell__panel--sm {
  width: min(100%, 360px);
}

.modal-shell__panel--lg {
  width: min(100%, 440px);
}

.modal-shell__header {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin-bottom: 14px;
}

.modal-shell__icon {
  display: grid;
  width: 28px;
  height: 28px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 8px;
  background: rgba(34, 197, 94, 0.12);
  color: #22c55e;
}

.modal-shell__heading {
  min-width: 0;
}

.modal-shell__title {
  margin: 0;
  font-size: 15px;
  font-weight: 650;
  line-height: 1.35;
  letter-spacing: 0;
}

.modal-shell__description {
  margin: 4px 0 0;
  color: #94a3b8;
  font-size: 12px;
  line-height: 1.45;
}

.modal-shell__body {
  min-width: 0;
}

.modal-shell__actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
</style>
