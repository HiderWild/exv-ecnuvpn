<script setup lang="ts">
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useVpnStore } from '../stores/vpn'
import {
  Shield, ShieldCheck, ShieldOff, Terminal, Download, Trash2, AlertTriangle, CheckCircle2, ArrowRight
} from 'lucide-vue-next'

const vpn = useVpnStore()
const router = useRouter()

onMounted(() => {
  vpn.fetchServiceStatus()
})

const isDesktop = typeof window !== 'undefined' && !!window.ecnuVpn
</script>

<template>
  <div class="max-w-3xl mx-auto">
    <h1 class="text-xl font-semibold text-foreground mb-6">VPN 服务</h1>

    <!-- State 4: Browser mode — desktop-unmanageable -->
    <div v-if="!isDesktop" class="bg-surface border border-border rounded-xl p-6 mb-6">
      <div class="flex items-center gap-3 mb-4">
        <ShieldOff class="w-6 h-6 text-muted" />
        <div>
          <h2 class="text-lg font-medium text-foreground">服务管理需要桌面客户端</h2>
          <p class="text-sm text-muted">
            在浏览器中无法管理 VPN 服务。请使用桌面应用安装和管理服务。
          </p>
        </div>
      </div>
      <div class="border-t border-border pt-4 mt-3">
        <p class="text-sm text-muted mb-3">如需通过命令行操作，请参考以下命令：</p>
        <div class="space-y-3 text-sm">
          <div>
            <p class="text-muted mb-1">安装服务：</p>
            <code class="block bg-bg rounded px-3 py-2 text-xs font-mono text-foreground overflow-x-auto">
              sudo exv service install
            </code>
          </div>
          <div>
            <p class="text-muted mb-1">卸载服务：</p>
            <code class="block bg-bg rounded px-3 py-2 text-xs font-mono text-foreground overflow-x-auto">
              sudo exv service uninstall
            </code>
          </div>
          <div>
            <p class="text-muted mb-1">查看服务状态：</p>
            <code class="block bg-bg rounded px-3 py-2 text-xs font-mono text-foreground overflow-x-auto">
              exv service status
            </code>
          </div>
        </div>
      </div>
    </div>

    <!-- States 1-3: Desktop mode service management -->
    <template v-else>
      <!-- Current service state -->
      <div class="bg-surface border border-border rounded-xl p-6 mb-6">
        <div class="flex items-center gap-3 mb-4">
          <!-- State 1: Installed + running -->
          <ShieldCheck v-if="vpn.serviceInstalled && vpn.serviceRunning" class="w-6 h-6 text-green-400" />
          <!-- State 2: Installed not running -->
          <Shield v-else-if="vpn.serviceInstalled" class="w-6 h-6 text-yellow-400" />
          <!-- State 3: Not installed -->
          <ShieldOff v-else class="w-6 h-6 text-muted" />
          <div>
            <h2 class="text-lg font-medium text-foreground">
              {{ vpn.serviceInstalled ? (vpn.serviceRunning ? '服务运行中' : '服务已安装（未运行）') : '服务未安装' }}
            </h2>
            <p class="text-sm text-muted">
              {{ vpn.serviceInstalled
                ? (vpn.serviceRunning ? 'VPN 服务正在后台运行，支持开机自启' : '服务已安装但当前未运行')
                : '安装服务后 VPN 可开机自启，无需每次手动授权' }}
            </p>
          </div>
        </div>

        <!-- Install/Uninstall actions -->
        <div class="flex items-center gap-3">
          <button
            v-if="!vpn.serviceInstalled"
            :disabled="vpn.loading"
            class="flex items-center gap-2 bg-accent text-white rounded-lg px-5 py-2.5 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
            @click="vpn.installService()"
          >
            <Download class="w-4 h-4" />
            安装服务
          </button>
          <button
            v-else
            :disabled="vpn.loading"
            class="flex items-center gap-2 border border-border text-muted rounded-lg px-5 py-2.5 text-sm hover:text-foreground hover:border-destructive/50 disabled:opacity-50 transition-colors"
            @click="vpn.uninstallService()"
          >
            <Trash2 class="w-4 h-4" />
            卸载服务
          </button>

          <!-- Go to dashboard to connect (when service is installed and running) -->
          <button
            v-if="vpn.serviceInstalled && vpn.serviceRunning"
            class="flex items-center gap-2 border border-accent/50 text-accent rounded-lg px-5 py-2.5 text-sm font-medium hover:bg-accent/10 transition-colors"
            @click="router.push('/')"
          >
            前往连接
            <ArrowRight class="w-4 h-4" />
          </button>
        </div>
      </div>

      <!-- Recommended path explanation -->
      <div class="bg-surface border border-border rounded-xl p-6 mb-6">
        <h2 class="text-sm font-medium text-foreground mb-3">推荐使用方式</h2>
        <div class="space-y-3">
          <div class="flex items-start gap-3">
            <CheckCircle2 class="w-4 h-4 text-green-400 mt-0.5 shrink-0" />
            <div class="min-w-0">
              <p class="text-sm text-foreground">安装服务（推荐）</p>
              <p class="text-xs text-muted mt-0.5">
                安装后 VPN 随系统启动，连接稳定，无需重复授权。适合日常使用。
              </p>
            </div>
          </div>
        </div>

        <!-- Fallback separator -->
        <div class="border-t border-border pt-3 mt-4">
          <p class="text-xs text-muted font-medium mb-2">备选方案</p>
          <div class="flex items-start gap-3">
            <AlertTriangle class="w-4 h-4 text-yellow-400 mt-0.5 shrink-0" />
            <div class="min-w-0">
              <p class="text-sm text-foreground">仅本次连接</p>
              <p class="text-xs text-muted mt-0.5">
                此方式每次连接都需要手动授权，仅作为未安装服务时的临时方案。前往 Dashboard 可使用此方式连接。
              </p>
            </div>
          </div>
        </div>
      </div>

      <!-- CLI compatibility reference (secondary) -->
      <details class="bg-surface border border-border rounded-xl">
        <summary class="px-6 py-4 text-sm text-muted cursor-pointer hover:text-foreground transition-colors flex items-center gap-2">
          <Terminal class="w-4 h-4" />
          命令行操作（高级）
        </summary>
        <div class="px-6 pb-5 space-y-3 text-sm">
          <div>
            <p class="text-muted mb-1">安装服务：</p>
            <code class="block bg-bg rounded px-3 py-2 text-xs font-mono text-foreground overflow-x-auto">
              sudo exv service install
            </code>
          </div>
          <div>
            <p class="text-muted mb-1">卸载服务：</p>
            <code class="block bg-bg rounded px-3 py-2 text-xs font-mono text-foreground overflow-x-auto">
              sudo exv service uninstall
            </code>
          </div>
          <div>
            <p class="text-muted mb-1">查看服务状态：</p>
            <code class="block bg-bg rounded px-3 py-2 text-xs font-mono text-foreground overflow-x-auto">
              exv service status
            </code>
          </div>
        </div>
      </details>
    </template>
  </div>
</template>
