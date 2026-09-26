//! Core `LogsList` 语义层（MAC-OBS-13 S1；适配 win32 `log_control.rs`）。
//!
//! 把 wire `LogsListRequest`（`after_seq`/`limit`/`filter`）映射到
//! [`crate::log_aggregator::LogAggregator`] 的读取路径，语义对齐 win32 host：
//!
//! * `after_seq == 0`：初始拉取，返回**尾部**（最近 `limit` 条匹配项——日志页
//!   初始加载期望最近的日志）；`after_seq > 0`：增量，返回 `seq > after_seq` 的
//!   前 `limit` 条匹配项。
//! * `limit == 0` → 默认 [`LOG_LIST_DEFAULT_LIMIT`]（100）；上限
//!   [`LOG_LIST_MAX_LIMIT`]（500）（对齐 win32/C++ `kDefaultLogEntries`/
//!   `kMaxLogEntriesPerResponse`）。
//! * `filter`：可选，大小写不敏感子串，匹配 `level` 或 `message`。
//! * 返回 wire `LogsListReply`：`entries` 为 wire `LogEvent` 数组（wire 契约条目
//!   **不带 seq**，条目按 seq 升序），`next_seq` 为「下一预期 seq」（最后返回
//!   条目 `seq + 1`；空页 = `last_seq + 1`）——客户端续拉传 **last-seen**
//!   （最后已见条目的 seq，即 `next_seq - 1`）：增量过滤为严格
//!   `seq > after_seq`，回传 `next_seq` 会恰漏 seq 恰等的那一条
//!   （见 proto `LogsListReply` 注释，W1-B/P9 钉死）。
//!
//! `clear`（S4 `LogsClear` 底座）在本层预留：truncate 聚合存储 + 游标归零 +
//! 返回清除条目数；S1 只单测不接 RPC（`KernelControl::logs_clear` 占位保持）。

use std::sync::Arc;

use exv_vpn_wire::generated as wire;

use crate::log_aggregator::{LogAggregator, LogEntry};

/// `limit` 缺省值（对齐 win32 `kDefaultLogEntries = 100`）。
pub const LOG_LIST_DEFAULT_LIMIT: usize = 100;
/// `limit` 上限（对齐 win32 `kMaxLogEntriesPerResponse = 500`）。
pub const LOG_LIST_MAX_LIMIT: usize = 500;

/// `LogsList`/`LogsClear`（S4）语义层：包装共享聚合存储。
pub struct LogControl {
    aggregator: Arc<LogAggregator>,
}

impl LogControl {
    /// 包装共享聚合存储（Core 组合时构造一次，全进程共享）。
    #[must_use]
    pub fn new(aggregator: Arc<LogAggregator>) -> Self {
        Self { aggregator }
    }

    /// 共享聚合存储（ingest / `append_core` 节点复用同一实例）。
    #[must_use]
    pub fn aggregator(&self) -> &Arc<LogAggregator> {
        &self.aggregator
    }

    /// `LogsList`：历史分页（尾部/增量 + `limit` clamp + 可选 `filter`）。
    #[must_use]
    pub fn list(&self, request: &wire::LogsListRequest) -> wire::LogsListReply {
        let limit = effective_limit(request.limit);
        let filter = normalized_filter(&request.filter);

        if filter.is_some() || request.after_seq == 0 {
            // 过滤或初始尾部需要全量扫描（条目 ≤ MAX_ENTRIES，控制面量级可接受）。
            let all = self.aggregator.list(request.after_seq, 0);
            let mut entries: Vec<LogEntry> = match filter {
                Some(needle) => all
                    .entries
                    .into_iter()
                    .filter(|entry| matches_filter(entry, needle))
                    .collect(),
                None => all.entries,
            };
            if request.after_seq == 0 {
                // 尾部：保留最近 `limit` 条匹配项（初始加载期望最近的日志）。
                let tail_start = entries.len().saturating_sub(limit);
                entries.drain(..tail_start);
            } else {
                // 过滤后的增量：保留前 `limit` 条。
                entries.truncate(limit);
            }
            let next_seq = entries
                .last()
                .map_or_else(|| self.aggregator.last_seq() + 1, |entry| entry.seq + 1);
            return wire::LogsListReply {
                entries: entries.iter().map(entry_to_wire).collect(),
                next_seq,
            };
        }

        // 无过滤的增量拉取：聚合存储原生路径。
        let page = self.aggregator.list(request.after_seq, limit);
        wire::LogsListReply {
            entries: page.entries.iter().map(entry_to_wire).collect(),
            next_seq: page.next_seq,
        }
    }

    /// `LogsClear`（S4 底座；S1 预留）：truncate 聚合存储 + 游标归零，
    /// 返回 `(cleared, removed_entries)`。
    ///
    /// # Errors
    /// truncate 失败 → 聚合存储的 IO 错误（存储进入内存-only 降级态）。
    pub fn clear(&self) -> std::io::Result<(bool, u64)> {
        let removed_entries = self.aggregator.last_seq();
        self.aggregator.clear()?;
        Ok((true, removed_entries))
    }
}

/// 规范化 `limit`：`0` → 默认 100；越界夹到 `[1, 500]`（wire 注释：0 = default，
/// out-of-range clamped）。
#[must_use]
pub fn effective_limit(limit: u32) -> usize {
    if limit == 0 {
        LOG_LIST_DEFAULT_LIMIT
    } else {
        usize::try_from(limit)
            .unwrap_or(LOG_LIST_MAX_LIMIT)
            .clamp(1, LOG_LIST_MAX_LIMIT)
    }
}

/// 规范化 `filter`：trim 后空串视为无过滤。
fn normalized_filter(filter: &str) -> Option<&str> {
    let trimmed = filter.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// `filter` 匹配：大小写不敏感子串，命中 `level` 或 `message` 任一即匹配。
fn matches_filter(entry: &LogEntry, filter: &str) -> bool {
    let needle = filter.to_lowercase();
    entry.level.to_lowercase().contains(&needle) || entry.message.to_lowercase().contains(&needle)
}

/// 内部条目 → wire `LogEvent`（wire 契约条目不带 seq；`fields` 回到 wire map）。
fn entry_to_wire(entry: &LogEntry) -> wire::LogEvent {
    entry.to_wire_event()
}

// ---------------------------------------------------------------------------
// 单元测试：分页边界矩阵（计划 §5.1）——尾部/增量/clamp/filter/空库/游标。
// ---------------------------------------------------------------------------
