<script setup lang="ts">
import { onMounted } from 'vue'
import { useVpnStore } from '../stores/vpn'
import { useConfigStore } from '../stores/config'
import { useUiStore } from '../stores/ui'
import StatusBadge from '../components/StatusBadge.vue'
import ConfirmDialog from '../components/ConfirmDialog.vue'
import { Terminal, Download, Trash2, RefreshCw } from 'lucide-vue-next'

const vpn = useVpnStore()
const config = useConfigStore()
const ui = useUiStore()

onMounted(() => {
  vpn.fetchServiceStatus()
  config.fetchHelperStatus()
})

function install() {
  ui.requestConfirm(
    '将安装 launchd 辅助服务为系统守护进程。需要管理员权限授权。',
    async () => {
      try {
        await vpn.installService()
        await config.fetchHelperStatus()
        ui.addToast('辅助服务安装成功', 'success')
      } catch {
        ui.addToast('辅助服务安装失败', 'error')
      }
    },
  )
}

function uninstall() {
  ui.requestConfirm(
    '将卸载 launchd 辅助服务。需要管理员权限授权。',
    async () => {
      try {
        await vpn.uninstallService()
        await config.fetchHelperStatus()
        ui.addToast('辅助服务卸载成功', 'success')
      } catch {
        ui.addToast('辅助服务卸载失败', 'error')
      }
    },
  )
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6">辅助服务</h1>

    <!-- Helper status card -->
    <div class="bg-surface border border-border rounded-xl p-6">
      <div class="flex items-center justify-between mb-6">
        <div class="flex items-center gap-3">
          <Terminal class="w-5 h-5 text-muted" />
          <div>
            <p class="text-sm font-medium text-foreground">launchd 辅助服务</p>
            <p class="text-xs text-muted">
              {{ config.helperStatus?.label || 'com.ecnu.exv.helper' }}
            </p>
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

      <!-- Description -->
      <div class="text-sm text-muted mb-6">
        <p v-if="!vpn.serviceStatus?.installed">
          辅助服务未安装。安装后，VPN 连接将由系统守护进程管理，无需每次输入管理员密码。
        </p>
        <p v-else-if="vpn.serviceStatus?.running">
          辅助服务正在运行，VPN 连接将通过守护进程自动管理。
        </p>
        <p v-else>
          辅助服务已安装但未运行。请尝试重启服务或重新安装。
        </p>
      </div>

      <!-- Action buttons -->
      <div class="flex items-center gap-3 border-t border-border pt-5">
        <button
          v-if="!vpn.serviceStatus?.installed"
          class="flex items-center gap-2 bg-accent text-white rounded-lg px-5 py-2 text-sm font-medium hover:bg-accent/90 transition-colors"
          @click="install"
        >
          <Download class="w-4 h-4" />
          安装辅助服务（推荐）
        </button>

        <template v-else>
          <button
            class="flex items-center gap-2 bg-destructive text-white rounded-lg px-5 py-2 text-sm font-medium hover:bg-destructive/90 transition-colors"
            @click="uninstall"
          >
            <Trash2 class="w-4 h-4" />
            卸载辅助服务
          </button>
        </template>

        <button
          class="flex items-center gap-2 border border-border text-muted rounded-lg px-5 py-2 text-sm hover:text-foreground hover:bg-surface transition-colors"
          @click="vpn.fetchServiceStatus(); config.fetchHelperStatus()"
        >
          <RefreshCw class="w-4 h-4" />
          刷新
        </button>
      </div>
    </div>

    <!-- Socket info (desktop only) -->
    <div v-if="vpn.serviceStatus?.installed && config.helperStatus?.socket_path" class="bg-surface border border-border rounded-xl p-6 mt-4">
      <h2 class="text-sm font-medium text-foreground mb-3">服务详情</h2>
      <div class="text-sm space-y-2">
        <div>
          <span class="text-muted">Socket 路径：</span>
          <span class="text-foreground ml-2 font-mono text-xs">{{ config.helperStatus.socket_path }}</span>
        </div>
        <div>
          <span class="text-muted">服务标签：</span>
          <span class="text-foreground ml-2 font-mono text-xs">{{ config.helperStatus.label }}</span>
        </div>
      </div>
    </div>

    <!-- Terminal fallback (collapsed) -->
    <div class="bg-surface border border-border rounded-xl p-6 mt-4">
      <h2 class="text-sm font-medium text-foreground mb-3">终端命令</h2>
      <p class="text-xs text-muted mb-3">
        如果界面无法管理服务，可在终端中使用以下命令：
      </p>
      <div class="bg-bg rounded-lg p-4 font-mono text-xs text-foreground space-y-2">
        <div>
          <span class="text-muted"># 安装辅助服务</span>
          <br />
          <span class="text-accent">sudo</span> exv service install
        </div>
        <div>
          <span class="text-muted"># 卸载辅助服务</span>
          <br />
          <span class="text-accent">sudo</span> exv service uninstall
        </div>
        <div>
          <span class="text-muted"># 查看状态</span>
          <br />
          exv service status
        </div>
      </div>
    </div>

    <ConfirmDialog />
  </div>
</template>