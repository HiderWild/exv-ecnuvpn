<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useVpnStore } from '../stores/vpn'
import { useUiStore } from '../stores/ui'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import { Plus, Trash2, RotateCcw } from 'lucide-vue-next'

const vpn = useVpnStore()
const ui = useUiStore()
const newCidr = ref('')

const defaultRoutes = [
  '10.0.0.0/8',
  '172.16.0.0/12',
  '192.168.0.0/16',
]

onMounted(() => {
  vpn.fetchRoutes()
})

async function add() {
  const cidr = newCidr.value.trim()
  if (!cidr) return
  try {
    await vpn.addRoute(cidr)
    newCidr.value = ''
    ui.addToast('Route added', 'success')
  } catch {
    ui.addToast('Failed to add route', 'error')
  }
}

async function remove(cidr: string) {
  try {
    await vpn.removeRoute(cidr)
    ui.addToast('Route removed', 'success')
  } catch {
    ui.addToast('Failed to remove route', 'error')
  }
}

function reset() {
  ui.requestConfirm('Reset all routes to defaults? This will remove custom routes.', async () => {
    await vpn.resetRoutes()
    ui.addToast('Routes reset to defaults', 'success')
  })
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6">Route Configuration</h1>

    <div class="bg-surface border border-border rounded-xl p-6">
      <!-- Add route -->
      <div class="flex gap-2 mb-6">
        <input
          v-model="newCidr"
          type="text"
          placeholder="CIDR (e.g., 10.0.0.0/8)"
          class="flex-1 bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
          @keyup.enter="add"
        />
        <button
          class="flex items-center gap-1.5 bg-accent text-white rounded-lg px-4 py-2 text-sm font-medium hover:bg-accent/90 transition-colors"
          @click="add"
        >
          <Plus class="w-4 h-4" />
          Add
        </button>
      </div>

      <!-- Route list -->
      <div v-if="vpn.routes.length" class="space-y-2 mb-6">
        <div
          v-for="route in vpn.routes"
          :key="route.cidr"
          class="flex items-center justify-between bg-bg border border-border rounded-lg px-4 py-2.5 group"
        >
          <code class="text-sm text-foreground font-mono">{{ route.cidr }}</code>
          <button
            class="text-muted hover:text-destructive transition-colors opacity-0 group-hover:opacity-100"
            @click="remove(route.cidr)"
          >
            <Trash2 class="w-4 h-4" />
          </button>
        </div>
      </div>

      <div v-else class="text-center py-8 text-muted text-sm">
        No routes configured. Add a CIDR above or reset to defaults.
      </div>

      <!-- Reset button -->
      <div class="border-t border-border pt-4">
        <button
          class="flex items-center gap-1.5 text-sm text-warning hover:text-warning/80 transition-colors"
          @click="reset"
        >
          <RotateCcw class="w-4 h-4" />
          Reset to Default Routes
        </button>
        <p class="text-xs text-muted mt-2">
          Defaults: {{ defaultRoutes.join(', ') }}
        </p>
      </div>
    </div>

    <ConfirmDialog />
  </div>
</template>
