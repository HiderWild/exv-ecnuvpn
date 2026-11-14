import { ref, type Ref } from "vue";

import type { ConnectPhase, RuntimeSnapshot, ServiceControlAction, ServiceControlReply } from "../lib/ipc";
import { CONNECT_PHASE_ORDER, phaseIndexOf } from "../lib/status-map";
import { present } from "./presenter";
import type { ProductRuntime } from "./runtime";
import type { ProductStatus, ProductUiState } from "./types";

export type RuntimeSource = "real" | "mock";
export type PreviewScenario = "missing-sample" | "reconnecting" | "unknown-phase" | "cleanup-failed" | "inflight" | "compact-long"
  | "long-error" | "service-missing" | "browser-fallback" | "core-stopped";
export type PreviewPage = "connect" | "logs" | "settings" | "about";

/** 预览只能由开发环境中的明确 query 启用，生产地址永远保持真实数据源。 */
export function selectRuntimeSource(dev: boolean, search: string): RuntimeSource {
  return dev && new URLSearchParams(search).get("preview") === "1" ? "mock" : "real";
}

/** 动画验收控件沿用双门，额外要求明确 scene，不进入正式产品页面。 */
export function selectProgressPreview(dev: boolean, search: string): boolean {
  return selectRuntimeSource(dev, search) === "mock" && new URLSearchParams(search).get("scene") === "progress";
}

export interface MockProductRuntime extends ProductRuntime {
  /** 仅替换内存样本，不派发连接或认证动作；不属于 ProductRuntime 业务接口。 */
  showPreview(status: ProductStatus, phase?: ConnectPhase, scenario?: PreviewScenario): void;
}

/** 页面选择与模拟源共用开发门；真实启动始终从连接页进入。 */
export function selectPreviewPage(dev: boolean, search: string): PreviewPage {
  if (selectRuntimeSource(dev, search) !== "mock") return "connect";
  const page = new URLSearchParams(search).get("page");
  return page === "logs" || page === "settings" || page === "about" ? page : "connect";
}

/** 复现各视觉状态只作用于明确标记的开发预览，不进入生产数据链。 */
export function selectPreviewStatus(dev: boolean, search: string): ProductStatus {
  if (selectRuntimeSource(dev, search) !== "mock") return "idle";
  const status = new URLSearchParams(search).get("state");
  return ["idle", "connecting", "awaiting", "connected", "stopping", "reconciling", "failed"].includes(status ?? "")
    ? status as ProductStatus : "idle";
}

/** 额外边界只在明确开发预览中生效，仍由已有 presenter 消费模拟快照。 */
export function selectPreviewScenario(dev: boolean, search: string): PreviewScenario | null {
  if (selectRuntimeSource(dev, search) !== "mock") return null;
  const scenario = new URLSearchParams(search).get("scenario");
  return scenario === "missing-sample" || scenario === "reconnecting" || scenario === "unknown-phase" || scenario === "cleanup-failed" || scenario === "inflight" || scenario === "compact-long"
    || scenario === "long-error" || scenario === "service-missing" || scenario === "browser-fallback" || scenario === "core-stopped" ? scenario : null;
}

/** 阶段 query 只接受现有枚举；未知阶段由显式 unknown-phase 场景单独模拟。 */
export function selectPreviewPhase(dev: boolean, search: string): ConnectPhase | null {
  if (selectRuntimeSource(dev, search) !== "mock") return null;
  const phase = new URLSearchParams(search).get("phase");
  return CONNECT_PHASE_ORDER.includes(phase as ConnectPhase) ? phase as ConnectPhase : null;
}

export const PREVIEW_MODALS = ["credentials", "auth-failure", "auth-invalidated", "auth-wait", "service", "resolver", "overlay", "quick", "quick-retry", "routes"] as const;
export type PreviewModal = typeof PREVIEW_MODALS[number];
/** 只接本宿主已存在的生产模态，不新增宿主能力。 */
export function selectPreviewModal(dev: boolean, search: string): PreviewModal | null {
  if (selectRuntimeSource(dev, search) !== "mock") return null;
  const modal = new URLSearchParams(search).get("modal");
  return PREVIEW_MODALS.includes(modal as PreviewModal) ? modal as PreviewModal : null;
}

function scenarioSnapshot(scenario: PreviewScenario | null, status: ProductStatus, nowMs: number): RuntimeSnapshot {
  if (scenario === "service-missing") return { ...mockSnapshot("idle", nowMs), service_status: { installed: false, state: "stopped", health_state: "not_installed" } };
  if (scenario === "browser-fallback") return mockSnapshot("awaiting", nowMs);
  if (scenario === "core-stopped") return { ...mockSnapshot("idle", nowMs), mode: "unknown" };
  if (scenario === "long-error") return {
    ...mockSnapshot("failed", nowMs),
    runtime: { state: "failed_clean", error: { code: "PREVIEW_LONG_ERROR", message: "这是开发预览中的长错误说明：连接请求已经到达认证服务，但服务在建立安全隧道时未能完成本次请求，当前连接已结束。请先确认校园账户与 VPN 服务器设置，再重新连接；如果需要浏览器确认，请完成登录后返回应用。反复失败时可通过查看日志获取完整诊断，并导出日志反馈。此说明用于检查矮窗口中的换行、内部滚动和恢复入口，所有内容均为模拟，不代表正式连接结果。" } },
  };
  if (scenario === "compact-long") return { ...mockSnapshot("connected", nowMs), runtime: { state: "connected", session_established_at_ms: nowMs - 360_061_000, summary: null } };
  if (scenario === "missing-sample") return { ...mockSnapshot("connected", nowMs), stats: null };
  if (scenario === "reconnecting") return { ...mockSnapshot("connecting", nowMs), reconnect: { auto_reconnect: true, max_attempts: 3, current_attempt: 2, active: true } };
  // 仅模拟未来 wire 相位，验证呈现回退；不扩展正式协议或阶段枚举。
  if (scenario === "unknown-phase") return { ...mockSnapshot("connecting", nowMs), runtime: { state: "connecting", phase: "future_phase" as ConnectPhase, phase_index: -1 } };
  if (scenario === "cleanup-failed") return { ...mockSnapshot("failed", nowMs), runtime: { state: "failed_dirty", has_obligation: true, error: { code: "ERROR_CODE_OBSERVED_CONFLICT", stage: "ERROR_STAGE_TEARDOWN", message: "" } } };
  return mockSnapshot(scenario === "inflight" ? "idle" : status, nowMs);
}

function mockSnapshot(status: ProductStatus, nowMs: number): RuntimeSnapshot {
  const base = {
    monotonic_tick: 1,
    operation_id: null,
    proxy_tun: null,
    service_status: null,
    mode: "auto",
    reconnect: status === "connected"
      ? { auto_reconnect: true, max_attempts: 3, current_attempt: 0, active: false }
      : null,
  };

  switch (status) {
    case "connecting":
      return {
        ...base,
        runtime: { state: "connecting", phase: "connecting_control", phase_index: 2 },
        stats: null,
      };
    case "awaiting":
      return {
        ...base,
        runtime: { state: "awaiting_interaction", prompt_deadline_ms: null },
        stats: null,
      };
    case "connected":
      return {
        ...base,
        runtime: { state: "connected", session_established_at_ms: nowMs - 61_000, summary: null },
        stats: {
          rx_bytes: 4_096,
          tx_bytes: 2_048,
          rx_rate_bps: 2_048,
          tx_rate_bps: 1_024,
          latency_ms: 24,
          phase: "connected",
          engine_sequence: 1,
          sample_tick: 1,
        },
      };
    case "stopping":
      return { ...base, runtime: { state: "stopping", reason: null }, stats: null };
    case "reconciling":
      return {
        ...base,
        runtime: {
          state: "reconciling",
          blocking_error: { code: "ERROR_CODE_OBSERVED_CONFLICT", message: "" },
        },
        stats: null,
      };
    case "failed":
      return {
        ...base,
        runtime: { state: "failed_clean", error: { code: "ERROR_CODE_DEADLINE_EXCEEDED", message: "" } },
        stats: null,
      };
    case "idle":
    default:
      return { ...base, runtime: { state: "idle", last_cleanup_at_ms: null }, stats: null };
  }
}

/**
 * 浏览器预览专用的同构运行时。它不导入 Tauri API，也不假称真实业务数据。
 * `source: 'mock'` 由 main.ts 的开发预览标识显式呈现给设计验收者。
 */
export function createMockRuntime(
  initialStatus: ProductStatus = "idle",
  now: () => number = Date.now,
  scenario: PreviewScenario | null = null,
  phase: ConnectPhase | null = null,
): MockProductRuntime {
  let currentSnapshot = scenarioSnapshot(scenario, initialStatus, now());
  if (phase && currentSnapshot.runtime.state === "connecting") {
    currentSnapshot = { ...currentSnapshot, runtime: { state: "connecting", phase, phase_index: phaseIndexOf(phase) } };
  }
  let previewRevision = 0;
  let finishFlight: (() => void) | null = null;
  const previewInfo = {
    protocol: "cisco",
    routeRuleCount: 2,
    account: scenario === "compact-long" ? null : "preview-user",
    ...(scenario === "compact-long" ? { accountSource: "unavailable" as const } : {}),
    vpnServer: scenario === "compact-long" ? "very-long-preview-vpn-server.example.edu:8443" : "vpn.preview.example",
    campusIp: "10.88.88.5",
  } as const;
  const initial = present(currentSnapshot, now(), previewInfo, scenario === "core-stopped" ? "stopped" : "normal");
  const state: Ref<ProductUiState> = ref(initial);

  function setStatus(status: ProductStatus): void {
    currentSnapshot = mockSnapshot(status, now());
    state.value = present(currentSnapshot, now(), previewInfo);
  }

  return {
    source: "mock",
    state,
    showPreview(status, phase, previewScenario) {
      finishFlight?.();
      finishFlight = null;
      currentSnapshot = scenarioSnapshot(previewScenario ?? null, status, now());
      if (status === "connecting" && phase) {
        currentSnapshot = { ...currentSnapshot, runtime:{ state:"connecting", phase, phase_index:phaseIndexOf(phase) } };
      }
      currentSnapshot = { ...currentSnapshot, monotonic_tick: ++previewRevision,
        operation_id: previewRevision.toString(16).padStart(32, "0") };
      state.value = present(currentSnapshot, now(), previewInfo, previewScenario === "core-stopped" ? "stopped" : "normal");
    },
    async start(): Promise<void> {},
    async configurationChanged(): Promise<void> {},
    async connect(): Promise<void> {
      setStatus("connecting");
      if (scenario === "inflight") await new Promise<void>(resolve => { finishFlight = resolve; });
    },
    async stop(): Promise<void> {
      setStatus("idle");
      finishFlight?.();
      finishFlight = null;
    },
    async triggerLatencyRefresh(): Promise<void> {
      // 模拟运行时不制造虚假延迟样本；保持现有快照（延迟经真实 snapshot 到达）。
    },
    async serviceControl(_action: ServiceControlAction): Promise<ServiceControlReply> {
      // 模拟模式不伪装真实 SCM 操作；返回占位 reply（界面明确「模拟」）。
      return { service_status: null, ok: false, message: "模拟模式不执行真实服务控制。" };
    },
    dispose(): void { finishFlight?.(); finishFlight = null; },
  };
}
