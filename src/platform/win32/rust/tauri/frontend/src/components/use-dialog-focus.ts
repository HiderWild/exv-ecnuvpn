import { nextTick, onBeforeUnmount, ref, watch } from "vue";

const returnTargets = new WeakMap<HTMLElement, HTMLElement | null>();
const controls = 'button, input, select, textarea, a[href], [tabindex], [contenteditable="true"]';

function visibleTarget(element: HTMLElement): boolean {
  if (!element.isConnected || element.matches(":disabled")) return false;
  for (let node: HTMLElement | null = element; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (node.hidden || node.hasAttribute("inert") || style.display === "none"
      || style.visibility === "hidden" || style.visibility === "collapse") return false;
  }
  return true;
}

/** 共用键盘边界；关闭仍委托各模态自己的 busy／取消语义。 */
export function useDialogFocus(visible: () => boolean, requestClose: () => void) {
  const dialog = ref<HTMLElement | null>(null);
  let previous: HTMLElement | null = null;
  let revision = 0;

  function focusableElements(root: HTMLElement): HTMLElement[] {
    return Array.from(root.querySelectorAll<HTMLElement>(controls))
      .filter(element => element.tabIndex >= 0 && visibleTarget(element));
  }

  function onDialogKeydown(event: KeyboardEvent): void {
    const root = dialog.value;
    if (!root || !visible() || event.defaultPrevented) return;
    // 嵌套对话框有自己的关闭边界，不能冒泡关闭整个编辑器。
    const owner = event.target instanceof Element ? event.target.closest('[role="dialog"], [role="alertdialog"]') : null;
    if (owner !== root) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      requestClose();
      return;
    }
    if (event.key !== "Tab") return;
    const elements = focusableElements(root);
    const first = elements[0];
    const last = elements.at(-1);
    if (!first || !last) {
      event.preventDefault();
      root.focus();
    } else if (event.shiftKey && (document.activeElement === first || document.activeElement === root)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === root)) {
      event.preventDefault();
      first.focus();
    }
  }

  watch(visible, async open => {
    const currentRevision = ++revision;
    if (open) {
      const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const owner = active?.closest<HTMLElement>('[role="dialog"], [role="alertdialog"]');
      await nextTick();
      if (currentRevision !== revision || !visible() || !dialog.value) return;
      previous = owner && !owner.contains(dialog.value) && returnTargets.has(owner)
        ? returnTargets.get(owner)! : active;
      returnTargets.set(dialog.value, previous);
      (focusableElements(dialog.value)[0] ?? dialog.value).focus();
      return;
    }
    const closing = dialog.value;
    const ownedFocus = closing?.contains(document.activeElement) ?? false;
    const target = previous;
    previous = null;
    await nextTick();
    // 模式控件或新模态已取得焦点时不抢回；隐藏／已卸载的旧页面也不恢复。
    if (currentRevision !== revision || !ownedFocus || !target || !visibleTarget(target)) return;
    if (document.activeElement === document.body || closing?.contains(document.activeElement)) target.focus();
  }, { immediate: true });

  onBeforeUnmount(() => { revision += 1; previous = null; });
  return { dialog, onDialogKeydown };
}
