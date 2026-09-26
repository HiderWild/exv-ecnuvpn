<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useDialogFocus } from "./use-dialog-focus";

import { mergeManyCidr, parseCidr, type MergedCidr } from "../lib/cidr";

/**
 * 可嵌入或由模态窗承载的唯一路由编辑实现。
 * `change` 仅更新调用方草稿；只有设置页容器发出的 `save` 才代表持久化确认。
 */
const props = withDefaults(defineProps<{
  routes: string[];
  showActions?: boolean;
  sideBySide?: boolean;
  busy?: boolean;
}>(), {
  showActions: false,
  sideBySide: false,
  busy: false,
});

const emit = defineEmits<{
  change: [routes: string[]];
  save: [routes: string[]];
  close: [];
}>();

const localRoutes = ref<string[]>([]);
const selectedIndices = ref<number[]>([]);
const draftInput = ref("");
const addFailures = ref<Array<{ value: string; reason: string }>>([]);
const mergeError = ref<string | null>(null);
const confirmState = ref<MergedCidr | null>(null);
const { dialog: addErrorDialog, onDialogKeydown: onAddErrorKeydown } = useDialogFocus(
  () => addFailures.value.length > 0, () => { addFailures.value = []; },
);
const { dialog: mergeErrorDialog, onDialogKeydown: onMergeErrorKeydown } = useDialogFocus(
  () => mergeError.value !== null, () => { mergeError.value = null; },
);
const { dialog: confirmDialog, onDialogKeydown: onConfirmKeydown } = useDialogFocus(
  () => confirmState.value !== null, () => { confirmState.value = null; },
);

function routesMatch(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((route, index) => route === right[index]);
}

watch(
  () => props.routes,
  (routes) => {
    // QuickStart 会把 change 立即回写为新的 props 数组。若内容未变，这只是受控
    // 回显而非重新初始化，不能清掉同一次添加产生的无效/重复项反馈。
    if (routesMatch(routes, localRoutes.value)) return;
    localRoutes.value = [...routes];
    selectedIndices.value = [];
    draftInput.value = "";
    addFailures.value = [];
    mergeError.value = null;
    confirmState.value = null;
  },
  { immediate: true },
);

const selectedCount = computed(() => selectedIndices.value.length);
const ROUTE_CANDIDATE_RE = /(?:\d{1,3}\.){3}\d{1,3}(?:\s*\/\s*[^\s,;]*)?/g;

function cleanCandidate(value: string): string {
  return value.trim().replace(/[()[\]{}<>]+$/g, "");
}

function candidatesForLine(line: string): string[] {
  const matches = line.match(ROUTE_CANDIDATE_RE)?.map(cleanCandidate).filter(Boolean) ?? [];
  if (matches.length > 0) return matches;
  const fallback = cleanCandidate(line);
  return fallback ? [fallback] : [];
}

const mergePreview = computed<MergedCidr | null>(() => {
  if (selectedIndices.value.length < 2) return null;
  const parsed = selectedIndices.value.map((index) => parseCidr(localRoutes.value[index]));
  if (parsed.some((entry) => entry === null)) return null;
  return mergeManyCidr(parsed as NonNullable<(typeof parsed)[number]>[]);
});

function publishChange(): void {
  emit("change", [...localRoutes.value]);
}

function toggleSelect(index: number): void {
  if (props.busy) return;
  selectedIndices.value = selectedIndices.value.includes(index)
    ? selectedIndices.value.filter((item) => item !== index)
    : [...selectedIndices.value, index];
  mergeError.value = null;
}

function addRoute(): void {
  if (props.busy) return;
  if (draftInput.value.trim() === "") return;
  const failures: Array<{ value: string; reason: string }> = [];
  const valid: string[] = [];
  const known = new Set(localRoutes.value);

  for (const line of draftInput.value.split(/\r?\n/)) {
    for (const candidate of candidatesForLine(line)) {
      const parsed = parseCidr(candidate);
      if (parsed === null) {
        failures.push({ value: candidate, reason: "不是合法的 IP 或 CIDR（例如 10.0.0.0/8 或 10.0.0.1）。" });
      } else if (known.has(candidate) || valid.includes(candidate)) {
        failures.push({ value: candidate, reason: "该路由已存在。" });
      } else {
        known.add(candidate);
        valid.push(candidate);
      }
    }
  }

  if (valid.length > 0) {
    localRoutes.value.push(...valid);
    publishChange();
  }
  draftInput.value = "";
  addFailures.value = failures;
}

function removeRoute(index: number): void {
  if (props.busy) return;
  localRoutes.value.splice(index, 1);
  selectedIndices.value = [];
  mergeError.value = null;
  publishChange();
}

function applyMergeResult(result: MergedCidr): void {
  const indexes = [...selectedIndices.value].sort((a, b) => a - b);
  if (indexes.length < 2) return;
  const next = [...localRoutes.value];
  for (const index of [...indexes].reverse()) next.splice(index, 1);
  next.splice(indexes[0], 0, result.cidr);
  localRoutes.value = next;
  selectedIndices.value = [];
  mergeError.value = null;
  confirmState.value = null;
  publishChange();
}

function requestMerge(): void {
  if (props.busy) return;
  if (selectedIndices.value.length < 2) return;
  const preview = mergePreview.value;
  if (preview === null) {
    mergeError.value = "无法合并：所选条目中存在非法的 IP/CIDR。";
  } else if (preview.prefix < 16) {
    confirmState.value = preview;
  } else {
    applyMergeResult(preview);
  }
}
</script>

<template>
  <div class="routes-editor" :class="{ 'routes-editor--split': sideBySide }" data-testid="routes-editor">
    <div class="routes-workspace">
    <div class="routes-list" data-testid="routes-list">
      <p v-if="localRoutes.length === 0" class="routes-empty" data-testid="routes-empty">暂无路由。</p>
      <label v-for="(route, index) in localRoutes" :key="`${route}-${index}`" class="route-item" :class="{ 'route-item--selected': selectedIndices.includes(index) }" :data-testid="`route-item-${index}`">
        <input type="checkbox" class="route-item__select" :data-testid="`route-select-${index}`" :checked="selectedIndices.includes(index)" :aria-label="`选择路由 ${route}`" :disabled="busy" @change="toggleSelect(index)">
        <span class="route-item__text">{{ route }}</span>
        <button type="button" class="route-item__remove" :data-testid="`route-remove-${index}`" :aria-label="`删除路由 ${route}`" :disabled="busy" @click.prevent="removeRoute(index)">删除</button>
      </label>
    </div>

      <section class="routes-panel routes-add-panel" aria-labelledby="routes-add-title">
        <h3 id="routes-add-title">添加路由</h3>
        <div class="routes-add-composer" data-testid="routes-add-composer">
          <textarea v-model="draftInput" data-testid="routes-add-input" rows="3" autocomplete="off" placeholder="支持多行，每行一条" aria-label="新增路由" :disabled="busy" @keyup.ctrl.enter="addRoute" />
          <button type="button" data-testid="routes-add" :disabled="busy" @click="addRoute">添加</button>
        </div>
      </section>
    </div>
      <section class="routes-panel routes-merge-panel" aria-labelledby="routes-merge-title">
        <h3 id="routes-merge-title">合并路由</h3>
        <div class="routes-merge">
          <span class="routes-selection-count" data-testid="routes-selection-count">已选 {{ selectedCount }} 条</span>
          <span class="routes-merge-preview" data-testid="routes-merge-preview">合并预览：<span v-if="mergePreview">{{ mergePreview.cidr }}</span><span v-else>未选择</span></span>
          <button type="button" class="routes-merge-button" data-testid="routes-merge" title="可选择并合并多条路由；掩码短于 /16 时会再次确认" :disabled="busy || selectedCount < 2" @click="requestMerge">合并</button>
        </div>
      </section>

    <div v-if="showActions" class="modal-actions routes-editor__actions">
      <button type="button" data-testid="routes-cancel" :disabled="busy" @click="emit('close')">取消</button>
      <button type="button" class="routes-save" data-testid="routes-save" :disabled="busy" @click="emit('save', [...localRoutes])">{{ busy ? "保存中…" : "确认修改" }}</button>
    </div>

    <div v-if="addFailures.length" ref="addErrorDialog" tabindex="-1" @keydown="onAddErrorKeydown" class="routes-nested-overlay" data-testid="routes-add-error-modal" role="alertdialog" aria-modal="true" aria-labelledby="routes-add-error-title" @click.self="addFailures = []">
      <div class="routes-nested-modal">
        <h3 id="routes-add-error-title">部分路由未添加</h3>
        <p>以下项目校验失败或已经存在，请修正后重新添加：</p>
        <ul class="routes-failure-list"><li v-for="failure in addFailures" :key="`${failure.value}-${failure.reason}`"><code>{{ failure.value }}</code><span>{{ failure.reason }}</span></li></ul>
        <div class="modal-actions"><button type="button" data-testid="routes-add-error-close" @click="addFailures = []">知道了</button></div>
      </div>
    </div>
    <div v-if="mergeError" ref="mergeErrorDialog" tabindex="-1" @keydown="onMergeErrorKeydown" class="routes-nested-overlay" data-testid="routes-merge-error-modal" role="alertdialog" aria-modal="true" aria-labelledby="routes-merge-error-title" @click.self="mergeError = null">
      <div class="routes-nested-modal"><h3 id="routes-merge-error-title">无法合并路由</h3><p>{{ mergeError }}</p><div class="modal-actions"><button type="button" data-testid="routes-merge-error-close" @click="mergeError = null">知道了</button></div></div>
    </div>
    <div v-if="confirmState" ref="confirmDialog" tabindex="-1" @keydown="onConfirmKeydown" class="routes-nested-overlay" data-testid="routes-confirm" role="alertdialog" aria-modal="true" aria-labelledby="routes-confirm-title" @click.self="confirmState = null">
      <div class="routes-nested-modal"><h3 id="routes-confirm-title">确认合并</h3><p>合并网段过大，可能导致不必要流量流经 VPN 服务器影响体验，是否继续？</p><p class="routes-confirm-result" data-testid="routes-confirm-result">合并结果：{{ confirmState.cidr }}</p><div class="modal-actions"><button type="button" data-testid="routes-confirm-cancel" @click="confirmState = null">取消</button><button type="button" class="routes-save" data-testid="routes-confirm-ok" @click="confirmState !== null && applyMergeResult(confirmState)">继续合并</button></div></div>
    </div>
  </div>
</template>

<style scoped>
.routes-editor { position: relative; display: flex; min-height: 0; flex-direction: column; color: var(--text-primary); }
.routes-list { flex: 1 1 auto; min-height: 0; max-height: min(48vh, 460px); display: grid; gap: var(--space-1); margin-bottom: var(--space-3); overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; }
.routes-list::-webkit-scrollbar { width: 8px; }
.routes-empty { margin: 0; padding: var(--space-2) 0; color: var(--text-secondary); font-size: 13px; }
.route-item { display: flex; align-items: center; gap: var(--space-2); min-height: 34px; padding: var(--space-1) var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); cursor: pointer; }
.route-item--selected { border-color: var(--accent); background: var(--accent-subtle); }
.route-item__select { flex: none; width: 16px; height: 16px; accent-color: var(--accent); }
.route-item__text { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; font-family: var(--font-mono); font-size: 13px; }
.route-item__remove { flex: none; min-height: 26px; padding: 0 var(--space-2); font-size: 12px; color: var(--text-secondary); }
.route-item__remove:hover { color: var(--state-danger); border-color: var(--state-danger); }
.routes-workspace { display:flex; flex-direction:column; min-height:0; }
.routes-editor--split .routes-workspace { display:grid; grid-template-columns:minmax(0,1.15fr) minmax(240px,1fr); gap:var(--space-4); height:clamp(180px,38vh,360px); }
.routes-editor--split .routes-list { margin:0; align-content:start; max-height:none; }
.routes-editor--split .routes-add-composer { flex:1; min-height:0; }
.routes-editor--split .routes-add-composer textarea { height:100%; resize:none; }
.routes-merge-panel { margin-top:var(--space-3); flex:none; }
@media (max-width:560px) {
  .routes-editor--split .routes-workspace { display:flex; height:auto; overflow-y:auto; }
  .routes-editor--split .routes-list { min-height:80px; max-height:24vh; flex:none; }
  .routes-editor--split .routes-add-composer textarea { min-height:110px; }
}
.routes-panel { min-width: 0; padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-subtle); }
.routes-panel h3 { margin: 0 0 var(--space-2); font-size: 13px; }
.routes-add-panel { display: flex; flex-direction: column; gap: var(--space-2); }
.routes-add-composer { position: relative; min-width: 0; }
.routes-add-composer textarea { width: 100%; min-height: 74px; resize: vertical; padding: var(--space-2) var(--space-2) 42px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-panel); color: var(--text-primary); font-family: var(--font-mono); font-size: 13px; }
.routes-add-composer > button { position: absolute; right: var(--space-2); bottom: var(--space-2); min-height: 28px; padding: 0 var(--space-3); }
.routes-merge-panel { display: flex; flex-direction: column; justify-content: center; padding-block: var(--space-2); }
.routes-merge { display: flex; align-items: center; gap: var(--space-2); }
.routes-selection-count { color: var(--text-secondary); font-size: 13px; }
.routes-merge-button { min-height: 32px; padding: 0 var(--space-3); }
.routes-merge-preview { min-width: 0; flex: 1 1 auto; color: var(--state-success); font-family: var(--font-mono); font-size: 12px; line-height: 1.5; overflow-wrap: anywhere; }
.modal-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-4); }
.routes-editor__actions { margin-top: var(--space-3); }
.modal-actions button { min-height: 34px; padding: var(--space-1) var(--space-3); font-size: 13px; }
.modal-actions button:disabled { border-color: var(--border-subtle); background: var(--surface-subtle); color: var(--text-secondary); }
.routes-save { border-color: var(--accent); background: var(--accent); color: var(--accent-on); font-weight: 600; }
.routes-save:hover { border-color: var(--accent-strong); background: var(--accent-strong); }
.routes-nested-overlay { position: absolute; inset: 0; z-index: 2; display: flex; align-items: center; justify-content: center; padding: var(--space-5); background: rgb(0 0 0 / 0.28); }
.routes-nested-modal { width: min(100%, 480px); max-height: min(70vh, 540px); overflow-y: auto; padding: var(--space-5); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: var(--surface-panel); box-shadow: 0 16px 40px rgb(0 0 0 / 0.35); }
.routes-nested-modal h3 { margin: 0 0 var(--space-2); font-size: 16px; }
.routes-nested-modal p { margin: 0; color: var(--text-secondary); font-size: 13px; line-height: 1.5; }
.routes-failure-list { display: grid; gap: var(--space-2); margin: var(--space-3) 0 0; padding-left: var(--space-4); color: var(--text-secondary); font-size: 12px; }
.routes-failure-list li { display: grid; gap: 2px; }
.routes-failure-list code { color: var(--text-primary); font-family: var(--font-mono); overflow-wrap: anywhere; }
.routes-confirm-result { color: var(--text-primary) !important; font-family: var(--font-mono); }
</style>
