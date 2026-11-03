import { toRaw } from "vue";
import type { LogEvent } from "../lib/ipc";
import {
  DEFAULT_CONNECTION_MODE,
  DEFAULT_PROTOCOL,
  type CoreConfigGateway,
  type CoreConfigKey,
} from "./core-config";
import type { LogsGateway } from "./logs";
import { DEFAULT_UI_PREFERENCES, type UiPrefsGateway } from "./ui-prefs";

export type PreviewPageFault = "logs-read-failure" | "settings-save-failure" | "credentials-save-failure";

/** 错误视觉场景与真实入口隔离，未知 query 不启用。 */
export function selectPreviewPageFault(dev: boolean, search: string): PreviewPageFault | null {
  const params = new URLSearchParams(search);
  if (!dev || params.get("preview") !== "1") return null;
  const fault = params.get("pageFault");
  return fault === "logs-read-failure" || fault === "settings-save-failure" || fault === "credentials-save-failure" ? fault : null;
}

const failedPreviewLogs = new WeakSet<LogsGateway>();

/** 只释放注册过的模拟故障，不扩展正式 LogsGateway 能力。 */
export function retryPreviewLogsFailure(dev: boolean, search: string, gateway: LogsGateway): void {
  if (selectPreviewPageFault(dev, search) === "logs-read-failure") failedPreviewLogs.delete(toRaw(gateway));
}

const previewConfig: ReadonlyArray<{ key: CoreConfigKey; value: string }> = [
  // 预览保持 Cisco：与旧预览行为一致，协议能力由设置页/快速入门的真实选择覆盖。
  { key: "protocol", value: DEFAULT_PROTOCOL },
  { key: "server", value: "vpn.preview.example" },
  { key: "username", value: "preview-user" },
  { key: "remember_password", value: "false" },
  { key: "connection_mode", value: DEFAULT_CONNECTION_MODE },
  { key: "routes", value: "10.0.0.0/8,172.16.0.0/12" },
  { key: "user_agent", value: "EXV Tauri Preview" },
  { key: "mtu", value: "1420" },
];

/**
 * 仅供开发预览使用的内存配置。生产入口不会创建它，也不会写入本机或 core。
 */
export function createMockCoreConfigGateway(fault: PreviewPageFault | null = null): CoreConfigGateway {
  let rejectCredentials = fault === "credentials-save-failure";
  const values = new Map(previewConfig.map((item) => [item.key, item.value]));

  return {
    async configGet() {
      return [...[...values].filter(([key]) => key !== "password").map(([key, value]) => ({ key, value })),
        { key: "has_stored_password", value: String(values.get("remember_password") === "true" && !!values.get("password")) },
      ];
    },
    async configSet(items) {
      if (rejectCredentials && items.some((item) => item.key === "password" || item.key === "remember_password")) {
        rejectCredentials = false;
        return false;
      }
      const previousUsername = values.get("username");
      for (const item of items) values.set(item.key, item.value);
      if (values.get("username") !== previousUsername && !items.some((item) => item.key === "password" && !!item.value)) values.delete("password");
      if (values.get("remember_password") !== "true") values.delete("password");
      return true;
    },
  };
}

/**
 * 只提供一段静态历史以校验日志布局；不使用定时器或 fake emitter 冒充生产实时流。
 *
 * W1-B（P9）：`logsList` 分页语义与真实 core（经前端桥翻译后）同构——条目
 * 1 基 seq（第 i 条 seq = i）、增量过滤严格 `>`（`seq > after_seq`）、
 * `next_after_seq` = 本分片末条 seq（last-seen；空页钳制不回退，镜像桥的
 * `max(next_seq - 1, after_seq)`）。旧版把 `after_seq` 当 0 基数组下标、把
 * 计数当游标，属于另一套索引方案，无法暴露 off-by-one 回归，已重塑。
 * `append` 是测试注入 seam，手工追加新条目（生产实时流仍只来自轮询与 onLogs）。
 */
export function createMockLogsGateway(
  now: () => number = Date.now,
  fault: PreviewPageFault | null = null,
): LogsGateway & { append(entry: LogEvent): void } {
  const timestampMs = now();
  const history: LogEvent[] = [
    {
      level: "info",
      component: "preview",
      code: "preview.history.loaded",
      message: "预览数据：正式运行时由 core 提供历史日志。",
      fields: {},
      timestamp_ms: timestampMs - 18_000,
    },
    {
      level: "info",
      component: "preview",
      code: "preview.layout.ready",
      message: "该记录仅用于检查日志页的密度与排版。",
      fields: {},
      timestamp_ms: timestampMs - 4_000,
    },
  ];

  const gateway: LogsGateway & { append(entry: LogEvent): void } = {
    async logsList(afterSeq, limit) {
      if (failedPreviewLogs.has(gateway)) throw new Error("模拟日志读取失败；重试后恢复。");
      const cursor = Math.max(0, Math.floor(afterSeq));
      const safeLimit = Math.max(1, Math.floor(limit));
      // after_seq 是 last-seen 游标：<= 0 走初始尾部（最近 limit 条匹配项）；
      // 否则严格 seq > cursor（第 i 条 seq = i ⇒ 0 基下标 >= cursor）。
      const events =
        cursor <= 0
          ? history.slice(-safeLimit)
          : history.slice(cursor, cursor + safeLimit);
      // 本分片末条 seq（last-seen）：尾部页末条即历史末尾；增量页 = 起点 +
      // 本页条数。空页钳制不回退（= max(cursor, last_seq)，镜像前端桥）。
      const lastSeq = history.length;
      const endExclusive = cursor <= 0 ? lastSeq : cursor + events.length;
      const nextAfterSeq = events.length > 0 ? endExclusive : Math.max(cursor, lastSeq);
      return {
        events: [...events],
        next_after_seq: nextAfterSeq,
        has_more: events.length > 0 && events.length >= safeLimit,
      };
    },
    async logsClear() {
      const removedEntries = history.length;
      history.length = 0;
      return { cleared: true, removed_entries: removedEntries };
    },
    async onLogs() {
      return () => undefined;
    },
    append(entry) {
      history.push(entry);
    },
  };
  if (fault === "logs-read-failure") failedPreviewLogs.add(gateway);
  return gateway;
}

/** 稳定复现已有偏好逐项反馈；其他写入和启动读取不消耗目标故障。 */
export function createMockUiPrefsGateway(fault: PreviewPageFault | null = null): UiPrefsGateway {
  let values = { ...DEFAULT_UI_PREFERENCES };
  let rejectLatency = fault === "settings-save-failure";
  return {
    async get() { return { ...values }; },
    async set(patch) {
      if (rejectLatency && patch.show_latency !== undefined && patch.show_latency !== values.show_latency) {
        rejectLatency = false;
        throw new Error("模拟时延偏好保存失败；请重试。");
      }
      values = { ...values, ...patch };
      return { ...values };
    },
    async setAutostart() { return { ok: true }; },
  };
}
