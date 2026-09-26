
//! P2-c `logs.list`/`logs.clear` 契约（`exv_core::log_control`）集成测试。
//!
//! 从 crate 外验证契约层（非聚合器内层）语义：
//!
//! - `after_seq` 增量：只返回 `seq > after_seq` 的条目，`next_seq` 可续轮询；
//! - `limit` 截断：默认 100 / 上限 500，初始拉取返回尾部；
//! - `filter`：按 level 或文本（大小写不敏感子串）；
//! - `clear`：truncate 聚合文件 + 游标归零，后续从 `seq 1` 重新开始。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use exv_core::log_aggregator::LogAggregator;
use exv_core::log_control::{ListLogsRequest, LogControlService};

/// 每测试独立的临时聚合日志路径（pid 隔离并发）。
fn temp_log_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("exv-log-ctl-it-{tag}-{}.jsonl", std::process::id()))
}

fn service(tag: &str) -> (Arc<LogAggregator>, LogControlService) {
    let agg = Arc::new(LogAggregator::open(&temp_log_path(tag)).expect("open aggregator"));
    let svc = LogControlService::new(Arc::clone(&agg));
    (agg, svc)
}

fn append_core(agg: &LogAggregator, level: &str, message: &str) {
    agg.append_core(level, "core", "", message, &BTreeMap::new())
        .expect("append core");
}

fn seqs(reply: &exv_core::log_control::LogListReply) -> Vec<u64> {
    reply.entries.iter().map(|e| e.seq).collect()
}

/// 端到端：初始尾部 + `after_seq` 增量轮询（`next_seq` 续接）+ filter 组合。
#[test]
fn list_initial_tail_then_incremental_polling_and_filter() {
    let (agg, svc) = service("it-flow");
    for i in 1..=5 {
        append_core(&agg, "info", &format!("event {i}"));
    }

    // 初始拉取：尾部最近 3 条（webui 初始加载期望最近的日志）。
    let initial = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: Some(3),
            filter: None,
        })
        .expect("initial");
    assert_eq!(seqs(&initial), [3, 4, 5]);
    assert_eq!(initial.next_seq, 6);

    // 增量轮询：从 next_seq - 1 继续，只返回新增。
    append_core(&agg, "error", "tunnel dropped");
    let incremental = svc
        .list(&ListLogsRequest {
            after_seq: initial.next_seq - 1,
            limit: Some(10),
            filter: None,
        })
        .expect("incremental");
    assert_eq!(seqs(&incremental), [6]);
    assert_eq!(incremental.entries[0].message, "tunnel dropped");

    // filter 按 level：只返回 error。
    let filtered = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: None,
            filter: Some("error".to_string()),
        })
        .expect("filtered");
    assert_eq!(seqs(&filtered), [6]);

    // 已追平：空页，next_seq = last_seq + 1。
    let caught_up = svc
        .list(&ListLogsRequest {
            after_seq: 6,
            limit: Some(10),
            filter: None,
        })
        .expect("caught up");
    assert!(caught_up.entries.is_empty());
    assert_eq!(caught_up.next_seq, 7);
}

/// 端到端：`limit` 截断 + 默认/上限 + clear 后游标归零并从 seq 1 重新开始。
#[test]
fn limit_truncation_and_clear_resets_cursor() {
    let (agg, svc) = service("it-limit-clear");
    for i in 1..=600 {
        append_core(&agg, "info", &format!("line {i}"));
    }

    // 默认 100 条尾部 = seq 501..=600。
    let dflt = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: None,
            filter: None,
        })
        .expect("default limit");
    assert_eq!(dflt.entries.len(), 100);
    assert_eq!(dflt.entries.first().expect("first").seq, 501);

    // 上限 500：尾部 = seq 101..=600。
    let capped = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: Some(10_000),
            filter: None,
        })
        .expect("max limit");
    assert_eq!(capped.entries.len(), 500);
    assert_eq!(capped.entries.first().expect("first").seq, 101);

    // clear：truncate 聚合文件 + 游标归零 + 返回清除条目数。
    let cleared = svc.clear().expect("clear");
    assert!(cleared.cleared);
    assert_eq!(cleared.removed_entries, 600);
    assert_eq!(agg.last_seq(), 0);

    let after = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: None,
            filter: None,
        })
        .expect("empty after clear");
    assert!(after.entries.is_empty());

    // 清空后从 seq 1 重新开始（对齐 logs.clear 语义）。
    append_core(&agg, "warn", "fresh start");
    let fresh = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: None,
            filter: None,
        })
        .expect("fresh");
    assert_eq!(seqs(&fresh), [1]);
    assert_eq!(fresh.entries[0].message, "fresh start");
}

/// 端到端：filter 按文本（大小写不敏感）+ 过滤后 seq 槽保持原值。
#[test]
fn filter_by_text_keeps_original_seq_slots() {
    let (agg, svc) = service("it-filter-text");
    append_core(&agg, "info", "Config loaded");
    append_core(&agg, "error", "certificate verify FAILED");
    append_core(&agg, "warn", "slow RTT");

    let reply = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: None,
            filter: Some("failed".to_string()),
        })
        .expect("filter text");
    assert_eq!(seqs(&reply), [2], "seq 槽保持原值（不因过滤重排）");
    assert_eq!(reply.entries[0].message, "certificate verify FAILED");
    assert_eq!(reply.next_seq, 3);
}

/// W1-B（P9）：last-seen + 严格 `>` 游标契约矩阵（与 darwin core 同构钉死）。
///
/// 统一契约：客户端续拉传「最后已见 seq」，core 返回 `seq > after_seq`；
/// 本测试钉死五个分支——last-seen 续拉恰返回下一条、以 `next_seq` 续拉恰等条
/// **不**返回（旧桥 off-by-one 漏条机理的反例）、`after_seq` 回退重发、
/// `after_seq > last_seq` 空页 `next_seq` 稳定、clear 后 seq 重启。
#[test]
fn last_seen_strict_greater_cursor_contract_matrix() {
    let (agg, svc) = service("it-lastseen");
    for i in 1..=5 {
        append_core(&agg, "info", &format!("e{i}"));
    }

    // 上一页：after_seq=2 → seq 3..=5，next_seq=6；last-seen=5。
    let page = svc
        .list(&ListLogsRequest {
            after_seq: 2,
            limit: None,
            filter: None,
        })
        .expect("page");
    assert_eq!(seqs(&page), [3, 4, 5]);
    assert_eq!(page.next_seq, 6);

    append_core(&agg, "info", "e6-new");

    // (1) last-seen（next_seq - 1 = 5）续拉：恰返回下一条 seq 6。
    let next = svc
        .list(&ListLogsRequest {
            after_seq: page.next_seq - 1,
            limit: Some(10),
            filter: None,
        })
        .expect("last-seen continuation");
    assert_eq!(seqs(&next), [6], "last-seen 续拉恰返回下一条");
    assert_eq!(next.next_seq, 7);

    // (2) 反例钉死：以 next_seq（6）续拉 → seq 恰等条目不返回（严格 `>`）。
    let skipped = svc
        .list(&ListLogsRequest {
            after_seq: page.next_seq,
            limit: Some(10),
            filter: None,
        })
        .expect("next_seq continuation counter-example");
    assert!(
        skipped.entries.is_empty(),
        "seq 恰等于 next_seq 的条目不得返回——回传 next_seq 会漏条"
    );
    assert_eq!(skipped.next_seq, 7);

    // (3) after_seq 回退（< 已见）→ 重发（前端按事件键去重，重复无害）。
    let resent = svc
        .list(&ListLogsRequest {
            after_seq: 3,
            limit: Some(10),
            filter: None,
        })
        .expect("resend");
    assert_eq!(seqs(&resent), [4, 5, 6], "游标回退重发");

    // (4) after_seq > last_seq → 空页 + next_seq 稳定（= last_seq + 1）。
    let beyond = svc
        .list(&ListLogsRequest {
            after_seq: 100,
            limit: Some(10),
            filter: None,
        })
        .expect("beyond");
    assert!(beyond.entries.is_empty());
    assert_eq!(beyond.next_seq, 7);

    // (5) clear 后 seq 重启：旧游标只见空页（客户端必须清零游标）；
    //     清零重拉得到从 seq 1 重新开始的新条目。
    svc.clear().expect("clear");
    let stale = svc
        .list(&ListLogsRequest {
            after_seq: 6,
            limit: Some(10),
            filter: None,
        })
        .expect("stale cursor after clear");
    assert!(stale.entries.is_empty(), "clear 后旧游标拉不到重启的 seq");

    append_core(&agg, "info", "fresh");
    let fresh = svc
        .list(&ListLogsRequest {
            after_seq: 0,
            limit: None,
            filter: None,
        })
        .expect("fresh");
    assert_eq!(seqs(&fresh), [1], "清空后从 seq 1 重新开始");
    assert_eq!(fresh.next_seq, 2);
}
