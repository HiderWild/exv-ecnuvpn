
//! P2-c `logs.list` / `logs.clear` 契约（host 内方法，面向 P4 Tauri UI）。
//!
//! 语义对齐既有 webui（C++ 参考）：`src/core/rpc/log_actions.cpp` 的 `read_log_lines`
//! （`after_seq`/`limit`/`filter` 语义与返回形状）与 `webui/src/pages/LogsPage.vue`
//! （消费 `seq`/`level`/`message` 字段）。
//!
//! ## 契约
//!
//! - [`LogControlService::list`]：拉历史日志。
//!   - `after_seq`：只返回 `seq > after_seq` 的条目（增量轮询）；`0` = 初始拉取。
//!   - `limit`：截断；默认 [`LOG_LIST_DEFAULT_LIMIT`]（100），上限
//!     [`LOG_LIST_MAX_LIMIT`]（500）——对齐 C++ `kDefaultLogEntries=100` /
//!     `kMaxLogEntriesPerResponse=500`。
//!   - `filter`：可选；大小写不敏感子串，匹配 `level` 或 `message` 文本。
//!   - `after_seq == 0` 返回**尾部**（最近 `limit` 条匹配项——webui 初始加载期望
//!     最近的日志）；`after_seq > 0` 返回 `seq > after_seq` 的前 `limit` 条匹配项。
//!   - 返回 [`LogListReply`]：`entries` 是与 webui 消费一致的事件数组，
//!     `next_seq` 是稳健增量游标（P2-a [`LogPage`] 语义：最后返回条目 `seq + 1`；
//!     空页 = `last_seq + 1`）。
//! - [`LogControlService::clear`]：truncate 聚合文件 + 游标归零，返回
//!   [`LogClearReply`]（`cleared` + 清除前条目数）。
//!
//! ## 与 C++/webui 的对齐点与有意差异
//!
//! | 项 | C++/webui | 本契约 |
//! | --- | --- | --- |
//! | `after_seq` | `seq > after_seq` 增量 | 相同；`0` = 尾部初始拉取 |
//! | `limit` | 默认 100 / 上限 500 | 相同 |
//! | `filter` | 整行子串 | 结构化：`level`/`message` 大小写不敏感子串 |
//! | 返回形状 | `[{seq,timestamp,level,message}]` 数组 | `{entries,next_seq}`；`entries` 即数组，条目字段是 P2-a 结构化超集 |
//! | 响应体量上限 | 6000 字节（legacy 文本行） | 由条目上限 500 覆盖（结构化 JSON 无此问题） |
//! | 清空 | webui 仅前端清 store（C++ 桌面端不调 host） | 本契约提供真实 host 清空（P4 用） |
//!
//! ## 接线
//!
//! - P3：core 组合时构造 `Arc<LogAggregator>`（`LogAggregator::open_default`），
//!   包为 [`LogControlService`]，供 `KernelControl` 语义网关 / host 内方法分发。
//! - P4：Tauri Command 直接调用 `list`/`clear`（进程内方法）。若未来需跨进程
//!   gRPC 暴露（`logs.list`/`logs.clear` 不在冻结的 `KernelControl` proto 中），
//!   须由协调者评估 proto 变更——本模块不改 proto。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::log_aggregator::{LogAggError, LogAggregator, LogEntry};

/// `logs.list` 的 `limit` 默认值（对齐 C++ `kDefaultLogEntries = 100`）。
pub const LOG_LIST_DEFAULT_LIMIT: usize = 100;
/// `logs.list` 的 `limit` 上限（对齐 C++ `kMaxLogEntriesPerResponse = 500`）。
pub const LOG_LIST_MAX_LIMIT: usize = 500;

/// `logs.list` 请求参数（对齐 C++ `read_log_lines` 的 payload 键：`after_seq`/
/// `limit`/`filter`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListLogsRequest {
    /// 只返回 `seq > after_seq` 的条目；`0` = 初始拉取（返回尾部）。
    #[serde(default)]
    pub after_seq: u64,
    /// 截断上限；`None` → 默认 [`LOG_LIST_DEFAULT_LIMIT`]，越界夹到
    /// `[1, LOG_LIST_MAX_LIMIT]`。
    #[serde(default)]
    pub limit: Option<usize>,
    /// 可选过滤：大小写不敏感子串，匹配 `level` 或 `message` 文本。
    #[serde(default)]
    pub filter: Option<String>,
}

/// `logs.list` 返回页。
///
/// `entries` 是与 webui 消费一致的事件数组（C++ 返回形状的超集）；`next_seq`
/// 是稳健增量游标（最后返回条目的 `seq + 1`；空页 = `last_seq + 1`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogListReply {
    pub entries: Vec<LogEntry>,
    pub next_seq: u64,
}

/// `logs.clear` 返回。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogClearReply {
    /// 恒为 `true`（失败走错误返回）。
    pub cleared: bool,
    /// 清除前聚合文件中的条目数（游标归零前的 `last_seq`）。
    pub removed_entries: u64,
}

/// P2-c `logs.list`/`logs.clear` 契约实现：包装 [`LogAggregator`]，提供
/// webui/C++ 语义对齐的 host 内方法（P4 Tauri Command 直接调用）。
pub struct LogControlService {
    aggregator: Arc<LogAggregator>,
}

impl LogControlService {
    /// 包装共享的聚合服务（P3 core 组合时构造一次，全进程共享）。
    #[must_use]
    pub fn new(aggregator: Arc<LogAggregator>) -> Self {
        Self { aggregator }
    }

    /// `logs.list`：拉历史日志（增量/尾部 + `limit` 截断 + 可选 `filter`）。
    ///
    /// # Errors
    /// 聚合文件不可读 → `LogAggError::Io`。
    ///
    /// # Panics
    /// 内部互斥锁中毒 → panic（透传自 [`LogAggregator::list`]）。
    pub fn list(&self, req: &ListLogsRequest) -> Result<LogListReply, LogAggError> {
        let limit = effective_limit(req.limit);
        let filter = req
            .filter
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if filter.is_some() || req.after_seq == 0 {
            // 过滤或初始尾部需要全量扫描（seq 槽由聚合器保留；控制面日志量级小）。
            let all = self.aggregator.list(req.after_seq, 0)?;
            let mut entries: Vec<LogEntry> = match filter {
                Some(needle) => all
                    .entries
                    .into_iter()
                    .filter(|e| matches_filter(e, needle))
                    .collect(),
                None => all.entries,
            };
            if req.after_seq == 0 {
                // 尾部：保留最近 `limit` 条（webui 初始加载期望最近的日志）。
                entries = entries.split_off(entries.len().saturating_sub(limit));
            } else {
                // 过滤后的增量：保留前 `limit` 条。
                entries.truncate(limit);
            }
            return Ok(LogListReply {
                next_seq: next_seq_after(&entries, self.aggregator.last_seq()),
                entries,
            });
        }

        // 无过滤的增量拉取：直接委托聚合器原生路径（含其 `next_seq` 语义）。
        let page = self.aggregator.list(req.after_seq, limit)?;
        Ok(LogListReply {
            entries: page.entries,
            next_seq: page.next_seq,
        })
    }

    /// `logs.clear`：truncate 聚合文件 + 游标归零。
    ///
    /// # Errors
    /// 截断失败 → `LogAggError::Io`。
    ///
    /// # Panics
    /// 内部互斥锁中毒 → panic（透传自 [`LogAggregator::clear`]）。
    pub fn clear(&self) -> Result<LogClearReply, LogAggError> {
        let removed_entries = self.aggregator.last_seq();
        self.aggregator.clear()?;
        Ok(LogClearReply {
            cleared: true,
            removed_entries,
        })
    }
}

/// 规范化 `limit`：`None` → 默认 100；夹到 `[1, LOG_LIST_MAX_LIMIT]`（对齐 C++
/// `kDefaultLogEntries=100` / `kMaxLogEntriesPerResponse=500`，`< 1` 视为 1）。
fn effective_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(LOG_LIST_DEFAULT_LIMIT)
        .clamp(1, LOG_LIST_MAX_LIMIT)
}

/// `filter` 匹配：大小写不敏感子串，命中 `level` 或 `message` 任一即匹配。
fn matches_filter(entry: &LogEntry, filter: &str) -> bool {
    let needle = filter.to_lowercase();
    entry.level.to_lowercase().contains(&needle) || entry.message.to_lowercase().contains(&needle)
}

/// `next_seq` = 最后返回条目的 `seq + 1`；空页 = `last_seq + 1`（对齐
/// [`LogAggregator`] `LogPage` 的游标语义：跳过后继已消费的槽，客户端不空转）。
fn next_seq_after(entries: &[LogEntry], last_seq: u64) -> u64 {
    entries
        .last()
        .map_or_else(|| last_seq + 1, |entry| entry.seq + 1)
}

// ---------------------------------------------------------------------------
// 单元测试：after_seq 增量、limit 截断、filter 行为、clear 游标归零、JSON 形状。
// ---------------------------------------------------------------------------
