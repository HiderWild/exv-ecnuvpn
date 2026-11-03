//! 原恢复轮的单次材料续段；只由现有恢复 worker 执行，不另建调度 owner。
use super::*;
use exv_vpn_win32_ipc::auth_deadline::AuthDeadline;
use prost::Message;
use tokio::sync::oneshot;

// 调用者持有部分消息；尾部解码错误也须清除已写入的候选字节。
pub(super) fn merge_acquisition_result(
    decoded: &mut wire::AtrustAuthAcquisitionResult,
    bytes: &[u8],
) -> Result<(), Status> {
    decoded.merge(bytes).map_err(|_| {
        if let Some(wire::atrust_auth_acquisition_result::Result::Candidate(candidate)) =
            decoded.result.as_mut()
        {
            candidate.zeroize();
        }
        Status::invalid_argument("认证回执格式无效")
    })
}

pub(super) struct RecoveryRound {
    /// 原恢复轮关联不随新的 Engine 执行键变化。
    pub round_operation: [u8; 16],
    pub attempt: u32,
    pub dispatch: Uuid,
    pub execution: wire::ConnectIntent,
    pub retired_execution: Option<Vec<u8>>,
    pub context: RecoveryAtrustContext,
    pub acquisition_started: bool,
    pub continuation_queued: bool,
    pub ended: bool,
    pub deadline: Option<AuthDeadline>,
    pub first_error: Option<wire::VpnError>,
    pub pending: Option<PendingAcquisition>,
    pub interaction_id: Option<Vec<u8>>,
    pub cleanup: Option<AuthCleanup>,
}

pub(super) struct AuthCleanup {
    operation: Vec<u8>,
    generation: Option<u64>,
    sender: Option<oneshot::Sender<wire::StopCompletion>>,
    completion: Option<wire::StopCompletion>,
}

pub(super) struct PendingAcquisition {
    request: wire::AtrustAuthAcquisition,
    sender: oneshot::Sender<Result<UiConnectionCredentials, wire::VpnError>>,
}

pub(super) fn deadline_error() -> Status {
    Status::deadline_exceeded("atrust_auth_acquisition|认证已超过本次截止时间")
}

pub(super) async fn prepare_deadline_elapsed(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}

// 只在 Core 两处原获取调用栈内保留完整回执；不编码进 Status 或新增跨进程协议。
pub(super) enum AcquisitionFailure {
    Status(Status),
    Receipt(wire::VpnError),
}

impl From<Status> for AcquisitionFailure {
    fn from(status: Status) -> Self {
        Self::Status(status)
    }
}

impl AcquisitionFailure {
    pub(super) fn status(&self) -> Status {
        match self {
            Self::Status(status) => status.clone(),
            Self::Receipt(error) => Status::new(
                match wire::ErrorCode::try_from(error.code) {
                    Ok(wire::ErrorCode::DeadlineExceeded) => Code::DeadlineExceeded,
                    Ok(wire::ErrorCode::CancelledBeforeStart) => Code::Cancelled,
                    Ok(wire::ErrorCode::Unauthorized) => Code::Unauthenticated,
                    Ok(wire::ErrorCode::PlatformDependencyUnavailable) => Code::Unavailable,
                    _ => Code::Unknown,
                },
                error.reason.clone(),
            ),
        }
    }

    fn wire_error(&self) -> wire::VpnError {
        match self {
            Self::Status(status) => acquisition_error(status),
            Self::Receipt(error) => error.clone(),
        }
    }

    fn service_failure(&self) -> bool {
        match self {
            Self::Status(status) => status.code() == Code::Unavailable,
            Self::Receipt(error) => {
                error.code == wire::ErrorCode::PlatformDependencyUnavailable as i32
            }
        }
    }
}

fn acquisition_error(status: &Status) -> wire::VpnError {
    if !reconnect_authentication_terminal(status, Protocol::Atrust)
        && !matches!(status.code(), Code::Cancelled | Code::DeadlineExceeded)
    {
        let mut error = route_failure_wire_error(status);
        if status.code() != Code::Unavailable {
            error.code = wire::ErrorCode::SessionBusy as i32;
        }
        error.reason = status.message().into();
        return error;
    }
    wire::VpnError {
        code: match status.code() {
            Code::Cancelled => wire::ErrorCode::CancelledBeforeStart,
            Code::DeadlineExceeded => wire::ErrorCode::DeadlineExceeded,
            _ => wire::ErrorCode::Unauthorized,
        } as i32,
        stage: wire::ErrorStage::AwaitingInteraction as i32,
        reason: status.message().into(),
        ..Default::default()
    }
}

pub(super) fn explicit_auth_failure(error: &wire::VpnError) -> bool {
    error.code == wire::ErrorCode::Unauthorized as i32
}

impl KernelControlService {
    pub(super) fn clear_auth_round(&self) {
        *self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.events.set_auth_acquisition(None);
    }

    /// 调用者持 composition；所有记录的接纳、取消与替换都沿这个有序边界。
    pub(super) fn auth_event(
        &self,
        event: &EngineStatusEvent,
        composition: &mut HostComposition,
    ) -> bool {
        let mut record = self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(round) = record.as_mut() else {
            return false;
        };
        if let EngineStatusEvent::Stopped {
            operation_id,
            stop_completion,
            ..
        } = event
            && round
                .cleanup
                .as_ref()
                .is_some_and(|cleanup| &cleanup.operation == operation_id)
        {
            let summary = stop_completion.clone().unwrap_or_else(|| {
                stop_failure_completion(
                    "completion",
                    &Status::unknown("stop event has no completion"),
                )
            });
            self.accept_auth_cleanup(round, composition, summary);
            return true;
        }
        let operation = round
            .execution
            .lookup_key
            .as_ref()
            .map(|key| key.operation_id.as_slice());
        let event_operation = match event {
            EngineStatusEvent::Progress { operation_id, .. }
            | EngineStatusEvent::Connected { operation_id, .. }
            | EngineStatusEvent::Failed { operation_id, .. }
            | EngineStatusEvent::Stopped { operation_id, .. } => operation_id,
        };
        if round.retired_execution.as_deref() == Some(event_operation.as_slice()) {
            return true;
        }
        if operation != Some(event_operation.as_slice()) {
            return false;
        }
        if round.ended || round.continuation_queued {
            return true;
        }
        if let EngineStatusEvent::Failed { error, .. } = event
            && explicit_auth_failure(error)
        {
            round.first_error.get_or_insert_with(|| error.clone());
            round.continuation_queued = true;
            composition.set_last_wire_error(error.clone());
            composition.apply(HostEvent::ConnectFailed(wire_vpn_error_to_domain(error)));
            self.events.publish(
                wire::RuntimeEventKind::Transition,
                snapshot_with_reconnect(
                    snapshot_for_phase(composition.phase(), composition),
                    self.session_reconnect_status(),
                ),
            );
            if let Some(tx) = self
                .reconnect_tx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                let _ = tx.send((ReconnectTrigger::AuthContinuation, round.dispatch));
            }
            return true;
        }
        if matches!(
            event,
            EngineStatusEvent::Connected { .. } | EngineStatusEvent::Failed { .. }
        ) {
            *record = None;
        }
        false
    }

    pub(super) fn auth_round_holds_channel(&self) -> bool {
        let state = self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|round| {
                (
                    round.continuation_queued || round.pending.is_some() || round.ended,
                    round.acquisition_started,
                    round.dispatch,
                )
            });
        match state {
            Some((true, _, _)) => true,
            Some((false, true, id)) => self
                .connect_dispatch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|dispatch| dispatch.id == id && !dispatch.attempt_applied),
            _ => false,
        }
    }

    pub(super) fn auth_prepare_deadline(&self, dispatch_id: Uuid) -> Option<tokio::time::Instant> {
        self.recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|round| round.dispatch == dispatch_id)
            .and_then(|round| round.deadline)
            .map(|deadline| {
                deadline
                    .tokio_deadline()
                    .unwrap_or_else(|_| tokio::time::Instant::now())
            })
    }

    pub(super) fn check_auth_deadline(&self, dispatch_id: Uuid) -> Result<(), Status> {
        let record = self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(round) = record
            .as_ref()
            .filter(|round| round.dispatch == dispatch_id)
            && let Some(deadline) = round.deadline
        {
            deadline.remaining().map_err(|_| deadline_error())?;
        }
        Ok(())
    }

    pub(super) async fn auth_deadline_elapsed(&self, dispatch_id: Uuid) {
        let deadline = self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|round| round.dispatch == dispatch_id)
            .and_then(|round| round.deadline);
        if let Some(deadline) = deadline {
            if let Ok(deadline) = deadline.tokio_deadline() {
                tokio::time::sleep_until(deadline).await;
            }
        } else {
            std::future::pending::<()>().await;
        }
    }

    // 调用者持原 composition 与 recovery 锁；只接纳本轮原清理的实际完成。
    fn accept_auth_cleanup(
        &self,
        round: &mut RecoveryRound,
        composition: &mut HostComposition,
        summary: wire::StopCompletion,
    ) {
        let Some(cleanup) = round.cleanup.as_mut() else {
            return;
        };
        if self.current_reconnect_dispatch_id() != Some(round.dispatch)
            || *self.engine.subscribe_swaps().borrow() != cleanup.generation.unwrap_or(u64::MAX)
            || cleanup.completion.is_some()
        {
            return;
        }
        cleanup.completion = Some(summary.clone());
        if let Some(sender) = cleanup.sender.take() {
            let _ = sender.send(summary.clone());
        }
        if summary.stopped && !summary.blocks_new_connect {
            if let Some(state) = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
                .filter(|state| state.id == round.dispatch)
            {
                state.stop_required = false;
                state.attempt_applied = false;
            }
        }
        composition.stop_completion = Some(summary);
        if round.ended {
            self.events.publish(
                wire::RuntimeEventKind::Transition,
                snapshot_with_reconnect(
                    snapshot_for_phase(composition.phase(), composition),
                    self.session_reconnect_status(),
                ),
            );
        }
    }

    async fn finish_auth_round(
        &self,
        dispatch_id: Uuid,
        error: wire::VpnError,
        summary: Option<wire::StopCompletion>,
    ) {
        let mut composition = self.composition.lock().await;
        if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
            return;
        }
        {
            let mut record = self
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(round) = record
                .as_mut()
                .filter(|round| round.dispatch == dispatch_id)
            else {
                return;
            };
            round.ended = true;
            round.pending = None;
            // 原清理关联保留到真实回执；截止结束等待并不结束 owned RPC。
            if let Some(done) = round
                .cleanup
                .as_ref()
                .and_then(|cleanup| cleanup.completion.clone())
            {
                composition.stop_completion = Some(done);
            } else if let Some(summary) = summary {
                composition.stop_completion = Some(summary);
            }
            // 精确用户取消决定当前终态，旧认证首因仍保留在原轮中。
            let origin = if error.code == wire::ErrorCode::CancelledBeforeStart as i32
                && error.reason == "atrust/auth_cancelled"
            {
                error
            } else {
                round.first_error.clone().unwrap_or(error)
            };
            composition.set_last_wire_error(origin.clone());
            composition.apply(HostEvent::ConnectFailed(wire_vpn_error_to_domain(&origin)));
        }
        self.events.set_auth_acquisition(None);
        self.reconnect_active.store(false, Ordering::Release);
        self.events.publish(
            wire::RuntimeEventKind::Transition,
            snapshot_with_reconnect(
                snapshot_for_phase(composition.phase(), &composition),
                self.session_reconnect_status(),
            ),
        );
    }

    pub(super) async fn finish_auth_before_apply(
        &self,
        dispatch_id: Uuid,
        status: &Status,
    ) -> bool {
        let acquired = self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|round| round.dispatch == dispatch_id && round.acquisition_started);
        let applied = self
            .connect_dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|state| state.id == dispatch_id && state.attempt_applied);
        if acquired
            && !applied
            && (reconnect_authentication_terminal(status, Protocol::Atrust)
                || status.code() == Code::Cancelled)
        {
            self.finish_auth_round(dispatch_id, acquisition_error(status), None)
                .await;
            true
        } else {
            false
        }
    }

    async fn publish_auth_failure(&self, dispatch_id: Uuid, error: wire::VpnError) {
        let mut composition = self.composition.lock().await;
        if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
            return;
        }
        let _ = self.accept_session_input(&mut composition, SessionInput::ConnectFailed(error));
        self.events.publish(
            wire::RuntimeEventKind::Transition,
            snapshot_with_reconnect(
                snapshot_for_phase(composition.phase(), &composition),
                self.session_reconnect_status(),
            ),
        );
    }

    async fn fail_fresh_service_execution(&self, dispatch_id: Uuid, status: &Status) {
        self.fail_auth_service_execution(dispatch_id, acquisition_error(status))
            .await;
    }

    async fn fail_auth_service_execution(&self, dispatch_id: Uuid, error: wire::VpnError) {
        self.publish_auth_failure(dispatch_id, error).await;
        let _composition = self.composition.lock().await;
        if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
            return;
        }
        self.clear_auth_round();
        self.reconnect_active.store(false, Ordering::Release);
        // 真实服务暂错仍回到原预算；该续段不重新获取或重置本轮截止。
        if matches!(
            ServiceMode::from_u8(self.selected_mode.load(Ordering::Relaxed)),
            ServiceMode::Service | ServiceMode::Auto
        ) && let Some(tx) = self
            .reconnect_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            let _ = tx.send((ReconnectTrigger::EngineLostService, dispatch_id));
        }
    }

    pub(super) async fn acquire_recovery_material(
        &self,
        dispatch_id: Uuid,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<(UiConnectionCredentials, ExvConfig), AcquisitionFailure> {
        let result = self
            .acquire_recovery_material_inner(dispatch_id, cancelled)
            .await;
        if let Err(failure) = &result {
            let ended = self
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_none_or(|round| round.dispatch != dispatch_id || round.ended);
            if !ended && !failure.service_failure() {
                self.finish_auth_round(dispatch_id, failure.wire_error(), None)
                    .await;
            } else if !ended {
                self.publish_auth_failure(dispatch_id, failure.wire_error())
                    .await;
            }
        }
        result
    }

    async fn acquire_recovery_material_inner(
        &self,
        dispatch_id: Uuid,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<(UiConnectionCredentials, ExvConfig), AcquisitionFailure> {
        self.require_background_reconnect()?;
        let (receiver, deadline, config) = {
            let composition = self.composition.lock().await;
            self.require_background_reconnect()?;
            if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
                return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED).into());
            }
            let mut record = self
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let round = record
                .as_mut()
                .filter(|round| round.dispatch == dispatch_id && !round.ended)
                .ok_or_else(|| Status::cancelled(AUTO_RECONNECT_SUPERSEDED))?;
            if round.acquisition_started {
                return Err(Status::failed_precondition(
                    "atrust_auth_acquisition|本轮认证获取已经结束",
                )
                .into());
            }
            // 进入即消费，不因失败或重放重新给时间和获取机会。
            round.acquisition_started = true;
            let deadline =
                AuthDeadline::after(Duration::from_secs(300)).map_err(|_| deadline_error())?;
            round.deadline = Some(deadline);
            let id = Uuid::new_v4().as_bytes().to_vec();
            let context = &round.context;
            let gateway = crate::atrust_browser::validated_gateway(&context.config.server)
                .map_err(|_| {
                    Status::failed_precondition("atrust_recovery_material|原会话网关无效")
                })?;
            let profile = context.profile.as_ref().map_err(|reason| {
                Status::failed_precondition(format!("atrust_recovery_material|{reason}"))
            })?;
            let request = wire::AtrustAuthAcquisition {
                interaction_id: id.clone(),
                runtime_epoch: self.auth_runtime_epoch.as_bytes().to_vec(),
                deadline_monotonic_ns: deadline.monotonic_ns(),
                gateway,
                mode: context.effective_mode().into(),
                auto_login: true,
                config_dir: self.config_dir.to_string_lossy().into_owned(),
                browser_profile: profile.cache_profile().to_string_lossy().into_owned(),
                user_agent: String::new(),
                browser_profile_explicit: profile.is_explicit(),
            };
            let (sender, receiver) = oneshot::channel();
            round.interaction_id = Some(id);
            round.pending = Some(PendingAcquisition {
                request: request.clone(),
                sender,
            });
            self.events.set_auth_acquisition(Some(request));
            self.events.publish(
                wire::RuntimeEventKind::Transition,
                snapshot_with_reconnect(
                    snapshot_for_phase(composition.phase(), &composition),
                    self.session_reconnect_status(),
                ),
            );
            (receiver, deadline, round.context.config.clone())
        };
        let local_deadline = deadline.tokio_deadline().map_err(|_| deadline_error())?;
        let result = tokio::select! {
            biased;
            () = connect_cancelled(cancelled) => Err(acquisition_error(&Status::cancelled(AUTO_RECONNECT_SUPERSEDED))),
            result = tokio::time::timeout_at(local_deadline, receiver) => match result {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(acquisition_error(&Status::cancelled(AUTO_RECONNECT_SUPERSEDED))),
                Err(_) => Err(acquisition_error(&deadline_error())),
            }
        };
        match result {
            Ok(credentials) => {
                self.check_auth_deadline(dispatch_id)?;
                self.require_background_reconnect()?;
                if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
                    return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED).into());
                }
                Ok((credentials, config))
            }
            Err(error) => Err(AcquisitionFailure::Receipt(error)),
        }
    }

    /// 已完成授权与字段校验；只取自己的槽，其他交互保留原 Engine 路径。
    pub(super) async fn respond_auth_acquisition(
        &self,
        response: &mut InteractionResponse,
    ) -> Result<bool, Status> {
        let composition = self.composition.lock().await;
        let mut record = self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let local_id = record.as_ref().is_some_and(|round| {
            round.interaction_id.as_deref() == Some(response.interaction_id.as_slice())
        });
        if !local_id && response.runtime_epoch.as_slice() != self.auth_runtime_epoch.as_bytes() {
            return Ok(false);
        }
        // 取消、替代和已消费回执仍属本地认证域，先接管清零再拒绝。
        let mut bytes = zeroize::Zeroizing::new(std::mem::take(&mut response.response_payload));
        let round = record
            .as_mut()
            .filter(|round| {
                round.interaction_id.as_deref() == Some(response.interaction_id.as_slice())
            })
            .ok_or_else(|| Status::failed_precondition("认证回执已经消费或取消"))?;
        let pending = round
            .pending
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("认证回执已经消费或取消"))?;
        if pending.request.runtime_epoch != response.runtime_epoch
            || self.auth_runtime_epoch.as_bytes().as_slice() != response.runtime_epoch.as_slice()
        {
            return Err(Status::failed_precondition("认证回执不属于当前会话"));
        }
        if round.ended || self.current_reconnect_dispatch_id() != Some(round.dispatch) {
            return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED));
        }
        round
            .deadline
            .ok_or_else(deadline_error)?
            .remaining()
            .map_err(|_| deadline_error())?;
        let mut decoded = wire::AtrustAuthAcquisitionResult::default();
        let merged = merge_acquisition_result(&mut decoded, bytes.as_slice());
        bytes.zeroize();
        merged?;
        let result = match decoded.result {
            Some(wire::atrust_auth_acquisition_result::Result::Candidate(mut candidate)) => {
                let session =
                    exv_vpn_win32_ipc::browser_session::BrowserSession::decode(&candidate);
                candidate.zeroize();
                let credentials = UiConnectionCredentials::Atrust(
                    session.map_err(|_| Status::invalid_argument("认证会话材料无效"))?,
                );
                let binding = wire::AuthBinding {
                    protocol: "atrust".into(),
                    gateway: pending.request.gateway.clone(),
                    mode: pending.request.mode.clone(),
                };
                validate_auth_binding(&binding, &round.context.config, Some(&credentials))?;
                Ok(credentials)
            }
            Some(wire::atrust_auth_acquisition_result::Result::Failed(error)) => Err(error),
            None => return Err(Status::invalid_argument("认证回执缺少结果")),
        };
        let pending = round.pending.take().expect("checked pending");
        let _ = pending.sender.send(result);
        self.events.set_auth_acquisition(None);
        self.events.publish(
            wire::RuntimeEventKind::Transition,
            snapshot_with_reconnect(
                snapshot_for_phase(composition.phase(), &composition),
                self.session_reconnect_status(),
            ),
        );
        Ok(true)
    }

    /// 原 worker 的同轮续段：内部真实 Stop，不经过结束会话的用户 Stop 接纳。
    pub(super) async fn continue_auth_recovery(&self, dispatch_id: Uuid) {
        let round_operation = self
            .recovery_round
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|round| round.dispatch == dispatch_id)
            .map(|round| round.round_operation);
        let result = self.continue_auth_recovery_inner(dispatch_id).await;
        if let Err((error, summary)) = result {
            let current = self
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .filter(|round| Some(round.round_operation) == round_operation)
                .map(|round| round.dispatch);
            if let Some(current) = current {
                self.finish_auth_round(current, error, summary).await;
            }
        }
    }

    async fn continue_auth_recovery_inner(
        &self,
        dispatch_id: Uuid,
    ) -> Result<(), (wire::VpnError, Option<wire::StopCompletion>)> {
        let (generation, mut cancelled, already_acquired, first_error) = {
            let _composition = self.composition.lock().await;
            let dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(state) = dispatch
                .as_ref()
                .filter(|state| state.id == dispatch_id && !*state.cancelled.borrow())
            else {
                return Ok(());
            };
            let record = self
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(round) = record.as_ref().filter(|round| {
                round.dispatch == dispatch_id
                    && round.continuation_queued
                    && !round.ended
                    && round.attempt == self.reconnect_attempts.load(Ordering::Relaxed)
            }) else {
                return Ok(());
            };
            (
                state.target_generation,
                state.cancelled.subscribe(),
                round.acquisition_started,
                round.first_error.clone().expect("failure continuation"),
            )
        };
        let summary = self
            .cleanup_auth_execution(dispatch_id, generation, &mut cancelled)
            .await;
        let summary = match summary {
            Ok(summary) => summary,
            Err(status) => stop_failure_completion("auth_recovery", &status),
        };
        if !summary.stopped || summary.blocks_new_connect {
            return Err((first_error, Some(summary)));
        }
        if already_acquired {
            return Err((first_error, Some(summary)));
        }
        let prepared = self
            .acquire_recovery_material(dispatch_id, &mut cancelled)
            .await;
        let result = match prepared {
            Ok((credentials, config)) => self
                .resume_auth_apply(dispatch_id, generation, credentials, config, &mut cancelled)
                .await
                .map_err(AcquisitionFailure::Status),
            Err(failure) => Err(failure),
        };
        match result {
            Ok(()) => Ok(()),
            Err(failure) if failure.service_failure() => {
                let current = self
                    .recovery_round
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .filter(|round| round.dispatch == dispatch_id)
                    .map(|round| round.dispatch);
                if let Some(current) = current {
                    self.fail_auth_service_execution(current, failure.wire_error())
                        .await;
                }
                Ok(())
            }
            Err(failure) => Err((failure.wire_error(), None)),
        }
    }

    async fn cleanup_auth_execution(
        &self,
        dispatch_id: Uuid,
        generation: Option<u64>,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<wire::StopCompletion, Status> {
        let deadline = tokio::time::Instant::now() + STOP_TOTAL_BUDGET;
        let intent = reconnect_intent();
        let mut key = intent.lookup_key.expect("local intent key");
        key.method = wire::OperationMethod::StopTunnel as i32;
        let (sender, receiver) = oneshot::channel();
        {
            let _composition = self.composition.lock().await;
            if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
                return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED));
            }
            let mut record = self
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let round = record
                .as_mut()
                .filter(|round| round.dispatch == dispatch_id)
                .ok_or_else(|| Status::cancelled(AUTO_RECONNECT_SUPERSEDED))?;
            round.cleanup = Some(AuthCleanup {
                operation: key.operation_id.clone(),
                generation,
                sender: Some(sender),
                completion: None,
            });
        }
        let handle = self.engine.current().await;
        let service = self.clone();
        let stop_cancelled = cancelled.clone();
        let stop = tokio::spawn(async move {
            let operation = key.operation_id.clone();
            let result = async {
                let mut engine = tokio::time::timeout_at(deadline, handle.lock())
                    .await
                    .map_err(|_| stop_deadline_status("engine_lock"))?;
                if *service.engine.subscribe_swaps().borrow() != generation.unwrap_or(u64::MAX) {
                    return Err(Status::unavailable("原认证执行的引擎已更换"));
                }
                tokio::time::timeout_at(deadline, engine.ensure_owner_lease())
                    .await
                    .map_err(|_| stop_deadline_status("owner"))?
                    .map_err(grpc_error_to_status)?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(stop_deadline_status("before_dispatch"));
                }
                if service.current_reconnect_dispatch_id() != Some(dispatch_id)
                    || *stop_cancelled.borrow()
                {
                    return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED));
                }
                let reply = engine
                    .stop_tunnel(StopTunnelRequest {
                        lookup_key: Some(key),
                        request_digest: intent.request_digest,
                    })
                    .await
                    .map_err(grpc_error_to_status)?;
                drop(engine);
                match reply.result {
                    Some(wire::stop_tunnel_reply::Result::Pending(_)) | None => receiver
                        .await
                        .map_err(|_| Status::cancelled(AUTO_RECONNECT_SUPERSEDED)),
                    Some(wire::stop_tunnel_reply::Result::Stopped(_)) => {
                        Ok(reply.stop_completion.unwrap_or(wire::StopCompletion {
                            stopped: true,
                            blocks_new_connect: false,
                            errors: vec![],
                        }))
                    }
                    Some(wire::stop_tunnel_reply::Result::Failed(failed)) => {
                        let mut summary = reply.stop_completion.unwrap_or_else(|| {
                            stop_failure_completion(
                                "engine_reply",
                                &Status::unknown("stop failed without cleanup confirmation"),
                            )
                        });
                        if let Some(error) = failed.error
                            && !summary.errors.contains(&error)
                        {
                            summary.errors.push(error);
                        }
                        Ok(summary)
                    }
                }
            }
            .await;
            let mut composition = service.composition.lock().await;
            let mut record = service
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(round) = record.as_mut().filter(|round| {
                round.dispatch == dispatch_id
                    && round
                        .cleanup
                        .as_ref()
                        .is_some_and(|cleanup| cleanup.operation == operation)
            }) {
                let summary = result
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|status| stop_failure_completion("auth_recovery", status));
                service.accept_auth_cleanup(round, &mut composition, summary);
            }
            result
        });
        let summary = tokio::select! {
            biased;
            () = connect_cancelled(cancelled) => return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED)),
            result = tokio::time::timeout_at(deadline, stop) => result.map_err(|_| stop_deadline_status("auth_recovery"))?
                .map_err(|_| Status::internal("认证清理任务结束"))??,
        };
        let mut composition = self.composition.lock().await;
        if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
            return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED));
        }
        if summary.stopped && !summary.blocks_new_connect {
            if let Some(state) = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
            {
                state.stop_required = false;
                state.attempt_applied = false;
            }
        }
        composition.stop_completion = Some(summary.clone());
        Ok(summary)
    }

    async fn resume_auth_apply(
        &self,
        old_dispatch: Uuid,
        generation: Option<u64>,
        credentials: UiConnectionCredentials,
        config: ExvConfig,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<(), Status> {
        let _route = tokio::select! { biased;
            () = connect_cancelled(cancelled) => return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED)),
            () = self.auth_deadline_elapsed(old_dispatch) => return Err(deadline_error()),
            guard = self.connect_route_lock.lock() => guard,
        };
        self.check_auth_deadline(old_dispatch)?;
        let generation = generation.ok_or_else(|| Status::unavailable("原认证执行没有引擎代次"))?;
        tokio::select! { biased;
            () = connect_cancelled(cancelled) => return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED)),
            () = self.auth_deadline_elapsed(old_dispatch) => return Err(deadline_error()),
            result = self.await_status_ready(generation) => result?,
        }
        self.check_auth_deadline(old_dispatch)?;
        let intent = reconnect_intent();
        let dispatch_id = Uuid::new_v4();
        {
            let mut composition = self.composition.lock().await;
            let mut dispatch = self
                .connect_dispatch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = dispatch
                .as_mut()
                .filter(|state| state.id == old_dispatch && !*state.cancelled.borrow())
                .ok_or_else(|| Status::cancelled(AUTO_RECONNECT_SUPERSEDED))?;
            let mut record = self
                .recovery_round
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let round = record
                .as_mut()
                .filter(|round| round.dispatch == old_dispatch && !round.ended)
                .ok_or_else(|| Status::cancelled(AUTO_RECONNECT_SUPERSEDED))?;
            round
                .deadline
                .ok_or_else(deadline_error)?
                .remaining()
                .map_err(|_| deadline_error())?;
            // 本轮预算保留；新材料使用新的真实执行键，旧终态不能命中新请求。
            if matches!(
                composition.apply(HostEvent::Connect),
                HostEffect::ConnectRefused(_)
            ) {
                return Err(Status::failed_precondition("认证续段连接未被接纳"));
            }
            let cleanup_summary = composition.stop_completion.clone();
            composition.set_operation_id(
                intent
                    .lookup_key
                    .as_ref()
                    .and_then(|key| key.operation_id.as_slice().try_into().ok())
                    .expect("local operation"),
            );
            composition.stop_completion = cleanup_summary;
            state.id = dispatch_id;
            state.attempt_applied = false;
            round.dispatch = dispatch_id;
            round.retired_execution = round
                .execution
                .lookup_key
                .as_ref()
                .map(|key| key.operation_id.clone());
            round.execution = intent.clone();
            round.continuation_queued = false;
            round.cleanup = None;
            self.events.publish(
                wire::RuntimeEventKind::Transition,
                snapshot_with_reconnect(
                    snapshot_for_phase(composition.phase(), &composition),
                    self.session_reconnect_status(),
                ),
            );
        }
        let handle = self.engine.current().await;
        let mut engine = tokio::select! { biased;
            () = connect_cancelled(cancelled) => return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED)),
            () = self.auth_deadline_elapsed(dispatch_id) => return Err(deadline_error()),
            engine = handle.lock() => engine,
        };
        let mode = config.connection_mode;
        let execution_operation = intent
            .lookup_key
            .as_ref()
            .expect("local intent key")
            .operation_id
            .clone();
        let result = execute_connect_tracked_before_deadline(
            &mut *engine,
            &self.config_dir,
            intent,
            Some(credentials),
            false,
            Some(mode),
            Some(Protocol::Atrust),
            Some(config),
            &self.reconnect_config_cache,
            self.ui_pid(),
            cancelled,
            self.auth_prepare_deadline(dispatch_id),
            || {
                self.check_auth_deadline(dispatch_id)?;
                self.require_background_reconnect()?;
                let mut dispatch = self
                    .connect_dispatch
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let state = dispatch
                    .as_mut()
                    .filter(|state| state.id == dispatch_id && !*state.cancelled.borrow())
                    .ok_or_else(|| Status::cancelled(AUTO_RECONNECT_SUPERSEDED))?;
                let attachment = *self.status_attachment.borrow();
                if *self.engine.subscribe_swaps().borrow() != generation
                    || attachment.generation != generation
                    || !attachment.attached
                {
                    return Err(Status::unavailable(
                        "status_stream_not_ready|认证续段的引擎订阅已更换",
                    ));
                }
                state.attempt_applied = true;
                state.stop_required = true;
                Ok(())
            },
        )
        .await;
        drop(engine);
        if self.current_reconnect_dispatch_id() != Some(dispatch_id) {
            return Ok(());
        }
        match result {
            Ok((reply, _)) => {
                if let Some(wire::apply_tunnel_reply::Result::Failed(failed)) = reply.result
                    && let Some(error) = failed.error
                {
                    let mut composition = self.composition.lock().await;
                    let event = EngineStatusEvent::Failed {
                        operation_id: execution_operation.clone(),
                        error,
                        reconnect_state: None,
                    };
                    let _ = self
                        .accept_session_input(&mut composition, SessionInput::EngineEvent(&event));
                }
                Ok(())
            }
            Err(status) => {
                if self.finish_auth_before_apply(dispatch_id, &status).await {
                    return Ok(());
                }
                if reconnect_authentication_terminal(&status, Protocol::Atrust) {
                    let error = acquisition_error(&status);
                    let mut composition = self.composition.lock().await;
                    let event = EngineStatusEvent::Failed {
                        operation_id: execution_operation.clone(),
                        error,
                        reconnect_state: None,
                    };
                    let _ = self
                        .accept_session_input(&mut composition, SessionInput::EngineEvent(&event));
                } else {
                    self.fail_fresh_service_execution(dispatch_id, &status)
                        .await;
                }
                Ok(())
            }
        }
    }
}
