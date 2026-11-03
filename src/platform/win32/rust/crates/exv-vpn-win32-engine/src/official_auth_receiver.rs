//! Windows 官方认证接收端：首次原配置落盘后禁用、正常停止及清理；用户主动恢复。
//!
//! 固定管理 aTrustService 及其安装目录内的认证进程，退出时保留禁用配置。
use exv_vpn_wire::generated;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

const STOPPED: u32 = 1;
const STOP_PENDING: u32 = 3;
const DISABLED: u32 = 4;
const PROCESS_NAMES: [&str; 3] = ["aTrustTray.exe", "aTrustAgent.exe", "aTrustXtunnel.exe"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Configuration {
    start_type: u32,
    delayed: bool,
    binary_path: String,
    account: String,
}
#[derive(Clone, Debug)]
struct Observation {
    config: Configuration,
    state: u32,
    checkpoint: u32,
    wait_hint: Duration,
}
#[derive(Clone, Debug)]
struct Process {
    pid: u32,
    path: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Failure {
    step: &'static str,
    code: u32,
    detail: String,
}
impl Failure {
    fn new(step: &'static str, code: u32, detail: impl Into<String>) -> Self {
        Self {
            step,
            code,
            detail: detail.into(),
        }
    }
    fn io(step: &'static str, error: std::io::Error) -> Self {
        Self::new(
            step,
            error
                .raw_os_error()
                .and_then(|c| u32::try_from(c).ok())
                .unwrap_or(31),
            error.to_string(),
        )
    }
}
trait Backend: Send {
    fn observe(&mut self) -> Result<Option<Observation>, Failure>;
    fn set_start_type(&mut self, value: u32) -> Result<(), Failure>;
    fn set_delayed(&mut self, value: bool) -> Result<(), Failure>;
    fn stop(&mut self) -> Result<(), Failure>;
    fn processes(&mut self) -> Result<Vec<Process>, Failure>;
    fn terminate(&mut self, process: &Process, deadline: Instant) -> Result<(), Failure>;
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
trait Store: Send {
    fn load(&self) -> Result<Option<Configuration>, Failure>;
    fn save_first(&self, config: &Configuration) -> Result<(), Failure>;
    fn clear(&self) -> Result<(), Failure>;
}
struct FileStore(PathBuf);
struct State {
    backend: Box<dyn Backend>,
    store: Box<dyn Store>,
}
pub struct OfficialAuthReceiver {
    state: Mutex<State>,
}
#[derive(Default, Debug)]
struct Outcome {
    current: Option<Observation>,
    original: Option<Configuration>,
    recovery_available: bool,
    completed_steps: Vec<String>,
    error: Option<Failure>,
    processes_running: bool,
}

fn remaining(deadline: Instant, step: &'static str) -> Result<Duration, Failure> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| Failure::new(step, 1460, "认证接收端操作期限已到"))
}
fn executable(config: &Configuration) -> Result<PathBuf, Failure> {
    let raw = config.binary_path.trim();
    let image = if let Some(quoted) = raw.strip_prefix('"') {
        quoted.split_once('"').map(|(image, _)| image)
    } else {
        raw.to_ascii_lowercase()
            .find(".exe")
            .map(|end| &raw[..end + 4])
    }
    .ok_or_else(|| {
        Failure::new(
            "installation_identity",
            13,
            "aTrustService 的映像路径缺少可执行文件",
        )
    })?;
    let path = PathBuf::from(image);
    if !path.is_absolute()
        || !path.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .eq_ignore_ascii_case("aTrustAgent.exe")
        })
    {
        return Err(Failure::new(
            "installation_identity",
            13,
            "aTrustService 的映像来源与认证 Agent 不匹配",
        ));
    }
    Ok(path)
}
fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_ascii_lowercase()
}
fn belongs_to_installation(path: &Path, config: &Configuration) -> Result<bool, Failure> {
    if !path.file_name().is_some_and(|name| {
        PROCESS_NAMES
            .iter()
            .any(|known| name.to_string_lossy().eq_ignore_ascii_case(known))
    }) {
        return Ok(false);
    }
    let image = executable(config)?;
    let root = image
        .parent()
        .ok_or_else(|| Failure::new("installation_identity", 13, "aTrustService 的安装目录为空"))?;
    let candidate = normalized_path(path);
    if candidate == normalized_path(&image) {
        return Ok(true);
    }
    if path.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .eq_ignore_ascii_case("aTrustXtunnel.exe")
    }) {
        return Ok(candidate.starts_with(&format!(
            "{}\\",
            normalized_path(root).trim_end_matches('\\')
        )));
    }
    // 标准安装将 Agent 与 Tray 放在同一产品目录的两个子目录。
    let tray_dir = if root
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("aTrustAgent"))
    {
        root.parent().unwrap_or(root).join("aTrustTray")
    } else {
        root.to_path_buf()
    };
    Ok(candidate == normalized_path(&tray_dir.join("aTrustTray.exe")))
}
fn same_installation(left: &Configuration, right: &Configuration) -> bool {
    left.binary_path.eq_ignore_ascii_case(&right.binary_path)
        && left.account.eq_ignore_ascii_case(&right.account)
}

impl OfficialAuthReceiver {
    /// journal_dir 来自既有 Engine 组装参数，通常为 %ProgramData%/ExvVpn/journal。
    pub(crate) fn real(journal_dir: PathBuf) -> Self {
        Self {
            state: Mutex::new(State {
                backend: Box::new(native::NativeBackend),
                store: Box::new(FileStore(journal_dir.join("official-auth-receiver.json"))),
            }),
        }
    }
    fn execute(&self, action: i32, deadline: Instant) -> Outcome {
        let mut outcome = Outcome::default();
        // 在途操作串行执行；队列等待消耗发起方同一绝对期限。
        let mut state = loop {
            if let Err(error) = remaining(deadline, "deadline") {
                outcome.error = Some(error);
                return outcome;
            }
            match self.state.try_lock() {
                Ok(state) => break state,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(
                    remaining(deadline, "deadline")
                        .unwrap_or_default()
                        .min(Duration::from_millis(5)),
                ),
            }
        };
        let result = (|| -> Result<(), Failure> {
            remaining(deadline, "read_recovery")?;
            outcome.original = state.store.load()?;
            outcome.recovery_available = outcome.original.is_some();
            remaining(deadline, "inspect_service")?;
            outcome.current = state.backend.observe()?;
            match action {
                1 | 4 => observe_processes(&mut state, &mut outcome, deadline)?,
                2 => disable_and_stop(&mut state, &mut outcome, deadline)?,
                3 => {
                    restore(&mut state, &mut outcome, deadline)?;
                    observe_processes(&mut state, &mut outcome, deadline)?;
                }
                _ => return Err(Failure::new("action", 87, "认证接收端操作无效")),
            }
            Ok(())
        })();
        if let Err(error) = result {
            // 原错误与完成步骤保留；重新读取原生状态展示已发生的部分结果。
            if remaining(deadline, "inspect_service").is_ok() {
                if let Ok(current) = state.backend.observe() {
                    outcome.current = current;
                }
            }
            // 二次观察保留首个业务错误；成功观察才更新残余运行事实。
            let _ = observe_processes(&mut state, &mut outcome, deadline);
            outcome.error = Some(error);
        }
        outcome
    }
    pub(crate) fn control(
        &self,
        action: i32,
        deadline: Instant,
        runtime_epoch: &[u8],
    ) -> generated::AuthReceiverControlReply {
        self.execute(action, deadline).into_reply(runtime_epoch)
    }
}
fn observe_processes(
    state: &mut State,
    outcome: &mut Outcome,
    deadline: Instant,
) -> Result<(), Failure> {
    let Some(config) = outcome
        .current
        .as_ref()
        .map(|current| &current.config)
        .or(outcome.original.as_ref())
    else {
        return Ok(());
    };
    remaining(deadline, "inspect_processes")?;
    let mut running = false;
    for process in state.backend.processes()? {
        if belongs_to_installation(&process.path, config)? {
            running = true;
        }
    }
    outcome.processes_running = running;
    Ok(())
}
fn disable_and_stop(
    state: &mut State,
    outcome: &mut Outcome,
    deadline: Instant,
) -> Result<(), Failure> {
    let current = outcome
        .current
        .as_ref()
        .ok_or_else(|| Failure::new("inspect_service", 1060, "aTrustService 未安装"))?
        .config
        .clone();
    executable(&current)?;
    if let Some(original) = &outcome.original {
        if !same_installation(original, &current) {
            return Err(Failure::new(
                "installation_identity",
                13,
                "aTrustService 的安装来源已变更，恢复记录保留原安装配置",
            ));
        }
    } else {
        remaining(deadline, "save_original_config")?;
        if let Err(error) = state.store.save_first(&current) {
            // 原子改名后的目录刷新可能失败；返回已实际存在的首次恢复记录。
            if remaining(deadline, "read_recovery").is_ok() {
                if let Ok(original) = state.store.load() {
                    outcome.recovery_available = original.is_some();
                    outcome.original = original;
                }
            }
            return Err(error);
        }
        outcome.original = Some(current.clone());
        outcome.recovery_available = true;
    }
    outcome.completed_steps.push("save_original_config".into());
    remaining(deadline, "disable_service")?;
    if current.start_type != DISABLED {
        state.backend.set_start_type(DISABLED)?;
    }
    if let Some(current) = &mut outcome.current {
        current.config.start_type = DISABLED;
    }
    outcome.completed_steps.push("disable_service".into());
    remaining(deadline, "stop_service")?;
    let status = state
        .backend
        .observe()?
        .ok_or_else(|| Failure::new("stop_service", 1060, "停止前 aTrustService 已被移除"))?;
    outcome.current = Some(status.clone());
    if status.state != STOPPED && status.state != STOP_PENDING {
        state.backend.stop()?;
    }
    let mut checkpoint = status.checkpoint;
    let mut progress_at = Instant::now();
    loop {
        remaining(deadline, "wait_stopped")?;
        let observed = state.backend.observe()?.ok_or_else(|| {
            Failure::new("wait_stopped", 1060, "等待停止时 aTrustService 已被移除")
        })?;
        let stopped = observed.state == STOPPED;
        let wait_hint = observed.wait_hint;
        if checkpoint != observed.checkpoint {
            checkpoint = observed.checkpoint;
            progress_at = Instant::now();
        }
        outcome.current = Some(observed);
        if stopped {
            break;
        }
        if progress_at.elapsed() >= wait_hint.max(Duration::from_secs(1)) {
            return Err(Failure::new(
                "wait_stopped",
                1460,
                "aTrustService 停止进度超时",
            ));
        }
        let interval =
            (wait_hint / 10).clamp(Duration::from_millis(20), Duration::from_millis(500));
        state
            .backend
            .pause(interval.min(remaining(deadline, "wait_stopped")?));
    }
    outcome.completed_steps.push("stop_service".into());
    remaining(deadline, "cleanup_processes")?;
    for process in state.backend.processes()? {
        if belongs_to_installation(&process.path, &current)? {
            outcome.processes_running = true;
            remaining(deadline, "cleanup_processes")?;
            state.backend.terminate(&process, deadline)?;
        }
    }
    outcome.processes_running = false;
    outcome.completed_steps.push("cleanup_processes".into());
    Ok(())
}
fn restore(state: &mut State, outcome: &mut Outcome, deadline: Instant) -> Result<(), Failure> {
    let Some(original) = outcome.original.clone() else {
        return Ok(());
    };
    let current = outcome.current.as_ref().ok_or_else(|| {
        Failure::new(
            "restore_start_type",
            1060,
            "aTrustService 未安装，原配置记录已保留",
        )
    })?;
    if !same_installation(&original, &current.config) {
        return Err(Failure::new(
            "installation_identity",
            13,
            "aTrustService 的安装来源已变更，恢复记录保留原安装配置",
        ));
    }
    remaining(deadline, "restore_start_type")?;
    state.backend.set_start_type(original.start_type)?;
    outcome
        .current
        .as_mut()
        .expect("current service")
        .config
        .start_type = original.start_type;
    outcome.completed_steps.push("restore_start_type".into());
    remaining(deadline, "restore_delayed_auto_start")?;
    state.backend.set_delayed(original.delayed)?;
    outcome
        .current
        .as_mut()
        .expect("current service")
        .config
        .delayed = original.delayed;
    outcome
        .completed_steps
        .push("restore_delayed_auto_start".into());
    remaining(deadline, "inspect_service")?;
    outcome.current = state.backend.observe()?;
    remaining(deadline, "clear_recovery")?;
    state.store.clear()?;
    outcome.recovery_available = false;
    outcome.completed_steps.push("clear_recovery".into());
    Ok(())
}
impl Outcome {
    fn into_reply(self, runtime_epoch: &[u8]) -> generated::AuthReceiverControlReply {
        let summary = |config: &Configuration| generated::AuthReceiverConfigSummary {
            windows_start_type: Some(config.start_type),
            windows_delayed_auto_start: Some(config.delayed),
            launchd_targets: vec![],
        };
        let source_config = self
            .current
            .as_ref()
            .map(|current| &current.config)
            .or(self.original.as_ref());
        let source = source_config.map_or_else(
            || "aTrustService".to_owned(),
            |config| {
                let image = executable(config)
                    .map_or_else(|_| config.binary_path.clone(), |p| p.display().to_string());
                format!("aTrustService; image={image}; account={}", config.account)
            },
        );
        generated::AuthReceiverControlReply {
            receiver: Some(generated::AuthReceiverStatus {
                running: self.processes_running
                    || self
                        .current
                        .as_ref()
                        .is_some_and(|current| current.state != STOPPED),
                can_start: self
                    .current
                    .as_ref()
                    .is_some_and(|current| current.config.start_type != DISABLED),
                recovery_available: self.recovery_available,
                source,
                original_config: self.original.as_ref().map(summary),
                current_config: self
                    .current
                    .as_ref()
                    .map(|current| summary(&current.config)),
            }),
            completed_steps: self.completed_steps,
            error: self.error.map(|error| generated::VpnError {
                code: if error.code == 1460 {
                    generated::ErrorCode::DeadlineExceeded as i32
                } else {
                    generated::ErrorCode::EffectUnknown as i32
                },
                stage: generated::ErrorStage::Recovery as i32,
                certainty: 0,
                retry: generated::RetryAdvice::UseNewOperation as i32,
                subject: Some(generated::ErrorSubject {
                    subject: Some(generated::error_subject::Subject::Runtime(
                        generated::RuntimeSubject {
                            runtime_epoch: runtime_epoch.to_vec(),
                        },
                    )),
                }),
                resource: None,
                native: Some(generated::RedactedNativeError {
                    category: if error.code == 5 {
                        generated::NativeErrorCategory::Permission as i32
                    } else {
                        generated::NativeErrorCategory::Unknown as i32
                    },
                    namespace: generated::NativeErrorNamespace::Win32 as i32,
                    code: i64::from(error.code),
                }),
                reason: format!("platform/auth_receiver_{}", error.step),
                operation_detail: Some(
                    format!(
                        "Windows Engine {}: {} (Win32 {})",
                        error.step, error.detail, error.code
                    )
                    .chars()
                    .take(200)
                    .collect(),
                ),
            }),
        }
    }
}
impl Store for FileStore {
    fn load(&self) -> Result<Option<Configuration>, Failure> {
        match std::fs::read(&self.0) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| Failure::new("read_recovery", 13, e.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Failure::io("read_recovery", error)),
        }
    }
    fn save_first(&self, config: &Configuration) -> Result<(), Failure> {
        let dir = self
            .0
            .parent()
            .ok_or_else(|| Failure::new("save_original_config", 3, "恢复记录目录为空"))?;
        std::fs::create_dir_all(dir).map_err(|e| Failure::io("save_original_config", e))?;
        let bytes = serde_json::to_vec(config)
            .map_err(|e| Failure::new("save_original_config", 13, e.to_string()))?;
        // 复用 Engine 的原子持久写入和提权身份 DACL；首次文件禁止覆盖。
        crate::secure_write::write_atomic_scoped(
            &self.0.with_extension("json.tmp"),
            &self.0,
            dir,
            &bytes,
            None,
            false,
        )
        .map_err(|error| match error {
            crate::secure_write::SecureWriteError::AlreadyExists => Failure::new(
                "save_original_config",
                183,
                "原配置恢复记录已存在，请重试以读取首次记录",
            ),
            crate::secure_write::SecureWriteError::Native(error) => {
                Failure::new("save_original_config", error.code, error.message)
            }
            crate::secure_write::SecureWriteError::Io(error) => {
                Failure::io("save_original_config", error)
            }
        })
    }
    fn clear(&self) -> Result<(), Failure> {
        std::fs::remove_file(&self.0).map_err(|error| Failure::io("clear_recovery", error))
    }
}
mod native;
