
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use exv_vpn_wire::generated;
use serde::Serialize;
use tokio::sync::mpsc;
use tonic::Status;

/// 有界历史环容量（resume_tick 补拉窗口；超过即从环中逐出，断线超窗事件只留 raw）。
pub const HISTORY_CAPACITY: usize = 256;
/// StreamLogs mpsc 通道容量（与历史环同量级，推送不阻塞 emit）。
const STREAM_CAPACITY: usize = 256;

/// 日志级别，映射 proto `LogEvent.level` 字符串集（debug | info | warn | error）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// 调试（网络快照、会话诊断；始终保留，由界面筛选显示）。
    Debug,
    /// 信息（正常流程里程碑）。
    Info,
    /// 警告（可恢复异常、拒绝）。
    Warn,
    /// 错误（失败/异常终止）。
    Error,
}

impl LogLevel {
    /// proto `LogEvent.level` 字符串（debug | info | warn | error）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// raw 文件的一行（NDJSON）：wire LogEvent 字段 + 单调 tick（与流序列对账）。
///
/// 序列化参考对齐 legacy JSON-frame LogEvent（win32-ipc log_pipe.rs，acceptance-only）；
/// 离线调试文件用 NDJSON（每行一个事件）——raw-dump 仅离线诊断（O4），行格式比
/// u32 BE 长度前缀帧更便于 tail/grep。
#[derive(Debug, Serialize)]
struct RawLogLine<'a> {
    level: &'a str,
    component: &'a str,
    code: &'a str,
    message: &'a str,
    fields: &'a std::collections::HashMap<String, String>,
    timestamp_ms: i64,
    tick: u64,
}

/// engine raw-dump 兜底：mutex + 文件 + append。
///
/// 文件按需打开（首个 `dump` 才建目录/文件——无 raw 事件时不落盘）；任何写失败
/// （目录不可建、文件不可写）自禁用（后续 `dump` 返回 false），并 `tracing::warn!`
/// 记录一次——engine 无完整日志框架，这是最小兜底。
pub struct RawLogDumper {
    /// 是否允许落盘（`disabled` 构造或写失败后为 false）。
    enabled: bool,
    /// raw 文件目录（`engine-raw-<pid>.log` 所在）。
    dir: PathBuf,
    /// 已打开的 append 文件（None = 尚未打开 / 已禁用）。
    file: Mutex<Option<std::fs::File>>,
}

impl RawLogDumper {
    /// 建一个写往 `dir/engine-raw-<pid>.log` 的 dumper（目录在首个 dump 时按需创建）。
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            enabled: true,
            dir,
            file: Mutex::new(None),
        }
    }

    /// 禁用的 dumper：永不落盘、`dump` 恒返回 false（测试/无 wiring 的默认构造用）。
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            dir: PathBuf::new(),
            file: Mutex::new(None),
        }
    }

    /// 机器级默认 raw 日志目录（对齐 W14 journal 约定：ProgramData 优先，回退
    /// LOCALAPPDATA）。
    #[must_use]
    pub fn engine_default_dir() -> PathBuf {
        if let Some(program_data) = std::env::var_os("PROGRAMDATA") {
            let candidate = PathBuf::from(program_data).join("ExvVpn").join("logs");
            if dir_is_usable(&candidate) {
                return candidate;
            }
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("ExvVpn").join("logs");
        }
        std::env::temp_dir().join("ExvVpn").join("logs")
    }

    /// 本 dumper 的 raw 文件路径（`dir/engine-raw-<pid>.log`）。
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.dir
            .join(format!("engine-raw-{}.log", std::process::id()))
    }

    /// 追加一行到 raw 文件（best-effort：打开失败/写失败 → 自禁用并返回 false）。
    ///
    /// 永不 panic、永不阻塞业务路径；禁用后返回 false（调用方可忽略）。
    ///
    /// # Panics
    /// 仅当文件 mutex 被 poison（一个线程在持锁时 panic）时。
    pub fn dump(&self, line: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let mut guard = self.file.lock().expect("raw dump mutex");
        if guard.is_none() {
            match open_append(&self.dir, &self.path()) {
                Ok(file) => *guard = Some(file),
                Err(error) => {
                    self.disable(guard, error);
                    return false;
                }
            }
        }
        {
            // `file` 借用只在 scoped 块内存活，随后 `guard` 才能移给 `disable`。
            let file = guard.as_mut().expect("opened above");
            if let Err(error) = writeln!(file, "{line}") {
                self.disable(guard, error);
                return false;
            }
        }
        true
    }

    /// 自禁用：关掉已打开的文件（若开过），记一次 warn。
    fn disable(
        &self,
        mut guard: std::sync::MutexGuard<'_, Option<std::fs::File>>,
        error: std::io::Error,
    ) {
        *guard = None;
        tracing::warn!(path = %self.path().display(), error = %error, "engine raw-dump disabled; logs fall back to push-only");
    }
}

/// 建目录并打开 append 文件（目录不可建/文件不可开 → `Err`）。
fn open_append(dir: &Path, path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

/// 目录是否可建可用（对齐 W14 journal 的 probe 约定）。
fn dir_is_usable(dir: &Path) -> bool {
    if !dir.is_dir() && std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe_id = std::process::id();
    let probe = dir.join(format!(".exv-log-write-probe-{probe_id}"));
    match std::fs::File::create(&probe) {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// LogSink 的内部可变态。
struct LogSinkInner {
    /// 挂接的 StreamLogs 推送通道集（R3 扇出：多消费方可同时挂接，各持独立 mpsc
    /// 通道、容量隔离、互不饿死——一个慢消费者填满自己的通道只丢自己的事件，不
    /// 影响其他消费者）。断线（接收端 drop → `try_send` 返回 `Closed`）即从集合移除。
    push: Vec<mpsc::Sender<Result<generated::LogEvent, Status>>>,
    /// 有界历史环（(单调 tick, 事件)），供 resume_tick 补拉。
    history: VecDeque<(u64, generated::LogEvent)>,
    /// 下一个单调 tick（从 1 起；resume_tick 的序列基础）。
    next_tick: u64,
    /// raw-dump 兜底。
    dumper: RawLogDumper,
}

/// engine 日志端点：推送 StreamLogs + raw-dump 兜底 + resume 补拉。
pub struct LogSink {
    inner: Mutex<LogSinkInner>,
}

impl LogSink {
    /// 用指定 dumper 建 sink（产品路径传机器级 dumper；测试传临时目录 dumper）。
    #[must_use]
    pub fn new(dumper: RawLogDumper) -> Self {
        Self {
            inner: Mutex::new(LogSinkInner {
                push: Vec::new(),
                history: VecDeque::with_capacity(HISTORY_CAPACITY),
                next_tick: 0,
                dumper,
            }),
        }
    }

    /// 默认产品 sink：raw 落盘到机器级日志目录（compose/engine-startup wiring 用）。
    #[must_use]
    pub fn engine_default() -> Self {
        Self::new(RawLogDumper::new(RawLogDumper::engine_default_dir()))
    }

    /// 空 sink：raw-dump 禁用（`HelperControlService::new()` 的测试安全默认，不落盘）。
    #[must_use]
    pub fn null() -> Self {
        Self::new(RawLogDumper::disabled())
    }

    /// emit 一条结构化日志事件（同步、非阻塞）。
    ///
    /// 流程：分配严格递增 tick → 入有界历史环 → 扇出给所有挂接通道（逐通道
    /// `try_send`：成功即投递；通道满 = 该消费者慢 → 只丢它的事件；通道已关闭 =
    /// 断线 → 从集合移除）→ **至少一个**通道接受即完成；无挂接 / 全部通道满 /
    /// 最后一个通道刚断开 → 落 raw 兜底。永不阻塞业务路径（无 `await`，通道满不等待）。
    ///
    /// 调用方保证 `fields` 不含 secret/capability/凭据（wire 契约同款禁令）。
    pub fn emit(
        &self,
        level: LogLevel,
        component: &str,
        code: &str,
        message: &str,
        fields: &[(&str, &str)],
    ) {
        let mut inner = self.inner.lock().expect("log sink mutex");
        inner.next_tick = inner.next_tick.saturating_add(1);
        let tick = inner.next_tick;
        let event = build_event(level, component, code, message, fields);

        if inner.history.len() == HISTORY_CAPACITY {
            inner.history.pop_front();
        }
        inner.history.push_back((tick, event.clone()));

        // Fan out to every attached consumer channel. `drain` lets us drop closed
        // senders (dead gRPC streams) while iterating; live senders are re-collected.
        let mut live = Vec::with_capacity(inner.push.len());
        let mut delivered = false;
        for tx in inner.push.drain(..) {
            match tx.try_send(Ok(event.clone())) {
                Ok(()) => {
                    delivered = true;
                    live.push(tx);
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // 通道满（该消费者消费不及时）：只丢这个消费者的事件，不饿死其他。
                    live.push(tx);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    // 断线：接收端 drop（gRPC stream 关闭）→ 从集合移除该消费者。
                }
            }
        }
        inner.push = live;

        // At least one live consumer accepted the event → push-only; otherwise fall
        // back to the raw dump (no attachment / all channels full / last closed).
        if delivered {
            return;
        }
        dump_ev(&inner.dumper, tick, &event);
    }

    /// 挂接一条 StreamLogs 推送流，返回接收端（由 gRPC server 转成 ReceiverStream）。
    ///
    /// `resume_tick > 0`：先按 tick 序补拉有界历史环中 `tick > resume_tick` 的事件
    /// （best-effort，通道满即停），再实时推送；`resume_tick == 0`：从当前流位置开始
    /// （不补拉）。R3 扇出：每次挂接**追加**一条独立通道（各消费者容量隔离、互不
    /// 饿死）；断线由 `emit` 在 `try_send` 时识别 `Closed` 移除，不替换其他消费者。
    #[must_use]
    pub fn open_stream(
        &self,
        resume_tick: u64,
    ) -> mpsc::Receiver<Result<generated::LogEvent, Status>> {
        let mut inner = self.inner.lock().expect("log sink mutex");
        let (tx, rx) = mpsc::channel(STREAM_CAPACITY);
        if resume_tick > 0 {
            for (tick, event) in &inner.history {
                if *tick > resume_tick {
                    if tx.try_send(Ok(event.clone())).is_err() {
                        break; // 通道满：补拉 best-effort，不阻塞。
                    }
                }
            }
        }
        inner.push.push(tx);
        rx
    }

    /// 最近一次 emit 的单调 tick（0 = 尚无事件）。core 用它做重连的 resume_tick。
    #[must_use]
    pub fn last_tick(&self) -> u64 {
        self.inner.lock().expect("log sink mutex").next_tick
    }

    /// 是否挂接着至少一条推送通道（测试/观测）。
    #[must_use]
    pub fn is_push_attached(&self) -> bool {
        self.inner.lock().expect("log sink mutex").push.is_empty() == false
    }

    /// 当前 raw dump 文件路径（未创建时也只是候选路径）。
    #[must_use]
    pub fn dump_path(&self) -> PathBuf {
        self.inner.lock().expect("log sink mutex").dumper.path()
    }
}

/// 组一条 wire `LogEvent`（wall-clock epoch ms；fields 复制为 `String`）。
fn build_event(
    level: LogLevel,
    component: &str,
    code: &str,
    message: &str,
    fields: &[(&str, &str)],
) -> generated::LogEvent {
    let mut field_map = std::collections::HashMap::with_capacity(fields.len());
    for (key, value) in fields {
        field_map.insert((*key).to_string(), (*value).to_string());
    }
    generated::LogEvent {
        level: level.as_str().to_string(),
        component: component.to_string(),
        code: code.to_string(),
        message: message.to_string(),
        fields: field_map,
        timestamp_ms: wall_clock_ms(),
    }
}

/// 写一条事件到 raw dump（NDJSON 行 = wire 字段 + tick）。
fn dump_ev(dumper: &RawLogDumper, tick: u64, event: &generated::LogEvent) {
    let line = RawLogLine {
        level: &event.level,
        component: &event.component,
        code: &event.code,
        message: &event.message,
        fields: &event.fields,
        timestamp_ms: event.timestamp_ms,
        tick,
    };
    let json = serde_json::to_string(&line).unwrap_or_else(|_| {
        format!(
            r#"{{"level":{},"component":{},"code":{},"message":{},"timestamp_ms":{},"tick":{}}}"#,
            serde_json::to_string(&event.level).unwrap_or_default(),
            serde_json::to_string(&event.component).unwrap_or_default(),
            serde_json::to_string(&event.code).unwrap_or_default(),
            serde_json::to_string(&event.message).unwrap_or_default(),
            event.timestamp_ms,
            tick,
        )
    });
    dumper.dump(&json);
}

/// 当前 wall-clock epoch 毫秒（UTC；proto 约定 0 = unknown，本实现恒填真实值）。
fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// ---------------------------------------------------------------------------
// 单元测试：序列化往返 + tick 单调（集成契约测试在 tests/log_sink.rs）。
// ---------------------------------------------------------------------------
