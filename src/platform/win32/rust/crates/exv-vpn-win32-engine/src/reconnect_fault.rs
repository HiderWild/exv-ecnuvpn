//! 本轮隔离验收标记；只在真实传输任务持有期消费。
const HOST: &str = "windows";
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[doc(hidden)]
pub struct ReconnectFault {
    directory: PathBuf,
    next_poll: Instant,
}

impl ReconnectFault {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory, next_poll: Instant::now() }
    }

    pub(crate) fn poll(&mut self, operation_id: &[u8]) -> bool {
        if Instant::now() < self.next_poll { return false; }
        self.next_poll = Instant::now() + Duration::from_millis(100);
        consume(&self.directory, operation_id)
    }
}

fn consume(directory: &Path, operation_id: &[u8]) -> bool {
    if operation_id.len() != 16 || std::fs::read(directory.join("exv-native-acceptance.opt-in")).ok().as_deref() != Some(b"connected-transport\n") {
        return false;
    }
    let request = directory.join("reconnect-fault.request");
    let Ok(payload) = std::fs::read(&request) else { return false; };
    let operation: String = operation_id.iter().map(|byte| format!("{byte:02x}")).collect();
    let Ok(text) = std::str::from_utf8(&payload) else { return false; };
    let parts: Vec<_> = text.split('\n').collect();
    if parts.len() != 4 || parts[0] != HOST || parts[1] != operation || !parts[3].is_empty()
        || parts[2].len() != 32 || !parts[2].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    // 先移除请求，下一次重拨沿用 operation 时也不会重复消费。
    if std::fs::remove_file(&request).is_err() { return false; }
    if let Err(error) = std::fs::write(directory.join("reconnect-fault.consumed"), payload) {
        eprintln!("重连验收故障消费回执写入失败: {error}");
    }
    true
}
