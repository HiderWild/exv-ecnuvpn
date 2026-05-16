<script setup lang="ts">
import { onMounted } from 'vue'
import { useVpnStore } from '../stores/vpn'
import { useUiStore } from '../stores/ui'
import StatusBadge from '../components/StatusBadge.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import { Terminal, Download, Trash2, RefreshCw } from 'lucide-vue-next'

const vpn = useVpnStore()
const ui = useUiStore()

onMounted(() => {
  vpn.fetchServiceStatus()
})

function install() {
  ui.requestConfirm(
    '将安装 VPN 辅助服务为系统服务。可能需要管理员权限。',
    async () => {
      try {
        await vpn.installService()
        ui.addToast('服务安装成功', 'success')
      } catch {
        ui.addToast('服务安装失败', 'error')
      }
    },
  )
}

function uninstall() {
  ui.requestConfirm(
    '将卸载 VPN 辅助服务。可能需要管理员权限。',
    async () => {
      try {
        await vpn.uninstallService()
        ui.addToast('服务卸载成功', 'success')
      } catch {
        ui.addToast('服务卸载失败', 'error')
      }
    },
  )
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6">服务管理</h1>

    <div class="bg-surface border border-border rounded-xl p-6">
      <!-- Status -->
      <div class="flex items-center justify-between mb-6">
        <div class="flex items-center gap-3">
          <Terminal class="w-5 h-5 text-muted" />
          <div>
            <p class="text-sm font-medium text-foreground">守护进程服务</p>
            <p class="text-xs text-muted">{{ vpn.serviceStatus?.path || '未安装' }}</p>
          </div>
        </div>
        <div class="flex items-center gap-3">
          <StatusBadge
            v-if="vpn.serviceStatus?.installed"
            :status="vpn.serviceStatus?.running ? 'running' : 'stopped'"
          />
          <StatusBadge v-else status="disconnected" />
        </div>
      </div>

      <!-- Action buttons -->
      <div class="flex items-center gap-3 border-t border-border pt-5">
        <button
          v-if="!vpn.serviceStatus?.installed"
          class="flex items-center gap-2 bg-accent text-white rounded-lg px-5 py-2 text-sm font-medium hover:bg-accent/90 transition-colors"
          @click="install"
        >
          <Download class="w-4 h-4" />
          安装服务
        </button>

        <template v-else>
          <button
            class="flex items-center gap-2 bg-destructive text-white rounded-lg px-5 py-2 text-sm font-medium hover:bg-destructive/90 transition-colors"
            @click="uninstall"
          >
            <Trash2 class="w-4 h-4" />
            卸载服务
          </button>
        </template>

        <button
          class="flex items-center gap-2 border border-border text-muted rounded-lg px-5 py-2 text-sm hover:text-foreground hover:bg-surface transition-colors"
          @click="vpn.fetchServiceStatus()"
        >
          <RefreshCw class="w-4 h-4" />
          刷新
        </button>
      </div>
    </div>

    <div class="bg-surface border border-border rounded-xl p-6 mt-4">
      <h2 class="text-sm font-medium text-foreground mb-3">终端命令</h2>
      <p class="text-xs text-muted mb-3">
        如果界面无法管理服务，可在终端中使用以下命令：
      </p>
      <div class="bg-bg rounded-lg p-4 font-mono text-xs text-foreground space-y-2">
        <div>
          <span class="text-muted"># 安装服务</span>
          <br />
          <span class="text-accent">sudo</span> ecnu-vpn service install
        </div>
        <div>
          <span class="text-muted"># 卸载服务</span>
          <br />
          <span class="text-accent">sudo</span> ecnu-vpn service uninstall
        </div>
        <div>
          <span class="text-muted"># 查看状态</span>
          <br />
          ecnu-vpn service status
        </div>
      </div>
    </div>

    <ConfirmDialog />
  </div>
</template>