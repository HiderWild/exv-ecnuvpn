
//! 系统代理账本的 journal 读写编排（engine 唯一写入者/清除者；2026-09-05 账本
//! 计划 §4.4/§4.5/§4.7）。
//!
//! 组合三件既有基建，不新造框架：
//! - [`WinJournalStore`](exv_vpn_win32_resource::journal_store::WinJournalStore)：
//!   单目录 `journal.bin` 的 FlushFileBuffers 持久追加 + `recover` 三态 +
//!   `compact` 前缀重写；
//! - [`SystemProxyFamilyStep`](exv_vpn_win32_resource::system_proxy_family::
//!   SystemProxyFamilyStep) v0x03 编解码（自带写入后指纹）；
//! - [`replay_system_proxy_step`](exv_vpn_win32_resource::system_proxy_family_exec::
//!   replay_system_proxy_step)：三态裁决 + 真机精确还原。
//!
//! 冻结契约（§4.4）：清除 = 前缀 compact，**只丢严格后缀**（`verify_chain` 要求
//! sequence==index、链式 digest）；活跃步骤记录恒为 journal 严格后缀（apply 前内联
//! 回放 + 启动回放共同维持），故前缀规则总可满足。
//!
//! 冻结契约（§4.5）：`Corrupt` 隔离序 = **先释放文件句柄（drop）→ 改名
//! `journal.bin.corrupt-<unix_ts>` → 重开**——Windows 上带句柄改名必 `ACCESS_DENIED`
//! （同族证据：`journal_store.rs` compact 自身的「先 `file.take()` 再
//! `MoveFileExW` 再重开」）；`TornTail` 先 `compact` 修复（撕尾字节不出现在新文件）
//! 再回放。回放失败只记日志，绝不阻塞启动/连接。
//!
//! admission/retirement 轨（J51 payload）共存不受扰：非步骤 payload 一律跳过
//! （`decode_admission` 对非 J51 同款互斥语义），既不回放也不清除。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use exv_vpn_resource::journal::JournalRecord;
use exv_vpn_win32_resource::journal_path::JournalPath;
use exv_vpn_win32_resource::journal_store::{RecoverOutcome, WinJournalStore};
use exv_vpn_win32_resource::system_proxy_family::SystemProxyFamilyStep;
use exv_vpn_win32_resource::system_proxy_family_exec::replay_system_proxy_step;
use exv_vpn_win32_resource::system_proxy_family_exec::SystemProxyReplayOutcome;

use crate::log_sink::{LogLevel, LogSink};

/// journal 目录内的单文件名（`journal_store::JOURNAL_FILE` 的稳定目录布局契约）。
const JOURNAL_FILE: &str = "journal.bin";

/// 系统代理账本编排器：`Arc<Mutex<Option<WinJournalStore>>>` + 目录 + 日志 sink。
///
/// `None` = **禁用账本态**（open 失败 / 损坏隔离失败）：`append_step` 返回 false
/// （调用方降级「未豁免」），其余方法 no-op——本会话无账本保护，系统代理族按既有
/// best-effort 运行，连接业务不受阻（§2 失败表现）。
///
/// 克隆共享同一 store 句柄（`RouteOwner` 经克隆在 clear 时清账）。
#[derive(Clone)]
pub struct SystemProxyJournal {
    inner: Arc<Mutex<Option<WinJournalStore>>>,
    dir: PathBuf,
    log: Arc<LogSink>,
}

impl SystemProxyJournal {
    /// 启动回放入口（§4.5；oneshot 与 service 两形态共用）：open → `recover` 分派
    /// （Corrupt 隔离 / TornTail compact 修复）→ LIFO 回放未清步骤记录。
    ///
    /// 永不 panic、永不阻塞：任何失败只记日志并落入禁用账本态/继续。
    ///
    /// 签名说明（§4.7 实现级偏差）：`log` 取 `Arc<LogSink>`——编排器需持有 sink
    /// 供运行期 `append_step`/`clear_tail` 记日志，`LogSink` 非 `Clone`，`&LogSink`
    /// 无法转为持有的 `Arc`；调用方（`grpc_server::with_real_tunnel`）手头即持有
    /// `Arc<LogSink>`。
    #[must_use]
    pub fn open_and_replay(dir: &Path, log: Arc<LogSink>) -> Self {
        let journal = Self::open(dir, log);
        journal.replay_pending(&journal.log);
        journal
    }

    /// 打开（或禁用）账本：open 失败只记日志，返回禁用账本态。
    #[must_use]
    fn open(dir: &Path, log: Arc<LogSink>) -> Self {
        let inner = match WinJournalStore::open(&JournalPath::from_dir(dir.to_path_buf())) {
            Ok(store) => Some(store),
            Err(e) => {
                log.emit(
                    LogLevel::Warn,
                    "engine",
                    "system_proxy_journal.open_failed",
                    "system-proxy journal unavailable (ledger disabled this session)",
                    &[("dir", &dir.display().to_string()), ("error", &format!("{e:?}"))],
                );
                None
            }
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
            dir: dir.to_path_buf(),
            log,
        }
    }

    /// 禁用账本态（测试与旧构造器过渡用）：`append_step` 恒 false，其余 no-op。
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            dir: PathBuf::new(),
            log: Arc::new(LogSink::null()),
        }
    }

    /// 编排器持有的日志 sink（`RouteOwner::apply` 内联回放传参用）。
    #[must_use]
    pub fn log(&self) -> Arc<LogSink> {
        Arc::clone(&self.log)
    }

    /// journal 目录（诊断）。
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 追加一条步骤记录（§4.2 持久点；调用方在 plan 与 commit 之间调用）。
    ///
    /// 返回 false = 记账失败（禁用态 / recover 失败 / 追加或刷盘失败）——调用方
    /// 降级「未豁免」并跳过 commit（特性级 fail-closed：宁可不豁免，不留无账本
    /// 效果；不是隧道级失败）。
    ///
    /// 链头推进：新记录 `sequence = 末记录.sequence + 1`、`previous_digest = 末记录
    /// .digest`（空文件 = sequence 0 + 全零 digest），保持 `verify_chain` 前缀有效。
    pub fn append_step(&self, step: &SystemProxyFamilyStep) -> bool {
        let mut guard = self.lock();
        // 损坏中段：绝不在损坏文件后追加（否则后续记录永不可恢复）——先隔离。
        let corrupt = {
            let Some(store) = guard.as_mut() else {
                self.log_disabled_append();
                return false;
            };
            match store.recover() {
                Ok(RecoverOutcome::Clean(records))
                | Ok(RecoverOutcome::TornTail { records }) => {
                    let (seq, prev) = match records.last() {
                        Some(last) => (last.sequence + 1, last.digest),
                        None => (0, [0u8; 32]),
                    };
                    Some(Ok((seq, prev)))
                }
                Ok(RecoverOutcome::Corrupt { offset }) => Some(Err(offset)),
                Err(e) => {
                    self.log.emit(
                        LogLevel::Warn,
                        "engine",
                        "system_proxy_journal.append_recover_failed",
                        "system-proxy journal append skipped (recover failed; degraded)",
                        &[("error", &format!("{e:?}"))],
                    );
                    None
                }
            }
        };
        let Some(chain_head) = corrupt else {
            return false;
        };
        let (seq, prev) = match chain_head {
            Ok((seq, prev)) => (seq, prev),
            Err(offset) => {
                if !self.quarantine_corrupt(&mut guard, offset) {
                    return false;
                }
                (0, [0u8; 32])
            }
        };
        let record = JournalRecord::new(seq, prev, step.to_payload());
        let outcome = guard
            .as_mut()
            .expect("store present after successful chain read or quarantine")
            .append_synced(&record);
        match outcome {
            Ok(_) => true,
            Err(e) => {
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "system_proxy_journal.append_failed",
                    "system-proxy journal append failed (degraded: unexempted, no commit)",
                    &[("error", &format!("{e:?}"))],
                );
                false
            }
        }
    }

    /// 前缀清除（§4.4）：compact 掉第 `keep_len` 条之后的严格后缀。失败只记日志
    /// ——记录遗留，下次启动回放的 `AlreadyClean`/`SkipForfeit` 路径自然清除（自愈）。
    pub fn clear_tail(&self, keep_len: usize) {
        let mut guard = self.lock();
        let Some(store) = guard.as_mut() else {
            return;
        };
        let records = match store.recover() {
            Ok(RecoverOutcome::Clean(records)) | Ok(RecoverOutcome::TornTail { records }) => records,
            Ok(RecoverOutcome::Corrupt { offset }) => {
                // 运行期中段损坏：无账本语义下不清账（隔离留给下次启动回放路径）。
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "system_proxy_journal.clear_skipped_corrupt",
                    "system-proxy journal clear skipped (corrupt mid-record)",
                    &[("offset", &offset.to_string())],
                );
                return;
            }
            Err(e) => {
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "system_proxy_journal.clear_recover_failed",
                    "system-proxy journal clear skipped (recover failed)",
                    &[("error", &format!("{e:?}"))],
                );
                return;
            }
        };
        let keep_len = keep_len.min(records.len());
        if keep_len >= records.len() {
            return; // 无后缀可丢（幂等）。
        }
        // 前缀规则（§4.4）：keep 只能是现存记录的严格前缀，compact 原样重写不重排链。
        let compacted = store.compact(&records[..keep_len]);
        if let Err(e) = compacted {
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "system_proxy_journal.compact_failed",
                "system-proxy journal compact failed (record left behind; self-heals on next startup replay)",
                &[("error", &format!("{e:?}"))],
            );
            Self::recover_store_after_compact_failure(&mut guard, &self.dir, &self.log);
        }
    }

    /// 当前是否存在活跃步骤记录（按后缀集合语义：journal 尾部存在可解码为步骤
    /// 记录的条目即 true——「通常至多一条」，失败路径可多条）。
    #[must_use]
    pub fn has_pending_step(&self) -> bool {
        let guard = self.lock();
        let Some(store) = guard.as_ref() else {
            return false;
        };
        match store.recover() {
            Ok(RecoverOutcome::Clean(records)) | Ok(RecoverOutcome::TornTail { records }) => records
                .iter()
                .any(|r| SystemProxyFamilyStep::from_payload(&r.payload).is_ok()),
            Ok(RecoverOutcome::Corrupt { .. }) | Err(_) => false,
        }
    }

    /// 当前 journal 记录总数（`RouteOwner` 记录 apply 前长度供 clear 时的 keep_len；
    /// 禁用态 / 读失败 = 0）。
    #[must_use]
    pub fn record_count(&self) -> usize {
        let guard = self.lock();
        let Some(store) = guard.as_ref() else {
            return 0;
        };
        match store.recover() {
            Ok(RecoverOutcome::Clean(records)) | Ok(RecoverOutcome::TornTail { records }) => {
                records.len()
            }
            Ok(RecoverOutcome::Corrupt { .. }) | Err(_) => 0,
        }
    }

    /// 启动/内联回放（§4.3/§4.5）：recover 分派 → 对步骤记录 **LIFO（journal 逆序）**
    /// 三态裁决（后写先撤；被后继覆盖的前继自然落入 SkipForfeit 清除）→ compact
    /// 掉已裁决的严格后缀。
    ///
    /// - 非步骤 payload（admission/retirement 轨等）不属本账本：不回放、不清除，
    ///   且其后缀规则使其成为回放停止点（前缀 compact 只能丢后缀）。
    /// - `HardFailure`（还原硬失败）：保留该记录及其之前的记录，下次重试。
    /// - 任一失败只记日志；**绝不阻塞调用方**（启动/serving/连接）。
    pub fn replay_pending(&self, log: &LogSink) {
        let mut guard = self.lock();
        let Some(store) = guard.as_mut() else {
            return;
        };
        let records = match store.recover() {
            Ok(RecoverOutcome::Clean(records)) => records,
            Ok(RecoverOutcome::TornTail { records }) => {
                // 先 compact 修复（撕尾字节不出现在新文件），再回放。
                let repaired = store.compact(&records);
                if let Err(e) = repaired {
                    log.emit(
                        LogLevel::Warn,
                        "engine",
                        "system_proxy_journal.torn_compact_failed",
                        "system-proxy journal torn tail repair failed (no replay this pass)",
                        &[("error", &format!("{e:?}"))],
                    );
                    Self::recover_store_after_compact_failure(&mut guard, &self.dir, log);
                    return;
                }
                log.emit(
                    LogLevel::Info,
                    "engine",
                    "system_proxy_journal.torn_tail_repaired",
                    "system-proxy journal torn tail compacted away",
                    &[("kept_records", &records.len().to_string())],
                );
                records
            }
            Ok(RecoverOutcome::Corrupt { offset }) => {
                // 隔离：释放句柄 → 改名 → 重开（本模块核心冻结序，见模块文档）。
                if !Self::quarantine_locked(&mut guard, &self.dir, offset, log) {
                    return; // 隔离失败：禁用态，本会话无账本，绝不追加。
                }
                return; // 新文件为空：无待回放。
            }
            Err(e) => {
                log.emit(
                    LogLevel::Warn,
                    "engine",
                    "system_proxy_journal.replay_recover_failed",
                    "system-proxy journal replay skipped (recover failed)",
                    &[("error", &format!("{e:?}"))],
                );
                return;
            }
        };
        if records.is_empty() {
            return;
        }
        // LIFO 回放：逆序三态裁决；已裁决（非硬失败）的连续尾段构成可丢后缀。
        let mut keep_len = records.len();
        for (idx, record) in records.iter().enumerate().rev() {
            let Ok(step) = SystemProxyFamilyStep::from_payload(&record.payload) else {
                // 非步骤记录（admission/retirement 轨或异构 payload）：回放停止点
                // ——它之前的后缀不可越过它丢弃（前缀规则）。
                break;
            };
            match replay_system_proxy_step(&step.originating_sid, &step) {
                SystemProxyReplayOutcome::Restored { write_backs, deletes } => {
                    keep_len = idx;
                    log.emit(
                        LogLevel::Info,
                        "engine",
                        "system_proxy_journal.replay_restored",
                        "system-proxy crash replay restored registry values",
                        &[
                            ("write_backs", &write_backs.to_string()),
                            ("deletes", &deletes.to_string()),
                            ("sid", &step.originating_sid),
                        ],
                    );
                }
                SystemProxyReplayOutcome::AlreadyClean => {
                    keep_len = idx;
                    log.emit(
                        LogLevel::Info,
                        "engine",
                        "system_proxy_journal.replay_already_clean",
                        "system-proxy crash replay found registry already at prestate (idempotent clear)",
                        &[("sid", &step.originating_sid)],
                    );
                }
                SystemProxyReplayOutcome::SkipForfeit => {
                    keep_len = idx;
                    log.emit(
                        LogLevel::Warn,
                        "engine",
                        "restore_failures",
                        "system-proxy crash replay forfeited (fingerprint mismatch; user state wins, no forced write)",
                        &[("sid", &step.originating_sid)],
                    );
                }
                SystemProxyReplayOutcome::HardFailure => {
                    log.emit(
                        LogLevel::Warn,
                        "engine",
                        "system_proxy_journal.replay_hard_failure",
                        "system-proxy crash replay hard failure (records kept for next retry)",
                        &[("sid", &step.originating_sid)],
                    );
                    break;
                }
            }
        }
        if keep_len >= records.len() {
            return; // 无已裁决后缀可清。
        }
        let compacted = store.compact(&records[..keep_len]);
        if let Err(e) = compacted {
            log.emit(
                LogLevel::Warn,
                "engine",
                "system_proxy_journal.replay_compact_failed",
                "system-proxy journal post-replay compact failed (records left behind; self-heals on next startup)",
                &[("error", &format!("{e:?}"))],
            );
            Self::recover_store_after_compact_failure(&mut guard, &self.dir, log);
        }
    }

    /// compact 失败后的 store 恢复（编排器「永不 panic」的关键）：
    /// `WinJournalStore::compact` 在 `file.take()` 之后失败（改名/目录刷盘/重开）
    /// 会留下内部句柄为 `None` 的 store——此后对它调用 `recover`/`append` 会 panic。
    /// 因此任何 compact 失败都必须立即重开全新句柄；重开失败则禁用账本态。
    fn recover_store_after_compact_failure(
        guard: &mut Option<WinJournalStore>,
        dir: &Path,
        log: &LogSink,
    ) {
        match WinJournalStore::open(&JournalPath::from_dir(dir.to_path_buf())) {
            Ok(fresh) => *guard = Some(fresh),
            Err(e) => {
                log.emit(
                    LogLevel::Warn,
                    "engine",
                    "system_proxy_journal.reopen_after_compact_failure_failed",
                    "system-proxy journal reopen failed after compact failure (ledger disabled)",
                    &[("error", &format!("{e:?}"))],
                );
                *guard = None;
            }
        }
    }

    /// 损坏隔离（§4.5 冻结序；`self.lock()` 已持有的调用方变体）：
    /// 先 drop 释放句柄 → `journal.bin` 改名 `journal.bin.corrupt-<unix_ts>`
    /// （best-effort 保留物证）→ 重开全新文件。改名失败 → 禁用账本态（绝不在
    /// 损坏文件后追加）。
    fn quarantine_corrupt(&self, guard: &mut Option<WinJournalStore>, offset: usize) -> bool {
        Self::quarantine_locked(guard, &self.dir, offset, &self.log)
    }

    /// [`Self::quarantine_corrupt`] 的锁内静态实现（`replay_pending` 复用）。
    fn quarantine_locked(
        guard: &mut Option<WinJournalStore>,
        dir: &Path,
        offset: usize,
        log: &LogSink,
    ) -> bool {
        // 1. 先释放句柄：`WinJournalStore::open` 持有的句柄不带 FILE_SHARE_DELETE，
        //    句柄未关就对 `journal.bin` 改名在 Windows 上必 ACCESS_DENIED
        //    （journal_store.rs compact 同族证据：先 file.take() 再 MoveFileExW）。
        *guard = None;
        // 2. 改名隔离（best-effort 保留物证）。
        let src = dir.join(JOURNAL_FILE);
        let unix_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let dst = dir.join(format!("{JOURNAL_FILE}.corrupt-{unix_ts}"));
        if let Err(e) = std::fs::rename(&src, &dst) {
            log.emit(
                LogLevel::Warn,
                "engine",
                "system_proxy_journal.quarantine_rename_failed",
                "system-proxy journal corrupt file rename failed (ledger disabled; startup continues)",
                &[
                    ("src", &src.display().to_string()),
                    ("dst", &dst.display().to_string()),
                    ("error", &format!("{e}")),
                ],
            );
            return false;
        }
        log.emit(
            LogLevel::Warn,
            "engine",
            "system_proxy_journal.corrupt_quarantined",
            "system-proxy journal corrupt file quarantined (fresh ledger started; startup continues)",
            &[
                ("offset", &offset.to_string()),
                ("quarantined_to", &dst.display().to_string()),
            ],
        );
        // 3. 重开全新文件（失败 = 禁用账本态）。
        match WinJournalStore::open(&JournalPath::from_dir(dir.to_path_buf())) {
            Ok(store) => {
                *guard = Some(store);
                true
            }
            Err(e) => {
                log.emit(
                    LogLevel::Warn,
                    "engine",
                    "system_proxy_journal.reopen_failed",
                    "system-proxy journal reopen after quarantine failed (ledger disabled)",
                    &[("error", &format!("{e:?}"))],
                );
                false
            }
        }
    }

    /// 禁用态追加的日志（append_step 首行短路；一次会话只刷屏一次由调用方降级
    /// 日志承担，这里保持安静避免每次连接重复告警）。
    fn log_disabled_append(&self) {}

    /// 毒化安全的锁获取（编排器自身绝不 panic：mutex 中毒时恢复内部数据继续）。
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<WinJournalStore>> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// ---------------------------------------------------------------------------
// 单测（§5.2 测试 11）：temp-dir 真文件。回放判定经真 `replay_system_proxy_step`
// ——回放路径用「当前真实用户 SID + 随机指纹」保证 SkipForfeit（读真注册表但不
// 写：随机指纹不可能等于真实指纹）；HardFailure 路径用假 SID（capture 失败）。
// ---------------------------------------------------------------------------
