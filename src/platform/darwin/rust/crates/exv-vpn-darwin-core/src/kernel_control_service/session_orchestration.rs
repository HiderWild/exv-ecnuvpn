//! Core本地停止事实接纳。报告描述实际完成和剩余义务，不猜测资源已清空。
use exv_vpn_wire::generated as wire;

pub(super) fn completion(reply: &wire::StopTunnelReply) -> wire::StopCompletion {
    let mut completion = reply.stop_completion.clone().unwrap_or_else(|| {
        if matches!(
            reply.result,
            Some(wire::stop_tunnel_reply::Result::Stopped(_))
        ) {
            wire::StopCompletion {
                stopped: true,
                blocks_new_connect: false,
                errors: vec![],
            }
        } else {
            unconfirmed()
        }
    });
    if (!completion.stopped || completion.blocks_new_connect) && completion.errors.is_empty() {
        completion.errors = match &reply.result {
            Some(wire::stop_tunnel_reply::Result::Failed(failed)) if failed.error.is_some() => {
                vec![failed.error.clone().unwrap()]
            }
            _ => unconfirmed().errors,
        };
    }
    completion
}
pub(super) fn unconfirmed() -> wire::StopCompletion {
    wire::StopCompletion {
        stopped: false,
        blocks_new_connect: false,
        errors: vec![wire::VpnError {
            code: wire::ErrorCode::ObservationFailed as i32,
            stage: wire::ErrorStage::Teardown as i32,
            reason: "darwin/stop_unconfirmed".into(),
            retry: wire::RetryAdvice::UseNewOperation as i32,
            ..Default::default()
        }],
    }
}

pub(super) fn deadline(stage: &str) -> wire::StopCompletion {
    let mut completion = unconfirmed();
    completion.errors[0].code = wire::ErrorCode::DeadlineExceeded as i32;
    completion.errors[0].reason = format!("darwin/stop_deadline_exceeded/{stage}");
    completion
}

pub(super) fn rpc_failure(error: &crate::EngineLifecycleError) -> wire::StopCompletion {
    let mut completion = unconfirmed();
    let mut original = completion.errors[0].clone();
    original.reason = format!("darwin/stop_rpc: {error:?}");
    completion.errors.push(original);
    completion
}
pub(super) fn snapshot(
    id: &[u8],
    mut completion: wire::StopCompletion,
    origin: Option<wire::VpnError>,
) -> wire::RuntimeSnapshot {
    if let Some(origin) = origin
        && !completion.errors.contains(&origin)
    {
        completion.errors.insert(0, origin);
    }
    let error = completion.errors.first().cloned();
    let state = if completion.blocks_new_connect || !completion.stopped {
        wire::runtime_snapshot::State::FailedDirty(wire::FailedDirtyState {
            last_error: error.clone(),
            context: None,
            obligation: completion
                .blocks_new_connect
                .then(|| wire::RecoveryObligation {
                    retirement_operation_id: id.to_vec(),
                    blocking_error: error,
                    ..Default::default()
                }),
        })
    } else if let Some(error) = error {
        wire::runtime_snapshot::State::FailedClean(wire::FailedCleanState {
            last_error: Some(error),
            proof: None,
        })
    } else {
        wire::runtime_snapshot::State::Idle(wire::IdleState::default())
    };
    wire::RuntimeSnapshot {
        operation_id: id.to_vec(),
        stop_completion: Some(completion),
        state: Some(state),
        ..Default::default()
    }
}
pub(super) fn reply(mut completion: wire::StopCompletion) -> wire::OperationReply {
    if (!completion.stopped || completion.blocks_new_connect) && completion.errors.is_empty() {
        completion.errors = unconfirmed().errors;
    }
    let result = if let Some(error) = completion.errors.first() {
        wire::operation_terminal::Result::Failed(wire::OperationFailed {
            receipt: None,
            error: Some(error.clone()),
        })
    } else {
        wire::operation_terminal::Result::Succeeded(wire::MutationReceipt::default())
    };
    wire::OperationReply {
        terminal: Some(wire::OperationTerminal {
            stop_completion: Some(completion),
            result: Some(result),
        }),
    }
}

/// Core局部意图代次及当前派发来源；只在短同步临界区接纳事实，不持锁做RPC。
#[derive(Default)]
pub(super) struct SessionAdmission {
    pub(super) recovery: std::sync::Mutex<Option<super::auth_continuation::RecoveryRound>>,
    generation: std::sync::atomic::AtomicU64,
    preparing: std::sync::atomic::AtomicBool,
    source: std::sync::Mutex<Option<(u64, Vec<u8>, Option<wire::SessionIdentity>)>>,
}
impl SessionAdmission {
    pub(super) fn current(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }
    pub(super) fn advance(&self) -> u64 {
        let source = self.source.lock().expect("session source lock");
        self.recovery.lock().expect("recovery round lock").take();
        let next = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            .wrapping_add(1);
        self.preparing
            .store(true, std::sync::atomic::Ordering::Release);
        // 新请求可能真实被拒；已接纳执行的事实寿命保留到替代成功或明确Stop。
        drop(source);
        next
    }
    pub(super) fn clear_source(&self, generation: u64) {
        let mut source = self.source.lock().expect("session source lock");
        if self.current() == generation {
            *source = None;
        }
    }
    pub(super) fn retire(&self) -> u64 {
        let mut source = self.source.lock().expect("session source lock");
        self.recovery.lock().expect("recovery round lock").take();
        let next = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            .wrapping_add(1);
        *source = None;
        self.preparing
            .store(false, std::sync::atomic::Ordering::Release);
        next
    }
    pub(super) fn advance_if(&self, expected: u64) -> Option<u64> {
        let mut source = self.source.lock().expect("session source lock");
        if self.current() != expected {
            return None;
        }
        self.recovery.lock().expect("recovery round lock").take();
        let next = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            .wrapping_add(1);
        *source = None;
        self.preparing
            .store(false, std::sync::atomic::Ordering::Release);
        Some(next)
    }
    pub(super) fn finish_request(&self, generation: u64) {
        let _source = self.source.lock().expect("session source lock");
        if self.current() == generation {
            self.preparing
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }
    pub(super) fn source_generation(&self, id: &[u8]) -> Option<u64> {
        let source = self.source.lock().expect("session source lock");
        if self.preparing.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        source
            .as_ref()
            .filter(|(_, source_id, _)| source_id == id)
            .map(|_| self.current())
    }
    /// 仅在source接纳临界区调用；准备中的用户意图优先于自动恢复。
    pub(super) fn recovery_generation(&self) -> Option<u64> {
        (!self.preparing.load(std::sync::atomic::Ordering::Acquire)).then(|| self.current())
    }
    pub(super) fn install<R>(
        &self,
        generation: u64,
        id: &[u8],
        identity: Option<wire::SessionIdentity>,
        apply: impl FnOnce(Option<wire::SessionIdentity>) -> R,
    ) -> Option<R> {
        let mut source = self.source.lock().expect("session source lock");
        if self.current() != generation {
            return None;
        }
        *source = Some((generation, id.to_vec(), identity.clone()));
        self.preparing
            .store(false, std::sync::atomic::Ordering::Release);
        Some(apply(identity))
    }
    pub(super) fn accept<R>(
        &self,
        generation: u64,
        id: &[u8],
        apply: impl FnOnce(&mut Option<wire::SessionIdentity>) -> R,
    ) -> Option<R> {
        let mut source = self.source.lock().expect("session source lock");
        let (g, operation, identity) = source.as_mut()?;
        if *g != generation || operation != id {
            return None;
        }
        Some(apply(identity))
    }
    /// 仅用于锁外快照装饰；新请求准备中仍可读取已接纳来源，旧快照不得贴新身份。
    pub(super) fn identity_for(&self, id: &[u8]) -> Option<wire::SessionIdentity> {
        self.source
            .lock()
            .expect("session source lock")
            .as_ref()
            .filter(|(_, operation, _)| operation == id)
            .and_then(|(_, _, identity)| identity.clone())
    }
    pub(super) fn in_generation<R>(&self, generation: u64, apply: impl FnOnce() -> R) -> Option<R> {
        let _source = self.source.lock().expect("session source lock");
        if self.current() != generation {
            return None;
        }
        Some(apply())
    }

    /// 自动恢复已占本轮预算，旧执行的状态/EOF 不再接纳为新一轮触发。
    pub(super) fn begin_recovery<R>(
        &self,
        generation: u64,
        retire_source: bool,
        apply: impl FnOnce() -> R,
    ) -> Option<R> {
        let mut source = self.source.lock().expect("session source lock");
        if self.current() != generation {
            return None;
        }
        if retire_source {
            *source = None;
        }
        Some(apply())
    }
}
