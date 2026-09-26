<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useAttrs, watch } from "vue";

defineOptions({ inheritAttrs: false });
const props = withDefaults(defineProps<{
  modelValue: string;
  label?: string;
  disabled?: boolean;
  stored?: boolean;
  revealStored?: () => Promise<string | null>;
  compact?: boolean;
  identity?: string;
}>(), { label: "密码", disabled: false, stored: false, compact: false });
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const attrs = useAttrs();
const inputAttrs = computed(() => Object.fromEntries(Object.entries(attrs).filter(([key]) => !["class", "style", "placeholder", "type"].includes(key))));
const holding = ref(false);
const revealed = ref("");
const revealFailed = ref(false);
let requestId = 0;
const saved = computed(() => props.stored && !props.modelValue);
const visible = computed(() => holding.value && (!!props.modelValue || !!revealed.value));
const displayValue = computed(() => saved.value ? (visible.value ? revealed.value : "") : props.modelValue);

function conceal(): void {
  holding.value = false;
  revealed.value = "";
  requestId += 1;
}

async function reveal(): Promise<void> {
  if (props.disabled || holding.value || (!props.modelValue && !props.stored)) return;
  holding.value = true;
  revealFailed.value = false;
  if (!saved.value || !props.revealStored) return;
  const current = ++requestId;
  try {
    const value = await props.revealStored();
    if (current === requestId && holding.value && !props.disabled && saved.value) {
      revealed.value = value ?? "";
      revealFailed.value = !value;
    }
  } catch {
    if (current === requestId) revealFailed.value = true;
  }
}

function pointerDown(event: PointerEvent): void {
  if (event.button !== 0) return;
  event.preventDefault();
  void reveal();
}
function keyDown(event: KeyboardEvent): void {
  if (event.key !== " " && event.key !== "Enter") return;
  event.preventDefault();
  void reveal();
}
function keyUp(event: KeyboardEvent): void {
  if (event.key === " " || event.key === "Enter" || event.key === "Escape") conceal();
}
function onInput(event: Event): void {
  conceal();
  emit("update:modelValue", (event.target as HTMLInputElement).value);
}
function onVisibility(): void { if (document.hidden) conceal(); }
watch(() => [props.disabled, props.stored, props.modelValue, props.identity], conceal);
onMounted(() => {
  window.addEventListener("blur", conceal);
  window.addEventListener("pointerup", conceal);
  window.addEventListener("pointercancel", conceal);
  window.addEventListener("keyup", keyUp);
  document.addEventListener("visibilitychange", onVisibility);
});
onBeforeUnmount(() => {
  conceal();
  window.removeEventListener("blur", conceal);
  window.removeEventListener("pointerup", conceal);
  window.removeEventListener("pointercancel", conceal);
  window.removeEventListener("keyup", keyUp);
  document.removeEventListener("visibilitychange", onVisibility);
});
</script>

<template>
  <span class="password-field" :class="[attrs.class, { 'password-field--compact': compact }]" :style="attrs.style as any">
    <input v-bind="inputAttrs" :value="displayValue" :type="visible ? 'text' : 'password'" :aria-label="label"
      :disabled="disabled" :readonly="saved && visible" :autocomplete="(attrs.autocomplete as string) ?? 'current-password'" spellcheck="false"
      @input="onInput" @blur="conceal">
    <span v-if="saved && !visible" class="password-field__saved" aria-hidden="true">••••••••</span>
    <button type="button" class="password-field__eye" :disabled="disabled || (!modelValue && !stored)"
      :aria-label="revealFailed ? '无法读取已保存密码' : '按住显示密码'"
      :title="revealFailed ? '无法读取已保存密码，请重新输入' : '按住显示密码'"
      @pointerdown="pointerDown" @pointerup="conceal" @pointercancel="conceal" @pointerleave="conceal"
      @keydown="keyDown" @keyup="keyUp" @blur="conceal" @click.prevent>
      <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" focusable="false">
        <path d="M2.5 12s3.4-6 9.5-6 9.5 6 9.5 6-3.4 6-9.5 6-9.5-6-9.5-6Z" />
        <circle cx="12" cy="12" r="2.5" />
      </svg>
    </button>
  </span>
</template>

<style scoped>
.password-field { position:relative; display:inline-flex; width:100%; min-width:0; vertical-align:middle; }
.password-field input { box-sizing:border-box; width:100%; min-width:0; min-height:34px; padding:6px 36px 6px 10px; border:1px solid var(--border-strong); border-radius:var(--radius-md); background:var(--surface-raised); color:var(--text-primary); font:inherit; }
.password-field input:focus-visible { outline:2px solid var(--accent); outline-offset:1px; }
.password-field__saved { position:absolute; left:11px; top:50%; transform:translateY(-50%); pointer-events:none; color:var(--text-primary); letter-spacing:1px; }
.password-field__eye { position:absolute; top:1px; right:1px; bottom:1px; display:grid; place-items:center; width:32px; min-width:0; min-height:0; padding:0; border:0; border-radius:var(--radius-md); background:transparent; color:var(--text-secondary); touch-action:none; cursor:pointer; }
.password-field__eye:hover:not(:disabled) { color:var(--text-primary); background:var(--surface-subtle); }
.password-field__eye:disabled { opacity:.45; cursor:default; }
.password-field__eye svg { width:17px; height:17px; stroke:currentColor; stroke-width:1.65; stroke-linecap:round; stroke-linejoin:round; }
.password-field--compact input { height:32px; min-height:32px; padding:0 30px 0 7px; border-radius:6px; font-size:12px; }
.password-field--compact .password-field__saved { left:8px; }
.password-field--compact .password-field__eye { width:28px; }
</style>
