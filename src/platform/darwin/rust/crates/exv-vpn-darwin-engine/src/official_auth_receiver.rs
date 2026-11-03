use exv_vpn_wire::generated as wire;
use serde::{Deserialize, Serialize};
use std::time::Instant;
mod native;
use native::{FileRecoveryStore, NativeHost};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetState {
    domain: String,
    label: String,
    plist: String,
    argv: Vec<String>,
    // None is an absent launchd override; it is not an explicit false.
    disabled: Option<bool>,
    #[serde(default)]
    default_disabled: bool,
    registered: bool,
    running: bool,
}
impl TargetState {
    fn target(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }
    fn effectively_disabled(&self) -> bool {
        self.disabled.unwrap_or(self.default_disabled)
    }
    fn restore_step(&self) -> String {
        let action = match self.disabled {
            None if self.default_disabled => "disabled_from_unset",
            None => "enabled_from_unset",
            Some(_) => "restore_disabled_override",
        };
        format!("{action}:{}", self.target())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginalReceiverConfig {
    schema: u32,
    owner_uid: u32,
    targets: Vec<TargetState>,
}
struct Observation {
    targets: Vec<TargetState>,
    residual_running: bool,
}
#[derive(Clone, Debug)]
struct ReceiverError {
    step: String,
    code: Option<i32>,
    namespace: wire::NativeErrorNamespace,
    detail: String,
}
impl ReceiverError {
    fn new(step: &str, code: Option<i32>, detail: &str) -> Self {
        Self {
            step: step.into(),
            code,
            namespace: wire::NativeErrorNamespace::PosixErrno,
            detail: detail.chars().take(140).collect(),
        }
    }
    fn io(step: &str, error: std::io::Error) -> Self {
        Self::new(step, error.raw_os_error(), &error.to_string())
    }
    fn recovery_read(self) -> Self {
        if self.step == "read_recovery" {
            self
        } else {
            let detail = format!("{}: {}", self.step, self.detail);
            Self {
                step: "read_recovery".into(),
                detail,
                ..self
            }
        }
    }
    fn wire(&self, partial: bool) -> wire::VpnError {
        wire::VpnError {
            code: if self.code == Some(libc::ETIMEDOUT) {
                wire::ErrorCode::DeadlineExceeded
            } else {
                wire::ErrorCode::EffectUnknown
            } as i32,
            stage: wire::ErrorStage::ConnectingControl as i32,
            certainty: if partial {
                wire::EffectCertainty::Partial
            } else {
                wire::EffectCertainty::NoEffect
            } as i32,
            retry: wire::RetryAdvice::RetrySameOperation as i32,
            reason: format!("official_auth_receiver/{}", self.step),
            native: self.code.map(|code| wire::RedactedNativeError {
                category: wire::NativeErrorCategory::Resource as i32,
                namespace: self.namespace as i32,
                code: i64::from(code),
            }),
            operation_detail: Some(
                format!("{}: {}", self.step, self.detail)
                    .chars()
                    .take(200)
                    .collect(),
            ),
            ..Default::default()
        }
    }
}
trait ReceiverHost {
    fn inspect(&self, deadline: Instant) -> Result<Observation, ReceiverError>;
    fn disable(&self, target: &TargetState, deadline: Instant) -> Result<(), ReceiverError>;
    fn bootout(&self, target: &TargetState, deadline: Instant) -> Result<(), ReceiverError>;
    fn wait_stopped(&self, targets: &[TargetState], deadline: Instant)
    -> Result<(), ReceiverError>;
    fn cleanup(&self, deadline: Instant) -> Result<(), ReceiverError>;
    fn restore(
        &self,
        target: &TargetState,
        deadline: Instant,
        steps: &mut Vec<String>,
    ) -> Result<(), ReceiverError>;
}
trait RecoveryStore {
    fn load(&self) -> Result<Option<OriginalReceiverConfig>, ReceiverError>;
    fn save(&self, value: &OriginalReceiverConfig) -> Result<(), ReceiverError>;
    fn remove(&self) -> Result<(), ReceiverError>;
}
struct ReceiverOutcome {
    status: wire::AuthReceiverStatus,
    completed_steps: Vec<String>,
    error: Option<ReceiverError>,
}
struct OfficialAuthReceiver<'a, H, S> {
    host: &'a H,
    store: &'a S,
    owner_uid: u32,
}
impl<'a, H: ReceiverHost, S: RecoveryStore> OfficialAuthReceiver<'a, H, S> {
    fn new(host: &'a H, store: &'a S, owner_uid: u32) -> Self {
        Self {
            host,
            store,
            owner_uid,
        }
    }
    fn read_recovery(&self) -> Result<Option<OriginalReceiverConfig>, ReceiverError> {
        let original = self.store.load().map_err(ReceiverError::recovery_read)?;
        if original
            .as_ref()
            .is_some_and(|s| s.schema != 1 || s.owner_uid != self.owner_uid)
        {
            return Err(ReceiverError::new(
                "recovery_identity",
                None,
                "recovery owner or schema differs",
            )
            .recovery_read());
        }
        Ok(original)
    }
    fn outcome(
        &self,
        steps: Vec<String>,
        mut error: Option<ReceiverError>,
        deadline: Instant,
    ) -> ReceiverOutcome {
        // 查询当前服务与恢复记录分别取值，记录读取失败仍保留实际服务状态。
        let original = self.read_recovery();
        let state = match self.host.inspect(deadline) {
            Ok(current) => status(&current, original.as_ref().ok().and_then(|s| s.as_ref())),
            Err(observation_error) => {
                if error.is_none() {
                    error = Some(observation_error);
                }
                wire::AuthReceiverStatus {
                    recovery_available: original.as_ref().ok().is_some_and(|s| s.is_some()),
                    source: "macOS aTrust launchd; current observation failed".into(),
                    ..Default::default()
                }
            }
        };
        if error.is_none() {
            error = original.err();
        }
        ReceiverOutcome {
            status: state,
            completed_steps: steps,
            error,
        }
    }
    fn disable_and_stop(&self, deadline: Instant) -> ReceiverOutcome {
        let mut steps = Vec::new();
        let result = (|| {
            check_deadline(deadline, "inspect")?;
            let observation = self.host.inspect(deadline)?;
            let saved = self.read_recovery()?;
            let first_save = saved.is_none();
            let mut original = saved.unwrap_or_else(|| OriginalReceiverConfig {
                schema: 1,
                owner_uid: self.owner_uid,
                targets: Vec::new(),
            });
            let saved_count = original.targets.len();
            for target in &observation.targets {
                if !original
                    .targets
                    .iter()
                    .any(|saved| saved.domain == target.domain && saved.label == target.label)
                {
                    original.targets.push(target.clone());
                }
            }
            if first_save || original.targets.len() != saved_count {
                self.store.save(&original)?;
                steps.push("save_original_launchd_state".into());
            }
            // ALL relaunch sources become disabled before ANY normal bootout.
            for target in &observation.targets {
                check_deadline(deadline, "disable")?;
                self.host.disable(target, deadline)?;
                steps.push(format!("disable:{}", target.target()));
            }
            for target in observation
                .targets
                .iter()
                .filter(|target| target.registered)
            {
                check_deadline(deadline, "bootout")?;
                self.host.bootout(target, deadline)?;
                steps.push(format!("bootout:{}", target.target()));
            }
            self.host.wait_stopped(&observation.targets, deadline)?;
            steps.push("wait_launchd_stopped".into());
            self.host.cleanup(deadline)?;
            steps.push("cleanup_verified_official_residuals".into());
            Ok(())
        })();
        self.outcome(steps, result.err(), deadline)
    }
    fn restore_action(&self, deadline: Instant, steps: &mut Vec<String>) -> Result<(), ReceiverError> {
        let Some(original) = self.read_recovery()? else { return Ok(()); };
        for target in &original.targets {
            check_deadline(deadline, "restore")?;
            self.host.restore(target, deadline, steps)?;
        }
        self.store.remove()?;
        steps.push("remove_recovery_after_restore".into());
        Ok(())
    }
    fn restore(&self, deadline: Instant) -> ReceiverOutcome {
        let mut steps = Vec::new();
        let result = self.restore_action(deadline, &mut steps);
        self.outcome(steps, result.err(), deadline)
    }
}
fn status(
    current: &Observation,
    original: Option<&OriginalReceiverConfig>,
) -> wire::AuthReceiverStatus {
    wire::AuthReceiverStatus {
        running: current.residual_running || current.targets.iter().any(|t| t.running),
        can_start: current.residual_running
            || current.targets.iter().any(|t| !t.effectively_disabled()),
        recovery_available: original.is_some(),
        source: "macOS aTrust fixed launchd jobs and verified Core/Tray executables".into(),
        original_config: original.map(|s| summary(&s.targets)),
        current_config: Some(summary(&current.targets)),
    }
}
fn summary(targets: &[TargetState]) -> wire::AuthReceiverConfigSummary {
    wire::AuthReceiverConfigSummary {
        launchd_targets: targets
            .iter()
            .map(|t| wire::AuthReceiverLaunchdTarget {
                domain: t.domain.clone(),
                label: t.label.clone(),
                disabled: t.effectively_disabled(),
            })
            .collect(),
        ..Default::default()
    }
}
fn check_deadline(deadline: Instant, step: &str) -> Result<(), ReceiverError> {
    if Instant::now() >= deadline {
        Err(ReceiverError::new(
            step,
            Some(libc::ETIMEDOUT),
            "operation deadline reached",
        ))
    } else {
        Ok(())
    }
}

pub(crate) async fn control(
    owner_uid: u32,
    request: wire::AuthReceiverControlRequest,
) -> Result<wire::AuthReceiverControlReply, tonic::Status> {
    if request.operation_id.len() != 16 {
        return Err(tonic::Status::invalid_argument(
            "auth_receiver_control/operation_id must be 16 bytes",
        ));
    }
    let action = wire::AuthReceiverAction::try_from(request.action)
        .map_err(|_| tonic::Status::invalid_argument("auth_receiver_control/action is required"))?;
    if !matches!(
        action,
        wire::AuthReceiverAction::Inspect
            | wire::AuthReceiverAction::DisableAndStop
            | wire::AuthReceiverAction::Restore
            | wire::AuthReceiverAction::ReadRecovery
    ) {
        return Err(tonic::Status::invalid_argument(
            "auth_receiver_control/action is required",
        ));
    }
    let auth_deadline = exv_vpn_darwin_ipc::auth_deadline::AuthDeadline::from_monotonic_ns(
        request.deadline_monotonic_ns,
    )
    .map_err(|error| match error {
        exv_vpn_darwin_ipc::auth_deadline::AuthDeadlineError::Expired => {
            tonic::Status::deadline_exceeded("auth_receiver_control/deadline expired")
        }
        _ => tonic::Status::invalid_argument("auth_receiver_control/invalid deadline"),
    })?;
    let deadline = auth_deadline
        .tokio_deadline()
        .map_err(|_| tonic::Status::deadline_exceeded("auth_receiver_control/deadline expired"))?
        .into_std();
    // SAFETY: querying current credentials has no side effect. Both Engine forms
    // already authenticated the peer and resolved this same runtime owner.
    if unsafe { libc::geteuid() } != 0 {
        return Err(tonic::Status::permission_denied(
            "auth_receiver_control requires root Engine",
        ));
    }
    tokio::task::spawn_blocking(move || {
        let store = FileRecoveryStore::new(
            std::path::PathBuf::from(crate::connect_gate::TRACE_ROOT)
                .join(owner_uid.to_string())
                .join("official-auth-receiver.v1.json"),
        );
        let host = NativeHost::new(owner_uid);
        let receiver = OfficialAuthReceiver::new(&host, &store, owner_uid);
        let outcome = execute(&receiver, &store, action, deadline);
        wire::AuthReceiverControlReply {
            receiver: Some(outcome.status),
            error: outcome
                .error
                .map(|e| e.wire(!outcome.completed_steps.is_empty())),
            completed_steps: outcome.completed_steps,
        }
    })
    .await
    .map_err(|_| tonic::Status::internal("official_auth_receiver/worker failed"))
}

// 修改操作共享文件锁；只读查询沿原子发布读取完整版本，保持零写入。
fn execute<H: ReceiverHost>(
    receiver: &OfficialAuthReceiver<'_, H, FileRecoveryStore>,
    store: &FileRecoveryStore,
    action: wire::AuthReceiverAction,
    deadline: Instant,
) -> ReceiverOutcome {
    if matches!(
        action,
        wire::AuthReceiverAction::Inspect | wire::AuthReceiverAction::ReadRecovery
    ) {
        return receiver.outcome(Vec::new(), None, deadline);
    }
    let _lock = match store.lock(deadline) {
        Ok(lock) => lock,
        Err(error) => return receiver.outcome(Vec::new(), Some(error), deadline),
    };
    match action {
        wire::AuthReceiverAction::DisableAndStop => receiver.disable_and_stop(deadline),
        wire::AuthReceiverAction::Restore => receiver.restore(deadline),
        _ => unreachable!("control validates the action"),
    }
}

/// 完整卸载仅消费恢复动作结果；无记录保持零新增文件及零状态观察。
fn restore_for_uninstall_with<H: ReceiverHost>(
    host: &H, store: &FileRecoveryStore, owner_uid: u32, deadline: Instant,
) -> Result<(), ReceiverError> {
    let receiver = OfficialAuthReceiver::new(host, store, owner_uid);
    if receiver.read_recovery()?.is_none() { return Ok(()); }
    let lock = store.lock(deadline)?;
    receiver.restore_action(deadline, &mut Vec::new())?;
    drop(lock);
    store.cleanup_after_restore()
}

pub(crate) fn restore_for_uninstall(owner_uid: u32) -> Result<(), String> {
    // SAFETY: geteuid只读取当前进程凭据，内部卸载动作沿现有root执行边界。
    if unsafe { libc::geteuid() } != 0 {
        return Err("DARWIN_AUTH_RECEIVER_RESTORE_FAILED step=authorization native_code=1 detail=完整卸载恢复需要管理员权限".into());
    }
    let store = FileRecoveryStore::new(std::path::PathBuf::from(crate::connect_gate::TRACE_ROOT)
        .join(owner_uid.to_string()).join("official-auth-receiver.v1.json"));
    restore_for_uninstall_with(&NativeHost::new(owner_uid), &store, owner_uid,
        Instant::now() + std::time::Duration::from_secs(30))
        .map_err(|error| format!("DARWIN_AUTH_RECEIVER_RESTORE_FAILED step={} native_code={:?} detail={}",
            error.step, error.code, error.detail))
}
