//! S7 最小文件日志设施（darwin service-agent 侧，2026-10-26 验收修复计划）。
//!
//! 定位：服务代理此前完全无日志——launchd daemon 与一次性提权动词（安装/卸载/
//! 启动/拉起 engine/清扫残留）失败时只能靠 stderr 瞬态输出，无法事后取证。本设施
//! 提供固定路径的本地文件记录关键事件；行格式对齐 core `log_aggregator` 的
//! `aggregated.jsonl` 风格字段（与 engine 侧 `file_log` 同一形态，两 crate 各自内联，
//! 刻意不互相依赖：daemon 二进制保持仅 libc 依赖）。
//!
//! ## 形态
//!
//! * 固定路径（生产 [`DEFAULT_LOG_PATH`]，落在既有安装目录约定
//!   `/Library/Application Support/EXV/ServiceAgent/` 的 `logs/` 子目录；服务代理
//!   的全部提权入口均以 root 运行，可写），追加写 JSONL，单行一条事件；
//! * 单文件大小上限 [`DEFAULT_MAX_FILE_BYTES`]（1 MiB），写满即轮转，保留
//!   [`DEFAULT_ROTATED_GENERATIONS`] 个轮转副本（`.1`/`.2`），总占用约 3 MiB；
//! * 写失败（目录不可建/文件不可写）自禁用并静默降级：日志设施绝不阻断业务
//!   路径，不 panic、不阻塞（对齐 win32 engine `RawLogDumper` 的自禁用语义）。
//!
//! ## 脱敏（落盘边界）
//!
//! * `fields` 的 key 命中敏感子串名单（[`SENSITIVE_KEY_SUBSTRINGS`]，大小写不敏感）
//!   → 值整体替换为 [`REDACTED`]（对齐 core `log_aggregator` 的入库脱敏名单并按
//!   S7 扩充 `cookie`/`session`）；
//! * 调用点纪律：凭据/密码/cookie/token/session id 一律不得进入 `message` 或
//!   `fields`——redact 是兜底防线，不是许可；事件字段只允许 pid/owner uid/决策
//!   布尔/错误类别一类取证元数据（与 wire 契约「错误只编码稳定类别」同纪律）。

use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

/// 生产固定日志文件路径。
pub const DEFAULT_LOG_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/logs/service-agent.log";

/// 单文件大小上限（默认 1 MiB；当前行写入后超限即轮转）。
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// 轮转副本数（`.1`/`.2`；当前文件 + 2 副本 = 3 文件，总上限约 3 MiB）。
pub const DEFAULT_ROTATED_GENERATIONS: usize = 2;

/// 事件源标识（行内 `source` 字段；与 aggregated.jsonl 的 `source` 同位）。
pub const SOURCE: &str = "service-agent";

/// `fields` key 的敏感子串名单（大小写不敏感子串匹配；命中即整体脱敏值）。
///
/// 前五项与 core `log_aggregator` 的 `SENSITIVE_KEY_SUBSTRINGS` 对齐；
/// `cookie`/`session` 为 S7 按「凭据/密码/cookie/token/session id 一律不落日志」
/// 的脱敏纪律扩充（`session` 子串同时覆盖 `session_id`/`sessionId` 等变体）。
pub const SENSITIVE_KEY_SUBSTRINGS: [&str; 7] = [
    "password", "passwd", "secret", "token", "credential", "cookie", "session",
];

/// 落盘脱敏的固定 marker（对齐 core `log_aggregator` 与 wire redact 的 marker 风格）。
pub const REDACTED: &str = "<redacted>";

/// 判定 fields key 是否命中敏感子串名单（大小写不敏感子串匹配）。
#[must_use]
pub fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SENSITIVE_KEY_SUBSTRINGS
        .iter()
        .any(|needle| lowered.contains(needle))
}

/// 最小文件日志器：mutex + append + 大小轮转；写失败自禁用（静默降级）。
pub struct FileLog {
    state: Mutex<LogState>,
}

struct LogState {
    /// 是否仍允许落盘（构造禁用或写失败后为 false）。
    enabled: bool,
    /// 当前文件路径（轮转副本以 `.1`/`.2` 后缀挂在同目录）。
    path: PathBuf,
    /// 单文件大小上限。
    max_bytes: u64,
    /// 轮转副本数。
    generations: usize,
    /// 已打开的追加句柄（`None` = 尚未打开 / 已禁用）。
    file: Option<File>,
    /// 当前文件已写字节（轮转判据；重新打开既有文件时取其现有长度）。
    written: u64,
}

impl FileLog {
    /// 建一个写往固定路径、默认轮转上限（1 MiB × 3 文件）的日志器。
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self::with_limits(path, DEFAULT_MAX_FILE_BYTES, DEFAULT_ROTATED_GENERATIONS)
    }

    /// 可注入上限的构造（单测加速用；生产经 [`FileLog::new`] 取默认值）。
    #[must_use]
    pub fn with_limits(path: PathBuf, max_bytes: u64, generations: usize) -> Self {
        Self {
            state: Mutex::new(LogState {
                enabled: true,
                path,
                max_bytes: max_bytes.max(1),
                generations: generations.max(1),
                file: None,
                written: 0,
            }),
        }
    }

    /// 禁用的日志器：永不落盘（无 wiring 场景与测试的安全默认）。
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            state: Mutex::new(LogState {
                enabled: false,
                path: PathBuf::new(),
                max_bytes: DEFAULT_MAX_FILE_BYTES,
                generations: DEFAULT_ROTATED_GENERATIONS,
                file: None,
                written: 0,
            }),
        }
    }

    /// 卸载终态：释放文件句柄，后续事件及轮转不再创建日志目录。
    pub fn shutdown(&self) {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        disable(&mut state);
    }

    /// 本日志器的文件路径（禁用态返回空路径）。
    #[must_use]
    pub fn path(&self) -> PathBuf {
        match self.state.lock() {
            Ok(state) => state.path.clone(),
            Err(poisoned) => poisoned.into_inner().path.clone(),
        }
    }

    /// 追加一条事件（best-effort：任何失败自禁用，绝不 panic、不阻断业务）。
    ///
    /// `level` 取 `info` | `warn` | `error`；`fields` 中命中敏感子串 key 的值被
    /// 整体脱敏为 [`REDACTED`]。单行 JSON，字段与 aggregated.jsonl 同风格：
    /// `timestamp_ms`/`level`/`source`/`component`/`code`/`message`/`fields`。
    pub fn log(
        &self,
        level: &str,
        component: &str,
        code: &str,
        message: &str,
        fields: &[(&str, String)],
    ) {
        let line = encode_line(
            SOURCE,
            level,
            component,
            code,
            message,
            fields,
            wall_clock_ms(),
        );
        // 锁 poison：静默丢弃（日志设施不得拖垮持锁 panic 的业务线程）。
        if let Ok(mut state) = self.state.lock() {
            append_line(&mut state, &line);
        }
    }
}

/// 进程级默认日志器（固定生产路径；单测用 [`FileLog::with_limits`] 注入临时目录）。
#[must_use]
pub fn default_log() -> &'static FileLog {
    static DEFAULT: OnceLock<FileLog> = OnceLock::new();
    DEFAULT.get_or_init(|| FileLog::new(PathBuf::from(DEFAULT_LOG_PATH)))
}

/// 追加一行：按需打开 → 超限先轮转 → 写入；任一步失败自禁用。
fn append_line(state: &mut LogState, line: &str) {
    if !state.enabled {
        return;
    }
    if state.file.is_none() && !open_current(state) {
        return;
    }
    let line_bytes = u64::try_from(line.len()).unwrap_or(u64::MAX) + 1; // 追加 '\n'
    if state.written + line_bytes > state.max_bytes && !rotate(state) {
        return;
    }
    let Some(file) = state.file.as_mut() else {
        disable(state);
        return;
    };
    match writeln!(file, "{line}") {
        Ok(()) => state.written += line_bytes,
        Err(_) => disable(state),
    }
}

/// 打开（或重新打开）当前文件：建父目录 + 追加句柄；失败自禁用。
fn open_current(state: &mut LogState) -> bool {
    let opened: std::io::Result<File> = (|| {
        if let Some(parent) = state.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&state.path)
    })();
    if let Ok(file) = opened {
        // 追加语义：进程重启后接续既有文件，轮转判据从现有长度起算。
        state.written = file.metadata().map_or(0, |metadata| metadata.len());
        state.file = Some(file);
        true
    } else {
        disable(state);
        false
    }
}

/// 轮转：`.1`→`.2`，当前→`.1`；最老副本删除；失败自禁用（静默降级）。
fn rotate(state: &mut LogState) -> bool {
    state.file = None;
    let result: std::io::Result<()> = (|| {
        let oldest = rotated_path(&state.path, state.generations);
        if oldest.exists() {
            std::fs::remove_file(oldest)?;
        }
        for index in (1..state.generations).rev() {
            let from = rotated_path(&state.path, index);
            let to = rotated_path(&state.path, index + 1);
            if from.exists() {
                std::fs::rename(&from, &to)?;
            }
        }
        std::fs::rename(&state.path, rotated_path(&state.path, 1))?;
        Ok(())
    })();
    if let Ok(()) = result {
        open_current(state)
    } else {
        disable(state);
        false
    }
}

/// 自禁用：丢弃句柄、停写（写失败静默降级，不再尝试恢复）。
fn disable(state: &mut LogState) {
    state.file = None;
    state.enabled = false;
}

/// 轮转副本路径：`<name>.<generation>`（同目录；`service-agent.log.1` 形态）。
fn rotated_path(path: &Path, generation: usize) -> PathBuf {
    let mut name = path.file_name().map_or_else(
        || std::ffi::OsString::from("log"),
        std::ffi::OsStr::to_os_string,
    );
    let suffix = format!(".{generation}");
    name.push(suffix.as_str());
    path.with_file_name(name)
}

/// 编码单行 JSON 事件（固定键序；fields 保持调用方顺序，敏感值先脱敏）。
fn encode_line(
    source: &str,
    level: &str,
    component: &str,
    code: &str,
    message: &str,
    fields: &[(&str, String)],
    timestamp_ms: i64,
) -> String {
    let mut line = String::with_capacity(128);
    line.push_str("{\"timestamp_ms\":");
    line.push_str(&timestamp_ms.to_string());
    line.push_str(",\"level\":");
    push_json_string(&mut line, level);
    line.push_str(",\"source\":");
    push_json_string(&mut line, source);
    line.push_str(",\"component\":");
    push_json_string(&mut line, component);
    line.push_str(",\"code\":");
    push_json_string(&mut line, code);
    line.push_str(",\"message\":");
    push_json_string(&mut line, message);
    line.push_str(",\"fields\":{");
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            line.push(',');
        }
        push_json_string(&mut line, key);
        line.push(':');
        let value = if is_sensitive_key(key) {
            REDACTED
        } else {
            value.as_str()
        };
        push_json_string(&mut line, value);
    }
    line.push_str("}}");
    line
}

/// 追加一个 JSON 字符串字面量（转义 `"`/`\`/控制字符；非 ASCII 原样保留）。
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => push_control_escape(out, other as u32),
            other => out.push(other),
        }
    }
    out.push('"');
}

/// 控制字符转义为 `\u00XX`（显式 `fmt::Write` 调用，避免与模块级 `io::Write`
/// 导入在 `String` 上的方法歧义）。
fn push_control_escape(out: &mut String, code: u32) {
    let _ = std::fmt::Write::write_fmt(out, format_args!("\\u{code:04x}"));
}

/// 当前 wall-clock epoch 毫秒（UTC；与 core/engine 日志同一口径）。
fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

// ---------------------------------------------------------------------------
// 单元测试：JSONL 写入形态、脱敏规则、轮转上限、写失败静默降级。
// ---------------------------------------------------------------------------
