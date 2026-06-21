<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { KeyRound } from 'lucide-vue-next'
import { useVpnStore } from '../stores/vpn'
import ModalShell from './ModalShell.vue'

const props = withDefaults(defineProps<{
  compact?: boolean
}>(), {
  compact: false,
})
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
  <ModalShell
    :open="Boolean(interaction)"
    :title="title"
    :description="props.compact ? '' : interaction?.label"
    :close-on-scrim="false"
    :compact="props.compact"
    size="sm"
  >
    <template #icon>
      <KeyRound class="h-4 w-4" />
    </template>

    <form
      id="auth-continuation-form"
      :class="props.compact ? 'modal-compact-form' : 'space-y-3'"
      @submit.prevent="submit"
    >
      <select
        v-if="isGroupSelection"
        ref="inputRef"
        v-model="value"
        class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none transition-colors focus:border-primary"
        :disabled="vpn.authInteractionBusy"
        @change="error = ''"
      >
        <option
          v-for="option in interaction?.options ?? []"
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

      <p v-if="error" :class="props.compact ? 'modal-compact-error' : 'text-xs text-destructive'">{{ error }}</p>
    </form>

    <template #actions>
      <button
        type="submit"
        form="auth-continuation-form"
        class="rounded-lg bg-primary px-3 py-2 text-sm text-white hover:bg-primary/90 disabled:opacity-50"
        :disabled="vpn.authInteractionBusy"
      >
        继续
      </button>
    </template>
  </ModalShell>
</template>
