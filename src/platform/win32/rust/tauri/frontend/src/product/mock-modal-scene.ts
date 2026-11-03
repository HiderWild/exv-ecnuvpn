export const PREVIEW_MODALS = ["credentials", "auth-failure", "auth-wait", "auth-auto", "auth-error", "service", "config", "core", "overlay", "quick", "quick-retry", "routes"] as const;
export type PreviewModal = typeof PREVIEW_MODALS[number];
export function selectPreviewModal(dev: boolean, search: string): PreviewModal | null {
  const params = new URLSearchParams(search);
  if (!dev || params.get("preview") !== "1") return null;
  const modal = params.get("modal");
  return PREVIEW_MODALS.includes(modal as PreviewModal) ? modal as PreviewModal : null;
}
