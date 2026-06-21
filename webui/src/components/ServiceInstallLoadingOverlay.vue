<script setup lang="ts">
import LoadingSpinner from './LoadingSpinner.vue'
import ModalShell from './ModalShell.vue'

const props = withDefaults(defineProps<{
  message: string
  compact?: boolean
}>(), {
  compact: false,
})
</script>

<template>
  <ModalShell
    open
    :title="message"
    :description="props.compact ? '' : '请稍候，正在完成辅助服务操作。'"
    :close-on-scrim="false"
    :compact="props.compact"
    size="sm"
  >
    <p v-if="props.compact" class="modal-compact-message">
      {{ message }}，请稍候。
    </p>
    <div
      v-else
      class="flex flex-col items-center gap-2 py-1"
      role="alert"
      aria-busy="true"
      aria-live="assertive"
    >
      <LoadingSpinner size="lg" />
      <p class="text-sm font-medium leading-5">{{ message }}</p>
    </div>
  </ModalShell>
</template>
