<script setup lang="ts">
import type { ConfigReadFailure } from "../lib/ipc";

withDefaults(defineProps<{
  failure: Pick<ConfigReadFailure, "path"> | null;
  busy?: boolean;
  compact?: boolean;
  errorMessage?: string;
}>(), { busy: false, compact: false, errorMessage: "" });
const emit = defineEmits<{ retry: []; openLogs: [] }>();
</script>

<template>
  <section v-if="failure || errorMessage" class="config-read-failure" :class="{ 'config-read-failure--compact': compact }" role="alert" data-testid="config-read-failure-notice">
    <h2>{{ failure ? "无法读取配置" : "配置请求未完成" }}</h2>
    <div class="config-read-failure__body" role="region" aria-label="配置读取失败详情" tabindex="0">
    <template v-if="failure">
    <p>EXV 读取以下配置文件失败，未修改原文件：</p>
    <p class="config-read-failure__path" data-testid="config-read-failure-path">{{ failure.path }}</p>
    <p>请检查文件和上级目录的访问权限后重试；仍无法读取时，可查看日志。</p>
    </template>
    <p v-if="errorMessage" class="config-read-failure__error" role="alert" data-testid="config-read-retry-error">{{ errorMessage }}</p>
    </div>
    <div class="config-read-failure__actions">
      <button type="button" :disabled="busy" :aria-busy="busy" data-testid="config-read-retry" @click="emit('retry')">{{ busy ? '正在重试…' : '重试读取' }}</button>
      <button type="button" class="config-read-failure__logs" data-testid="config-read-logs" @click="emit('openLogs')">查看日志</button>
    </div>
  </section>
</template>

<style scoped>
.config-read-failure { display: flex; flex: none; min-width: 0; min-height: 0; max-height: min(55%, 320px); flex-direction: column; padding: 12px; margin-bottom: 12px; border: 1px solid var(--text-error, #d35b65); border-radius: var(--radius-md); background: var(--surface-panel); color: var(--text-primary); font-size: var(--font-size-body); }
h2 { flex: none; margin: 0 0 8px; font-size: var(--font-size-heading); }
p { margin: 0 0 8px; }
.config-read-failure__path { overflow-wrap: anywhere; white-space: pre-wrap; font-family: monospace; }
.config-read-failure__actions { display: flex; flex: none; flex-wrap: wrap; gap: 8px; }
button { padding: 6px 14px; border: 0; border-radius: var(--radius-md); background: var(--accent); color: var(--accent-on); cursor: pointer; }
button.config-read-failure__logs { background: transparent; color: var(--text-primary); border: 1px solid var(--border-subtle); }
button:disabled { opacity: .5; cursor: default; }
button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.config-read-failure__body { flex: 1 1 auto; min-width: 0; min-height: 1.3em; overflow-y: auto; overscroll-behavior: contain; overflow-wrap: anywhere; }
.config-read-failure__body:focus-visible { outline: 2px solid var(--focus-ring); outline-offset: -2px; }
.config-read-failure--compact { display: flex; flex: 1 1 0; min-height: 0; max-height: 100%; margin: 0; padding: 6px 10px; flex-direction: column; font-size: var(--font-size-caption); line-height: 1.3; }
.config-read-failure--compact h2 { flex: none; margin: 0 0 4px; font-size: var(--font-size-heading); line-height: 1.3; }
.config-read-failure--compact .config-read-failure__body { flex: 1 1 auto; min-height: 1.3em; overflow-y: auto; overscroll-behavior: contain; }
.config-read-failure--compact p { margin-bottom: 4px; }
.config-read-failure--compact .config-read-failure__actions { flex: none; gap: 4px; padding: 4px 4px 6px; margin: 0 -4px -6px; }
.config-read-failure--compact button { min-height: 24px; padding: 2px 7px; font-size: var(--font-size-body); line-height: 1.3; }
</style>
