<script setup lang="ts">
import { AlertOctagon, X } from 'lucide-vue-next'
import { useUiStore } from '../stores/ui'

const ui = useUiStore()
</script>

<template>
  <Teleport to="body">
    <div
      v-if="ui.errorModal.visible"
      class="fixed inset-0 z-[60] flex items-center justify-center bg-black/65 px-4 backdrop-blur-sm"
      role="alertdialog"
      aria-modal="true"
    >
      <section class="w-full max-w-md rounded-lg border border-destructive/40 bg-surface p-5 shadow-2xl shadow-black/40">
        <div class="flex items-start gap-3">
          <div class="mt-0.5 grid h-9 w-9 shrink-0 place-items-center rounded-full bg-destructive/15 text-destructive">
            <AlertOctagon class="h-5 w-5" />
          </div>
          <div class="min-w-0 flex-1">
            <h2 class="text-base font-semibold text-foreground">{{ ui.errorModal.title }}</h2>
            <p class="mt-2 whitespace-pre-wrap break-words text-sm leading-6 text-muted">
              {{ ui.errorModal.message }}
            </p>
          </div>
          <button
            type="button"
            class="rounded-md p-1 text-muted transition-colors hover:bg-bg hover:text-foreground"
            :aria-label="ui.errorModal.secondaryLabel"
            @click="ui.closeError"
          >
            <X class="h-4 w-4" />
          </button>
        </div>

        <div class="mt-5 flex justify-end gap-2">
          <button
            type="button"
            class="rounded-lg border border-border px-4 py-2 text-sm text-foreground transition-colors hover:border-destructive/50"
            @click="ui.closeError"
          >
            {{ ui.errorModal.secondaryLabel }}
          </button>
          <button
            v-if="ui.errorModal.onPrimary"
            type="button"
            class="rounded-lg bg-destructive px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-destructive/90"
            @click="ui.onErrorPrimary"
          >
            {{ ui.errorModal.primaryLabel }}
          </button>
        </div>
      </section>
    </div>
  </Teleport>
</template>
