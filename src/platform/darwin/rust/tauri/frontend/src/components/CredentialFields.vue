<script setup lang="ts">
import PasswordField from "./PasswordField.vue";
withDefaults(defineProps<{
  username: string;
  password: string;
  hasSavedPassword?: boolean;
  readonlyUsername?: boolean;
  busy?: boolean;
  compact?: boolean;
  showClear?: boolean;
  testIdPrefix?: string;
  usernameError?: string;
  passwordError?: string;
}>(), { hasSavedPassword: false, readonlyUsername: false, busy: false, compact: false, showClear: true, testIdPrefix: "credential" });
const emit = defineEmits<{
  "update:username": [value: string];
  "update:password": [value: string];
  clearPassword: [];
}>();
</script>
<template>
  <label class="credential-modal-field" :data-compact="compact ? 'true' : 'false'">
    <span>账户</span>
    <input :value="username" :data-testid="`${testIdPrefix}-username${readonlyUsername ? '-readonly' : ''}`"
      :class="{ 'credential-modal-username-readonly': readonlyUsername }" autocomplete="username"
      :readonly="readonlyUsername" :aria-readonly="readonlyUsername || undefined" :tabindex="readonlyUsername && !compact ? -1 : undefined"
      :disabled="busy" @input="emit('update:username', ($event.target as HTMLInputElement).value)">
    <span v-if="usernameError" class="field-error" role="alert">{{ usernameError }}</span>
  </label>
  <label class="credential-modal-field" :data-compact="compact ? 'true' : 'false'">
    <span>密码</span>
    <div class="credential-modal-password-row">
      <PasswordField :model-value="password" :data-testid="`${testIdPrefix}-password`"
        :has-saved="hasSavedPassword" :disabled="busy" :compact="compact"
        @update:model-value="emit('update:password', $event)" />
      <button v-if="showClear" type="button" class="credential-clear-password" :data-testid="`${testIdPrefix}-clear-password`"
        :disabled="busy" @click="emit('clearPassword')">清除密码</button>
    </div>
    <span v-if="passwordError" class="field-error" role="alert">{{ passwordError }}</span>
  </label>
</template>
<style scoped>
.credential-modal-field { display: grid; min-width: 0; gap: var(--space-2); color: var(--text-secondary); font-size: var(--font-size-body); }
.credential-modal-field input { min-width: 0; min-height: 36px; padding: 0 var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-primary); }
.credential-modal-password-row { display: flex; align-items: center; gap: 8px; min-width: 0; }
.credential-modal-password-row :deep(.password-field) { min-width: 0; flex: 1; }
.credential-clear-password { flex: none; min-height: 34px; padding: 0 10px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-secondary); }
.credential-modal-username-readonly { color: var(--text-secondary); background: var(--surface-subtle); cursor: default; }
.field-error { color: var(--state-danger); font-size: var(--font-size-caption); }
.credential-modal-field[data-compact="true"] { gap: 2px; }
.credential-modal-field[data-compact="true"] input { min-height: 24px; padding: 2px 6px; font-size: var(--font-size-body); }
</style>
