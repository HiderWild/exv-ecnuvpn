<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { AlertTriangle, CheckCircle, HardDriveDownload, RefreshCcw, Save } from 'lucide-vue-next'
import { useConfigStore, type SettingsConfig } from '../stores/config'
import { errorMessage } from '../utils/errors'

const config = useConfigStore()
const saving = ref(false)
const busyDriver = ref<'wintun' | 'tap' | null>(null)
const message = ref<{ type: 'success' | 'error'; text: string } | null>(null)
const installFeedback = ref<{ driver: 'wintun' | 'tap'; takes_effect: string } | null>(null)
const isDesktop = typeof window !== 'undefined' && !!window.ecnuVpn

const form = ref<SettingsConfig>({
  mtu: 1400,
  dtls: true,
  extra_args: '',
  log_path: '',
  webui_port: 18080,
  webui_host: '127.0.0.1',
  webui_enabled: true,
  openconnect_runtime: 'bundled',
  windows_tunnel_driver: 'auto',
  windows_tap_interface: '',
})

const driverSupported = computed(() => !!config.driverStatus?.supported)
const tapAdapters = computed(() => config.driverStatus?.tap_adapters || [])
const runtimeMissing = computed(() => config.runtimeStatus && !config.runtimeStatus.available)
const wintunMissing = computed(() => config.driverStatus?.wintun_missing)
const tapMissing = computed(() => config.driverStatus?.tap_missing)
const driverReadiness = computed(() => config.driverStatus?.effective_driver_status ?? 'unavailable')

// Whether any effective driver is available (at least one non-missing)
const anyDriverAvailable = computed(() => {
  if (!config.driverStatus) return false
  return !wintunMissing.value || !!config.driverStatus.tap_available
})

// Runtime readiness indicator
const runtimeReady = computed(() => config.runtimeStatus?.available ?? true)

const driverReadinessLabel = computed(() => {
  switch (driverReadiness.value) {
    case 'ready': return '驱动就绪'
    case 'degraded': return '驱动降级'
    case 'unavailable': return '驱动不可用'
    default: return '未知'
  }
})

const driverReadinessColor = computed(() => {
  switch (driverReadiness.value) {
    case 'ready': return 'text-green-400'
    case 'degraded': return 'text-yellow-400'
    case 'unavailable': return 'text-red-400'
    default: return 'text-muted'
  }
})

async function refreshRuntime() {
  if (!isDesktop) return
  await Promise.all([
    config.fetchRuntimeStatus(),
    config.fetchDriverStatus(),
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
    await config.saveSettings(form.value)
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

async function installDriver(driver: 'wintun' | 'tap') {
  busyDriver.value = driver
  message.value = null
  installFeedback.value = null
  try {
    const result = await config.installDriver(driver)
    await config.fetchDriverStatus()
    if (driver === 'tap' && !form.value.windows_tap_interface && tapAdapters.value.length > 0) {
      form.value.windows_tap_interface = tapAdapters.value[0]
    }
    // Show driver-specific feedback about when the change takes effect
    const takesEffect = result.takes_effect || (driver === 'wintun' ? 'next_connect' : 'immediately')
    installFeedback.value = { driver, takes_effect: takesEffect }
    if (takesEffect === 'next_connect') {
      message.value = {
        type: 'success',
        text: 'Wintun 运行时已就绪，下次连接时生效',
      }
    } else {
      message.value = {
        type: 'success',
        text: 'TAP 安装完成，立即生效。如已连接 VPN，建议断开重连。',
      }
    }
  } catch (error) {
    message.value = { type: 'error', text: errorMessage(error) }
  } finally {
    busyDriver.value = null
  }
}

async function switchToSystemRuntime() {
  form.value.openconnect_runtime = 'system'
  await save()
}
</script>

<template>
  <div class="py-8">
    <h1 class="text-xl font-semibold text-foreground mb-6">设置</h1>

    <div class="space-y-6 max-w-3xl">
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

      <div v-if="isDesktop" class="bg-surface border border-border rounded-xl p-6">
        <div class="flex items-start justify-between gap-4 mb-4">
          <div>
            <h2 class="text-sm font-medium text-foreground">运行时</h2>
            <p class="text-xs text-muted mt-1">桌面打包、OpenConnect 来源及驱动选择</p>
          </div>
          <button
            class="flex items-center gap-2 border border-border rounded-lg px-3 py-2 text-xs text-foreground hover:border-accent/50 transition-colors"
            @click="refreshRuntime"
          >
            <RefreshCcw class="w-3.5 h-3.5" />
            刷新
          </button>
        </div>

        <!-- Runtime readiness indicator -->
        <div class="flex items-center gap-2 mb-4">
          <span :class="runtimeReady ? 'text-green-400' : 'text-red-400'" class="text-lg leading-none">&#9679;</span>
          <span class="text-sm font-medium" :class="runtimeReady ? 'text-foreground' : 'text-red-400'">
            {{ runtimeReady ? '运行时就绪' : '运行时缺失 — 需要补齐' }}
          </span>
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

          <!-- Runtime missing warning card -->
          <div v-if="runtimeMissing" class="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300">
            <p class="font-medium">OpenConnect 运行时缺失</p>
            <p class="mt-1 text-xs text-red-300/80">
              {{ config.runtimeStatus?.missing_what || 'VPN 连接需要 OpenConnect 运行时。' }}
            </p>
            <p v-if="config.runtimeStatus?.recommended_action" class="mt-1 text-xs text-red-300/80">
              {{ config.runtimeStatus.recommended_action }}
            </p>
            <p v-if="config.runtimeStatus?.effect_on_connect" class="mt-1 text-xs text-red-300/60">
              {{ config.runtimeStatus.effect_on_connect }}
            </p>
            <button
              :disabled="saving"
              class="mt-3 flex items-center gap-2 border border-red-400/50 rounded-lg px-4 py-2 text-xs font-medium text-red-300 hover:bg-red-500/20 disabled:opacity-50 transition-colors"
              @click="switchToSystemRuntime"
            >
              切换到系统运行时
            </button>
          </div>

          <!-- Driver section -->
          <div v-if="driverSupported" class="space-y-4">
            <!-- Driver readiness summary -->
            <div class="flex items-center gap-2 text-sm">
              <CheckCircle v-if="driverReadiness === 'ready'" class="w-4 h-4 text-green-400" />
              <AlertTriangle v-else class="w-4 h-4" :class="driverReadinessColor" />
              <span :class="driverReadinessColor">{{ driverReadinessLabel }}</span>
              <span v-if="driverReadiness === 'unavailable'" class="text-red-400/80 text-xs">— 需要至少一个可用隧道驱动</span>
              <span v-else-if="driverReadiness === 'degraded'" class="text-yellow-400/80 text-xs">— 首选驱动缺失，回退可用</span>
            </div>

            <!-- Driver missing warning -->
            <div v-if="!anyDriverAvailable" class="rounded-lg border border-yellow-500/30 bg-yellow-500/10 px-4 py-3 text-sm text-yellow-300">
              <p class="font-medium">隧道驱动缺失</p>
              <p class="mt-1 text-xs text-yellow-300/80">
                需要至少一个可用的隧道驱动（Wintun 或 TAP）才能建立 VPN 连接。请点击下方按钮安装。
              </p>
            </div>

            <div>
              <label class="block text-xs font-medium text-muted mb-1.5">Windows 隧道驱动</label>
              <select
                v-model="form.windows_tunnel_driver"
                class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
              >
                <option value="auto">自动（优先 Wintun）</option>
                <option value="wintun">Wintun</option>
                <option value="tap">TAP</option>
              </select>
            </div>

            <div>
              <label class="block text-xs font-medium text-muted mb-1.5">TAP 接口</label>
              <select
                v-model="form.windows_tap_interface"
                :disabled="tapAdapters.length === 0"
                class="w-full bg-bg border border-border rounded-lg px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors disabled:opacity-50"
              >
                <option value="">选择 TAP 适配器</option>
                <option v-for="adapter in tapAdapters" :key="adapter" :value="adapter">
                  {{ adapter }}
                </option>
              </select>
            </div>

            <!-- Driver status badges -->
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4 text-sm">
              <div class="bg-bg border border-border rounded-lg p-4">
                <div class="flex items-center gap-2 mb-2">
                  <p class="text-xs text-muted">Wintun</p>
                  <span v-if="!wintunMissing" class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs bg-green-500/10 text-green-400">就绪</span>
                  <span v-else class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs bg-yellow-500/10 text-yellow-400">缺失</span>
                </div>
                <p class="text-foreground font-medium">
                  {{ config.driverStatus?.wintun_bundled ? '已检测到内置 wintun.dll' : '未检测到内置 wintun.dll' }}
                </p>
                <p v-if="wintunMissing && config.driverStatus?.wintun_recommended_action" class="text-xs text-yellow-400/80 mt-2">
                  {{ config.driverStatus.wintun_recommended_action }}
                </p>
                <p v-if="!wintunMissing" class="text-xs text-muted mt-2">
                  下次连接时自动生效
                </p>
              </div>
              <div class="bg-bg border border-border rounded-lg p-4">
                <div class="flex items-center gap-2 mb-2">
                  <p class="text-xs text-muted">TAP</p>
                  <span v-if="config.driverStatus?.tap_available" class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs bg-green-500/10 text-green-400">已安装</span>
                  <span v-else-if="config.driverStatus?.tap_can_install" class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs bg-yellow-500/10 text-yellow-400">可安装</span>
                  <span v-else class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs bg-red-500/10 text-red-400">不可用</span>
                </div>
                <p class="text-foreground font-medium">
                  {{ config.driverStatus?.tap_available ? '已检测到安装的适配器' : '未检测到 TAP 适配器' }}
                </p>
                <p v-if="tapMissing && config.driverStatus?.tap_recommended_action" class="text-xs text-yellow-400/80 mt-2">
                  {{ config.driverStatus.tap_recommended_action }}
                </p>
                <p v-if="config.driverStatus?.tap_available" class="text-xs text-muted mt-2">
                  立即生效；如已连接 VPN，建议断开重连
                </p>
              </div>
            </div>

            <div class="flex flex-wrap gap-3">
              <button
                :disabled="busyDriver === 'wintun'"
                class="flex items-center gap-2 border border-border rounded-lg px-4 py-2 text-sm text-foreground hover:border-accent/50 disabled:opacity-50 transition-colors"
                @click="installDriver('wintun')"
              >
                <HardDriveDownload class="w-4 h-4" />
                {{ busyDriver === 'wintun' ? '准备中...' : '准备 Wintun' }}
              </button>
              <button
                :disabled="busyDriver === 'tap'"
                class="flex items-center gap-2 border border-border rounded-lg px-4 py-2 text-sm text-foreground hover:border-accent/50 disabled:opacity-50 transition-colors"
                @click="installDriver('tap')"
              >
                <HardDriveDownload class="w-4 h-4" />
                {{ busyDriver === 'tap' ? '安装中...' : '安装 TAP' }}
              </button>
            </div>

            <!-- Install feedback -->
            <div v-if="installFeedback" class="text-xs text-muted bg-bg border border-border rounded-lg p-3">
              <template v-if="installFeedback.takes_effect === 'next_connect'">
                Wintun 运行时已就绪，下次连接时生效
              </template>
              <template v-else>
                TAP 安装完成，立即生效。如已连接 VPN，建议断开重连。
              </template>
            </div>
          </div>
        </div>
      </div>

      <!-- Browser compatibility (advanced) -->
      <details class="bg-surface border border-border rounded-xl">
        <summary class="px-6 py-4 text-sm text-muted cursor-pointer hover:text-foreground transition-colors">
          浏览器兼容设置（高级）
        </summary>
        <div class="px-6 pb-5 space-y-4">
          <div class="flex items-center justify-between py-1">
            <div>
              <p class="text-sm text-foreground">启用浏览器界面</p>
              <p class="text-xs text-muted">保留浏览器入口以兼容旧版</p>
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
      </details>

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
