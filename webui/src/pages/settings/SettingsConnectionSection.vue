<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { EthernetPort } from 'lucide-vue-next'
import ToggleSwitch from '../../components/ToggleSwitch.vue'
import { useConfigStore, type CoreInspection, type SettingsConfig } from '../../stores/config'
import { useUiStore } from '../../stores/ui'
import { normalizeError, useVpnStore } from '../../stores/vpn'

const props = defineProps<{
  settingsDraft: SettingsConfig
}>()

const emit = defineEmits<{
  'update:settingsDraft': [value: SettingsConfig]
}>()

const config = useConfigStore()
const vpn = useVpnStore()
const ui = useUiStore()
const isDesktop = typeof window !== 'undefined' && !!window.exv

const coreInspection = ref<CoreInspection | null>(null)
const coreMaintenanceBusy = ref(false)

const fallbackSettingsDraft: SettingsConfig = {
  mtu: 1400,
  dtls: true,
  extra_args: '',
  log_path: '',
  webui_port: 18080,
  webui_host: '127.0.0.1',
  webui_enabled: true,
  vpn_engine: 'native',
  windows_tunnel_driver: 'auto',
  windows_tap_interface: '',
  auto_reconnect: true,
  retry_limit: -1,
  minimal_mode: false,
  service_install_prompt_seen: false,
  minimal_install_service_before_connect: true,
  include_class_a_private_routes: false,
  include_class_b_private_routes: false,
  launch_at_login: false,
  auto_connect_on_launch: false,
}

const settingsForm = computed(() => props.settingsDraft ?? fallbackSettingsDraft)

const dtlsModel = computed({
  get: () => settingsForm.value.dtls,
  set: (value: boolean) => updateSettingField('dtls', value),
})

const autoReconnectModel = computed({
  get: () => settingsForm.value.auto_reconnect,
  set: (value: boolean) => updateSettingField('auto_reconnect', value),
})

const rememberedPasswordReady = computed(() =>
  Boolean(config.authConfig.remember_password && config.authConfig.password_stored),
)

const startupAutoConnectAllowed = computed(() =>
  rememberedPasswordReady.value && vpn.serviceInstalled,
)

const startupAutoConnectReason = computed(() => {
  if (!rememberedPasswordReady.value) return '需要先保存并记住密码。'
  if (!vpn.serviceInstalled) return '需要先安装 Helper 服务。'
  return ''
})

const startupAutoConnectModel = computed({
  get: () => settingsForm.value.auto_connect_on_launch,
  set: (value: boolean) => {
    if (value && !startupAutoConnectAllowed.value) {
      ui.requestError({
        title: '无法开启启动时自动连接',
        message: startupAutoConnectReason.value,
      })
      return
    }
    updateSettingField('auto_connect_on_launch', value)
  },
})

const showCoreMaintenanceBanner = computed(() => {
  const inspection = coreInspection.value
  if (!inspection) return false
  return inspection.state === 'broken' || inspection.risk === 'high'
})

function updateSettingField<K extends keyof SettingsConfig>(key: K, value: SettingsConfig[K]) {
  emit('update:settingsDraft', {
    ...settingsForm.value,
    [key]: value,
  })
}

function updateNumberField(key: 'mtu' | 'retry_limit', event: Event) {
  const value = Number((event.target as HTMLInputElement).value)
  updateSettingField(key, value)
}

async function inspectCoreSilently() {
  try {
    coreInspection.value = await config.inspectCore()
  } catch {
    coreInspection.value = null
  }
}

function killStaleCoreAction() {
  ui.requestConfirm('确认终止该内核进程？此操作不可撤销。', async () => {
    if (coreMaintenanceBusy.value) return
    coreMaintenanceBusy.value = true
    try {
      await config.killStaleCore(true)
      ui.addToast('已清理', 'success')
      await inspectCoreSilently()
    } catch (error) {
      ui.requestError({ title: '清理残留进程失败', message: normalizeError(error).message })
    } finally {
      coreMaintenanceBusy.value = false
    }
  })
}

onMounted(async () => {
  if (isDesktop) {
    void vpn.fetchServiceStatus()
    void inspectCoreSilently()
  }
})
</script>

<template>
  <section class="rounded-xl border border-border bg-surface p-5">
    <h2 class="mb-4 flex items-center gap-2 text-base font-semibold text-foreground">
      <EthernetPort class="h-5 w-5 text-accent" />
      连接
    </h2>

    <div
      v-if="showCoreMaintenanceBanner"
      class="mb-4 rounded-lg border border-warning/30 bg-warning/10 px-4 py-3 text-sm text-warning"
    >
      <p class="font-medium">检测到残留 / 异常的 VPN 内核进程</p>
      <p class="mt-1 text-xs text-warning/80">
        已发现可能无响应或卡死的内核（pid {{ coreInspection?.pid ?? '-' }}）。是否清理？
      </p>
      <button
        :disabled="coreMaintenanceBusy"
        class="mt-3 rounded-lg border border-warning/50 px-4 py-2 text-xs font-medium text-warning transition-colors hover:bg-warning/20 disabled:opacity-50"
        @click="killStaleCoreAction"
      >
        清理残留进程
      </button>
    </div>

    <div class="space-y-5">
      <div>
        <label class="mb-1.5 block text-xs font-medium text-muted">MTU</label>
        <input
          :value="settingsForm.mtu"
          type="number"
          min="576"
          max="1500"
          class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none"
          @input="updateNumberField('mtu', $event)"
        />
      </div>

      <div class="flex items-center justify-between rounded-lg border border-border bg-bg/40 px-4 py-3">
        <div>
          <p class="text-sm text-foreground">DTLS</p>
          <p class="text-xs text-muted">当前原生连接使用 CSTP-only；DTLS 后端加入前不会启用。</p>
        </div>
        <ToggleSwitch v-model="dtlsModel" />
      </div>

      <div class="flex items-center justify-between rounded-lg border border-border bg-bg/40 px-4 py-3">
        <div>
          <p class="text-sm text-foreground">断线重连</p>
          <p class="text-xs text-muted">连接进程意外退出后自动尝试重新连接</p>
        </div>
        <ToggleSwitch v-model="autoReconnectModel" />
      </div>

      <div class="flex items-center justify-between rounded-lg border border-border bg-bg/40 px-4 py-3">
        <div>
          <p class="text-sm text-foreground">启动时自动连接</p>
          <p class="text-xs text-muted">
            {{ startupAutoConnectAllowed ? '客户端启动后自动建立 VPN 连接' : startupAutoConnectReason }}
          </p>
        </div>
        <ToggleSwitch
          v-model="startupAutoConnectModel"
          :disabled="!startupAutoConnectAllowed && !settingsForm.auto_connect_on_launch"
        />
      </div>

      <div>
        <label class="mb-1.5 block text-xs font-medium text-muted">重连尝试次数</label>
        <input
          :value="settingsForm.retry_limit"
          type="number"
          min="-1"
          class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none"
          @input="updateNumberField('retry_limit', $event)"
        />
        <p class="mt-1 text-xs text-muted">-1 表示无限重连；0 表示不重连；正整数表示最大次数。</p>
      </div>
    </div>
  </section>
</template>
