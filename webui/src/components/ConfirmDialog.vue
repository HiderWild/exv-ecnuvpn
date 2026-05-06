<script setup lang="ts">
import { useUiStore } from '../stores/ui'
import { AlertTriangle } from 'lucide-vue-next'

const ui = useUiStore()
</script>

<template>
  <Teleport to="body">
    <div
      v-if="ui.showConfirm"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      @click.self="ui.closeConfirm"
    >
      <div class="bg-surface border border-border rounded-xl p-6 max-w-sm w-full mx-4 shadow-xl">
        <div class="flex items-start gap-3 mb-4">
          <AlertTriangle class="w-5 h-5 text-warning shrink-0 mt-0.5" />
          <p class="text-sm text-foreground">{{ ui.confirmMessage }}</p>
        </div>
        <div class="flex justify-end gap-3">
          <button
            class="px-4 py-2 text-sm rounded-lg border border-border text-muted hover:bg-surface/80 transition-colors"
            @click="ui.closeConfirm"
          >
            Cancel
          </button>
          <button
            class="px-4 py-2 text-sm rounded-lg bg-destructive text-white hover:bg-destructive/90 transition-colors"
            @click="ui.onConfirm"
          >
            Confirm
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
