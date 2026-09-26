//! Core 日志聚合存储（MAC-OBS-13 S1；适配 win32 `log_aggregator.rs` 语义）。
//!
//! 合并 Engine `StreamLogs` 事件（`append_engine`）与 Core 自身事件
//! （`append_core`）为**单一条目流**，供 `LogsList`（历史分页）与未来 S4
//! `LogsClear`（truncate）消费。
//!
//! ## 存储选型：会话期内存 + 落盘 JSONL 双轨
//!
//! * **会话期内存**（`VecDeque` 有界 ring）是读取路径（[`Self::list`]）的事实源：
//!   读不再每次重读文件（win32 单锁重读文件的形态在锁内做 IO，这里把 IO 移出
//!   热路径），追加/读取/淘汰同一 `Mutex` 串行一致。
//! * **落盘聚合文件**（`config_dir()/logs/aggregated.jsonl`，复用 `EXV_CONFIG_DIR`
//!   优先级）是持久层：Core 重启时回读文件重建内存 ring 与游标（保留最近
//!   [`MAX_ENTRIES`] 条）。每行一条 JSON（JSONL），**行内存 `seq`**。
//! * **`seq` 由 Core 派生、单调递增**（任务口径：wire `LogEvent` 行内无 seq）。
//!   与 win32"seq=行号"的有意差异：本存储有容量淘汰（删头部行会使行号平移、
//!   破坏增量游标），行内显式 `seq` + 重启恢复计数器让淘汰与游标正交；S4 清空
//!   后游标归零、seq 从 1 重启（保留 win32 clear 语义）。
//! * **落盘降级**：文件打不开/写失败（磁盘满、只读卷）时置 `file = None` 降级为
//!   会话期内存-only，聚合不因磁盘故障停摆；写失败不推进磁盘，内存游标继续
//!   （磁盘与内存分叉只发生在降级态，重启后以文件为准）。
//!
//! ## 容量上限与淘汰
//!
//! 内存与磁盘条目总量有界：`append` 时超过 [`MAX_ENTRIES`] 即触发 compact——
//! 丢弃最旧条目、重写文件只保留内存 ring 现存的最近条目（约一半量级，摊薄重写
//! 频率；5000 条 JSONL 约 1 MB，单锁内一次重写在控制面日志量级下可接受）。
//!
//! ## 脱敏（入库边界）
//!
//! 对齐 proto `LogEvent` 安全注解与 wire `redact.rs` 的固定 marker 风格：
//! * `fields` 中 key 含敏感子串（`password`/`secret`/`token`/`credential`/`auth`，
//!   大小写不敏感）→ 值整体替换 `<redacted>`；
//! * 经 [`Self::register_secret`] 登记的已知敏感值（如 saved password）在
//!   `message`/`fields` 值中出现 → 替换 `<redacted>`（sentinel 注入测试覆盖）。
//!
//! Engine sink 侧另有字段白名单（MAC-OBS-13 计划 §5.6），两层防御互不替代。

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Seek, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use exv_vpn_wire::generated::LogEvent;

/// 默认聚合日志文件名（`config_dir()/logs/` 下；对齐 win32）。
pub const AGGREGATED_LOG_FILE: &str = "aggregated.jsonl";
/// 日志子目录名（config 目录下；对齐 win32）。
pub const LOGS_SUBDIR: &str = "logs";

/// 条目总量上限（内存与磁盘同界；UI 2000 条窗口的 2.5 倍余量）。
pub const MAX_ENTRIES: usize = 5000;
/// compact 周期：每追加这么多条触发一次磁盘重写（只保留内存 ring 现存最近
/// 条目）；磁盘行数峰值 ≈ [`MAX_ENTRIES`] + [`COMPACTION_INTERVAL`]，有界。
pub const COMPACTION_INTERVAL: u64 = 2500;

/// fields key 的敏感子串名单（大小写不敏感子串匹配；命中即整体脱敏值）。
const SENSITIVE_KEY_SUBSTRINGS: [&str; 5] = ["password", "passwd", "secret", "token", "credential"];

/// 入库脱敏的固定 marker（对齐 wire `redact::redact_secret` 的 marker-only 风格）。
pub const REDACTED: &str = "<redacted>";

/// 聚合存储的条目（内存与磁盘行同构；`seq` 由 Core 派生，单调递增）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// 单调行号（Core 派生；重启从文件恢复；S4 清空后从 1 重启）。
    pub seq: u64,
    /// 事件产生时刻（epoch 毫秒 UTC；Engine 事件可为 0 = 未知）。
    pub timestamp_ms: i64,
    /// Core 接收/落盘时刻（epoch 毫秒 UTC）。
    pub received_ms: i64,
    /// `info` | `warn` | `error`。
    pub level: String,
    /// `engine` | `core`。
    pub source: String,
    /// 组件（如 `pipeline`/`packet`/`protocol`/`platform`/`core`）。
    pub component: String,
    /// 稳定诊断码，无则空串。
    pub code: String,
    /// 人类可读消息（不含诊断栈与凭据）。
    pub message: String,
    /// 结构化键值元数据（入库边界脱敏）。
    pub fields: BTreeMap<String, String>,
}

impl LogEntry {
    /// 聚合器保留键覆盖来源同名字段，复制/导出可追溯接收延迟及顺序。
    pub fn to_wire_event(&self) -> LogEvent {
        let mut fields: std::collections::HashMap<_, _> = self.fields.clone().into_iter().collect();
        fields.insert("log.seq".into(), self.seq.to_string());
        fields.insert("log.received_ms".into(), self.received_ms.to_string());
        fields.insert("log.source".into(), self.source.clone());
        LogEvent { level: self.level.clone(), component: self.component.clone(), code: self.code.clone(),
            message: self.message.clone(), fields, timestamp_ms: self.timestamp_ms }
    }
}

/// 增量拉取返回页：`seq > after_seq` 的至多 `limit` 条 + 稳健游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPage {
    pub entries: Vec<LogEntry>,
    /// 下一轮游标：最后返回条目 `seq + 1`；空页 = `last_seq + 1`。
    pub next_seq: u64,
}

struct AggregatorInner {
    /// 追加写句柄；`None` = 落盘降级（内存-only）。
    file: Option<File>,
    /// 会话期内存 ring（有界；读取路径事实源；重启从文件回放）。
    entries: VecDeque<LogEntry>,
    /// 已分配的最大 `seq`（重启从文件恢复；S4 clear 归零）。
    last_seq: u64,
    /// 已登记的敏感值（入库边界做 `<redacted>` 替换；如 saved password）。
    redact_values: Vec<String>,
}

/// 条目来源标记（磁盘 `source` 字段字面量：`engine` | `core`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogSource {
    /// 经 `HelperControl.StreamLogs` 从 Engine 推送的日志。
    Engine,
    /// Core 自身产生的日志。
    Core,
}

impl LogSource {
    /// 磁盘 `source` 字段的稳定字面量。
    const fn as_str(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Core => "core",
        }
    }
}

/// 待入库的原始条目（脱敏前；`append` 的入参包）。
struct IncomingEntry {
    source: LogSource,
    level: String,
    component: String,
    code: String,
    message: String,
    timestamp_ms: i64,
    fields: BTreeMap<String, String>,
}

/// Core 日志聚合存储：会话期内存 ring + 落盘 JSONL 双轨，单锁串行一致。
pub struct LogAggregator {
    inner: Mutex<AggregatorInner>,
}

impl fmt::Debug for LogAggregator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 不泄露 entries/redact_values 内容：调试渲染只暴露规模事实。
        let inner = lock_recover(&self.inner);
        formatter
            .debug_struct("LogAggregator")
            .field("persisted", &inner.file.is_some())
            .field("entries", &inner.entries.len())
            .field("last_seq", &inner.last_seq)
            .field("redact_values", &inner.redact_values.len())
            .finish()
    }
}

/// 默认聚合日志路径：`config_dir()/logs/aggregated.jsonl`（复用 `EXV_CONFIG_DIR`
/// 优先级，对齐 win32 host 的 `aggregated_log_path`）。
#[must_use]
pub fn aggregated_log_path() -> PathBuf {
    crate::config_paths::config_dir()
        .join(LOGS_SUBDIR)
        .join(AGGREGATED_LOG_FILE)
}

impl LogAggregator {
    /// 在 `path` 打开（或创建）聚合文件并回放历史（内存 ring 保留最近
    /// [`MAX_ENTRIES`] 条，游标从文件恢复）。
    ///
    /// 文件打不开（权限/只读卷）时**不失败**：降级为内存-only（`persisted` 假），
    /// 会话期聚合与 `LogsList` 照常工作——落盘是持久层而非可用性前提。
    #[must_use]
    pub fn open(path: &Path) -> Self {
        let file = open_append_handle(path);
        let (entries, last_seq) = replay_from_disk(path);
        Self {
            inner: Mutex::new(AggregatorInner {
                file,
                entries,
                last_seq,
                redact_values: Vec::new(),
            }),
        }
    }

    /// 在默认路径（`aggregated_log_path()`）打开聚合存储。
    #[must_use]
    pub fn open_default() -> Self {
        Self::open(&aggregated_log_path())
    }

    /// 登记一个已知敏感值（如 saved password）：此后入库的 `message`/`fields`
    /// 值中出现该值即替换 [`REDACTED`]。幂等（重复登记去重）。
    pub fn register_secret(&self, secret: &str) {
        if secret.is_empty() {
            return;
        }
        let mut inner = lock_recover(&self.inner);
        if !inner.redact_values.iter().any(|known| known == secret) {
            inner.redact_values.push(secret.to_owned());
        }
    }

    /// 落盘是否可用（`false` = 打开/写入失败后的内存-only 降级态）。
    #[must_use]
    pub fn persisted(&self) -> bool {
        lock_recover(&self.inner).file.is_some()
    }

    /// 已分配的最大 `seq`（= 条目总数，除非发生过淘汰/清空）。
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        lock_recover(&self.inner).last_seq
    }

    /// 追加一条 Engine 事件（`StreamLogs` 推送；`source = "engine"`）。
    ///
    /// 入库边界脱敏后落内存与磁盘；写失败降级内存-only，不中断调用方。
    pub fn append_engine(&self, event: &LogEvent) -> LogEntry {
        self.append(IncomingEntry {
            source: LogSource::Engine,
            level: event.level.clone(),
            component: event.component.clone(),
            code: event.code.clone(),
            message: event.message.clone(),
            timestamp_ms: event.timestamp_ms,
            fields: event
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        })
    }

    /// 追加一条 Core 自身事件（`source = "core"`；两个时间戳均为当前时刻）。
    pub fn append_core(
        &self,
        level: &str,
        component: &str,
        code: &str,
        message: &str,
        fields: &[(&str, String)],
    ) -> LogEntry {
        let now = now_ms();
        let mut entry = self.append(IncomingEntry {
            source: LogSource::Core,
            level: level.to_owned(),
            component: component.to_owned(),
            code: code.to_owned(),
            message: message.to_owned(),
            timestamp_ms: now,
            fields: fields
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        });
        entry.timestamp_ms = now;
        entry.received_ms = now;
        entry
    }

    /// 统一入库路径：脱敏 → 分配 seq → 内存 ring（超限淘汰）→ 落盘（周期
    /// compact；写失败降级）。
    fn append(&self, incoming: IncomingEntry) -> LogEntry {
        let received_ms = now_ms();
        let redact_values = lock_recover(&self.inner).redact_values.clone();
        let (message, fields) = redact(&incoming.message, incoming.fields, &redact_values);
        let mut entry = LogEntry {
            seq: 0,
            timestamp_ms: incoming.timestamp_ms,
            received_ms,
            level: incoming.level,
            source: incoming.source.as_str().to_owned(),
            component: incoming.component,
            code: incoming.code,
            message,
            fields,
        };
        let mut inner = lock_recover(&self.inner);
        inner.last_seq = inner.last_seq.saturating_add(1);
        entry.seq = inner.last_seq;
        inner.entries.push_back(entry.clone());
        if inner.entries.len() > MAX_ENTRIES {
            inner.entries.pop_front();
        }
        write_entry(&mut inner, &entry);
        // 周期性磁盘 compact：把磁盘行数收敛回内存 ring 内容（淘汰头部行）。
        if inner.last_seq.is_multiple_of(COMPACTION_INTERVAL) {
            compact(&mut inner);
        }
        entry
    }

    /// 增量拉取：`seq > after_seq` 的至多 `limit` 条（`limit == 0` 无限）。
    ///
    /// 从内存 ring 过滤（O(n)；n ≤ [`MAX_ENTRIES`]）；空页 `next_seq = last_seq + 1`
    /// （游标越过已消费槽，客户端不空转）。
    #[must_use]
    pub fn list(&self, after_seq: u64, limit: usize) -> LogPage {
        let inner = lock_recover(&self.inner);
        let entries: Vec<LogEntry> = inner
            .entries
            .iter()
            .filter(|entry| entry.seq > after_seq)
            .take(if limit == 0 { usize::MAX } else { limit })
            .cloned()
            .collect();
        let next_seq = entries
            .last()
            .map_or_else(|| inner.last_seq + 1, |entry| entry.seq + 1);
        LogPage { entries, next_seq }
    }

    /// 清空聚合日志（S4 `LogsClear` 底座；S1 只预留与单测，不接 RPC）：
    /// truncate 文件、内存清空、游标归零——此后条目从 `seq = 1` 重新开始。
    ///
    /// 与追加/读取同一 `Mutex` 串行一致；truncate 后写句柄 seek 复位避免空洞。
    ///
    /// # Errors
    /// truncate 失败 → `std::io::Error`（内存与游标仍清空；落盘进入降级态）。
    pub fn clear(&self) -> std::io::Result<()> {
        let mut inner = lock_recover(&self.inner);
        if let Some(file) = inner.file.as_mut() {
            let result = file.set_len(0).and_then(|()| {
                file.seek(std::io::SeekFrom::Start(0))?;
                file.flush()
            });
            if result.is_err() {
                inner.file = None; // truncate 失败：进入内存-only 降级态。
                return result;
            }
        }
        inner.entries.clear();
        inner.last_seq = 0;
        Ok(())
    }
}

/// 打开追加写句柄（create + write + truncate(false)；追加前 seek 到 EOF）。
///
/// 用 write 而非 append：`clear`（S4）的 `set_len(0)` 与句柄 seek 复位需要写句柄。
fn open_append_handle(path: &Path) -> Option<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)
            .ok()?;
        file.seek(std::io::SeekFrom::End(0)).ok()?;
        return Some(file);
    }
    None
}

/// 从磁盘回放：解析每行 JSON（坏行跳过），内存只保留最近 [`MAX_ENTRIES`] 条，
/// 游标 = 已解析条目的最大 `seq`（无文件/全坏行 → 空库游标 0）。
fn replay_from_disk(path: &Path) -> (VecDeque<LogEntry>, u64) {
    let Ok(file) = File::open(path) else {
        return (VecDeque::new(), 0);
    };
    let mut entries = VecDeque::new();
    let mut last_seq = 0;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
            last_seq = last_seq.max(entry.seq);
            entries.push_back(entry);
            if entries.len() > MAX_ENTRIES {
                entries.pop_front();
            }
        }
    }
    (entries, last_seq)
}

/// 把一条条目写入磁盘（追加 seek EOF + 行 JSON + flush）。
///
/// 写失败 → 句柄置 `None`（进入内存-only 降级态），不 panic、不回滚内存游标。
fn write_entry(inner: &mut AggregatorInner, entry: &LogEntry) {
    let Some(file) = inner.file.as_mut() else {
        return;
    };
    let write = (|| -> std::io::Result<()> {
        file.seek(std::io::SeekFrom::End(0))?;
        serde_json::to_writer(&mut *file, entry).map_err(std::io::Error::other)?;
        file.write_all(b"\n")?;
        file.flush()
    })();
    if write.is_err() {
        inner.file = None;
    }
}

/// 容量淘汰：重写文件只保留内存 ring 中现存（最近）条目。
///
/// 内存 ring 已在 `append` 中淘汰过头部队列；compact 把磁盘对齐到 ring 内容。
/// seq 不受影响（行内存 seq）——增量游标跨淘汰稳定。
fn compact(inner: &mut AggregatorInner) {
    let Some(file) = inner.file.as_mut() else {
        return;
    };
    let rewrite = (|| -> std::io::Result<()> {
        file.set_len(0)?;
        file.seek(std::io::SeekFrom::Start(0))?;
        for entry in &inner.entries {
            serde_json::to_writer(&mut *file, entry).map_err(std::io::Error::other)?;
            file.write_all(b"\n")?;
        }
        file.flush()
    })();
    if rewrite.is_err() {
        inner.file = None; // 重写失败：降级内存-only，不中断聚合。
    }
}

/// 入库边界脱敏：`fields` 敏感 key 的值替换 [`REDACTED`]；`message`/`fields` 值
/// 中出现已登记敏感值 → 替换 [`REDACTED`]。
fn redact(
    message: &str,
    fields: BTreeMap<String, String>,
    redact_values: &[String],
) -> (String, BTreeMap<String, String>) {
    let scrub_text = |text: &str| -> String {
        let mut scrubbed = text.to_owned();
        for secret in redact_values {
            if !secret.is_empty() && scrubbed.contains(secret.as_str()) {
                scrubbed = scrubbed.replace(secret.as_str(), REDACTED);
            }
        }
        scrubbed
    };
    let scrubbed_fields = fields
        .into_iter()
        .map(|(key, value)| {
            let value = if is_sensitive_key(&key) {
                REDACTED.to_owned()
            } else {
                scrub_text(&value)
            };
            (key, value)
        })
        .collect();
    (scrub_text(message), scrubbed_fields)
}

/// key 是否命中敏感子串名单（大小写不敏感）。
fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_lowercase();
    SENSITIVE_KEY_SUBSTRINGS
        .iter()
        .any(|needle| lowered.contains(needle))
}

/// 当前 epoch 毫秒（UTC）。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// 锁恢复（中毒即取回内部数据；与 `kernel_control_service::lock_recover` 同语义）。
fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// 单元测试：落盘/回放、seq 单调与重启恢复、容量淘汰、脱敏 sentinel、clear 预留。
// ---------------------------------------------------------------------------
