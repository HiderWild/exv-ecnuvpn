<script setup lang="ts">
import LoadingSpinner from './LoadingSpinner.vue'
import ModalShell from './ModalShell.vue'

defineProps<{
  message: string
}>()
</script>

<template>
  <ModalShell
    open
    :title="message"
    description="请稍候，正在完成辅助服务操作。"
    :close-on-scrim="false"
    size="sm"
  >
    <div
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
