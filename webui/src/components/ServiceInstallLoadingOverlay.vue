<script setup lang="ts">
import LoadingSpinner from './LoadingSpinner.vue'

defineProps<{
  message: string
}>()
</script>

<template>
  <div
    class="fixed inset-0 z-[70] grid place-items-center bg-black/65 text-foreground backdrop-blur-sm"
    role="alert"
    aria-busy="true"
    aria-live="assertive"
  >
    <div class="flex flex-col items-center gap-2">
      <LoadingSpinner size="lg" />
      <p class="text-sm font-medium leading-5">{{ message }}</p>
    </div>
  </div>
</template>
