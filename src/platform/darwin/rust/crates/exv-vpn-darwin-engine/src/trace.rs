//! T6（darwin 连接基建）：连接痕迹（trace）——engine 单写者。
//!
//! 吸收 Windows v6 计划的 trace 语义：连接留持久痕迹，**善后验证后才删**；
//! trace 存在的意义是"双亡格"兜底（core+engine 同亡无人清理，下次连接预检
//! 收口）。本模块是 darwin 侧的最小实现：
//!
//! - **载体**（D-d4 代裁）：`/Library/Application Support/EXV/Engine/<uid>/`
//!   下的 `trace.v1.json`——root 控制目录内的 per-uid 子目录（engine euid=0
//!   创建，属主置为 owner uid），engine 是唯一写者（v6 D1：core 不直接落盘，
//!   消除跨进程 last-writer-wins；T6 最小实现由 engine 在自身生命周期点自治
//!   写入，core RPC 面挂账 T3）。
//! - **写入点**：ApplyTunnel 受理（open+state=connecting）、管线 Connected
//!  （state=connected）、会话终结（state=terminal，记录终态与善后结论）。
//! - **删除权**（对齐 v6 四情形的 darwin 子集）：converge/StopTunnel 善后
//!   **已验证**（有界等待通过）→ 删除；崩溃/双亡路径不删——残留 trace 即下次
//!   连接的预检触发信号。
//! - **无 core 期事件去向**（T0 新事实）：engine 在无 core 会话期（keep 携带/
//!   headless）的关键生命周期事件（CORE_EXIT_KEEP/ADOPTED/TEARDOWN_*）追加到
//!   `events.v1.jsonl`——泵诊断文件之外的第二个结构化落点，聚合日志不再随
//!   core 死亡而截断。
//!
//! 不做的事（治理边界）：不建跨宿主联合状态机；schema 只记非秘密事实（服务器
//! 主机名、形态、时间戳、终态码——凭据/会话材料绝不入 trace）。

use std::{
    fs,
    io::Write as _,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// trace 状态（连接生命周期；v6 状态机的 darwin 子集）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TraceState {
    /// ApplyTunnel 已受理（组装中）。
    Connecting,
    /// 数据面已 Connected（管线持有）。
    Connected,
    /// 连接终结（字段 `terminal` 携带终态码与善后结论）。
    Terminal,
}

/// 单条连接痕迹（v1 schema）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TraceV1 {
    pub(crate) schema: u32,
    pub(crate) state: TraceState,
    /// owner uid（per-uid 目录的冗余记录，供单文件迁移场景校验）。
    pub(crate) owner_uid: u32,
    /// 引擎形态（"service" | "oneshot"）。
    pub(crate) mode: String,
    /// 网关主机名（非秘密；凭据绝不入 trace）。
    pub(crate) server: String,
    /// 连接用户名（与采纳判定同源的non-secret 面）。
    pub(crate) username: String,
    /// 事件纪元毫秒。
    pub(crate) updated_at_ms: i64,
    /// 终态码（state=Terminal 时；None=尚未终结）。
    pub(crate) terminal: Option<String>,
    /// 善后是否已验证（Terminal 时；true 才允许删 trace）。
    pub(crate) teardown_verified: bool,
}

/// 无 core 期事件行（events.v1.jsonl；append-only）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TraceEventV1 {
    pub(crate) at_ms: i64,
    pub(crate) level: String,
    pub(crate) code: String,
    pub(crate) message: String,
}

/// trace 存储的固定错误（稳定类别；清理失败不得覆盖主错误——对齐既有纪律）。
#[derive(Debug)]
pub(crate) enum TraceError {
    Directory,
    Write,
}

const SCHEMA_VERSION: u32 = 1;
const TRACE_FILE: &str = "trace.v1.json";
const EVENTS_FILE: &str = "events.v1.jsonl";
/// 生产根目录（D-d4 代裁：与既有安装根一致，root 控制）。
pub(crate) const TRACE_ROOT: &str = "/Library/Application Support/EXV/Engine";

/// per-uid trace 目录（engine 单写者句柄）。
pub(crate) struct TraceStore {
    dir: PathBuf,
    owner_uid: u32,
    mode: &'static str,
}

impl TraceStore {
    /// 打开（幂等创建）owner 的 trace 目录；返回单写者句柄。
    ///
    /// # Errors
    ///
    /// 目录创建/属主设置失败返回 [`TraceError::Directory`]——trace 失败不阻塞
    /// 连接主路径（调用方 best-effort 记日志）。
    pub(crate) fn open(base: &Path, owner_uid: u32, mode: &'static str) -> Result<Self, TraceError> {
        let dir = base.join(owner_uid.to_string());
        fs::create_dir_all(&dir).map_err(|_| TraceError::Directory)?;
        // C4（复审）：显式权限——目录 0700、文件 0600（umask 不可靠：默认 022
        // 下 create_dir_all 产物是 0755）。root engine 随后把属主置为 owner。
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
            .map_err(|_| TraceError::Directory)?;
        set_owner(&dir, owner_uid).map_err(|_| TraceError::Directory)?;
        Ok(Self {
            dir,
            owner_uid,
            mode,
        })
    }

    /// 是否存在残留 trace（预检触发信号；双亡格兜底）。
    pub(crate) fn trace_exists(base: &Path, owner_uid: u32) -> bool {
        base.join(owner_uid.to_string()).join(TRACE_FILE).exists()
    }

    fn trace_path(&self) -> PathBuf {
        self.dir.join(TRACE_FILE)
    }

    /// 写/更新当前连接痕迹（整文件原子替换：写临时文件后 rename）。
    pub(crate) fn record(
        &self,
        state: TraceState,
        server: &str,
        username: &str,
        terminal: Option<&str>,
        teardown_verified: bool,
    ) {
        let record = TraceV1 {
            schema: SCHEMA_VERSION,
            state,
            owner_uid: self.owner_uid,
            mode: self.mode.to_owned(),
            server: server.to_owned(),
            username: username.to_owned(),
            updated_at_ms: now_epoch_ms(),
            terminal: terminal.map(str::to_owned),
            teardown_verified,
        };
        let Ok(payload) = serde_json::to_vec(&record) else {
            return;
        };
        let target = self.trace_path();
        let temp = self.dir.join(format!("{TRACE_FILE}.tmp"));
        if let Ok(mut file) = fs::File::create(&temp)
            && file.write_all(&payload).is_ok()
            && file.sync_all().is_ok()
            && fs::rename(&temp, &target).is_ok()
        {
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o600));
            return;
        }
        let _ = fs::remove_file(&temp);
    }

    /// 追加一条无 core 期事件（append-only；失败静默——事件流不阻塞主路径）。
    pub(crate) fn append_event(&self, level: &str, code: &str, message: &str) {
        let line = TraceEventV1 {
            at_ms: now_epoch_ms(),
            level: level.to_owned(),
            code: code.to_owned(),
            message: message.to_owned(),
        };
        let Ok(payload) = serde_json::to_vec(&line) else {
            return;
        };
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(EVENTS_FILE))
        {
            let _ = file.write_all(&payload);
            let _ = file.write_all(b"\n");
        }
    }

    /// 善后已验证后删除 trace（删除权的唯一入口；双亡路径不调用本函数）。
    pub(crate) fn delete_after_verified_teardown(&self) {
        let _ = fs::remove_file(self.trace_path());
    }

    /// 读取当前痕迹（预检/诊断只读）。
    pub(crate) fn read(base: &Path, owner_uid: u32) -> Option<TraceV1> {
        let path = base.join(owner_uid.to_string()).join(TRACE_FILE);
        let payload = fs::read(path).ok()?;
        serde_json::from_slice::<TraceV1>(&payload).ok()
    }
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

/// root 引擎把目录属主置为 owner（chown；失败=不可用该目录）。
fn set_owner(dir: &Path, owner_uid: u32) -> std::io::Result<()> {
    // SAFETY: chown 只作用于本函数刚创建/打开的目录路径；无调用方可控指针。
    let c_path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let result = unsafe { libc::chown(c_path.as_ptr(), owner_uid, -1_i32 as libc::gid_t) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
