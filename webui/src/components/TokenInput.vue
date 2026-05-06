<script setup lang="ts">
import { ref } from 'vue'
import { Eye, EyeOff } from 'lucide-vue-next'

defineProps<{
  modelValue: string
  placeholder?: string
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

const show = ref(false)

function onInput(e: Event) {
  emit('update:modelValue', (e.target as HTMLInputElement).value)
}
</script>

<template>
  <div class="relative">
    <input
      :type="show ? 'text' : 'password'"
      :value="modelValue"
      :placeholder="placeholder"
      class="w-full bg-bg border border-border rounded-lg px-3 py-2 pr-10 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
      @input="onInput"
    />
    <button
      type="button"
      class="absolute right-2.5 top-1/2 -translate-y-1/2 text-muted hover:text-foreground transition-colors"
      @click="show = !show"
    >
      <EyeOff v-if="show" class="w-4 h-4" />
      <Eye v-else class="w-4 h-4" />
    </button>
  </div>
</template>
