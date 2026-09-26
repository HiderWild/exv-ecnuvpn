<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref, useId } from "vue";

const props = defineProps<{
  label: string;
  description?: string;
}>();

const helpOpen = ref(false);
const helpButton = ref<HTMLButtonElement | null>(null);
const popover = ref<HTMLElement | null>(null);
const helpId = `settings-help-${useId()}`;
const position = ref({ left: "0px", top: "0px" });
let hoverTimer: ReturnType<typeof setTimeout> | undefined;
let closeTimer: ReturnType<typeof setTimeout> | undefined;
let pinned = false;

function clearTimers() { clearTimeout(hoverTimer); clearTimeout(closeTimer); }
function placeHelp() {
  if (!helpButton.value || !popover.value) return;
  const anchor = helpButton.value.getBoundingClientRect();
  const panel = popover.value.getBoundingClientRect();
  const left = Math.max(12, Math.min(anchor.left - 12, window.innerWidth - panel.width - 12));
  const below = anchor.bottom + 8;
  const top = below + panel.height <= window.innerHeight - 12 ? below : Math.max(12, anchor.top - panel.height - 8);
  position.value = { left: `${left}px`, top: `${top}px` };
}
function outside(event: Event) {
  const target = event.target as Node;
  if (!helpButton.value?.contains(target) && !popover.value?.contains(target)) closeHelp(false);
}
function escape(event: KeyboardEvent) {
  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closeHelp(true); }
}
function showHelp() {
  clearTimers();
  if (!props.description) return;
  helpOpen.value = true;
  document.addEventListener("pointerdown", outside, true);
  document.addEventListener("keydown", escape, true);
  window.addEventListener("resize", placeHelp);
  document.addEventListener("scroll", placeHelp, true);
  void nextTick(placeHelp);
}
function hoverHelp() {
  clearTimers();
  if (!helpOpen.value) hoverTimer = setTimeout(showHelp, 1000);
}
function leaveHelp() {
  clearTimeout(hoverTimer);
  if (!pinned) closeTimer = setTimeout(() => closeHelp(false), 160);
}

function toggleHelp(): void {
  if (helpOpen.value && pinned) closeHelp(false);
  else { pinned = true; showHelp(); }
}

function closeHelp(restoreFocus = true): void {
  clearTimers();
  pinned = false;
  helpOpen.value = false;
  document.removeEventListener("pointerdown", outside, true);
  document.removeEventListener("keydown", escape, true);
  window.removeEventListener("resize", placeHelp);
  document.removeEventListener("scroll", placeHelp, true);
  if (restoreFocus) void nextTick(() => helpButton.value?.focus());
}
onBeforeUnmount(() => closeHelp(false));
</script>

<template>
  <div class="settings-row">
    <div class="settings-row__copy">
      <div class="settings-row__heading">
        <div class="settings-row__label">{{ label }}</div>
        <button
          v-if="props.description"
          ref="helpButton"
          type="button"
          class="settings-row__help"
          data-testid="settings-row-help"
          :aria-expanded="helpOpen"
          :aria-controls="helpId"
          :aria-describedby="helpOpen ? `${helpId}-text` : undefined"
          aria-haspopup="dialog"
          :aria-label="`${label}帮助`"
          @click="toggleHelp"
          @mouseenter="hoverHelp"
          @mouseleave="leaveHelp"
          @keydown.esc.stop.prevent="closeHelp(true)"
        >?</button>
      </div>
      <Teleport to="body">
      <div
        v-if="props.description && helpOpen"
        :id="helpId"
        ref="popover"
        :style="position"
        class="settings-row__help-content"
        data-testid="settings-row-help-content"
        role="dialog"
        :aria-label="`${label}帮助`"
        @mouseenter="clearTimers"
        @mouseleave="leaveHelp"
      >
        <span :id="`${helpId}-text`">{{ description }}</span>
        <button
          type="button"
          class="settings-row__help-close"
          data-testid="settings-row-help-close"
          :aria-label="`关闭${label}帮助`"
          @click="closeHelp(true)"
        >×</button>
      </div>
      </Teleport>
    </div>
    <div class="settings-row__control">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.settings-row {
  display: grid;
  grid-template-columns: minmax(180px, 0.8fr) minmax(240px, 1.2fr);
  gap: var(--space-4);
  align-items: center;
  min-width: 0;
  min-height: 56px;
  padding: 10px 0;

}
.settings-row__copy, .settings-row__control { min-width: 0; }
.settings-row__heading { display: flex; align-items: center; gap: 6px; }
.settings-row__label { overflow-wrap: anywhere; color: var(--text-primary); font-size: 13px; font-weight: 600; line-height: 1.3; }
.settings-row__help {
  display: inline-grid;
  width: 20px;
  height: 20px;
  min-width: 20px;
  min-height: 20px;
  padding: 0;
  place-items: center;
  border: 0;
  border-radius: 50%;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 650;
  cursor: pointer;
}
.settings-row__help:hover, .settings-row__help[aria-expanded="true"] { background: var(--surface-subtle); color: var(--text-primary); }
.settings-row__help:focus-visible, .settings-row__help-close:focus-visible { outline: 2px solid var(--focus-ring); outline-offset: 2px; }
.settings-row__help-content { position:fixed; z-index:80; display:flex; align-items:flex-start; gap:12px; width:max-content; max-width:min(320px,calc(100vw - 24px)); max-height:calc(100vh - 24px); overflow:auto; padding:12px 14px; border:1px solid var(--border-subtle); border-radius:var(--radius-md); box-shadow:var(--shadow-raised); background:var(--surface-panel); color:var(--text-secondary); font-size:13px; line-height:1.6; }
.settings-row__help-content span { min-width: 0; overflow-wrap: anywhere; }
.settings-row__help-close { flex: none; min-height: 22px; padding: 0 6px; border: 0; background: transparent; color: var(--accent); font-size: 11px; cursor: pointer; }
.settings-row__control { display: flex; min-height: 34px; align-items: center; justify-content: flex-end; }
@media (max-width: 700px) {
  .settings-row { grid-template-columns: minmax(0, 1fr) minmax(150px, 0.8fr); gap: var(--space-3); }
}
@media (max-width: 480px) {
  .settings-row { grid-template-columns: 1fr; gap: var(--space-2); padding-block: var(--space-3); }
  .settings-row__control { justify-content: stretch; }
}
</style>
