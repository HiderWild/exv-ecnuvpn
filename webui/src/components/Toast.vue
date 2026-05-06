<script setup lang="ts">
import { useUiStore } from '../stores/ui'
import { X, CheckCircle, AlertCircle, AlertTriangle, Info } from 'lucide-vue-next'

const ui = useUiStore()
</script>

<template>
  <Teleport to="body">
    <div class="fixed bottom-4 right-4 z-50 flex flex-col gap-2 pointer-events-none">
      <TransitionGroup name="toast">
        <div
          v-for="toast in ui.toasts"
          :key="toast.id"
          :class="[
            'pointer-events-auto flex items-center gap-3 px-4 py-3 rounded-lg shadow-lg border backdrop-blur-sm max-w-sm',
            toast.type === 'success' ? 'bg-accent/10 border-accent/30 text-accent' :
            toast.type === 'error' ? 'bg-destructive/10 border-destructive/30 text-destructive' :
            toast.type === 'warning' ? 'bg-warning/10 border-warning/30 text-warning' :
            'bg-primary/10 border-primary/30 text-foreground'
          ]"
        >
          <CheckCircle v-if="toast.type === 'success'" class="w-4 h-4 shrink-0" />
          <AlertCircle v-else-if="toast.type === 'error'" class="w-4 h-4 shrink-0" />
          <AlertTriangle v-else-if="toast.type === 'warning'" class="w-4 h-4 shrink-0" />
          <Info v-else class="w-4 h-4 shrink-0" />
          <span class="text-sm flex-1">{{ toast.text }}</span>
          <button class="shrink-0 hover:opacity-70" @click="ui.removeToast(toast.id)">
            <X class="w-3.5 h-3.5" />
          </button>
        </div>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style scoped>
.toast-enter-active {
  transition: all 0.3s ease-out;
}
.toast-leave-active {
  transition: all 0.2s ease-in;
}
.toast-enter-from {
  opacity: 0;
  transform: translateX(30px);
}
.toast-leave-to {
  opacity: 0;
  transform: translateX(30px);
}
</style>
