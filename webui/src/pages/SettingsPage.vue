<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { RefreshCcw, Save } from 'lucide-vue-next'
import { useConfigStore, type SettingsConfig } from '../stores/config'
import { errorMessage } from '../utils/errors'

const config = useConfigStore()
const saving = ref(false)
const message = ref<{ type: 'success' | 'error'; text: string } | null>(null)
const isDesktop = typeof window !== 'undefined' && !!window.ecnuVpn
const showAdvanced = ref(false)

const form = ref<SettingsConfig>({
  mtu: 1400,
  dtls: true,
  extra_args: '',
  log_path: '',
  webui_port: 18080,
  webui_host: '127.0.0.1',
  webui_enabled: true,
  openconnect_runtime: 'bundled',
})

const helperInstalled = computed(() => config.helperStatus?.installed ?? false)
const helperRunning = computed(() => config.helperStatus?.running ?? false)

async function refreshRuntime() {
  if (!isDesktop) return
  await Promise.all([
    config.fetchRuntimeStatus(),
    config.fetchHelperStatus(),
  ])
}

onMounted(async () => {
  await config.fetchSettings()
  form.value = { ...config.settings }

  if (isDesktop) {
    try {
      await refreshRuntime()
    } catch (error) {
      message.value = { type: 'error', text: errorMessage(error) }
    }
  }
})

async function save() {
  saving.value = true
  message.value = null
  try {
    await config.saveSettings({ ...form.value })
    if (isDesktop) {
      await refreshRuntime()
    }
    message.value = { type: 'success', text: '设置已保存' }
  } catch (error) {
    message.value = { type: 'error', text: errorMessage(error) }
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6">设置</h1>

    <div class="space-y-6 max-w-3xl">
      <!-- Connection settings -->
      <div class="bg-surface border border-border rounded-xl p-6">
        <h2 class="text-sm font-medium text-foreground mb-4">连接</h2>
        <div class="space-y-4">
          <div>
            <label class="block text-xs font-medium text-muted mb-1.5">MTU</label>
            <input
              v-model.number="form.mtu"
              type="number"
              min="576"
              max="1500"
              class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
            />
          </div>

          <div class="flex items-center justify-between py-1">
            <div>
              <p class="text-sm text-foreground">DTLS</p>
              <p class="text-xs text-muted">运行时支持时启用 DTLS 加密</p>
            </div>
            <input
              v-model="form.dtls"
              type="checkbox"
              class="w-4 h-4 rounded border-border accent-accent"
            />
          </div>

          <div>
            <label class="block text-xs font-medium text-muted mb-1.5">额外参数</label>
            <input
              v-model="form.extra_args"
              type="text"
              placeholder="openconnect 附加参数"
              class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
            />
          </div>

          <div>
            <label class="block text-xs font-medium text-muted mb-1.5">日志文件路径</label>
            <input
              v-model="form.log_path"
              type="text"
              placeholder="~/.ecnuvpn/ecnuvpn.log"
              class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground font-mono placeholder:text-muted focus:outline-none focus:border-accent/50 transition-colors"
            />
          </div>
        </div>
      </div>

      <!-- Runtime & Helper (desktop only) -->
      <div v-if="isDesktop" class="bg-surface border border-border rounded-xl p-6">
        <div class="flex items-start justify-between gap-4 mb-4">
          <div>
            <h2 class="text-sm font-medium text-foreground">运行时与服务</h2>
            <p class="text-xs text-muted mt-1">OpenConnect 来源与 launchd 辅助服务状态</p>
          </div>
          <button
            class="flex items-center gap-2 border border-border rounded-lg px-3 py-2 text-xs text-foreground hover:border-accent/50 transition-colors"
            @click="refreshRuntime"
          >
            <RefreshCcw class="w-3.5 h-3.5" />
            刷新
          </button>
        </div>

        <div class="space-y-4">
          <div>
            <label class="block text-xs font-medium text-muted mb-1.5">OpenConnect 运行时</label>
            <select
              v-model="form.openconnect_runtime"
              class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
            >
              <option value="bundled">内置</option>
              <option value="auto">自动</option>
              <option value="system">系统</option>
            </select>
          </div>

          <div class="grid grid-cols-1 md:grid-cols-2 gap-4 text-sm">
            <div class="bg-bg border border-border rounded-lg p-4">
              <p class="text-xs text-muted mb-2">当前运行时</p>
              <p class="text-foreground font-medium">
                {{ config.runtimeStatus?.available ? (config.runtimeStatus.source || '未知') : '缺失' }}
              </p>
              <p class="text-xs text-muted mt-2 break-all">
                {{ config.runtimeStatus?.path || '未找到 openconnect 二进制文件' }}
              </p>
            </div>
            <div class="bg-bg border border-border rounded-lg p-4">
              <p class="text-xs text-muted mb-2">版本</p>
              <p class="text-foreground font-medium">
                {{ config.runtimeStatus?.version || '未知' }}
              </p>
              <p class="text-xs text-muted mt-2 break-all">
                {{ config.runtimeStatus?.bundled_runtime_dir || '未检测到运行时目录' }}
              </p>
            </div>
          </div>

          <!-- Helper status -->
          <div class="bg-bg border border-border rounded-lg p-4">
            <p class="text-xs text-muted mb-2">launchd 辅助服务</p>
            <p class="text-foreground font-medium">
              {{ helperInstalled ? (helperRunning ? '运行中' : '已安装（未运行）') : '未安装' }}
            </p>
            <p class="text-xs text-muted mt-2 break-all">
              {{ config.helperStatus?.socket_path || '未安装辅助服务时，连接需要管理员授权' }}
            </p>
          </div>
        </div>
      </div>

      <!-- Advanced: WebUI compatibility -->
      <div class="bg-surface border border-border rounded-xl p-6">
        <div class="flex items-center justify-between mb-4">
          <h2 class="text-sm font-medium text-foreground">兼容模式</h2>
          <button
            class="text-xs text-muted hover:text-foreground transition-colors"
            @click="showAdvanced = !showAdvanced"
          >
            {{ showAdvanced ? '收起' : '展开' }}
          </button>
        </div>

        <p class="text-xs text-muted mb-4">
          浏览器 WebUI 是兼容入口，推荐使用桌面端完成所有操作。
        </p>

        <div v-if="showAdvanced" class="space-y-4">
          <div class="flex items-center justify-between py-1">
            <div>
              <p class="text-sm text-foreground">启用 WebUI</p>
              <p class="text-xs text-muted">允许通过浏览器访问管理界面（兼容模式）</p>
            </div>
            <input
              v-model="form.webui_enabled"
              type="checkbox"
              class="w-4 h-4 rounded border-border accent-accent"
            />
          </div>

          <div class="grid grid-cols-2 gap-4">
            <div>
              <label class="block text-xs font-medium text-muted mb-1.5">主机</label>
              <input
                v-model="form.webui_host"
                type="text"
                class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
              />
            </div>
            <div>
              <label class="block text-xs font-medium text-muted mb-1.5">端口</label>
              <input
                v-model.number="form.webui_port"
                type="number"
                class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
              />
            </div>
          </div>
        </div>
      </div>

      <button
        :disabled="saving"
        class="flex items-center gap-2 bg-accent text-white rounded-lg px-5 py-2.5 text-sm font-medium hover:bg-accent/90 disabled:opacity-50 transition-colors"
        @click="save"
      >
        <Save class="w-4 h-4" />
        {{ saving ? '保存中...' : '保存设置' }}
      </button>

      <div
        v-if="message"
        :class="[
          'text-sm rounded-lg px-4 py-2.5',
          message.type === 'success' ? 'bg-green-500/10 text-green-400' : 'bg-red-500/10 text-red-400'
        ]"
      >
        {{ message.text }}
      </div>
    </div>
  </div>
</template>