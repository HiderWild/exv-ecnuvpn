//! Core 既有 composition 的会话写入口。调用者持同一短锁，网络 I/O 只返回输入事实。
//! 不建立 actor、全局状态机或持久化账本；Engine owner/R1 材料仍由 Engine 持有。
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ObservationBasis {
    phase: HostPhase,
    operation_id: Option<[u8; 16]>,
    pub(super) dispatch_id: Option<Uuid>,
    engine_generation: u64,
}

pub(super) enum SessionInput<'a> {
    Connect {
        operation_id: Option<[u8; 16]>,
        attempt_id: Uuid,
        cancel_sender: watch::Sender<bool>,
        expected_dispatch_id: Option<Uuid>,
        policy: Option<ReconnectSessionPolicy>,
        identity: Option<&'a SessionIdentity>,
    },
    Stop(&'a wire::StopIntent),
    EngineEvent(&'a EngineStatusEvent),
    Query {
        basis: ObservationBasis,
        snapshot: Option<RuntimeSnapshot>,
    },
    AdoptObservedSession(&'a RuntimeSnapshot),
    ConnectFailed(wire::VpnError),
    StopReported {
        attempt_id: Option<Uuid>,
        operation_id: Option<[u8; 16]>,
        summary: wire::StopCompletion,
    },
    StopLocal {
        attempt_id: Option<Uuid>,
    },
    ChannelFault {
        basis: ObservationBasis,
        subscribed: bool,
    },
    ChannelSnapshot {
        basis: ObservationBasis,
        snapshot: &'a RuntimeSnapshot,
    },
    Reconnect {
        trigger: ReconnectTrigger,
        expected_origin: Option<Uuid>,
    },
    ReconnectFinished {
        operation_id: Option<[u8; 16]>,
        origin_dispatch_id: Uuid,
        admitted_dispatch_id: Option<Uuid>,
        attempt: u32,
    },
    ReopenAdmission,
    ServiceTransition {
        basis: ObservationBasis,
        summary: Option<wire::StopCompletion>,
        completed: bool,
    },
}

pub(super) struct StopAdmission {
    pub record: AcceptedStop,
    pub replay: bool,
    pub target_generation: Option<u64>,
}

pub(super) enum SessionOutput {
    None,
    Reconnect((ReconnectSessionPolicy, Uuid, u32, u32)),
    Origin(Option<Uuid>),
    Recovery(Option<(R2Reason, Option<ReconnectTrigger>, ServiceMode)>),
    Stop(StopAdmission),
    Event(Option<(wire::RuntimeEventKind, RuntimeSnapshot)>),
    Snapshot(RuntimeSnapshot, Option<i64>),
}
impl SessionOutput {
    pub(super) fn event(self) -> Option<(wire::RuntimeEventKind, RuntimeSnapshot)> {
        match self {
            Self::Event(event) => event,
            _ => None,
        }
    }
}

impl KernelControlService {
    pub(super) fn observation_basis(&self, composition: &HostComposition) -> ObservationBasis {
        ObservationBasis {
            phase: composition.phase(),
            operation_id: composition.operation_id(),
            dispatch_id: self
                .connect_dispatch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map(|state| state.id),
            engine_generation: *self.engine.subscribe_swaps().borrow(),
        }
    }

    pub(super) fn accept_session_input(
        &self,
        composition: &mut HostComposition,
        input: SessionInput<'_>,
    ) -> Result<SessionOutput, Status> {
        match input {
            SessionInput::Connect {
                operation_id,
                attempt_id,
                cancel_sender,
                expected_dispatch_id,
                policy,
                identity,
            } => {
                {
                    let mut dispatch = self
                        .connect_dispatch
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // 自动重连退避期间可能已经受理了新的手动连接，或用户已经 Stop。
                    // 在既有 composition -> dispatch 线性化边界内核对原 attempt，避免旧
                    // timer 在检查后到状态机受理之间发生 TOCTOU 并覆盖新会话。
                    if let Some(expected_id) = expected_dispatch_id {
                        let still_current = dispatch.as_ref().is_some_and(|state| {
                            state.id == expected_id && !*state.cancelled.borrow()
                        });
                        if !still_current {
                            return Err(Status::cancelled(AUTO_RECONNECT_SUPERSEDED));
                        }
                    }
                    if composition
                        .stop_completion
                        .as_ref()
                        .is_some_and(|summary| summary.blocks_new_connect)
                    {
                        return Err(Status::failed_precondition(
                            "stop_cleanup_pending|请重新断开以清理剩余资源",
                        ));
                    }
                    let effect = composition.apply(HostEvent::Connect);
                    if let HostEffect::ConnectRefused(reason) = effect {
                        return Err(Status::failed_precondition(format!("connect: {reason}")));
                    }
                    if let Some(id) = operation_id {
                        composition.set_operation_id(id);
                    }
                    let stop_required = dispatch.as_ref().is_some_and(|state| state.stop_required);
                    *dispatch = Some(ConnectDispatch {
                        id: attempt_id,
                        cancelled: cancel_sender,
                        target_generation: None,
                        attempt_applied: false,
                        stop_required,
                    });
                }
                self.clear_auth_round();
                if let Some(policy) = policy {
                    if let Ok(mut current) = self.reconnect_session_policy.lock() {
                        *current = Some(policy);
                    }
                    self.reconnect_attempts.store(0, Ordering::Relaxed);
                    self.reconnect_active.store(false, Ordering::Release);
                    // R2（T6b）：新用户会话开启——旧恢复事件清空（reason lane 随会话重置）。
                    *self
                        .reconnect_reason
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    // bug3（EXV_UNFREEZE 2026-10-04）：同一受理点冻结会话身份投影；设置页保存
                    // 不经过这里，故不会改写活动会话的连接页显示。自动重连不携带 identity，
                    // 沿用 lane（同一会话身份）。
                    self.events
                        .refresh_session_identity(identity.map(SessionIdentity::to_wire));
                }
                self.events.refresh_self_heal(None);
                Ok(SessionOutput::None)
            }
            SessionInput::Stop(intent) => {
                let key = intent.lookup_key.as_ref().expect("validated key");
                let key_bytes = key.runtime_epoch.clone();
                let replay = {
                    let accepted = self
                        .accepted_stops
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match accepted.get(&key_bytes) {
                        Some(original) => {
                            if original.request_digest != intent.request_digest {
                                return Err(Status::failed_precondition("idempotency conflict"));
                            }
                            Some(original.clone())
                        }
                        None => None,
                    }
                };
                if let Some(record) = replay {
                    return Ok(SessionOutput::Stop(StopAdmission {
                        record,
                        replay: true,
                        target_generation: None,
                    }));
                }
                let prior_cleanup_pending =
                    composition.stop_completion.as_ref().is_some_and(|summary| {
                        !summary.errors.is_empty() || summary.blocks_new_connect || !summary.stopped
                    });
                let (attempt_id, stop_required, target_generation) = {
                    let mut dispatch = self
                        .connect_dispatch
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match dispatch.as_mut() {
                        Some(state) => {
                            state.cancelled.send_replace(true);
                            (
                                Some(state.id),
                                state.stop_required || prior_cleanup_pending,
                                state.target_generation,
                            )
                        }
                        None => (None, true, None),
                    }
                };
                self.accepted_stops
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        key_bytes.clone(),
                        AcceptedStop {
                            request_digest: intent.request_digest.clone(),
                            engine_required: stop_required,
                            operation_key: key.clone(),
                            attempt_id,
                        },
                    );

                self.clear_auth_round();
                // 用户显式停止即结束本连接生命周期；下一次连接必须重新读取已保存的重连设置。
                if let Ok(mut policy) = self.reconnect_session_policy.lock() {
                    *policy = None;
                }
                *self.reconnect_atrust_context.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                // bug3：会话身份与策略同生命周期——显式 Stop 清除投影，连接页回到读当前配置。
                self.events.refresh_session_identity(None);
                self.reconnect_active.store(false, Ordering::Release);

                // R1：stop 受理即驱动状态机（Connected/Connecting/Failed → Stopping）。
                // 状态机单一事实源：用户 Stop 是唯一业务取消，立即反映到 phase。控制面（host）
                // 侧即加入 teardown 屏障；engine 状态流先发 Idle（数据面侧）→ 双侧齐 → Stopped
                //（R1 收敛；R2 再补 core 合成 Idle 双保险）。
                let operation_id = intent
                    .lookup_key
                    .as_ref()
                    .and_then(|k| <[u8; 16]>::try_from(k.operation_id.as_slice()).ok());
                {
                    composition.stop_completion = None;
                    composition.apply(HostEvent::Disconnect);
                    composition.apply(HostEvent::TeardownSideJoined(TeardownSide::ProtocolControl));
                    if let Some(id) = operation_id {
                        composition.set_operation_id(id);
                    }
                }
                Ok(SessionOutput::Stop(StopAdmission {
                    record: self
                        .accepted_stops
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get(&key_bytes)
                        .expect("registered stop")
                        .clone(),
                    replay: false,
                    target_generation,
                }))
            }
            SessionInput::EngineEvent(event) => {
                if self.auth_event(event, composition) { return Ok(SessionOutput::Event(None)); }
                let event = normalize_unexpected_stop(event.clone(), composition, &self.logs);
                let result = runtime_event_from_status_locked(&event, composition, &self.logs);
                if result.is_some() {
                    if matches!(event, EngineStatusEvent::Connected { .. }) {
                        self.reconnect_attempts.store(0, Ordering::Relaxed);
                    }
                    if matches!(
                        event,
                        EngineStatusEvent::Connected { .. }
                            | EngineStatusEvent::Failed { .. }
                            | EngineStatusEvent::Stopped { .. }
                    ) {
                        self.reconnect_active.store(false, Ordering::Release);
                        *self
                            .reconnect_reason
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    }
                }
                Ok(SessionOutput::Event(result))
            }
            SessionInput::ConnectFailed(error) => {
                composition.set_last_wire_error(error.clone());
                composition.apply(HostEvent::ConnectFailed(wire_vpn_error_to_domain(&error)));
                Ok(SessionOutput::None)
            }
            SessionInput::StopReported {
                attempt_id,
                operation_id,
                summary,
            } => {
                if composition.operation_id() != operation_id || composition.is_exiting() {
                    return Ok(SessionOutput::None);
                }
                let mut dispatch = self
                    .connect_dispatch
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if dispatch.as_ref().map(|state| state.id) != attempt_id {
                    return Ok(SessionOutput::None);
                }
                if summary.stopped {
                    if let Some(state) = dispatch.as_mut() {
                        state.stop_required = summary.blocks_new_connect;
                    }
                    if composition.phase() == HostPhase::Failed {
                        // 同次已受理Stop晚到的真实完成补齐原屏障，不合成未确认的停止。
                        composition.apply(HostEvent::TeardownSideJoined(TeardownSide::PacketData));
                    } else {
                        composition.synthesize_data_plane_join();
                    }
                    if composition.phase() == HostPhase::Stopped {
                        composition.apply(HostEvent::ReopenAdmission);
                    }
                } else {
                    // 这里只接纳原操作的完成回执，Pending 不进入本输入。
                    let error = summary.errors.first().cloned().unwrap_or_else(|| {
                        stop_failure_completion("engine_reply", &Status::unknown("stop unconfirmed")).errors.remove(0)
                    });
                    composition.set_last_wire_error(error.clone());
                    composition.apply(HostEvent::ConnectionLost(wire_vpn_error_to_domain(&error)));
                }
                composition.stop_completion = Some(summary);
                self.events.publish(
                    wire::RuntimeEventKind::Transition,
                    snapshot_for_phase(composition.phase(), composition),
                );
                Ok(SessionOutput::None)
            }
            SessionInput::StopLocal { attempt_id } => {
                {
                    let mut dispatch = self
                        .connect_dispatch
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if dispatch.as_ref().map(|state| state.id) != attempt_id {
                        return Ok(SessionOutput::None);
                    }
                    if let Some(state) = dispatch.as_mut() {
                        state.stop_required = false;
                    }
                }
                composition.synthesize_data_plane_join();
                if composition.phase() == HostPhase::Stopped {
                    composition.apply(HostEvent::ReopenAdmission);
                }
                // 同一有序边界内发布已确认的 Idle。显式 Stop 已结束重连会话，故无重连伴生。
                let snapshot = snapshot_for_phase(composition.phase(), composition);
                self.events
                    .publish(wire::RuntimeEventKind::Transition, snapshot);
                Ok(SessionOutput::None)
            }
            SessionInput::AdoptObservedSession(snapshot) => {
                let Some(wire::runtime_snapshot::State::Connected(connected)) =
                    snapshot.state.as_ref()
                else {
                    return Ok(SessionOutput::Event(None));
                };
                // 迟到采纳不能先覆盖Stop关联，再依赖事件reducer拒绝相位。
                if matches!(
                    composition.phase(),
                    HostPhase::Stopping | HostPhase::Stopped
                ) || (composition.phase() == HostPhase::Idle && composition.stop_requests() > 0)
                    || composition
                        .stop_completion
                        .as_ref()
                        .is_some_and(|summary| summary.blocks_new_connect || !summary.stopped)
                {
                    return Ok(SessionOutput::Event(None));
                }
                if self
                    .connect_dispatch
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some()
                {
                    return Ok(SessionOutput::Event(None));
                }
                // ①对齐关联：被采纳会话的 operation_id 入 composition 登记册（16 字节才登记；
                // 空 id 也可投影，只是无法参与陈旧过滤）。set_operation_id 顺带清空上一会话
                // 残留（全新 core 本就为空）。
                if let Ok(operation_id) = <[u8; 16]>::try_from(snapshot.operation_id.as_slice()) {
                    if composition.operation_id() != Some(operation_id) {
                        composition.set_operation_id(operation_id);
                    }
                }
                // ②投影 Connected：与状态转发器同一条事件路径（含会话起点采纳语义——engine
                // 提供正值起点则原样采纳，缺失以 core 本地时钟首次观察时间补值）。
                let event = EngineStatusEvent::Connected {
                    reconnect_state: snapshot.engine_reconnect.clone(),
                    operation_id: snapshot.operation_id.clone(),
                    session_established_at_ms: (connected.session_established_at_ms > 0)
                        .then_some(connected.session_established_at_ms),
                };
                let Some((kind, adopted_snapshot)) =
                    runtime_event_from_status_locked(&event, composition, &self.logs)
                else {
                    return Ok(SessionOutput::Event(None));
                };
                // 保留真实观察投影中的 R1、无头事实和身份；不产生新的 Connect 派发。
                let adopted_snapshot = RuntimeSnapshot {
                    headless: snapshot.headless.clone(),
                    engine_reconnect: snapshot.engine_reconnect.clone(),
                    relaunched_by_engine: snapshot.relaunched_by_engine,
                    session_identity: snapshot.session_identity.clone(),
                    ..adopted_snapshot
                };
                if snapshot.session_identity.is_some() {
                    self.events
                        .refresh_session_identity(snapshot.session_identity.clone());
                }
                if let Some(stats) = snapshot.stats {
                    self.events.publish_stats(runtime_stats_from_wire(stats));
                }
                self.events.publish(
                    kind,
                    snapshot_with_reconnect(
                        adopted_snapshot.clone(),
                        self.session_reconnect_status(),
                    ),
                );
                Ok(SessionOutput::Event(Some((kind, adopted_snapshot))))
            }
            SessionInput::Query {
                basis,
                snapshot: mut engine_snapshot,
            } => {
                if self.observation_basis(composition) != basis {
                    return Ok(SessionOutput::Snapshot(
                        snapshot_for_phase(composition.phase(), composition),
                        composition.session_established_at_ms(),
                    ));
                }
                // 同名服务管道可重拨新进程而不换 EngineSlot；新引擎的空会话不能复活旧连接。
                // 仅裁决查询前后仍属同一已连接操作的事实，避免查询途中建立新会话的竞态。
                if basis.phase == HostPhase::Connected
                    && composition.phase() == HostPhase::Connected
                    && basis.operation_id == composition.operation_id()
                    && engine_snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.operation_id.is_empty()
                            && matches!(
                                snapshot.state,
                                Some(wire::runtime_snapshot::State::Idle(_))
                            )
                    })
                {
                    let operation_id = composition
                        .operation_id()
                        .map(|id| id.to_vec())
                        .unwrap_or_default();
                    let _ = self.logs.append_core(
                        "warn",
                        "connection",
                        "kernel.connection.engine_session_missing",
                        "引擎报告没有活动会话，旧连接已经失效",
                        &BTreeMap::new(),
                    );
                    let event = EngineStatusEvent::Failed {
                        operation_id,
                        reconnect_state: None,
                        error: wire::VpnError {
                            code: wire::ErrorCode::EffectUnknown as i32,
                            stage: wire::ErrorStage::DataPlane as i32,
                            certainty: wire::EffectCertainty::NoEffect as i32,
                            retry: wire::RetryAdvice::RetrySameOperation as i32,
                            ..Default::default()
                        },
                    };
                    if let Some((kind, snapshot)) =
                        runtime_event_from_status_locked(&event, composition, &self.logs)
                    {
                        self.events.publish(
                            kind,
                            snapshot_with_reconnect(snapshot, self.session_reconnect_status()),
                        );
                        request_snapshot(
                            Arc::clone(&self.logs),
                            SnapshotContext::new(
                                "engine_session_missing",
                                composition.operation_id().map(Uuid::from_bytes),
                                None,
                                self.reconnect_attempts.load(Ordering::Relaxed),
                                *self.engine.subscribe_swaps().borrow(),
                            ),
                            self.network_probe,
                        );
                        // 明确空会话已投影终态；不启动第二套数据面预算。
                    }
                }
                if self.observation_basis(composition) == basis
                    && let Some(snapshot) = engine_snapshot.as_ref()
                    && (snapshot.engine_reconnect.is_some()
                        || (composition
                            .operation_id()
                            .is_some_and(|id| id.as_slice() == snapshot.operation_id)
                            && matches!(
                                snapshot.state,
                                Some(wire::runtime_snapshot::State::Idle(_))
                            )))
                    && let Some(event) = status_from_observed_snapshot(snapshot)
                {
                    let event = normalize_unexpected_stop(event, composition, &self.logs);
                    let current_idle =
                        matches!(snapshot.state, Some(wire::runtime_snapshot::State::Idle(_)))
                            && composition
                                .operation_id()
                                .is_some_and(|id| id.as_slice() == snapshot.operation_id);
                    if let Some((kind, snapshot)) =
                        runtime_event_from_status_locked(&event, composition, &self.logs)
                    {
                        self.events.publish(
                            kind,
                            snapshot_with_reconnect(
                                snapshot.clone(),
                                self.session_reconnect_status(),
                            ),
                        );
                        // 同会话 Idle 的首次及重复查询都保留既有失败，不选回原始 Idle。
                        if current_idle
                            && matches!(
                                snapshot.state,
                                Some(wire::runtime_snapshot::State::FailedDirty(_))
                            )
                        {
                            engine_snapshot = Some(snapshot);
                        }
                    }
                }
                let composition_snapshot = if engine_snapshot.is_none() {
                    disconnected_snapshot(composition)
                } else {
                    snapshot_for_phase(composition.phase(), composition)
                };
                let confirmed_session_start = composition.session_established_at_ms();
                let selected_snapshot = prefer_snapshot(engine_snapshot, composition_snapshot);
                let cached = self.events.current_snapshot();
                let uncertain = matches!(
                    selected_snapshot.state,
                    Some(wire::runtime_snapshot::State::Reconciling(_))
                );
                let recovered = matches!(
                    selected_snapshot.state,
                    Some(wire::runtime_snapshot::State::Connected(_))
                );
                if (uncertain
                    && cached.as_ref().is_some_and(|snapshot| {
                        matches!(
                            snapshot.state,
                            Some(wire::runtime_snapshot::State::Connected(_))
                        )
                    }))
                    || (recovered
                        && cached.as_ref().is_some_and(|snapshot| {
                            matches!(
                                snapshot.state,
                                Some(wire::runtime_snapshot::State::Reconciling(_))
                            )
                        }))
                {
                    // 统计事件复用 EventBus 的状态，查询与推送必须一起撤下或恢复 Connected。
                    self.events.publish(
                        wire::RuntimeEventKind::Transition,
                        snapshot_with_reconnect(
                            selected_snapshot.clone(),
                            self.session_reconnect_status(),
                        ),
                    );
                }
                Ok(SessionOutput::Snapshot(
                    selected_snapshot,
                    confirmed_session_start,
                ))
            }
            // 控制通道不可达只影响确认能力；没有 stopped 回执就没有 PacketData join。
            SessionInput::Reconnect {
                trigger,
                expected_origin,
            } => {
                if expected_origin.is_some()
                    && self.current_reconnect_dispatch_id() != expected_origin
                {
                    return Ok(SessionOutput::None);
                }
                if self.auth_round_holds_channel() { return Ok(SessionOutput::None); }
                // 防重入：上一次重连尝试仍在途（已派发、终态未到）→ 不重复触发。
                if self.reconnect_active.load(Ordering::Acquire) {
                    self.logs
                        .append_core(
                            "info",
                            "kernel",
                            "kernel.reconnect.skip_in_flight",
                            "auto reconnect skipped: a reconnect attempt is still in flight",
                            &BTreeMap::new(),
                        )
                        .ok();
                    return Ok(SessionOutput::None);
                }
                // 重连判定只读本会话冻结策略：设置保存只为下一次用户连接准备，绝不在
                // 当前连接掉线后改变重连预算、退避或 UI 显示。
                let Some(policy) = self
                    .reconnect_session_policy
                    .lock()
                    .ok()
                    .and_then(|policy| *policy)
                else {
                    return Ok(SessionOutput::None);
                };
                let ReconnectSessionPolicy {
                    auto_reconnect,
                    max_attempts: max,
                    on_oneshot_engine_crash,
                    ..
                } = policy;
                if !auto_reconnect {
                    self.logs
                        .append_core(
                            "info",
                            "kernel",
                            "kernel.reconnect.disabled",
                            "retryable data-plane drop observed but auto_reconnect is disabled",
                            &BTreeMap::new(),
                        )
                        .ok();
                    return Ok(SessionOutput::None);
                }
                // R2 oneshot 子门（§2.8，默认关）：重提权必然弹 UAC——仅当会话冻结的开关
                // 开启时才允许该形态的自动重连。
                if trigger == ReconnectTrigger::EngineLostOneshot && !on_oneshot_engine_crash {
                    self.logs
                .append_core(
                    "info",
                    "kernel",
                    "kernel.r2.oneshot_gated_off",
                    "engine-lost reconnect skipped: reconnect_on_oneshot_engine_crash is disabled for this session",
                    &BTreeMap::new(),
                )
                .ok();
                    *self
                        .reconnect_reason
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(R2Reason::OneshotGatedOff);
                    return Ok(SessionOutput::None);
                }
                // R2 上限回退：会话配置 0（无限）时以 T6a 邻接裁量值为上限（R1 语义不变）。
                let effective_max = if max == 0 {
                    R2_MAX_ATTEMPTS_FALLBACK
                } else {
                    max
                };
                let attempts = self.reconnect_attempts.load(Ordering::Relaxed);
                if effective_max != 0 && attempts >= effective_max {
                    self.logs
                .append_core(
                    "warn",
                    "kernel",
                    "kernel.reconnect.exhausted",
                    &format!("auto reconnect stopped: attempts={attempts} exhausted (max={effective_max})"),
                    &BTreeMap::new(),
                )
                .ok();
                    return Ok(SessionOutput::None);
                }

                let Some(origin_dispatch_id) = self.current_reconnect_dispatch_id() else {
                    self.logs
                        .append_core(
                            "info",
                            "kernel",
                            "kernel.reconnect.skip_superseded",
                            "auto reconnect skipped: no current originating session",
                            &BTreeMap::new(),
                        )
                        .ok();
                    return Ok(SessionOutput::None);
                };

                // 重连执行：占用在途标记 + 计数 + 复用 run_connect（凭据从磁盘重新组装）。
                self.reconnect_active.store(true, Ordering::Release);
                self.reconnect_attempts.fetch_add(1, Ordering::Relaxed);
                let attempt = attempts + 1;
                Ok(SessionOutput::Reconnect((
                    policy,
                    origin_dispatch_id,
                    attempt,
                    effective_max,
                )))
            }
            SessionInput::ReconnectFinished {
                operation_id,
                origin_dispatch_id,
                admitted_dispatch_id,
                attempt,
            } => {
                let current = self.current_reconnect_dispatch_id();
                // 受理前失败仍属于原dispatch；受理后必须匹配同锁点产出的真实回执及operation。
                // 既有尝试计数仅排除同origin后续轮次，不能替代dispatch/operation来源过滤。
                let owns_current = match admitted_dispatch_id {
                    Some(admitted) => {
                        current == Some(admitted) && composition.operation_id() == operation_id
                    }
                    None => current == Some(origin_dispatch_id),
                };
                if !owns_current || self.reconnect_attempts.load(Ordering::Relaxed) != attempt {
                    return Ok(SessionOutput::Origin(None));
                }
                self.reconnect_active.store(false, Ordering::Release);
                Ok(SessionOutput::Origin(current))
            }
            SessionInput::ChannelSnapshot { basis, snapshot } => {
                if self.auth_round_holds_channel() { return Ok(SessionOutput::None); }
                if self.observation_basis(composition) != basis {
                    return Ok(SessionOutput::None);
                }
                self.engine_lost.store(false, Ordering::Release);
                if let Some(event) = status_from_observed_snapshot(snapshot) {
                    let event = normalize_unexpected_stop(event, composition, &self.logs);
                    let _ = runtime_event_from_status_locked(&event, composition, &self.logs);
                }
                self.events.publish(
                    wire::RuntimeEventKind::Transition,
                    snapshot_with_reconnect(
                        snapshot_for_phase(composition.phase(), composition),
                        self.session_reconnect_status(),
                    ),
                );
                Ok(SessionOutput::None)
            }
            SessionInput::ChannelFault { basis, subscribed } => {
                if self.auth_round_holds_channel() { return Ok(SessionOutput::Recovery(None)); }
                if self.observation_basis(composition) != basis {
                    return Ok(SessionOutput::Recovery(None));
                }
                let mut r2_classified = None;
                // 新订阅周期的EOF可重新分类，但挂接本身不等待composition锁。
                let was_lost = self.engine_lost.swap(true, Ordering::AcqRel);
                let first_loss = subscribed || !was_lost;
                let phase = composition.phase();

                if first_loss
                    && matches!(
                        phase,
                        HostPhase::Connecting | HostPhase::Connected | HostPhase::Reconciling
                    )
                {
                    // engine 已失联：在途重连尝试的终态事件永远不会再到达
                    //（engine 重启即失忆，edge 19）——由 R2 接管恢复，先行释放
                    // 在途标记，防止后续 R2 信号被 skip_in_flight 永久卡死。
                    self.reconnect_active.store(false, Ordering::Release);
                    let form = ServiceMode::from_u8(self.selected_mode.load(Ordering::Relaxed));
                    let oneshot_gated_on = self
                        .reconnect_session_policy
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_some_and(|policy| policy.on_oneshot_engine_crash);
                    let (reason, mut signal) = match form {
                        // 服务形态（auto 无维护对象，与 service 同走被动等待——
                        // 绝不提权）：等 SCM 拉回 + run_connect 路由重拨；随主
                        // 开关（D11——drive_reconnect 的 auto_reconnect 判定）。
                        ServiceMode::Service | ServiceMode::Auto => {
                            (R2Reason::Service, Some(ReconnectTrigger::EngineLostService))
                        }
                        ServiceMode::Oneshot if oneshot_gated_on => {
                            (R2Reason::Oneshot, Some(ReconnectTrigger::EngineLostOneshot))
                        }
                        // oneshot 且设置项关（默认）：不自动重连——断开过渡由下方
                        // 快照发布呈现；通知出口归 T12。
                        ServiceMode::Oneshot => (R2Reason::OneshotGatedOff, None),
                    };
                    let recovery_enabled = self.reconnect_session_policy.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_some_and(|policy| policy.auto_reconnect);
                    if !recovery_enabled || signal.is_none() {
                        signal = None;
                        let mut summary = stop_failure_completion("engine_lost", &Status::unavailable("engine status stream ended"));
                        summary.errors[0].stage = wire::ErrorStage::ProtocolSession as i32;
                        if let Some(previous) = composition.stop_completion.as_ref() {
                            summary.blocks_new_connect = previous.blocks_new_connect;
                        }
                        let error = summary.errors[0].clone();
                        composition.set_last_wire_error(error.clone());
                        composition.apply(HostEvent::ConnectionLost(wire_vpn_error_to_domain(&error)));
                        composition.stop_completion = Some(summary);
                    }
                    // 先置 reason lane 再发布断线过渡——EOF 快照本身携带 reason id。
                    *self
                        .reconnect_reason
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reason);
                    r2_classified = Some((reason, signal, form));
                }
                let snapshot = snapshot_with_reconnect(
                    disconnected_snapshot(composition),
                    reconnect_status_from_session(
                        &self.reconnect_attempts,
                        &self.reconnect_active,
                        &self.reconnect_session_policy,
                        &self.reconnect_reason,
                    ),
                );
                self.events
                    .publish(wire::RuntimeEventKind::Transition, snapshot);
                Ok(SessionOutput::Recovery(r2_classified))
            }
            SessionInput::ReopenAdmission => {
                if composition.phase() == HostPhase::Stopped {
                    composition.apply(HostEvent::ReopenAdmission);
                }
                Ok(SessionOutput::None)
            }
            SessionInput::ServiceTransition {
                basis,
                summary,
                completed,
            } => {
                if self.observation_basis(composition) != basis {
                    return Err(Status::cancelled(
                        "service transition superseded by a newer session",
                    ));
                }
                composition.stop_completion = summary;
                if completed {
                    composition.apply(HostEvent::Disconnect);
                    composition.apply(HostEvent::TeardownSideJoined(TeardownSide::ProtocolControl));
                    composition.apply(HostEvent::TeardownSideJoined(TeardownSide::PacketData));
                    composition.apply(HostEvent::ReopenAdmission);
                }
                Ok(SessionOutput::None)
            }
        }
    }
}

pub(super) fn runtime_event_from_status_locked(
    ev: &EngineStatusEvent,
    guard: &mut HostComposition,
    logs: &LogAggregator,
) -> Option<(wire::RuntimeEventKind, RuntimeSnapshot)> {
    if guard.is_exiting() { return None; }
    if stale_terminal_operation(guard, ev) {
        return None;
    }
    // 已结束/正在停止的已登记会话，不接受旧连接进展或成功复活。
    if guard.operation_id().is_some()
        && (matches!(guard.phase(), HostPhase::Stopped | HostPhase::Stopping)
            || (guard.phase() == HostPhase::Idle && guard.stop_requests() > 0))
        && matches!(
            ev,
            EngineStatusEvent::Progress { .. } | EngineStatusEvent::Connected { .. }
        )
    {
        return None;
    }
    let telemetry = match ev {
        EngineStatusEvent::Progress {
            reconnect_state, ..
        }
        | EngineStatusEvent::Connected {
            reconnect_state, ..
        }
        | EngineStatusEvent::Failed {
            reconnect_state, ..
        }
        | EngineStatusEvent::Stopped {
            reconnect_state, ..
        } => reconnect_state,
    };
    if let EngineStatusEvent::Progress { operation_id, .. } = ev {
        let explicit_recovery = telemetry.as_ref().is_some_and(|state| state.attempt > 0)
            && guard
                .operation_id()
                .is_some_and(|id| id.as_slice() == operation_id);
        if explicit_recovery && matches!(guard.phase(), HostPhase::Failed | HostPhase::Connected) {
            // 只驱动既有 reducer，不经过 run_connect / ApplyTunnel。
            guard.apply(HostEvent::Connect);
        } else if guard.phase() == HostPhase::Failed {
            return None;
        }
    }
    if let EngineStatusEvent::Connected { operation_id, .. } = ev
        && guard.phase() == HostPhase::Failed
    {
        let healed = telemetry.as_ref().is_some_and(|state| {
            state.attempt > 0 && state.reason == "healed" && state.next_retry_at_ms == 0
        }) && guard
            .operation_id()
            .is_some_and(|id| id.as_slice() == operation_id);
        if !healed {
            return None;
        }
        // 断流可能丢失 Progress；明确成功仍经既有 reducer 恢复，无连接 RPC。
        guard.apply(HostEvent::Connect);
    }
    guard.engine_reconnect = telemetry.clone();
    match ev {
        EngineStatusEvent::Progress { connect_phase, .. } => {
            guard.apply(HostEvent::ConnectPhaseProgress(domain_connect_phase(
                *connect_phase,
            )));
            Some((
                wire::RuntimeEventKind::Transition,
                snapshot_for_phase(guard.phase(), guard),
            ))
        }
        EngineStatusEvent::Connected {
            session_established_at_ms,
            ..
        } => {
            if guard.phase() == HostPhase::Failed {
                let _ = logs.append_core(
                    "debug",
                    "connection",
                    "kernel.connection.late_connected_ignored",
                    "当前操作已经失败，忽略迟到的连接成功事件",
                    &BTreeMap::new(),
                );
                return None;
            }
            guard.apply(HostEvent::ProtocolEstablished);
            // RT-SESSION-04：engine 提供正值起点 → 原样采纳（无码）；缺正值
            //（`None` 或 `Some(<=0)`，`connect_status_from_wire` 已把非正归一为
            // `None`）→ core 以本地时钟首次可靠观察时间为 adopted 起点 + 码 11
            //（info，fields source="core_clock"）。同一会话已有确认事实时不重复
            // 采纳（补值只增不减，与 `set_session_established_at_ms` 语义一致；
            // `snapshot_with_confirmed_session_start` 行为不变）。
            if session_established_at_ms
                .as_ref()
                .is_some_and(|value| *value > 0)
            {
                guard.set_session_established_at_ms(*session_established_at_ms);
            } else if guard
                .session_established_at_ms()
                .as_ref()
                .is_none_or(|value| *value <= 0)
            {
                adopt_local_session_start(guard, logs, unix_now_ms());
            }
            let snapshot = snapshot_for_phase(guard.phase(), guard);
            Some((wire::RuntimeEventKind::Transition, snapshot))
        }
        EngineStatusEvent::Failed { error, .. } => {
            let previous_phase = guard.phase();
            guard.set_last_wire_error(error.clone());
            let domain_error = wire_vpn_error_to_domain(error);
            if previous_phase == HostPhase::Connected {
                guard.apply(HostEvent::ConnectionLost(domain_error));
            } else {
                guard.apply(HostEvent::ConnectFailed(domain_error));
            }
            let _ = logs.append_core(
                "warn",
                "connection",
                "kernel.connection.failed",
                "当前连接失败，已更新连接状态；是否重连由会话设置决定",
                &BTreeMap::from([
                    (
                        "operation_id".into(),
                        guard
                            .operation_id()
                            .map(|id| Uuid::from_bytes(id).to_string())
                            .unwrap_or_default(),
                    ),
                    ("previous_phase".into(), format!("{previous_phase:?}")),
                    ("next_phase".into(), format!("{:?}", guard.phase())),
                    ("error_stage".into(), error.stage.to_string()),
                    ("error_code".into(), error.code.to_string()),
                    ("retry_advice".into(), error.retry.to_string()),
                ]),
            );
            Some((
                wire::RuntimeEventKind::Transition,
                snapshot_for_phase(guard.phase(), guard),
            ))
        }
        EngineStatusEvent::Stopped {
            stop_completion, ..
        } => {
            guard.stop_completion = stop_completion.clone();
            if stop_completion
                .as_ref()
                .is_some_and(|summary| !summary.stopped)
            {
                let error = stop_completion.as_ref().and_then(|summary| summary.errors.first()).cloned()
                    .unwrap_or_else(|| stop_failure_completion("engine_event", &Status::unknown("stop unconfirmed")).errors.remove(0));
                if let Some(summary) = guard.stop_completion.as_mut() && summary.errors.is_empty() {
                    summary.errors.push(error.clone());
                }
                guard.set_last_wire_error(error.clone());
                guard.apply(HostEvent::ConnectionLost(wire_vpn_error_to_domain(&error)));
                return Some((
                    wire::RuntimeEventKind::Transition,
                    snapshot_for_phase(guard.phase(), guard),
                ));
            }
            // engine 确认 teardown 完成：数据面（engine）侧加入屏障 → 与 host 侧
            // 齐全后 Stopped（R1 收敛；R2 再补 core 合成 Idle 双保险）。
            //
            // S1.5 (D13) Paused 映射：减负断开后 engine 运行时进入 **Paused**（session
            // 结束 + 路由清 + NIC 保留 adapter），host 侧**复用既有 Stopped 映射**——
            // `connect_status_from_wire` 把 engine Idle 终态归一化为
            // `EngineStatusEvent::Stopped`，此处数据面侧加入屏障 → Stopped → 显式
            // ReopenAdmission → Idle。冻结 portable `HostComposition` 不动：Paused 是
            // engine 运行时状态，host 只以 Stopped + 既有 fine-phase（`connect_phase`
            // 寄存器）映射；网卡保留由退出清理阶段（engine teardown）收敛到 0 残留。
            guard.apply(HostEvent::TeardownSideJoined(TeardownSide::PacketData));
            let snapshot = snapshot_for_phase(guard.phase(), guard);
            // S1（D14）：Stopped 快照先可观测（快照已组装，Stopped → wire Idle），
            // 随后 controller 显式派发 ReopenAdmission——admission 重开 + phase 回落
            // Idle，同进程二次 Connect 可受理（问题 A 闩锁修复）。**不**在
            // TeardownSideJoined Released 臂内同步回落（Stopped pin 保持）；仅当
            // 屏障确实释放（phase == Stopped）才重开——Stopping/Failed 等其余相
            // 位 no-op，teardown 拒绝语义保留。
            if guard.phase() == HostPhase::Stopped {
                guard.apply(HostEvent::ReopenAdmission);
            }
            Some((wire::RuntimeEventKind::Transition, snapshot))
        }
    }
}

pub(super) fn adopt_local_session_start(
    guard: &mut HostComposition,
    logs: &LogAggregator,
    now_ms: i64,
) {
    emit_stats_diag(
        logs,
        StatsDiagCode::SessionAdopted,
        [("source", "core_clock".to_string())],
    );
    guard.set_session_established_at_ms(Some(now_ms));
}

/// 进程owner只投递已知的本地生命周期事实；spawn、退出等待和peer构造仍由原owner执行。
pub(crate) enum ProcessFact {
    ControlChannelLost,
    ExplicitQuit,
    Rebound(HostComposition),
}

/// 与服务入口共用原composition的纯适配；不持资源句柄、不进行I/O、不新建会话owner。
pub(crate) fn accept_process_fact(
    composition: &mut HostComposition,
    fact: ProcessFact,
) -> Result<(), String> {
    match fact {
        ProcessFact::ControlChannelLost => composition.on_helper_link_terminal(),
        ProcessFact::ExplicitQuit => composition.exit(),
        ProcessFact::Rebound(mut fresh) => {
            // 同一临界区读取实际当前意图；不能把await前捕获的Connect恢复到新Stop之后。
            if composition.phase() == HostPhase::Stopping
                || composition
                    .stop_completion
                    .as_ref()
                    .is_some_and(|summary| summary.blocks_new_connect || !summary.stopped)
            {
                return Err("engine rebind: stop is still unconfirmed".into());
            }
            if composition.phase() == HostPhase::Connecting {
                fresh.apply(HostEvent::Connect);
                if let Some(operation_id) = composition.operation_id() {
                    fresh.set_operation_id(operation_id);
                }
            }
            *composition = fresh;
        }
    }
    Ok(())
}
