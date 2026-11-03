// M7：aTrust 校园 resolver 外来同名冲突的 UI 三选（计划 M4 外来冲突语义 + M7
// 设置与 UI 交付）。engine 在 /etc/resolver/<domain> 遇到无 EXV 标记键的外来
// 同名文件时 typed 拒绝（稳定 reason `DARWIN_ATRUST_RESOLVER_CONFLICT`，绝不
// 覆盖外来文件）——该 reason 经**聚合日志**透出（管线把该失败投影为
// Failed("RESOLVER") → VpnError `OBSERVED_CONFLICT`；wire 无字符串域——M6 已
// 登记的边界），因此前端以「失败状态码 + 日志 reason」双证据判定，只在两条证据
// 都成立时弹三选，避免把普通路由冲突误报成 resolver 冲突。

import type { LogChunk, LogEvent } from "../lib/ipc";

/** wire `ErrorCode::ObservedConflict` 的 proto 名（壳层 `as_str_name` 映射）。 */
export const OBSERVED_CONFLICT_ERROR_CODE = "ERROR_CODE_OBSERVED_CONFLICT";

/** engine 聚合日志的 resolver 步骤失败 code（`RESOLVER_STEP_FAILED`）。 */
export const RESOLVER_STEP_FAILED_CODE = "RESOLVER_STEP_FAILED";

/** 外来同名冲突稳定 reason（engine `campus_resolver::CONFLICT_REASON` 同串）。 */
export const RESOLVER_CONFLICT_REASON = "DARWIN_ATRUST_RESOLVER_CONFLICT";

/** 判定一条聚合日志是否为外来同名 resolver 冲突（fields.reason 稳定串匹配）。 */
export function isResolverConflictEntry(entry: LogEvent): boolean {
  return entry.code === RESOLVER_STEP_FAILED_CODE
    && entry.fields?.reason === RESOLVER_CONFLICT_REASON;
}

/** 从冲突日志条目中提取冲突的校园域（message 形如
 *  `campus resolver step failed: DARWIN_ATRUST_RESOLVER_CONFLICT: <domain> (<path>)`）。 */
export function conflictDomainOf(entry: LogEvent): string | null {
  const match = entry.message.match(/DARWIN_ATRUST_RESOLVER_CONFLICT:\s*([^\s(]+)\s*\(/);
  return match?.[1] ?? null;
}

export interface ResolverConflictFact {
  /** 冲突的校园域（解析失败时为 null——仍弹三选，正文回退通用指引）。 */
  domain: string | null;
  /** 原始日志 message（非秘密：域与文件路径）。 */
  message: string;
}

/** 日志条目列表中的最近一次 resolver 冲突事实；无冲突返回 null。 */
export function latestResolverConflict(entries: ReadonlyArray<LogEvent>): ResolverConflictFact | null {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    if (isResolverConflictEntry(entry)) {
      return { domain: conflictDomainOf(entry), message: entry.message };
    }
  }
  return null;
}

/** 单页拉取上限（聚合存储 5000 条有界；2000×3 页即覆盖全量）。 */
const LOG_PAGE_LIMIT = 2000;
const MAX_PAGES = 3;

/**
 * 分页扫完聚合日志寻找最近的 resolver 冲突（失败观测点调用一次；core 侧条目
 * 有界，页数封顶防失控）。任一页失败即放弃（fail-safe：证据不全不弹三选）。
 */
export async function fetchLatestResolverConflict(
  logsList: (afterSeq: number, limit: number) => Promise<LogChunk>,
): Promise<ResolverConflictFact | null> {
  let cursor = 0;
  let entries: LogEvent[] = [];
  for (let page = 0; page < MAX_PAGES; page += 1) {
    let chunk: LogChunk;
    try {
      chunk = await logsList(cursor, LOG_PAGE_LIMIT);
    } catch {
      return null;
    }
    entries.push(...chunk.events);
    cursor = chunk.next_after_seq;
    if (!chunk.has_more) break;
  }
  return latestResolverConflict(entries);
}

/**
 * 手动清理指引文案（三选之一）：/etc/resolver 需要 root；EXV 不代替用户删除
 * 外来文件（可能是其它工具的配置）。
 */
export function manualCleanupSteps(domain: string | null): string[] {
  const file = domain ? `/etc/resolver/${domain}` : "/etc/resolver/<校园域名>";
  return [
    `打开「终端」执行：sudo cat ${file} 确认该文件不是其它工具的配置。`,
    `确认可删除后执行：sudo rm ${file}`,
    "回到 EXV 重新点击连接。",
  ];
}
