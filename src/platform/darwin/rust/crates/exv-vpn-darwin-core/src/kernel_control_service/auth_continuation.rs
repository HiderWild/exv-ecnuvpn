//! Darwin 原 generation 内的一次认证获取与同轮续段。
use super::*;
use prost::Message;

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
    pub(super) generation: u64,
    pub(super) round_operation: Vec<u8>,
    pub(super) execution: Vec<u8>,
    policy: SessionReconnectPolicy,
    trigger: ReconnectSignal,
    acquisition_started: bool,
    continuation_queued: bool,
    pub(super) ended: bool,
    pub(super) deadline: Option<AuthDeadline>,
    first_error: Option<wire::VpnError>,
    latest_error: Option<wire::VpnError>,
    pending: Option<PendingAcquisition>,
}

struct PendingAcquisition {
    request: wire::AtrustAuthAcquisition,
    sender: oneshot::Sender<Result<UiSecretPayload, wire::VpnError>>,
}

enum AcquisitionFailure {
    Status(Status),
    Receipt(wire::VpnError),
}

fn acquisition_status_error(status: &Status) -> wire::VpnError {
    let mut error = reconnect_dispatch_vpn_error(status);
    error.reason = status.message().to_owned();
    error.code = match status.code() {
        Code::DeadlineExceeded => wire::ErrorCode::DeadlineExceeded as i32,
        Code::Cancelled => wire::ErrorCode::CancelledBeforeStart as i32,
        Code::Unavailable => wire::ErrorCode::PlatformDependencyUnavailable as i32,
        _ => error.code,
    };
    error
}

impl RecoveryRound {
    pub(super) fn pending_request(&self) -> Option<wire::AtrustAuthAcquisition> {
        self.pending.as_ref().map(|pending| pending.request.clone())
    }

    pub(super) fn take_pending(&mut self) {
        self.pending.take();
    }

    pub(super) fn first_error(&self) -> Option<wire::VpnError> {
        self.first_error.clone()
    }
}

pub(super) fn interaction_id() -> Vec<u8> {
    let mut bytes = vec![0; 16];
    getrandom::fill(&mut bytes).expect("Core authentication interaction random source");
    bytes
}

impl LiveConnectionProjection {
    pub(super) async fn await_auth_preparation<T>(
        &self,
        generation: u64,
        deadline: Option<AuthDeadline>,
        prepare: impl Future<Output = T>,
    ) -> Result<T, Status> {
        match deadline {
            Some(_) => {
                self.await_service_connection(generation, deadline, prepare)
                    .await
            }
            None => Ok(prepare.await),
        }
    }

    pub(super) fn admitted_auth_material(
        &self,
        generation: u64,
        deadline: Option<AuthDeadline>,
    ) -> bool {
        self.reconnect_generation
            .in_generation(generation, || {
                lock_recover(&self.reconnect_generation.recovery)
                    .as_ref()
                    .is_some_and(|round| {
                        round.generation == generation
                            && !round.ended
                            && round.acquisition_started
                            && round.pending.is_none()
                            && round.deadline == deadline
                            && deadline.is_some()
                    })
            })
            .unwrap_or(false)
    }
    pub(super) fn auth_acquisition(&self) -> Option<wire::AtrustAuthAcquisition> {
        lock_recover(&self.reconnect_generation.recovery)
            .as_ref()
            .filter(|round| round.generation == self.reconnect_generation.current() && !round.ended)
            .and_then(|round| {
                round
                    .pending
                    .as_ref()
                    .map(|pending| pending.request.clone())
            })
    }

    pub(super) fn publish_auth_acquisition(&self) {
        if let Some(snapshot) = self.events.current_snapshot() {
            let identity = self
                .reconnect_generation
                .identity_for(&snapshot.operation_id);
            self.publish_with_identity(snapshot, identity);
        }
    }

    pub(super) fn begin_auth_round(
        &self,
        generation: u64,
        request: &ConnectRequest,
        policy: SessionReconnectPolicy,
        trigger: ReconnectSignal,
    ) {
        self.reconnect_generation.begin_recovery(
            generation,
            policy.protocol == Protocol::Atrust,
            || {
                *lock_recover(&self.reconnect_generation.recovery) =
                    (policy.protocol == Protocol::Atrust).then(|| {
                        let operation = operation_id_from_connect(request);
                        RecoveryRound {
                            generation,
                            round_operation: operation.clone(),
                            execution: operation,
                            policy,
                            trigger,
                            acquisition_started: false,
                            continuation_queued: false,
                            ended: false,
                            deadline: None,
                            first_error: None,
                            latest_error: None,
                            pending: None,
                        }
                    });
            },
        );
    }

    pub(super) fn has_auth_round(&self, generation: u64) -> bool {
        lock_recover(&self.reconnect_generation.recovery)
            .as_ref()
            .is_some_and(|round| round.generation == generation && !round.ended)
    }

    pub(super) fn auth_round_ended(&self, generation: u64) -> bool {
        lock_recover(&self.reconnect_generation.recovery)
            .as_ref()
            .is_some_and(|round| round.generation == generation && round.ended)
    }

    /// 已在原 source 接纳临界区内；重复失败及 EOF 不再重新安排本轮。
    pub(super) fn queue_auth_continuation(
        admission: &session_orchestration::SessionAdmission,
        generation: u64,
        execution: &[u8],
        error: &wire::VpnError,
    ) -> bool {
        let mut record = lock_recover(&admission.recovery);
        let Some(round) = record.as_mut().filter(|round| {
            round.generation == generation
                && admission.current() == generation
                && round.execution == execution
                && !round.ended
        }) else {
            return false;
        };
        let recognized = if round.acquisition_started {
            exv_vpn_darwin_ipc::atrust_auth_error::is_atrust_auth(error)
        } else {
            exv_vpn_darwin_ipc::atrust_auth_error::is_session_invalidated(error)
        };
        if !recognized {
            return false;
        }
        if round.continuation_queued {
            return true;
        }
        round.continuation_queued = true;
        round.latest_error = Some(error.clone());
        if round.first_error.is_none() {
            round.first_error = Some(error.clone());
        }
        true
    }

    async fn acquire_recovery_material(
        &self,
        generation: u64,
    ) -> Result<(UiSecretPayload, SessionReconnectPolicy, AuthDeadline), AcquisitionFailure> {
        self.require_background_reconnect()
            .map_err(AcquisitionFailure::Status)?;
        let registered = self
            .reconnect_generation
            .in_generation(generation, || {
                self.require_background_reconnect()?;
                let mut record = lock_recover(&self.reconnect_generation.recovery);
                let round = record
                    .as_mut()
                    .filter(|round| round.generation == generation && !round.ended)
                    .ok_or_else(|| Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"))?;
                if round.acquisition_started {
                    return Err(Status::failed_precondition("本轮认证获取已经结束"));
                }
                // 机会在获取起点消费；同步缺失与异步拒绝共享此事实。
                round.acquisition_started = true;
                let duration = Duration::from_secs(300);
                #[cfg(test)]
                let duration = self.auth_acquisition_duration.min(duration);
                let deadline = match round.deadline {
                    Some(value) => value,
                    None => AuthDeadline::after(duration).map_err(auth_deadline_status)?,
                };
                round.deadline = Some(deadline);
                let (sender, receiver) = oneshot::channel();
                let request = wire::AtrustAuthAcquisition {
                    interaction_id: interaction_id(),
                    runtime_epoch: self.auth_runtime_epoch.clone(),
                    deadline_monotonic_ns: deadline.monotonic_ns(),
                    gateway: format!("https://{}", round.policy.atrust_server),
                    mode: "headless".into(),
                    auto_login: true,
                    config_dir: round.policy.config_dir.clone(),
                    user_agent: round.policy.user_agent.clone(),
                    browser_profile: String::new(),
                    browser_profile_explicit: false,
                };
                round.pending = Some(PendingAcquisition { request, sender });
                Ok((receiver, round.policy.clone(), deadline))
            })
            .unwrap_or_else(|| Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED")))
            .map_err(AcquisitionFailure::Status)?;
        let (receiver, policy, deadline) = registered;
        self.publish_auth_acquisition();
        // 不持配置、session 或 source 锁；沿原 generation/期限等待。
        let result = self
            .await_service_connection(generation, Some(deadline), receiver)
            .await;
        let cleared_pending = self
            .reconnect_generation
            .in_generation(generation, || {
                lock_recover(&self.reconnect_generation.recovery)
                    .as_mut()
                    .is_some_and(|round| round.pending.take().is_some())
            })
            .unwrap_or(false);
        if cleared_pending {
            self.publish_auth_acquisition();
        }
        let credentials = result
            .map_err(AcquisitionFailure::Status)?
            .map_err(|_| {
                AcquisitionFailure::Status(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"))
            })?
            .map_err(AcquisitionFailure::Receipt)?;
        check_auth_deadline(Some(deadline)).map_err(AcquisitionFailure::Status)?;
        self.require_background_reconnect()
            .map_err(AcquisitionFailure::Status)?;
        Ok((credentials, policy, deadline))
    }

    /// 上游 Darwin 已认证通道进入；普通交互继续原 prep_only 合同。
    pub(super) fn respond_auth_acquisition(
        &self,
        response: &mut InteractionResponse,
    ) -> Result<bool, Status> {
        let local = lock_recover(&self.reconnect_generation.recovery)
            .as_ref()
            .and_then(|round| round.pending.as_ref())
            .is_some_and(|pending| pending.request.interaction_id == response.interaction_id);
        if !local && response.runtime_epoch != self.auth_runtime_epoch {
            return Ok(false);
        }
        let mut bytes = zeroize::Zeroizing::new(std::mem::take(&mut response.response_payload));
        let generation = self.reconnect_generation.current();
        let result = self
            .reconnect_generation
            .in_generation(generation, || {
                let mut record = lock_recover(&self.reconnect_generation.recovery);
                let round = record
                    .as_mut()
                    .filter(|round| round.generation == generation && !round.ended)
                    .ok_or_else(|| Status::failed_precondition("认证回执已经消费或取消"))?;
                let pending = round
                    .pending
                    .as_ref()
                    .filter(|pending| {
                        pending.request.interaction_id == response.interaction_id
                            && pending.request.runtime_epoch == response.runtime_epoch
                    })
                    .ok_or_else(|| Status::failed_precondition("认证回执不属于当前会话"))?;
                check_auth_deadline(round.deadline)?;
                let mut decoded = wire::AtrustAuthAcquisitionResult::default();
                let merged = merge_acquisition_result(&mut decoded, bytes.as_slice());
                bytes.zeroize();
                merged?;
                let result = match decoded.result {
                    Some(wire::atrust_auth_acquisition_result::Result::Candidate(
                        mut candidate,
                    )) => {
                        let package = parse_ui_secret_payload(&candidate);
                        candidate.zeroize();
                        let credentials = package.map_err(|_| {
                            Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE)
                        })?;
                        let UiSecretPayload::Atrust(package) = &credentials else {
                            return Err(Status::invalid_argument(
                                CONNECTION_CREDENTIALS_INVALID_CODE,
                            ));
                        };
                        let gateway = exv_vpn_darwin_ipc::browser_session::GatewayOrigin::parse(
                            &pending.request.gateway,
                        )
                        .map_err(|_| {
                            Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE)
                        })?;
                        let actual = exv_vpn_darwin_ipc::browser_session::GatewayOrigin::parse(
                            package.browser_session.gateway(),
                        )
                        .map_err(|_| {
                            Status::invalid_argument(CONNECTION_CREDENTIALS_INVALID_CODE)
                        })?;
                        if gateway.endpoint() != actual.endpoint() {
                            return Err(Status::invalid_argument(
                                CONNECTION_CREDENTIALS_INVALID_CODE,
                            ));
                        }
                        Ok(credentials)
                    }
                    Some(wire::atrust_auth_acquisition_result::Result::Failed(error)) => Err(error),
                    None => return Err(Status::invalid_argument("认证回执缺少结果")),
                };
                let pending = round.pending.take().expect("checked pending acquisition");
                let _ = pending.sender.send(result);
                Ok(true)
            })
            .unwrap_or_else(|| Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED")));
        self.publish_auth_acquisition();
        result
    }

    pub(super) async fn acquire_and_apply_recovery(
        &self,
        generation: u64,
    ) -> Result<OperationReply, Status> {
        let acquired = self.acquire_recovery_material(generation).await;
        let (credentials, policy, deadline) = match acquired {
            Ok(value) => value,
            Err(error) => {
                let (status, original, service_failure) = match error {
                    AcquisitionFailure::Status(status) => {
                        let service = status.code() == Code::Unavailable;
                        let original = acquisition_status_error(&status);
                        (status, original, service)
                    }
                    AcquisitionFailure::Receipt(error) => {
                        let service =
                            error.code == wire::ErrorCode::PlatformDependencyUnavailable as i32;
                        (
                            if service {
                                Status::unavailable(error.reason.clone())
                            } else {
                                Status::failed_precondition(error.reason.clone())
                            },
                            error,
                            service,
                        )
                    }
                };
                if generation != self.reconnect_generation.current() {
                    return Err(status);
                }
                self.reconnect_generation.in_generation(generation, || {
                    if let Some(round) = lock_recover(&self.reconnect_generation.recovery).as_mut()
                    {
                        round.pending.take();
                        round.ended = !service_failure;
                    }
                });
                self.reconnect_active.store(false, Ordering::Release);
                if !service_failure {
                    self.finish_auth_acquisition(generation, &status, original)
                        .await;
                } else {
                    self.publish(failed_clean_snapshot(
                        &self.round_operation(generation),
                        original,
                    ));
                }
                return Err(status);
            }
        };
        let request = self
            .reconnect_generation
            .in_generation(generation, || {
                let mut record = lock_recover(&self.reconnect_generation.recovery);
                let round = record
                    .as_mut()
                    .filter(|round| !round.ended && round.generation == generation)?;
                let intent = reconnect_intent();
                round.continuation_queued = false;
                Some(ConnectRequest {
                    auth_binding: None,
                    intent: Some(intent),
                    secret_payload: Vec::new(),
                })
            })
            .flatten()
            .ok_or_else(|| Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"))?;
        let result = self
            .connect_in_generation_material(
                &request,
                Some(credentials),
                false,
                generation,
                false,
                Some(policy),
                Some(deadline),
                true,
            )
            .await;
        if let Err(status) = &result {
            if generation == self.reconnect_generation.current()
                && status.code() != Code::Unavailable
                && !status.message().starts_with(SERVICE_NOT_RUNNING_PREFIX)
            {
                self.reconnect_generation.in_generation(generation, || {
                    if let Some(round) = lock_recover(&self.reconnect_generation.recovery).as_mut()
                    {
                        round.ended = true;
                    }
                });
                self.reconnect_active.store(false, Ordering::Release);
                self.finish_auth_acquisition(generation, status, acquisition_status_error(status))
                    .await;
            }
        }
        result
    }

    async fn finish_auth_acquisition(
        &self,
        generation: u64,
        status: &Status,
        error: wire::VpnError,
    ) {
        let first_error = lock_recover(&self.reconnect_generation.recovery)
            .as_ref()
            .filter(|round| round.generation == generation)
            .and_then(|round| round.first_error.clone());
        let user_cancelled = error.code == wire::ErrorCode::CancelledBeforeStart as i32
            && error.reason == "atrust/auth_cancelled";
        if user_cancelled || matches!(status.code(), Code::DeadlineExceeded | Code::Cancelled) {
            // 尚未 Apply 的准备等待到 D 即结束。沿现有停止接缝报告其原截止，
            // 不再排队等待被占用的 session 许可，也不合成停止成功。
            let previous = self
                .events
                .current_snapshot()
                .and_then(|snapshot| snapshot.stop_completion);
            let mut completion = previous
                .filter(|done| done.stopped && !done.blocks_new_connect)
                .unwrap_or_else(|| session_orchestration::deadline("authentication_before_apply"));
            if !user_cancelled && first_error.is_some() && !completion.errors.contains(&error) {
                completion.errors.push(error.clone());
            }
            self.reconnect_generation.in_generation(generation, || {
                self.publish(session_orchestration::snapshot(
                    &self.round_operation(generation),
                    completion,
                    Some(if user_cancelled {
                        error
                    } else {
                        first_error.unwrap_or(error)
                    }),
                ));
            });
        } else {
            self.finish_reconnect_and_cleanup(generation, error.clone())
                .await;
            if let Some(first) = first_error
                && first != error
            {
                self.reconnect_generation.in_generation(generation, || {
                    if let Some(snapshot) = self.events.current_snapshot()
                        && let Some(completion) = snapshot.stop_completion
                    {
                        self.publish(session_orchestration::snapshot(
                            &snapshot.operation_id,
                            completion,
                            Some(first),
                        ));
                    }
                });
            }
        }
    }

    fn round_operation(&self, generation: u64) -> Vec<u8> {
        lock_recover(&self.reconnect_generation.recovery)
            .as_ref()
            .filter(|round| round.generation == generation)
            .map(|round| round.round_operation.clone())
            .unwrap_or_default()
    }

    pub(super) async fn continue_auth_recovery(&self) {
        let generation = self.reconnect_generation.current();
        let Some((execution, error, latest_error, already_acquired, trigger)) = self
            .reconnect_generation
            .in_generation(generation, || {
                let record = lock_recover(&self.reconnect_generation.recovery);
                let round = record.as_ref().filter(|round| {
                    round.generation == generation && round.continuation_queued && !round.ended
                })?;
                Some((
                    round.execution.clone(),
                    round.first_error.clone()?,
                    round.latest_error.clone(),
                    round.acquisition_started,
                    round.trigger,
                ))
            })
            .flatten()
        else {
            return;
        };
        // 原执行的真实停止在获取之前；不经过用户 Stop 的 retire/policy 清空。
        let completion = self.cleanup_auth_execution(generation).await;
        if generation != self.reconnect_generation.current() {
            return;
        }
        let mut completion = match completion {
            Ok(done) => done,
            Err(_) => session_orchestration::deadline("authentication_cleanup"),
        };
        if let Some(latest) = latest_error
            && latest != error
            && !completion.errors.contains(&latest)
        {
            completion.errors.push(latest);
        }
        let allowed = completion.stopped && !completion.blocks_new_connect;
        self.reconnect_generation.in_generation(generation, || {
            if !allowed || already_acquired {
                if let Some(round) = lock_recover(&self.reconnect_generation.recovery).as_mut() {
                    round.ended = true;
                }
                self.reconnect_active.store(false, Ordering::Release);
            }
            self.publish(session_orchestration::snapshot(
                &execution,
                completion,
                Some(error.clone()),
            ));
        });
        if !allowed || already_acquired {
            return;
        }
        let result = self.acquire_and_apply_recovery(generation).await;
        if let Err(status) = result {
            if generation == self.reconnect_generation.current()
                && !self.auth_round_ended(generation)
            {
                // 真实服务准备错误仍消耗原 EngineLost 预算；其余沿原终结语义。
                if trigger == ReconnectSignal::EngineLost {
                    let backoff = lock_recover(&self.reconnect_generation.recovery)
                        .as_ref()
                        .is_some_and(|round| round.policy.backoff);
                    let delay = if backoff {
                        reconnect_backoff_delay(
                            self.reconnect_backoff_base,
                            self.reconnect_backoff_cap,
                            self.reconnect_attempts.load(Ordering::Relaxed),
                        )
                    } else {
                        Duration::from_secs(2)
                    };
                    tokio::time::sleep(delay).await;
                    if generation == self.reconnect_generation.current() {
                        Box::pin(self.drive_reconnect_reentry(trigger)).await;
                    }
                } else {
                    self.finish_reconnect_and_cleanup(
                        generation,
                        reconnect_dispatch_vpn_error(&status),
                    )
                    .await;
                }
            }
        }
    }

    async fn cleanup_auth_execution(
        &self,
        generation: u64,
    ) -> Result<wire::StopCompletion, Status> {
        let duration = STOP_TOTAL_BUDGET;
        #[cfg(test)]
        let duration = self.auth_cleanup_duration.min(duration);
        let deadline = tokio::time::Instant::now() + duration;
        let slot = Arc::clone(&self.session);
        let admission = Arc::clone(&self.reconnect_generation);
        let expected_execution = lock_recover(&admission.recovery)
            .as_ref()
            .map(|round| round.execution.clone())
            .unwrap_or_default();
        let late_admission = Arc::clone(&admission);
        let events = Arc::clone(&self.events);
        let proxy = Arc::clone(&self.proxy_tun);
        let policy = Arc::clone(&self.reconnect_session_policy);
        let attempts = Arc::clone(&self.reconnect_attempts);
        let active = Arc::clone(&self.reconnect_active);
        let service_status = Arc::clone(&self.service_status_cache);
        let mode = *lock_recover(&self.engine_mode);
        let report_late = move |completion: wire::StopCompletion| {
            late_admission.in_generation(generation, || {
                let origin = lock_recover(&late_admission.recovery)
                    .as_ref()
                    .filter(|round| round.ended && round.execution == expected_execution)
                    .and_then(|round| round.first_error.clone());
                if let Some(origin) = origin {
                    let mut snapshot = session_orchestration::snapshot(
                        &expected_execution,
                        completion,
                        Some(origin),
                    );
                    snapshot.mode = mode.into();
                    snapshot.service_status = lock_recover(&service_status).clone();
                    events.publish(
                        RuntimeEventKind::Transition,
                        snapshot_with_reconnect(
                            snapshot_with_environment(snapshot, &proxy),
                            session_reconnect_status(&policy, &attempts, &active),
                        ),
                    );
                }
            });
        };
        // 原有 owned Stop RPC 模式：外层截止/取消不丢弃已发出的真实清理。
        let dispatch = tokio::spawn(async move {
            let mut session = tokio::time::timeout_at(deadline, slot.lock_owned())
                .await
                .map_err(|_| auth_deadline_status(AuthDeadlineError::Expired))?;
            if generation != admission.current() {
                return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"));
            }
            let Some(engine) = session.as_mut() else {
                return Ok(session_orchestration::unconfirmed());
            };
            let mut stream =
                match tokio::time::timeout_at(deadline, engine.attach_connect_status()).await {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(error)) => return Ok(session_orchestration::rpc_failure(&error)),
                    Err(_) => return Ok(session_orchestration::deadline("attach")),
                };
            if generation != admission.current() {
                return Err(Status::cancelled("DARWIN_CORE_CONNECTION_CANCELLED"));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(session_orchestration::deadline("before_dispatch"));
            }
            let intent = reconnect_stop_intent();
            let mut key = intent.lookup_key.expect("generated stop key");
            key.method = wire::OperationMethod::StopTunnel as i32;
            let stop_id = key.operation_id.clone();
            let reply = match engine
                .stop_tunnel(wire::StopTunnelRequest {
                    lookup_key: Some(key),
                    request_digest: intent.request_digest,
                })
                .await
            {
                Ok(reply) => reply,
                Err(error) => {
                    let completion = session_orchestration::rpc_failure(&error);
                    report_late(completion.clone());
                    return Ok(completion);
                }
            };
            if !matches!(
                reply.result,
                Some(wire::stop_tunnel_reply::Result::Pending(_))
            ) {
                let completion = session_orchestration::completion(&reply);
                report_late(completion.clone());
                return Ok(completion);
            }
            loop {
                match tokio::time::timeout_at(deadline, stream.message()).await {
                    Ok(Ok(Some(event))) if event.operation_id == stop_id => {
                        if let Some(completion) = event.stop_completion {
                            report_late(completion.clone());
                            return Ok(completion);
                        }
                    }
                    Ok(Ok(Some(_))) => {}
                    Ok(_) => return Ok(session_orchestration::unconfirmed()),
                    Err(_) => return Ok(session_orchestration::deadline("completion")),
                }
            }
        });
        match self
            .await_service_connection(
                generation,
                None,
                tokio::time::timeout_at(deadline, dispatch),
            )
            .await?
        {
            Ok(Ok(result)) => result,
            _ => Ok(session_orchestration::deadline("completion")),
        }
    }
}
