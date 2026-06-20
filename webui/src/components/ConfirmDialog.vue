<script setup lang="ts">
import { AlertTriangle } from 'lucide-vue-next'
import { useUiStore } from '../stores/ui'
import ModalShell from './ModalShell.vue'

const ui = useUiStore()
</script>

<template>
  <ModalShell
    :open="ui.showConfirm"
    title="确认操作"
    :description="ui.confirmMessage"
    size="sm"
    @close="ui.closeConfirm"
  >
    <template #icon>
      <AlertTriangle class="h-4 w-4 text-warning" />
    </template>

    <template #actions>
      <button
        type="button"
        class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80"
        @click="ui.closeConfirm"
      >
        取消
      </button>
      <button
        type="button"
        class="rounded-lg bg-destructive px-3 py-2 text-sm text-white hover:bg-destructive/90"
        @click="ui.onConfirm"
      >
        确认
      </button>
    </template>
  </ModalShell>
</template>
