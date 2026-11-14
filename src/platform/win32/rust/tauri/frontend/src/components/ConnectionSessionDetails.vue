<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { ProductUiState } from "../product/types";

const props = defineProps<{ state: ProductUiState; expanded: boolean }>();
const emit = defineEmits<{ "return-focus": [] }>();
const panel = ref<HTMLElement | null>(null);
// 在模板更新 inert/aria-hidden 前读取原焦点；详情外的焦点不受退出影响。
watch(() => props.expanded, (expanded, wasExpanded) => {
  if (wasExpanded && !expanded && panel.value?.contains(document.activeElement)) emit("return-focus");
});
// 收起时保留活动会话的最后一帧；新空闲配置不能改写正在退出的会话。
const session = computed<ProductUiState>((previous) => props.expanded ? props.state : previous ?? props.state);
const metrics = computed(() => session.value.metrics?.availability === "available" ? session.value.metrics : null);
const account = computed(() => session.value.connectionInfo.account?.trim() || (session.value.connectionInfo.accountSource === "unavailable"
  ? "认证服务未返回账户信息" : "—"));
const protocol = computed(() => {
  const value = session.value.connectionInfo.protocol;
  return value === "cisco" ? "Cisco" : value === "atrust" ? "aTrust" : "—";
});
const mode = computed(() => ({ auto: "自动", service: "服务", oneshot: "一次性", unknown: "未知" })[session.value.service.mode]);
const reconnect = computed(() => session.value.reconnect.active
  ? `重连中 · 第 ${session.value.reconnect.currentAttempt} 次`
  : `已重连 ${session.value.reconnect.currentAttempt} 次`);

function display(value: string | null | undefined): string {
  return value?.trim() ? value : "—";
}

const columns = computed(() => [
  { title: "会话", rows: [
    { label: "在线时长", value: display(session.value.metrics?.online), id: "session-online" },
    { label: "用户名", value: account.value, id: "session-account" },
    ...(session.value.status === 'connected' && (session.value.reconnect.enabled || session.value.reconnect.active)
      ? [{ label: "自动重连", value: reconnect.value, id: "reconnect-status" }] : []),
  ] },
  { title: "网络", rows: [
    { label: "服务器", value: display(session.value.connectionInfo.vpnServer), id: "session-server" },
    { label: "校内地址", value: display(session.value.connectionInfo.campusIp), id: "session-campus-ip" },
    { label: "协议 / 执行模式", value: `${protocol.value} / ${mode.value}`, id: "session-channel" },
    { label: "路由规则", value: session.value.connectionInfo.routeRuleCount == null ? "—" : `${session.value.connectionInfo.routeRuleCount} 条`, id: "session-routes" },
  ] },
  { title: "流量", id: "session-traffic", rows: [
    { label: "上传速度", value: display(metrics.value?.uploadRate), id: "session-upload-rate" },
    { label: "下载速度", value: display(metrics.value?.downloadRate), id: "session-download-rate" },
    { label: "累计上传", value: display(metrics.value?.uploadTotal), id: "session-upload-total" },
    { label: "累计下载", value: display(metrics.value?.downloadTotal), id: "session-download-total" },
  ] },
]);
</script>

<template>
  <section
    ref="panel"
    class="session-details"
    data-testid="connection-session-details"
    aria-label="连接详情"
    :data-expanded="expanded"
    :aria-hidden="!expanded"
    :inert="!expanded"
  >
    <div class="session-details__clip">
      <div class="session-details__body">
        <div v-for="column in columns" :key="column.title" class="session-details__column" :data-testid="column.id">
          <h2>{{ column.title }}</h2>
          <dl>
            <div v-for="row in column.rows" :key="row.id">
              <dt>{{ row.label }}</dt>
              <dd :data-testid="row.id" :title="row.value" :aria-label="`${row.label}：${row.value}`" tabindex="0">
                <template v-if="row.id === 'session-channel'"><span data-testid="session-protocol">{{ protocol }}</span> / <span data-testid="session-mode">{{ mode }}</span></template>
                <template v-else>{{ row.value }}</template>
              </dd>
            </div>
          </dl>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.session-details { flex:none; display:grid; grid-template-rows:0fr; min-width:0; opacity:0; transition:grid-template-rows 360ms cubic-bezier(.22,.7,.2,1),opacity 240ms ease; }
.session-details[data-expanded="true"] { grid-template-rows:1fr; opacity:1; }
.session-details__clip { min-height:0; overflow:hidden; }
.session-details__body { display:grid; grid-template-columns:minmax(0,1fr) minmax(0,1.2fr) minmax(0,1fr); gap:20px; padding:16px 20px 18px; border-top:1px solid var(--border-subtle); background:var(--surface-canvas); }
.session-details__column { min-width:0; }
h2 { margin:0 0 10px; color:var(--text-primary); font-size: var(--font-size-heading); line-height:1.6; font-weight:600; }
dl { display:grid; gap:7px; margin:0; }
dl > div { display:grid; grid-template-columns:auto minmax(0,1fr); gap:10px; align-items:baseline; font-size: var(--font-size-caption); line-height:1.5; }
dt { color:var(--text-secondary); white-space:nowrap; }
dd { min-width:0; margin:0; text-align:right; color:var(--text-primary); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-variant-numeric:tabular-nums; }
dd:focus-visible { outline:2px solid var(--focus-ring); outline-offset:-2px; white-space:normal; overflow-wrap:anywhere; }
@container connection-content (max-height:420px) {
  .session-details__body { padding:10px 14px; gap:14px; }
  h2 { margin-bottom:6px; }
  dl { gap:4px; }
  dl > div { font-size: var(--font-size-caption); gap:6px; }
}
@container connection-content (max-width:420px) {
  .session-details__body { padding:12px; gap:12px; }
  dl > div { grid-template-columns:minmax(0,1fr); gap:1px; }
  dd { text-align:left; }
}
:global(:root[data-motion="reduced"] .session-details) { transition:none; }
@media (prefers-reduced-motion:reduce) {
  :global(:root:not([data-motion]) .session-details) { transition:none; }
}
</style>
