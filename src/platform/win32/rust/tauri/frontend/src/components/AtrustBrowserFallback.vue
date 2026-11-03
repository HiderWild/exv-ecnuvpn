<script setup lang="ts">
import { computed, inject } from "vue";
import { CONNECTION_INTERACTION_KEY } from "../product/connection-interaction";
const interaction = inject(CONNECTION_INTERACTION_KEY, null);
const fallbackAvailable = computed(() => interaction?.fallbackAvailable.value ?? false);
const fallbackBusy = computed(() => interaction?.fallbackBusy.value ?? false);
const fallbackError = computed(() => interaction?.fallbackError.value ?? "");
function useSystemBrowserFallback(): void { void interaction?.useSystemBrowserFallback(); }
</script>
<template>
<div v-if="fallbackAvailable || fallbackError" class="browser-fallback">
        <button
          v-if="fallbackAvailable"
          class="browser-fallback__button"
          type="button"
          :disabled="fallbackBusy"
          @click="useSystemBrowserFallback"
        >{{ fallbackBusy ? "正在切换…" : "使用系统浏览器登录" }}</button>
        <p v-if="fallbackError" role="alert">{{ fallbackError }}</p>
      </div>
</template>
<style scoped>
.browser-fallback { display:flex; align-items:center; gap:var(--space-2); }
.browser-fallback__button { border:1px solid var(--border-subtle); border-radius:var(--radius-sm); padding:7px 12px; background:var(--surface-panel); color:var(--text-primary); font:inherit; cursor:pointer; }
.browser-fallback__button:disabled { opacity:.6; cursor:wait; }
.browser-fallback p { margin:0; color:var(--state-danger); font-size: var(--font-size-caption); }
</style>
