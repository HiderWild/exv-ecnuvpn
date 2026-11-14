

#[cfg(feature = "timing")]
use std::io::Write;
#[cfg(feature = "timing")]
use std::path::PathBuf;
#[cfg(feature = "timing")]
use std::sync::OnceLock;
use std::time::Instant;

/// 计时 sink 的解析后路径（惰性一次；`EXV_TIMING_FILE` 覆盖，否则 `<temp>/exv-timing-<pid>.log`）。
#[cfg(feature = "timing")]
fn timing_file() -> PathBuf {
    if let Ok(p) = std::env::var("EXV_TIMING_FILE")
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
    std::env::temp_dir().join(format!("exv-timing-{}.log", std::process::id()))
}

/// 进程启动时刻（`SELF_MS` 基准；单调时钟）。
#[cfg(feature = "timing")]
fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// 追加写一条计时记录（只加计时不改行为；单次 `OpenOptions::append`，调用频率低，
/// 不在热路径——每分段一次）。
#[cfg(feature = "timing")]
pub fn record(label: &str, elapsed_ms: u64) {
    let wall_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    let self_ms = u64::try_from(process_start().elapsed().as_millis()).unwrap_or(u64::MAX);
    let line = format!("{wall_ms} {} {self_ms} {label}\n", std::process::id());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(timing_file())
        .and_then(|mut f| f.write_all(line.as_bytes()));
    let _ = elapsed_ms;
}

/// 空实现桩（feature 关闭：零开销）。
#[cfg(not(feature = "timing"))]
pub fn record(_label: &str, _elapsed_ms: u64) {}

/// 作用域计时 guard：`Drop` 时记录 elapsed（函数级 wrap 用）。feature 关闭时是
/// ZST，构造/析构均被优化消除——调用点可无条件使用。
#[must_use]
pub struct Timed {
    #[cfg(feature = "timing")]
    label: &'static str,
    #[cfg(feature = "timing")]
    start: Instant,
}

impl Timed {
    /// 开始计一段；`Drop` 时把 elapsed ms 写 sink（feature 关闭时为空 ZST）。
    /// 未使用返回值由类型级 `#[must_use]`（见上）兜底，函数级不再重复标注。
    pub fn new(label: &'static str) -> Self {
        #[cfg(feature = "timing")]
        {
            Self {
                label,
                start: Instant::now(),
            }
        }
        #[cfg(not(feature = "timing"))]
        {
            let _ = label;
            Self {}
        }
    }

    /// 手动收尾（提前记录后丢弃 guard；`drop` 不再重复记录）。
    #[cfg(feature = "timing")]
    pub fn finish(self) {
        let ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        record(self.label, ms);
    }
}

#[cfg(feature = "timing")]
impl Drop for Timed {
    fn drop(&mut self) {
        let ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        record(self.label, ms);
    }
}

/// 手动计时：`record_elapsed(label, start)` 记录自 `start` 起的分段耗时（跨多条语句 /
/// 条件等待用）。feature 关闭时空实现。
pub fn record_elapsed(label: &str, start: Instant) {
    #[cfg(feature = "timing")]
    {
        let ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        record(label, ms);
    }
    #[cfg(not(feature = "timing"))]
    {
        let _ = (label, start);
    }
}

