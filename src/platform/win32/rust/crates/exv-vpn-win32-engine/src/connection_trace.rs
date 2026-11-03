
//! 连接痕迹（connection trace）存储——T1 载体（2026-09-27 计划 §2.1–§2.4）。
//!
//! 用途：连接受理时落一个持久痕迹文件，**善后验证完成后才删除**；连接前痕迹仍在
//! （含损坏）即触发并行只读预检（计划 §三）。engine 是唯一写者（D1-b：全部写操作
//! 经本 store 串行化，core 不直接落盘），因此：
//!
//! - 进程内 [`WRITE_LOCK`] 即构成完整的写者互斥；**刻意不引入跨进程文件锁**——
//!   engine 是全机唯一写者进程，core 永不写本文件（复审 F4 裁定）。
//! - 载体路径：`%ProgramData%\ExvVpn\connection\trace-<user-sid>.json`（W14 journal
//!   同款机器级目录约定；目录不存在则建立并按 T7 D1 显式保护 DACL 加固——
//!   user-sid + SYSTEM + Administrators，见 [`crate::secure_write`]；user-sid 由
//!   调用方作参数传入，不从环境读取）。
//!
//! 冻结契约：
//! - **原子写**：同目录临时文件加 `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`
//!   改名、随后刷目录元数据（镜像 `journal_store.rs` 的 Windows 调用）；`create`
//!   不带 `REPLACE_EXISTING`——目标已存在即原子失败，**绝不静默覆盖遗留 trace**
//!   （遗留是预检的事，不是新连接的事）。
//! - **显式 DACL（T7 D1，v6 改判 b）**：trace 目录与全部文件（含原子写 temp）
//!   出生即带"user-sid + SYSTEM + Administrators"保护 DACL——写盘原语见
//!   [`crate::secure_write`]（temp 用 `CreateFileW + SECURITY_ATTRIBUTES`，无
//!   "先建后改 ACL"的窗口期；目录用 `SetNamedSecurityInfoW`）。
//! - **损坏=善后未竟**：读侧对不可解析/超限/未知 schema 一律返回 [`TraceReadOutcome::Corrupt`]
//!   标记而非"不存在"——预检必须看见它。
//! - **尺寸上限**（[`MAX_TRACE_BYTES`]，64 KiB）：读超限=Corrupt；写超限失败。
//! - **无密钥纪律**：字段只有 id/策略/PID/时间戳/名称清单，**绝不**写入凭据、
//!   密钥、令牌或任何 secret——文件落在 `%ProgramData%`，必须当作可被管理员侧
//!   工具读取的明文诊断数据对待。
//! - schema v1 不含 `engine_pipe`（§2.1 v6 修订：管道名统一常量后无需实例发现）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use exv_vpn_win32_resource::native_error::NativeError;
use serde::{Deserialize, Serialize};

use crate::secure_write::{SecureWriteError, write_atomic_scoped};

/// trace 文件 schema 版本；本实现固定写 1，读到其他值按损坏处理（保守向前：
/// 未知版本 = 善后未竟，交给预检裁定）。
const SCHEMA_V1: u32 = 1;

/// 单个 trace 文件的尺寸上限（读超限 = Corrupt；写超限失败）。
///
/// schema v1 全字段远小于此量级；上限是对"文件被外物撑大/篡改"的防御，不是对
/// 正常记录的约束。
const MAX_TRACE_BYTES: u64 = 64 * 1024;

/// 进程内写者互斥（D1-b）：engine 是全机唯一 trace 写者进程，core 永不落盘——
/// 进程内 `Mutex` 即完整互斥，**刻意不引入跨进程锁**（复审 F4 裁定；若未来出现
/// 第二写者进程，须升级为文件锁并在计划层面重新裁定）。T7 起同目录的版本标记
/// （[`crate::version_marker`]）写操作共用本互斥——connection 目录的全部写者
/// 在 engine 进程内串行。
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 获取进程内写者互斥（中毒时恢复数据继续——store 自身绝不因持有者 panic 而瘫痪）。
pub(crate) fn write_lock() -> MutexGuard<'static, ()> {
    WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 连接状态机（§2.1）：connecting → live → aftermath。
///
/// 删除权只在 aftermath 且善后验证通过后行使（§2.4 删除权矩阵归调用方；本 store
/// 只忠实承载状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceState {
    /// 受理中：连接请求已受理，隧道尚未建立。
    Connecting,
    /// 存活：隧道已建立并承载业务流。
    Live,
    /// 善后中：拆除/恢复流程进行中——此态及删除前的任何残留都是预检信号。
    Aftermath,
}

/// engine 形态（§2.6 双模式矩阵）：决定 core 死亡语义、注销边界与崩溃回归路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceEngineMode {
    /// 常驻服务（LocalSystem，SCM 崩溃自恢复）。
    Service,
    /// 一次性提权进程（随连接建立，注销即亡）。
    Oneshot,
}

/// 隧道种类（预检域与拆除路径按种类分派）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceKind {
    /// aTrust 线。
    Atrust,
    /// Cisco 线。
    Cisco,
}

/// core 退出时的连接策略（D6：默认 keep）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceOnCoreExit {
    /// engine 保连接等新 core 采纳（EOF + PID 复核后不拆隧道）。
    Keep,
    /// engine 随 core 退出执行拆除善后。
    Teardown,
}

/// 重连策略块（§2.8 两设置项的镜像，随连接请求送达并落 trace）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceReconnectPolicy {
    /// 主开关：断线自动重连（R1，engine 侧自愈）。
    pub enabled: bool,
    /// oneshot engine 崩溃导致断连时仍然重连（R2 oneshot 分支门控；默认关——
    /// 重连必然重新提权弹 UAC）。
    pub on_oneshot_engine_crash: bool,
    /// 自动重连时发送通知（既有托盘气泡通道；默认开）。
    pub notify: bool,
}

/// trace 文件记录（schema v1）。
///
/// 字段集合 = 预检域清单 + 状态机 + 策略镜像；除下列字段外**没有任何**凭据/
/// 密钥/令牌（见模块级无密钥纪律）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRecord {
    /// schema 版本（当前恒为 1；读侧遇其他值按损坏处理）。
    pub schema: u32,
    /// 连接代次标识（D10：R1(engine)/R2(core) 双状态机的边界判据；重连成功时由
    /// [`ConnectionTraceStore::update_established_at`] 换新）。
    pub connection_id: String,
    /// 连接状态机当前态。
    pub state: TraceState,
    /// 写本记录的 engine 形态。
    pub engine_mode: TraceEngineMode,
    /// 隧道种类。
    pub kind: TraceKind,
    /// 服务器地址（诊断/显示用，不含凭据）。
    pub server: String,
    /// 连接模式：`tun` | `system-proxy` | `tun-system-proxy`。
    pub mode: String,
    /// core 退出策略。
    pub on_core_exit: TraceOnCoreExit,
    /// 重连策略镜像（§2.8）。
    pub reconnect: TraceReconnectPolicy,
    /// 本连接受理时刻（Unix 毫秒）。
    pub created_at_ms: u64,
    /// 最近一次隧道建立时刻（Unix 毫秒；0 = 尚未建立；重连成功时随 connection_id
    /// 一并更新——采纳路径不得改动此字段）。
    pub established_at_ms: u64,
    /// 受理本连接的 core 进程 PID（engine 拉起 core 与崩溃环判定用；engine 侧取自
    /// **传输层已验证的 peer 身份**，不经任何客户端自报字段）。
    pub core_pid: u32,
    /// core 背后的 UI 进程 PID（§2.9 缺口①：engine 拉起 core 时携带，使活着的 UI
    /// 能拨到新 core 的管道；0 = 未知/旧 core 未上报）。schema v1 后补字段：
    /// `#[serde(default)]` 使 T1 时期写下的旧文件仍可解析（缺键读 0）。
    #[serde(default)]
    pub ui_pid: u32,
    /// 写本文件的 engine 进程 PID（诊断/所有权核对；装配后经 patch 补写）。
    pub engine_pid: u32,
    /// 本连接创建的网卡名清单（预检网卡域）。
    pub adapter_names: Vec<String>,
    /// 本连接注册的 NRPT 规则 owner GUID 清单（预检 NRPT 域；engine 装配后补写）。
    pub nrpt_owner_guids: Vec<String>,
    /// 本连接占用的本地代理端口清单（预检系统代理域；engine 装配后补写）。
    pub proxy_ports: Vec<u16>,
}

/// 读 trace 文件的三态结局。
///
/// [`TraceReadOutcome::Corrupt`] 不是"不存在"：损坏 = 善后未竟，预检必须看见它
/// （计划 §三：触发条件是"trace 存在（含损坏）"）。
#[derive(Debug, Clone, PartialEq)]
pub enum TraceReadOutcome {
    /// 文件不存在（干净状态）。
    Absent,
    /// 文件存在且可解析。
    Present(TraceRecord),
    /// 文件存在但不可解析 / 超过尺寸上限 / 未知 schema——按善后未竟处理。
    Corrupt,
}

/// 装配后的字段补丁（engine 在网卡/NRPT/代理/自身 PID 就绪后回填）。
///
/// 仅覆盖 §2.2 中"创建时不可知、装配后确定"的四个字段；`None` = 不改动。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TracePatch {
    /// 网卡名清单（整组替换）。
    pub adapter_names: Option<Vec<String>>,
    /// NRPT 规则 owner GUID 清单（整组替换）。
    pub nrpt_owner_guids: Option<Vec<String>>,
    /// 本地代理端口清单（整组替换）。
    pub proxy_ports: Option<Vec<u16>>,
    /// engine 进程 PID。
    pub engine_pid: Option<u32>,
}

/// store 操作错误。
#[derive(Debug)]
pub enum TraceStoreError {
    /// user-sid 参数不合法（非 SID 文法——路径注入防御）。
    InvalidSid,
    /// 记录的 schema 字段不是本实现支持的版本（create 拒绝写出未知 schema）。
    UnexpectedSchema(u32),
    /// create 时目标文件已存在（绝不静默覆盖遗留 trace）。
    AlreadyExists,
    /// 更新类操作时目标文件不存在（连接流程应先 create）。
    Absent,
    /// 更新类操作时现有文件损坏（不可读改写；也不覆盖——留给预检/善后裁定）。
    Corrupt,
    /// 序列化结果超过 [`MAX_TRACE_BYTES`]。
    TooLarge {
        /// 超限的字节数。
        bytes: usize,
    },
    /// 安全目录建立 / 原子改名 / 目录刷盘的 Win32 失败。
    Native(NativeError),
    /// 其他 I/O 失败。
    Io(std::io::Error),
}

/// 连接痕迹存储：engine 侧唯一写者载体（D1-b）。
///
/// 无状态句柄（每次操作开临时文件 + 原子改名，不长期持有目标文件句柄——
/// 这让 `MoveFileExW` 的 `REPLACE_EXISTING` 永不撞上自持句柄的 `ACCESS_DENIED`）。
/// 全部操作经 [`WRITE_LOCK`] 串行。
pub struct ConnectionTraceStore {
    /// trace 目录（已按 W14 建立并加固）。
    dir: PathBuf,
}

impl ConnectionTraceStore {
    /// 机器级默认 store：`%ProgramData%\ExvVpn\connection`（不可用时按 W14 约定
    /// 回退 `%LOCALAPPDATA%\ExvVpn\connection`，再回退临时目录）。
    ///
    /// `user_sid`：目录保护 DACL 的用户段（T7 D1——发起用户 +SYSTEM+
    /// Administrators；`None` = 以当前进程用户为作用域）。
    ///
    /// # Errors
    ///
    /// 目录无法建立或无法加固（ACL 应用失败）时返回 [`TraceStoreError`]。
    pub fn machine_default(user_sid: Option<&str>) -> Result<Self, TraceStoreError> {
        Self::open_scoped(&machine_connection_dir(), user_sid)
    }

    /// 指定目录上的 store（测试与自定义布局用；目录不存在则安全建立，作用域为
    /// 当前进程用户）。
    ///
    /// # Errors
    ///
    /// 目录无法建立或无法加固（ACL 应用失败）时返回 [`TraceStoreError`]。
    pub fn open(dir: &Path) -> Result<Self, TraceStoreError> {
        Self::open_scoped(dir, None)
    }

    /// 指定目录 + 用户作用域的 store（T7 D1：目录 DACL = user-sid + SYSTEM +
    /// Administrators，保护不继承；每次 open 重申一次——幂等替换，多用户共用目录
    /// 时各 open 不放宽彼此文件的显式 DACL）。
    ///
    /// # Errors
    ///
    /// 目录无法建立或无法加固（ACL 应用失败）时返回 [`TraceStoreError`]。
    pub fn open_scoped(dir: &Path, user_sid: Option<&str>) -> Result<Self, TraceStoreError> {
        crate::secure_write::protect_connection_dir(dir, user_sid)
            .map_err(TraceStoreError::Native)?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// trace 目录（诊断用）。
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 创建 trace（连接受理时调用一次）。
    ///
    /// 目标文件已存在即失败（[`TraceStoreError::AlreadyExists`]）——新连接绝不
    /// 静默覆盖遗留 trace；遗留的处置是预检的业务。原子性由 `MoveFileExW` 不带
    /// `REPLACE_EXISTING` 保证（预检只作快速失败，非原子判定）。
    ///
    /// # Errors
    ///
    /// - user-sid 不合法 / schema 非当前版本 / 已存在 / 序列化超限 / I/O 或
    ///   Win32 失败，详见各变体。
    pub fn create(&self, user_sid: &str, record: &TraceRecord) -> Result<(), TraceStoreError> {
        validate_sid(user_sid)?;
        if record.schema != SCHEMA_V1 {
            return Err(TraceStoreError::UnexpectedSchema(record.schema));
        }
        let _guard = write_lock();
        match fs::symlink_metadata(self.trace_path(user_sid)) {
            Ok(_) => return Err(TraceStoreError::AlreadyExists),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(TraceStoreError::Io(e)),
        }
        self.write_atomic(user_sid, record, false)
    }

    /// 读 trace：不存在 / 存在且可解析 / 损坏三态（见 [`TraceReadOutcome`]）。
    ///
    /// # Errors
    ///
    /// 读文件的 I/O 失败（权限等基础设施错误——不是文件内容问题）。
    pub fn read(&self, user_sid: &str) -> Result<TraceReadOutcome, TraceStoreError> {
        validate_sid(user_sid)?;
        let _guard = write_lock();
        read_trace_at(&self.trace_path(user_sid))
    }

    /// 更新连接状态（connecting → live → aftermath）。
    ///
    /// # Errors
    ///
    /// 文件不存在（[`TraceStoreError::Absent`]）/ 现有文件损坏 / 写入失败。
    pub fn update_state(&self, user_sid: &str, new_state: TraceState) -> Result<(), TraceStoreError> {
        self.modify(user_sid, |record| record.state = new_state)
    }

    /// 重连成功时更新隧道建立时刻与连接代次（§2.4：engine 在重连成功时调用；
    /// 采纳路径不得触碰）。
    ///
    /// # Errors
    ///
    /// 文件不存在 / 现有文件损坏 / 写入失败。
    pub fn update_established_at(
        &self,
        user_sid: &str,
        new_connection_id: &str,
        established_at_ms: u64,
    ) -> Result<(), TraceStoreError> {
        self.modify(user_sid, |record| {
            new_connection_id.clone_into(&mut record.connection_id);
            record.established_at_ms = established_at_ms;
        })
    }

    /// 装配后补写网卡/NRPT/代理端口/engine PID（其余字段不动）。
    ///
    /// # Errors
    ///
    /// 文件不存在 / 现有文件损坏 / 写入失败（含超限）。
    pub fn patch_fields(&self, user_sid: &str, patch: &TracePatch) -> Result<(), TraceStoreError> {
        self.modify(user_sid, |record| {
            if let Some(names) = &patch.adapter_names {
                record.adapter_names.clone_from(names);
            }
            if let Some(guids) = &patch.nrpt_owner_guids {
                record.nrpt_owner_guids.clone_from(guids);
            }
            if let Some(ports) = &patch.proxy_ports {
                record.proxy_ports.clone_from(ports);
            }
            if let Some(pid) = patch.engine_pid {
                record.engine_pid = pid;
            }
        })
    }

    /// 删除 trace。
    ///
    /// **只在善后验证通过后调用**（删除权矩阵归调用方，§2.4；本 store 只负责
    /// 原子移除）。文件已不存在视为成功（幂等——删除意图已达成）。
    ///
    /// # Errors
    ///
    /// 移除时的 I/O 失败。
    pub fn delete(&self, user_sid: &str) -> Result<(), TraceStoreError> {
        validate_sid(user_sid)?;
        let _guard = write_lock();
        match fs::remove_file(self.trace_path(user_sid)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(TraceStoreError::Io(e)),
        }
    }

    /// `<dir>/trace-<user-sid>.json`。
    fn trace_path(&self, user_sid: &str) -> PathBuf {
        self.dir.join(format!("trace-{user_sid}.json"))
    }

    /// 与目标同目录的临时文件路径（进程内写者互斥下固定名即安全）。
    fn temp_path(&self, user_sid: &str) -> PathBuf {
        self.dir.join(format!("trace-{user_sid}.json.tmp"))
    }

    /// 锁内读-改-写：读现有记录 → `mutate` → 原子替换。
    fn modify<F>(&self, user_sid: &str, mutate: F) -> Result<(), TraceStoreError>
    where
        F: FnOnce(&mut TraceRecord),
    {
        validate_sid(user_sid)?;
        let _guard = write_lock();
        let path = self.trace_path(user_sid);
        let mut record = match read_trace_at(&path)? {
            TraceReadOutcome::Present(record) => record,
            TraceReadOutcome::Absent => return Err(TraceStoreError::Absent),
            TraceReadOutcome::Corrupt => return Err(TraceStoreError::Corrupt),
        };
        mutate(&mut record);
        self.write_atomic(user_sid, &record, true)
    }

    /// 序列化并原子落盘（`replace_existing` = 覆盖现有目标；false = 目标存在即失败）。
    ///
    /// T7 D1：temp 与目标（改名携带同一 SD）出生即带显式 user-sid+SYSTEM+
    /// Administrators 保护 DACL（原语见 [`crate::secure_write::write_atomic_scoped`]）。
    fn write_atomic(
        &self,
        user_sid: &str,
        record: &TraceRecord,
        replace_existing: bool,
    ) -> Result<(), TraceStoreError> {
        let bytes = serde_json::to_vec(record)
            .map_err(|e| TraceStoreError::Io(std::io::Error::other(e)))?;
        if bytes.len() as u64 > MAX_TRACE_BYTES {
            return Err(TraceStoreError::TooLarge { bytes: bytes.len() });
        }
        write_atomic_scoped(
            &self.temp_path(user_sid),
            &self.trace_path(user_sid),
            &self.dir,
            &bytes,
            Some(user_sid),
            replace_existing,
        )
        .map_err(|error| match error {
            SecureWriteError::AlreadyExists => TraceStoreError::AlreadyExists,
            SecureWriteError::Native(native) => TraceStoreError::Native(native),
            SecureWriteError::Io(io) => TraceStoreError::Io(io),
        })
    }
}

/// 锁内共享的读实现（尺寸门 + 解析 + schema 校验）。
fn read_trace_at(path: &Path) -> Result<TraceReadOutcome, TraceStoreError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TraceReadOutcome::Absent);
        }
        Err(e) => return Err(TraceStoreError::Io(e)),
    };
    if meta.len() > MAX_TRACE_BYTES {
        return Ok(TraceReadOutcome::Corrupt);
    }
    let bytes = fs::read(path).map_err(TraceStoreError::Io)?;
    let Ok(record) = serde_json::from_slice::<TraceRecord>(&bytes) else {
        return Ok(TraceReadOutcome::Corrupt);
    };
    if record.schema != SCHEMA_V1 {
        return Ok(TraceReadOutcome::Corrupt);
    }
    Ok(TraceReadOutcome::Present(record))
}

/// 校验 user-sid 参数：SID 文法（`S-1-` 前缀 + 仅数字与连字符）+ 长度上限。
///
/// user-sid 最终源自 core 的 RPC 请求并直接拼入文件名——必须杜绝路径注入
/// （`..`、分隔符、任意名）。canonical 实现自 T11 起在
/// `exv_vpn_win32_ipc::pipe_names`（控制面管道名与 trace 文件名共用同一文法），
/// 本函数是其薄委托；校验只看字符集，不验证 SID 是否真实存在。
fn validate_sid(user_sid: &str) -> Result<(), TraceStoreError> {
    exv_vpn_win32_ipc::pipe_names::validate_user_sid(user_sid)
        .map_err(|_| TraceStoreError::InvalidSid)
}

/// 机器级 trace 目录（W14 约定：`%ProgramData%` 优先，回退 `%LOCALAPPDATA%`，再回退
/// 临时目录；对齐 `journal_path.rs` / `log_sink.rs`）。
fn machine_connection_dir() -> PathBuf {
    if let Some(program_data) = std::env::var_os("PROGRAMDATA") {
        let candidate = PathBuf::from(program_data).join("ExvVpn").join("connection");
        if dir_is_usable(&candidate) {
            return candidate;
        }
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data).join("ExvVpn").join("connection");
    }
    std::env::temp_dir().join("ExvVpn").join("connection")
}

/// 目录是否可建可用（W14 journal 同款 write-probe 约定）。
fn dir_is_usable(dir: &Path) -> bool {
    if !dir.is_dir() && fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe_id = std::process::id();
    let probe = dir.join(format!(".exv-trace-write-probe-{probe_id}"));
    match fs::File::create(&probe) {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// 单测：temp-dir 真文件（不触碰真实 %ProgramData%；machine_default 的目录解析
// 属环境相关路径，不在单测覆盖——见 T1 执行记录）。
// ---------------------------------------------------------------------------
