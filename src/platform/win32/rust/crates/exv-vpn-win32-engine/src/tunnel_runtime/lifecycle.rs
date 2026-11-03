//! Engine 局部生命周期入口：受理许可与原装配提交锁分别保护控制请求和真实资源。
//! 许可只占用访问资格；等待完成事件、工作线程 join 期间不持资源槽或提交互斥锁。
use super::*;
use std::sync::atomic::Ordering;

/// 原提交临界区的短期访问资格，不保存资源、操作回执或第二份连接状态。
#[derive(Default)]
pub(super) struct CommitState {
    pub(super) stopping: usize,
    cleaning: bool,
}

/// 控制入口共用的异步许可；被已受理 Stop 的独立任务持有至必要收尾和发布结束。
#[derive(Default)]
pub(crate) struct Admission {
    /// true 仅由 Engine 退出置位；普通 Stop 释放许可后仍可重新连接。
    pub(crate) access: Arc<tokio::sync::Mutex<bool>>,
}

pub(super) struct CleanupPermit<'a>(&'a RealTunnelRuntime);
impl Drop for CleanupPermit<'_> {
    fn drop(&mut self) {
        let mut commit = self
            .0
            .lifecycle_commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        commit.cleaning = false;
        commit.stopping -= 1;
        self.0.assembly_done.notify_all();
    }
}

/// 释放原 assembling 的唯一收口；包括装配及其错误发布再次 panic 的出口。
pub(super) struct AssemblyCompletion<'a>(pub(super) &'a RealTunnelRuntime);
impl Drop for AssemblyCompletion<'_> {
    fn drop(&mut self) {
        let _commit = self
            .0
            .lifecycle_commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.0.assembling.store(false, Ordering::SeqCst);
        self.0.assembly_done.notify_all();
    }
}

impl RealTunnelRuntime {
    /// 同一提交锁内登记停止占用再检查装配；Condvar 原子释放锁并订阅，完成方
    /// 在此锁内复位原 assembling 后通知，不存在先检查后订阅的丢失唤醒窗口。
    pub(super) fn begin_cleanup(
        &self,
        cancel: bool,
        timeout: Duration,
    ) -> Option<CleanupPermit<'_>> {
        self.begin_cleanup_until(cancel, Some(Instant::now() + timeout))
    }

    pub(super) fn begin_cleanup_until(
        &self,
        cancel: bool,
        deadline: Option<Instant>,
    ) -> Option<CleanupPermit<'_>> {
        let mut commit = self
            .lifecycle_commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        commit.stopping += 1;
        if cancel {
            self.cancel_flag.store(true, Ordering::SeqCst);
        }
        while self.assembling.load(Ordering::SeqCst) || commit.cleaning {
            let Some(deadline) = deadline else {
                commit = self
                    .assembly_done
                    .wait(commit)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                commit.stopping -= 1;
                self.assembly_done.notify_all();
                return None;
            }
            (commit, _) = self
                .assembly_done
                .wait_timeout(commit, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        commit.cleaning = true;
        Some(CleanupPermit(self))
    }

    fn assembly_pending_report() -> StopReport {
        StopReport {
            stopped: false,
            remaining: vec![StopObligation {
                domain: LedgerDomain::SessionLease,
                owner: "runtime_assembly_or_cleanup".into(),
                step: "assembly_finish",
                blocks_new_connect: true,
            }],
            ..Default::default()
        }
    }

    pub(super) fn disconnect_lifecycle(&self) -> StopReport {
        self.disconnect_lifecycle_with_timeout(crate::heartbeat::ASSEMBLY_CANCEL_WAIT)
    }

    pub(super) fn disconnect_lifecycle_with_timeout(&self, timeout: Duration) -> StopReport {
        let Some(_cleanup) = self.begin_cleanup(true, timeout) else {
            return Self::assembly_pending_report();
        };
        self.disconnect_owned()
    }

    fn disconnect_owned(&self) -> StopReport {
        let diagnostics = self.diagnostic_context();
        let started = Instant::now();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.disconnect",
                "caller_disconnect_request",
                "started",
                [],
            );
        }
        let mut report = StopReport::default();
        let stopped = (|| -> Result<(), String> {
            let _resources = self
                .resources
                .lock()
                .map_err(|_| "runtime resources lock".to_string())?;
            let mut live = {
                let _commit = self
                    .lifecycle_commit
                    .lock()
                    .map_err(|_| "runtime commit lock".to_string())?;
                self.cancel_flag
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                self.live
                    .lock()
                    .map_err(|_| "live lock".to_string())?
                    .take()
            };
            drop(_resources);
            if let Some(tunnel) = live.as_mut() {
                let (live_report, _) = tunnel.disconnect_for_report("runtime_disconnect");
                let retained = !live_report.remaining.is_empty();
                report.merge(live_report);
                if retained {
                    *self.live.lock().map_err(|_| "live lock".to_string())? = live;
                }
            }
            let mut observed = PreviousCleanupObservation::default();
            let _ = self.retry_pending_routes_observed(&mut observed);
            report.merge(StopReport::from_rollback(&observed.outcome));
            let pending = self
                .pending_routes
                .lock()
                .map_err(|_| "pending routes lock".to_string())?;
            for routes in pending.iter() {
                for owner in routes.owner_identities() {
                    report.remaining.push(StopObligation {
                        domain: LedgerDomain::RouteFamily,
                        owner,
                        step: "pending_routes",
                        blocks_new_connect: true,
                    });
                }
            }
            report.stopped = !self.assembling.load(std::sync::atomic::Ordering::SeqCst);
            if !report.stopped {
                report.remaining.push(StopObligation {
                    domain: LedgerDomain::SessionLease,
                    owner: "runtime_assembly".into(),
                    step: "assembly_finish",
                    blocks_new_connect: true,
                });
            } else {
                self.paused.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        })();
        if let Err(error) = stopped {
            report.action(LedgerDomain::SessionLease, "stop_access", &Err(error));
            report.stopped = false;
        }
        if self.nic.lock().map(|nic| nic.is_some()).unwrap_or(false) {
            report.actions.push(StopAction {
                domain: LedgerDomain::Nic,
                step: "nic_retained",
                state: StopActionState::Retained,
                error: None,
            });
        }
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.disconnect",
                "caller_disconnect_request",
                if report.succeeded() {
                    "completed"
                } else {
                    "failed"
                },
                [(
                    "elapsed_ms".into(),
                    started.elapsed().as_millis().to_string(),
                )],
            );
        }
        report
    }

    pub(super) fn teardown_lifecycle(&self) -> Result<(), String> {
        let diagnostics = self.diagnostic_context();
        let started = Instant::now();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.teardown",
                "caller_teardown_request",
                "started",
                [],
            );
        }
        let result = (|| {
            // 退出清理（D12/D15）：先减负断开（session-end + route 清 + 有界 join），再
            // NIC owner 关 adapter（0 网卡残留兜底）。
            let Some(_cleanup) = self.begin_cleanup(true, crate::heartbeat::ASSEMBLY_CANCEL_WAIT)
            else {
                return Self::assembly_pending_report().result();
            };
            let report = self.disconnect_owned();
            close_nic_after_stop(&report, || {
                let nic = self.nic.lock().map_err(|_| "nic lock".to_string())?.take();
                drop(nic); // 原 NicOwner；关闭在槽锁外，并仍持清理占用许可。
                Ok(())
            })
        })();
        if let Some(diag) = &diagnostics {
            diag.emit(
                "tunnel.runtime.teardown",
                "caller_teardown_request",
                if result.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
                [(
                    "elapsed_ms".into(),
                    started.elapsed().as_millis().to_string(),
                )],
            );
        }
        result
    }
}

impl RealTunnelRuntime {
    pub(super) fn start_apply_recorded(
        self: Arc<Self>,
        ctx: ApplyContext,
        admission: Option<TraceAdmission>,
        reconnect_attempt: Option<u32>,
    ) -> Result<u64, TunnelError> {
        self.start_apply_recorded_with_receipt(ctx, admission, reconnect_attempt)
            .map(|(cursor, _)| cursor)
    }

    pub(super) fn start_apply_recorded_with_receipt(
        self: Arc<Self>,
        ctx: ApplyContext,
        admission: Option<TraceAdmission>,
        reconnect_attempt: Option<u32>,
    ) -> Result<(u64, ApplyAcceptance), TunnelError> {
        use std::sync::atomic::Ordering;
        // v1/无载荷保持既有 Cisco 语义；v2 明确请求浏览器认证的 aTrust。
        let requested_protocol = match ctx.credentials.as_ref() {
            Some(ConnectionCredentials::Atrust(_)) => Protocol::Atrust,
            _ => Protocol::Cisco,
        };
        let _commit = self.lifecycle_commit.lock().expect("runtime commit lock");
        let decision = decide_start_apply(
            self.assembling.load(Ordering::SeqCst) || _commit.stopping > 0,
            self.is_connected(),
            ctx.credentials.is_some(),
        );
        if decision == StartDecision::RefuseDuplicate {
            return Err(TunnelError {
                kind: TunnelErrorKind::DuplicateConnect,
                connect_phase: ConnectPhase::ApplyingPlatformTunnel,
                code: 0,
                a0: 0,
                detail: "engine: duplicate connect (assembling)".into(),
                reason: "",
            });
        }
        // 已确认受理；旧装配不能跨此锁发布终态。Adopt 也必须在同步完成前取游标。
        let cursor = ctx.status.snapshot().0;
        if let Some(attempt) = reconnect_attempt {
            ctx.status.publish(StatusEvent::reconnecting(
                ctx.operation_id.clone(),
                attempt,
                crate::reconnect::reason::TUNNEL_LOST,
                0,
            ));
        }
        match decision {
            StartDecision::RefuseDuplicate => unreachable!("duplicate already refused"),
            StartDecision::Adopt => {
                // P2：服务引擎已有活跃隧道——采纳，不重建。直接为本次 operation
                // 发布 Connected 状态，复用现有隧道。
                // 掉线报告与身份转交由线程组的同一短锁串行；死亡后的旧会话不能
                // 被包装成新 operation，存活会话转交后的失败必须携带新的身份。
                let rebound = self
                    .live
                    .lock()
                    .expect("live lock")
                    .as_ref()
                    .filter(|live| {
                        live.connection_mode == ctx.connection_mode
                            && live.protocol == requested_protocol
                    })
                    .and_then(|live| live.data_plane_threads.as_ref())
                    .is_some_and(|threads| {
                        threads.rebind_operation_with(&ctx.operation_id, || {
                            ctx.stats
                                .registry()
                                .set_phase(generated::StatsPhase::Connected);
                            ctx.status
                                .publish(StatusEvent::connected(ctx.operation_id.clone()));
                        })
                    });
                if rebound {
                    return Ok((cursor, ApplyAcceptance::Adopted));
                }
            }
            StartDecision::Proceed => { /* 正常进入组装 */ }
        }
        let trace = match admission {
            Some(TraceAdmission::Skipped { .. }) => TraceAdmission::Skipped {
                connection_id: self.current_trace_id(&ctx.log),
            },
            Some(trace) => trace,
            None => self
                .current_trace_id(&ctx.log)
                .map(|connection_id| TraceAdmission::Existing { connection_id })
                .unwrap_or(TraceAdmission::Unwired),
        };
        // 同一 lifecycle_commit 锁内，真实装配裁决后、线程启动前开槽。
        self.attempt_ledger
            .lock()
            .expect("attempt ledger lock")
            .begin(&ctx.operation_id, trace);
        let diagnostics = RuntimeDiagnostics::new(Arc::clone(&ctx.log), &ctx.operation_id);
        let connection_mode = ctx.connection_mode;
        ctx.log.emit(
            LogLevel::Info,
            "connection",
            "tunnel.connection_mode.selected",
            "本次连接使用已冻结的连接模式",
            &[
                ("connection_mode", connection_mode.as_str()),
                ("operation_id", &diagnostics.operation_id),
            ],
        );
        diagnostics.emit("tunnel.runtime.apply", "apply_accepted", "started", []);
        *self.diagnostics.lock().expect("diagnostics lock") = Some(diagnostics);
        // 重置取消令牌 → 清 Paused → 标记在途 → 后台 thread 跑组装（受理即回）。
        self.cancel_flag.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        self.assembling.store(true, Ordering::SeqCst);
        let rt = Arc::clone(&self);
        let cancel = Arc::clone(&self.cancel_flag);
        // 终态由后台 thread 统一发布（受理即回后 handler 无法知道结果）：成功 →
        // Connected + stats Connected；失败/取消 → Failed + stats Failed。
        let op_id = ctx.operation_id.clone();
        let status = Arc::clone(&ctx.status);
        let stats = Arc::clone(&ctx.stats);
        std::thread::spawn(move || {
            // run_assemble_with_reset 内部处理 assemble 结果的 status 推送；
            // assembling 复位在该函数内完成（含 panic 保护）。返回 ()，无需绑定。
            RealTunnelRuntime::run_assemble_with_reset(&rt, ctx, cancel, &stats, &status, &op_id);
        });
        Ok((cursor, ApplyAcceptance::Started))
    }

    /// 原安装、Connected 发布和工作线程激活次序保持在同一短提交中。
    pub(super) fn install_live(
        self: &Arc<Self>,
        live: LiveTunnel,
        status: &StatusPublisher,
        stats: &crate::stats::StatsPublisher,
        operation_id: &[u8],
    ) -> Result<(), TunnelError> {
        use std::sync::atomic::Ordering;
        let owner = AttemptOwner::new(self, live, operation_id);
        let _commit = self.lifecycle_commit.lock().expect("runtime commit lock");
        if self.cancel_flag.load(Ordering::SeqCst) {
            drop(_commit);
            return Err(Self::cancelled_error(ConnectPhase::StartingDataPlane));
        }
        if !owner
            .data_plane_threads
            .as_ref()
            .is_some_and(EngineDataPlaneThreads::is_alive)
        {
            drop(_commit);
            return Err(TunnelError::plain_with_reason(
                ConnectPhase::StartingDataPlane,
                "engine: data-plane workers exited before activation",
                "platform/data_plane_start",
            ));
        }
        *self.live.lock().expect("live lock") = Some(owner.into_live());
        // 已持有 lifecycle_commit；两道前置检查通过后才提交记录，不重复取锁。
        self.attempt_ledger
            .lock()
            .expect("attempt ledger lock")
            .finish(operation_id, AttemptEnd::Committed);
        self.paused.store(false, Ordering::SeqCst);
        stats.registry().set_phase(generated::StatsPhase::Connected);
        status.publish(StatusEvent::connected(operation_id.to_vec()));
        if let Some(threads) = self
            .live
            .lock()
            .expect("live lock")
            .as_ref()
            .and_then(|live| live.data_plane_threads.as_ref())
        {
            threads.activate();
        }
        Ok(())
    }
}

/// Exit 只有在真实停止且没有剩余 owner 义务后才关闭保留 NIC；历史动作失败仍返回。
fn close_nic_after_stop(
    report: &StopReport,
    close: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !report.stopped || !report.remaining.is_empty() {
        return report.result();
    }
    close()?;
    report.result()
}
