<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useAttrs, watch } from "vue";

// 安全边界（2026-10-09 Area F）：本组件只接受「是否已存密码」的布尔（占位文案二态），
// 不存在任何读取已存密码值的通道；眼睛仅作用于本次输入的值（modelValue 为空即禁用）。
defineOptions({ inheritAttrs: false });
const props = withDefaults(defineProps<{
  modelValue: string;
  label?: string;
  disabled?: boolean;
  hasSaved?: boolean;
  compact?: boolean;
}>(), { label: "密码", disabled: false, hasSaved: false, compact: false });
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const attrs = useAttrs();
const inputAttrs = computed(() => Object.fromEntries(Object.entries(attrs).filter(([key]) => !["class", "style", "placeholder", "type"].includes(key))));
const placeholder = computed(() => (attrs.placeholder as string | undefined) ?? (props.hasSaved ? "输入新密码以修改" : "请输入密码"));
const holding = ref(false);
const visible = computed(() => holding.value && !!props.modelValue);

function conceal(): void {
  holding.value = false;
}

function press(): void {
  if (props.disabled || holding.value || !props.modelValue) return;
  holding.value = true;
}

function pointerDown(event: PointerEvent): void {
  if (event.button !== 0) return;
  event.preventDefault();
  press();
}
function keyDown(event: KeyboardEvent): void {
  if (event.key !== " " && event.key !== "Enter") return;
  event.preventDefault();
  press();
}
function keyUp(event: KeyboardEvent): void {
  if (event.key === " " || event.key === "Enter" || event.key === "Escape") conceal();
}
function onInput(event: Event): void {
  conceal();
  emit("update:modelValue", (event.target as HTMLInputElement).value);
}
function onVisibility(): void { if (document.hidden) conceal(); }
watch(() => [props.disabled, props.modelValue], conceal);
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
    <input v-bind="inputAttrs" :value="modelValue" :type="visible ? 'text' : 'password'" :aria-label="label" :placeholder="placeholder"
      :disabled="disabled" :autocomplete="(attrs.autocomplete as string) ?? 'new-password'" spellcheck="false"
      @input="onInput" @blur="conceal">
    <button type="button" class="password-field__eye" :disabled="disabled || !modelValue"
      aria-label="按住显示密码" title="按住显示密码（仅本次输入）"
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
/* 单眼保证（Area F/E）：隐藏 WebView2 原生密码显示/清除按钮，避免与自绘眼睛并列。 */
.password-field input::-ms-reveal,
.password-field input::-ms-clear { display:none; }
.password-field__eye { position:absolute; top:1px; right:1px; bottom:1px; display:grid; place-items:center; width:32px; min-width:0; min-height:0; padding:0; border:0; border-radius:var(--radius-md); background:transparent; color:var(--text-secondary); touch-action:none; cursor:pointer; }
.password-field__eye:hover:not(:disabled) { color:var(--text-primary); background:var(--surface-subtle); }
.password-field__eye:disabled { opacity:.45; cursor:default; }
.password-field__eye svg { width:17px; height:17px; stroke:currentColor; stroke-width:1.65; stroke-linecap:round; stroke-linejoin:round; }
.password-field--compact input { height:32px; min-height:32px; padding:0 30px 0 7px; border-radius:6px; font-size: var(--font-size-body); }
.password-field--compact .password-field__eye { width:28px; }
</style>
