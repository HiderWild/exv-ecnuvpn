<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { KeyRound } from 'lucide-vue-next'
import { useVpnStore } from '../stores/vpn'

const vpn = useVpnStore()
const value = ref('')
const error = ref('')
const inputRef = ref<HTMLInputElement | HTMLSelectElement | null>(null)

const interaction = computed(() => vpn.pendingAuthInteraction)
const isGroupSelection = computed(() =>
  interaction.value?.kind === 'group' && (interaction.value.options?.length ?? 0) > 0,
)
const inputType = computed(() =>
  interaction.value?.input_type === 'text' || isGroupSelection.value ? 'text' : 'password',
)
const title = computed(() =>
  isGroupSelection.value ? '选择认证组' : '继续认证',
)

watch(
  interaction,
  async (next) => {
    value.value = next && isGroupSelection.value ? next.options[0] ?? '' : ''
    error.value = ''
    if (next) {
      await nextTick()
      inputRef.value?.focus()
    }
  },
)

async function submit() {
  if (!interaction.value) return
  if (!value.value) {
    error.value = isGroupSelection.value ? '请选择认证组' : '请输入认证信息'
    return
  }
  const ok = await vpn.respondAuthInteraction(value.value)
  if (ok) {
    value.value = ''
    error.value = ''
  }
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="interaction"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4"
    >
      <form
        class="w-full max-w-sm rounded-xl border border-border bg-surface p-6 shadow-xl"
        @submit.prevent="submit"
      >
        <div class="mb-4 flex items-start gap-3">
          <KeyRound class="mt-0.5 h-5 w-5 shrink-0 text-primary" />
          <div class="min-w-0">
            <p class="text-sm font-semibold text-foreground">{{ title }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ interaction.label }}</p>
          </div>
        </div>

        <select
          v-if="isGroupSelection"
          ref="inputRef"
          v-model="value"
          class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none transition-colors focus:border-primary"
          :disabled="vpn.authInteractionBusy"
          @change="error = ''"
        >
          <option
            v-for="option in interaction.options"
            :key="option"
            :value="option"
          >
            {{ option }}
          </option>
        </select>
        <input
          v-else
          ref="inputRef"
          v-model="value"
          :type="inputType"
          autocomplete="one-time-code"
          class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none transition-colors focus:border-primary"
          :disabled="vpn.authInteractionBusy"
          placeholder="认证信息"
          @input="error = ''"
        />

        <p v-if="error" class="mt-2 text-xs text-destructive">{{ error }}</p>

        <div class="mt-5 flex justify-end gap-3">
          <button
            type="submit"
            class="rounded-lg bg-primary px-4 py-2 text-sm text-white transition-colors hover:bg-primary/90 disabled:opacity-50"
            :disabled="vpn.authInteractionBusy"
          >
            继续
          </button>
        </div>
      </form>
    </div>
  </Teleport>
</template>
