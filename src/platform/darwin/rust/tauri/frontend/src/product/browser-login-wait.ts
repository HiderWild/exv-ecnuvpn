import type { BrowserLoginEvent, BrowserLoginMilestone } from "../lib/ipc";

/** 过滤认证等待事件并保留本次恢复提示；提交和取消所有权始终在宿主连接请求。 */
export function createBrowserLoginWait() {
  let latest = -1;
  let retired = -1;
  let ticketRefreshReason: BrowserLoginEvent["ticket_refresh_reason"];
  let milestone: BrowserLoginMilestone | undefined;
  let recovery: BrowserLoginEvent["recovery"];
  return {
    get ticketRefreshReason() { return ticketRefreshReason; },
    get milestone() { return milestone; },
    get generation() { return latest; },
    get retired() { return retired; },
    get recovery() { return recovery; },
    accept(event: BrowserLoginEvent): boolean {
      if (!Number.isSafeInteger(event.generation) || event.generation < 0 || event.generation < latest) return false;
      // S3 终局事件豁免 retire 判定：本地取消（retire 已前置）后的终局事件仍可
      // 进入，供宿主收口；活动事件仍受 retire 拒收，旧代终局由
      // 「终局必须等于已见最新代」与宿主事件门拦截。
      if (event.active && event.generation <= retired) return false;
      if (event.outcome !== undefined && event.generation !== latest) return false;
      if (event.generation !== latest) recovery = event.active && event.recovery ? { ...event.recovery } : undefined;
      else if (event.active && event.channel_override && event.recovery) recovery = { ...event.recovery };
      if (!event.active || event.generation !== latest || event.milestone === "awaiting_manual" || event.channel_override) ticketRefreshReason = undefined;
      if (event.active && event.milestone !== "awaiting_manual" && event.ticket_refresh_reason) ticketRefreshReason = event.ticket_refresh_reason;
      // 只保留同一请求的恢复提示；真实人工阶段、新请求及终结按原事件显示。
      const keepRecovery = event.generation === latest && milestone === "recovering"
        && event.milestone !== "awaiting_manual";
      milestone = event.active ? (keepRecovery ? "recovering" : event.milestone) : undefined;
      latest = event.generation;
      if (!event.active) retired = Math.max(retired, event.generation);
      return true;
    },
    retire(): void { retired = Math.max(retired, latest); milestone = undefined; recovery = undefined; ticketRefreshReason = undefined; },
  };
}
