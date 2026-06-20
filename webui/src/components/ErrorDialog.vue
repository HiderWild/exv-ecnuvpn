<script setup lang="ts">
import { AlertOctagon } from 'lucide-vue-next'
import { useUiStore } from '../stores/ui'
import ModalShell from './ModalShell.vue'

const ui = useUiStore()
</script>

<template>
  <ModalShell
    :open="ui.errorModal.visible"
    :title="ui.errorModal.title"
    :description="ui.errorModal.message"
    size="sm"
    @close="ui.closeError"
  >
    <template #icon>
      <AlertOctagon class="h-4 w-4 text-destructive" />
    </template>

    <template #actions>
      <button
        type="button"
        class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80"
        @click="ui.closeError"
      >
        {{ ui.errorModal.secondaryLabel }}
      </button>
      <button
        v-if="ui.errorModal.onPrimary"
        type="button"
        class="rounded-lg bg-destructive px-3 py-2 text-sm text-white hover:bg-destructive/90"
        @click="ui.onErrorPrimary"
      >
        {{ ui.errorModal.primaryLabel }}
      </button>
    </template>
  </ModalShell>
</template>
