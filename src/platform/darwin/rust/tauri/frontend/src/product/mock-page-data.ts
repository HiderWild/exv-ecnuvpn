import type { LogEvent } from "../lib/ipc";
import type { CoreConfigGateway, CoreConfigKey } from "./core-config";
import type { LogsGateway } from "./logs";

const previewConfig: ReadonlyArray<{ key: CoreConfigKey; value: string }> = [
  { key: "server", value: "vpn.preview.example" },
  { key: "username", value: "preview-user" },
  { key: "remember_password", value: "false" },
  { key: "routes", value: "10.0.0.0/8,172.16.0.0/12" },
  { key: "user_agent", value: "EXV Tauri Preview" },
  { key: "mtu", value: "1420" },
];

/**
 * 仅供开发预览使用的内存配置。生产入口不会创建它，也不会写入本机或 core。
 */
export function createMockCoreConfigGateway(): CoreConfigGateway {
  const values = new Map(previewConfig.map((item) => [item.key, item.value]));

  return {
    async configGet() {
      return [...[...values].filter(([key]) => key !== "password").map(([key, value]) => ({ key, value })),
        { key: "has_stored_password", value: String(values.get("remember_password") === "true" && !!values.get("password")) },
      ];
    },
    async configSet(items) {
      const previousUsername = values.get("username");
      for (const item of items) values.set(item.key, item.value);
      if (values.get("username") !== previousUsername && !items.some((item) => item.key === "password" && !!item.value)) values.delete("password");
      if (values.get("remember_password") !== "true") values.delete("password");
      return true;
    },
    async savedPassword(username, server) {
      if (values.get("username") !== username || values.get("server") !== server) throw new Error("预览账户已变化");
      return values.get("remember_password") === "true" ? values.get("password") || null : null;
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

  return {
    async logsList(afterSeq, limit) {
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
}
