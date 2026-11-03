//! 本地用户连接请求：覆盖准备、取消和Core提交，不拥有认证或隧道状态。
use super::error::AppError;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, watch};

/// 本次请求的本机认证准备动作；不改变持久认证模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthPreparationAction {
    Retry,
    DisableOfficialAndRetry,
    UseEmbedded,
    TrySavedLogin,
    ApproveCertificate,
    Cancel,
}

/// 当前请求本地准备交互；Browser 仅接收本次渠道切换。
#[derive(Debug, Clone, Copy, Default)]
pub(super) enum AuthPreparationPhase {
    #[default]
    Idle,
    Exclusive,
    Certificate {
        approval_required: bool,
    },
    Browser,
}

#[derive(Debug)]
struct RequestState {
    next: u64,
    active: Option<ActiveRequest>,
    changed: watch::Sender<u64>,
}
impl Default for RequestState {
    fn default() -> Self {
        Self {
            next: 0,
            active: None,
            changed: watch::channel(0).0,
        }
    }
}
#[derive(Debug)]
struct ActiveRequest {
    preparation: watch::Sender<Option<AuthPreparationAction>>,
    preparation_phase: AuthPreparationPhase,
    preparation_busy: bool,
    auth_deadline_monotonic_ns: u64,
    temporary_auth_mode: Option<&'static str>,
    id: u64,
    cancel: watch::Sender<bool>,
    recovery: bool,
    submitted: bool,
    finished: watch::Sender<bool>,
}
#[derive(Debug, Default)]
pub(crate) struct ConnectionRequests {
    state: Arc<Mutex<RequestState>>,
    pub(crate) commit_gate: Arc<AsyncMutex<()>>,
    pub(super) acquisitions: super::auth_acquisition::AuthAcquisitions,
}
#[derive(Debug)]
pub(crate) struct ConnectionRequest {
    state: Arc<Mutex<RequestState>>,
    id: u64,
    cancelled: watch::Receiver<bool>,
}
impl ConnectionRequests {
    /// 请求取消后仍由原 lease 持有此槽，直到实际收尾完成。
    pub(super) fn has_active_request(&self) -> bool {
        self.state
            .lock()
            .expect("connection request state")
            .active
            .is_some()
    }

    pub(super) fn set_auth_preparation(
        &self,
        generation: u64,
        phase: AuthPreparationPhase,
        busy: bool,
    ) {
        if let Ok(mut state) = self.state.lock()
            && let Some(active) = state
                .active
                .as_mut()
                .filter(|active| active.id == generation)
        {
            active.preparation_phase = phase;
            active.preparation_busy = busy;
        }
    }

    pub(super) fn temporary_auth_mode(&self, generation: u64) -> Option<&'static str> {
        self.state
            .lock()
            .ok()?
            .active
            .as_ref()
            .filter(|active| active.id == generation)?
            .temporary_auth_mode
    }
    pub(crate) fn auth_preparation_actions(
        &self,
        generation: u64,
    ) -> Result<watch::Receiver<Option<AuthPreparationAction>>, String> {
        self.state
            .lock()
            .map_err(|_| "连接请求状态不可用")?
            .active
            .as_ref()
            .filter(|active| active.id == generation)
            .map(|active| active.preparation.subscribe())
            .ok_or_else(|| "本次认证请求已结束".into())
    }
    pub(crate) fn respond_auth_preparation(
        &self,
        generation: u64,
        action: AuthPreparationAction,
    ) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "连接请求状态不可用")?;
        let active = state
            .active
            .as_mut()
            .filter(|active| {
                active.id == generation && !active.submitted && !*active.cancel.borrow()
            })
            .ok_or("本次认证请求已结束")?;
        if action != AuthPreparationAction::Cancel {
            if active.preparation_busy {
                return Err("本次认证准备操作正在执行".into());
            }
            let allowed = match active.preparation_phase {
                AuthPreparationPhase::Idle => false,
                AuthPreparationPhase::Exclusive => matches!(
                    action,
                    AuthPreparationAction::Retry
                        | AuthPreparationAction::DisableOfficialAndRetry
                        | AuthPreparationAction::UseEmbedded
                        | AuthPreparationAction::TrySavedLogin
                ),
                AuthPreparationPhase::Certificate { approval_required } => {
                    matches!(
                        action,
                        AuthPreparationAction::Retry
                            | AuthPreparationAction::UseEmbedded
                            | AuthPreparationAction::TrySavedLogin
                    ) || (approval_required && action == AuthPreparationAction::ApproveCertificate)
                }
                AuthPreparationPhase::Browser => matches!(
                    action,
                    AuthPreparationAction::UseEmbedded | AuthPreparationAction::TrySavedLogin
                ),
            };
            if !allowed {
                return Err("当前认证准备阶段不接受此操作".into());
            }
            active.preparation_busy = true;
        }
        if action == AuthPreparationAction::Cancel {
            active.cancel.send_replace(true);
        }
        active.preparation.send_replace(Some(action));
        Ok(())
    }

    pub(crate) fn begin(&self) -> Result<Arc<ConnectionRequest>, AppError> {
        self.begin_kind(false)
    }
    pub(super) fn recovery_handoff(
        &self,
    ) -> (u64, watch::Receiver<u64>, Option<watch::Receiver<bool>>) {
        let state = self.state.lock().expect("connection request lock");
        (
            state.next,
            state.changed.subscribe(),
            state
                .active
                .as_ref()
                .filter(|active| active.recovery && active.submitted)
                .map(|active| active.finished.subscribe()),
        )
    }
    pub(super) fn begin_recovery(
        &self,
        generation: u64,
    ) -> Result<Arc<ConnectionRequest>, AppError> {
        self.begin_kind_at(true, Some(generation))
    }
    fn begin_kind(&self, recovery: bool) -> Result<Arc<ConnectionRequest>, AppError> {
        self.begin_kind_at(recovery, None)
    }
    fn begin_kind_at(
        &self,
        recovery: bool,
        expected: Option<u64>,
    ) -> Result<Arc<ConnectionRequest>, AppError> {
        let mut state = self.state.lock().expect("connection request lock");
        if expected.is_some_and(|generation| generation != state.next) {
            return Err(AppError::ConnectionCancelled);
        }
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.recovery && !active.submitted)
        {
            if let Some(active) = state.active.take() {
                active.cancel.send_replace(true);
            }
        }
        if state.active.is_some() {
            return Err(AppError::Internal(
                "已有连接请求正在进行，请先完成或取消。".into(),
            ));
        }
        state.next = state.next.wrapping_add(1);
        let id = state.next;
        state.changed.send_replace(id);
        let (send, cancelled) = watch::channel(false);
        state.active = Some(ActiveRequest {
            preparation: watch::channel(None).0,
            preparation_phase: AuthPreparationPhase::Idle,
            preparation_busy: false,
            auth_deadline_monotonic_ns: 0,
            temporary_auth_mode: None,
            id,
            cancel: send,
            recovery,
            submitted: false,
            finished: watch::channel(false).0,
        });
        Ok(Arc::new(ConnectionRequest {
            state: Arc::clone(&self.state),
            id,
            cancelled,
        }))
    }
    pub(crate) fn cancel(&self) {
        let mut state = self.state.lock().expect("connection request lock");
        state.next = state.next.wrapping_add(1);
        state.changed.send_replace(state.next);
        if let Some(active) = state.active.as_ref() {
            active.cancel.send_replace(true);
        }
    }
    /// 事件清除只能取消它所属的待认证请求；已提交的 RPC 持原 lease 收束。
    pub(super) fn cancel_owner(&self, generation: u64) {
        if let Some(active) = self
            .state
            .lock()
            .expect("connection request lock")
            .active
            .as_ref()
        {
            if active.id == generation && !active.submitted {
                active.cancel.send_replace(true);
            }
        }
    }
}
impl ConnectionRequest {
    /// 仅展示本次认证渠道；原配置快照仍用于Core绑定守卫。
    pub(super) fn set_temporary_auth_mode(&self, mode: &'static str) {
        if let Ok(mut state) = self.state.lock()
            && let Some(active) = state.active.as_mut().filter(|active| active.id == self.id)
        {
            active.temporary_auth_mode = Some(mode);
        }
    }
    pub(super) fn set_auth_control_deadline(&self, deadline: u64) {
        if let Ok(mut state) = self.state.lock()
            && let Some(active) = state.active.as_mut().filter(|active| active.id == self.id)
        {
            active.auth_deadline_monotonic_ns = deadline;
        }
    }
    pub(super) fn auth_control_deadline(&self) -> Result<u64, String> {
        self.state
            .lock()
            .map_err(|_| "连接请求状态不可用")?
            .active
            .as_ref()
            .filter(|active| active.id == self.id && active.auth_deadline_monotonic_ns != 0)
            .map(|active| active.auth_deadline_monotonic_ns)
            .ok_or_else(|| "本次认证期限已结束".into())
    }

    pub(super) fn mark_submitted(&self) -> Result<(), AppError> {
        self.begin_submit()?;
        let mut state = self.state.lock().expect("connection request lock");
        let active = state
            .active
            .as_mut()
            .filter(|active| active.id == self.id)
            .ok_or(AppError::ConnectionCancelled)?;
        if *self.cancelled.borrow() {
            return Err(AppError::ConnectionCancelled);
        }
        active.submitted = true;
        Ok(())
    }
    pub(crate) fn generation(&self) -> u64 {
        self.id
    }
    pub(crate) async fn cancelled(&self) {
        let mut cancelled = self.cancelled.clone();
        loop {
            if *cancelled.borrow() {
                return;
            }
            if cancelled.changed().await.is_err() {
                return;
            }
        }
    }
    /// 必须在commit gate许可内调用；之后RPC由独立任务持本请求和许可到回执。
    pub(crate) fn begin_submit(&self) -> Result<(), AppError> {
        let state = self.state.lock().expect("connection request lock");
        if state
            .active
            .as_ref()
            .is_none_or(|active| active.id != self.id)
            || *self.cancelled.borrow()
        {
            Err(AppError::ConnectionCancelled)
        } else {
            Ok(())
        }
    }
}
impl Drop for ConnectionRequest {
    fn drop(&mut self) {
        let mut state = self.state.lock().expect("connection request lock");
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.id)
        {
            if let Some(active) = state.active.take() {
                active.finished.send_replace(true);
            }
        }
    }
}

/// 调用方取消等待不撤销已提交RPC，也不释放其提交许可。
pub(crate) async fn await_submitted<T: Send + 'static>(
    task: impl std::future::Future<Output = Result<T, AppError>> + Send + 'static,
) -> Result<T, AppError> {
    tokio::spawn(task)
        .await
        .map_err(|_| AppError::Internal("连接控制任务异常退出，请重试或导出日志反馈。".into()))?
}
