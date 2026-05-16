<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useVpnStore } from '../stores/vpn'
import { Route, Plus, Trash2, Save } from 'lucide-vue-next'

const vpn = useVpnStore()

const routes = ref<string[]>([])
const newRoute = ref('')
const saving = ref(false)
const message = ref<{ type: 'success' | 'error'; text: string } | null>(null)

onMounted(async () => {
  await vpn.fetchRoutes()
  routes.value = vpn.routes.map(r => r.cidr)
})

function addRoute() {
  const r = newRoute.value.trim()
  if (r && !routes.value.includes(r)) {
    routes.value.push(r)
    newRoute.value = ''
  }
}

function removeRoute(index: number) {
  routes.value.splice(index, 1)
}

async function saveRoutes() {
  saving.value = true
  message.value = null
  try {
    const current = vpn.routes.map(r => r.cidr)
    const toAdd = routes.value.filter(r => !current.includes(r))
    const toRemove = current.filter(r => !routes.value.includes(r))
    for (const cidr of toRemove) {
      await vpn.removeRoute(cidr)
    }
    for (const cidr of toAdd) {
      await vpn.addRoute(cidr)
    }
    message.value = { type: 'success', text: '路由设置已保存' }
  } catch (e: any) {
    message.value = { type: 'error', text: e?.message || '保存失败' }
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6 flex items-center gap-2">
      <Route class="w-5 h-5 text-accent" />
      分流路由
    </h1>

    <div class="bg-surface border border-border rounded-xl p-6 max-w-3xl">
      <p class="text-sm text-muted mb-4">
        配置分流路由，仅将指定网段的流量通过 VPN 隧道。留空表示全局隧道。
      </p>

      <div class="space-y-3 mb-6">
        <div
          v-for="(route, index) in routes"
          :key="index"
          class="flex items-center gap-3 bg-bg border border-border rounded-lg px-4 py-2.5"
        >
          <span class="text-sm text-foreground font-mono flex-1">{{ route }}</span>
          <button
            class="text-muted hover:text-destructive transition-colors"
            @click="removeRoute(index)"
          >
            <Trash2 class="w-4 h-4" />
          </button>
        </div>
      </div>

      <div class="flex items-center gap-3 mb-6">
        <input
          v-model="newRoute"
          type="text"
          placeholder="例如 10.0.0.0/8"
          class="flex-1 bg-bg border border-border rounded-lg px-4 py-2.5 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
          @keyup.enter="addRoute"
        />
        <button
          class="flex items-center gap-2 border border-border rounded-lg px-4 py-2.5 text-sm text-foreground hover:border-accent/50 transition-colors"
          @click="addRoute"
        >
          <Plus class="w-4 h-4" />
          添加
        </button>
      </div>

      <button
        :disabled="saving"
        class="flex items-center gap-2 bg-accent text-white rounded-lg px-5 py-2.5 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
        @click="saveRoutes"
      >
        <Save class="w-4 h-4" />
        {{ saving ? '保存中...' : '保存路由' }}
      </button>

      <div
        v-if="message"
        :class="[
          'text-sm rounded-lg px-4 py-2.5 mt-4',
          message.type === 'success' ? 'bg-green-500/10 text-green-400' : 'bg-red-500/10 text-red-400'
        ]"
      >
        {{ message.text }}
      </div>
    </div>
  </div>
</template>