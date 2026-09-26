
//! core 进程生命周期（P3-c2 / D1 解耦）：启动 → 拉起 engine → 运行 → 停机（发退出包）
//! → core 退出 → engine 由三重保证随行退出。
//!
//! 本模块是 core 的生命周期编排：
//!
//! - [`UiLifetime`]：UI 生命周期信号（**O3 强绑定**）——UI 最小化到托盘区**不视作退出**
//!   （UI 进程仍存活、连接仍在 → 信号保持 `true`）；UI **彻底退出**（进程退出 / 传输层
//!   掉线 / 显式通知）→ 信号翻转为 `false` → core 发起停机。
//! - [`CoreRuntime`]：运行期编排——持有 UI 信号、共享 composition、engine 监督句柄与
//!   KernelControl 服务/转发器任务；[`CoreRuntime::run`] 等待 UI 退出（或显式停机命令）后
//!   执行有序停机；运行期监听 engine 掉线（liveness）→ 驱动
//!   `composition.on_helper_link_terminal`（engine 死亡 ≠ core 死亡：撤销 admission +
//!   teardown，core 继续服务 UI 直到 UI 退出）。
//! - [`shutdown_core`]：**有序停机**（D1 解耦 / 判据 8——UI 只等 core 不等 engine）：
//!   1. 停止接收 UI 请求（`serving` 翻 false + 中止服务任务 + 取消待决 RPC waiter——
//!      `composition.on_rpc_waiter_cancel`，RPC cancel ≠ 业务 Stop，spec §8.3/§9.1）；
//!   2. engine **发退出包**（`StopTunnel` 业务停机，若在连）后即返——不等 engine 退；
//!   3. `composition.exit()`（幂等业务 Stop，关 admission，撤 packet leg）→ core 退出。
//!   退出包 / core 进程句柄 / 心跳（P2）三重保证 engine 随 core 退出；有界等待/强制
//!   终止降为崩溃恢复/验证路径（`EngineSupervisor::verify_exit`）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tokio::sync::{Mutex, watch};

use crate::composition::{HostComposition, HostPhase};
use crate::crash_recovery::{
    CrashRecovery, RespawnOutcome, SelfHealReporter, SelfHealStage, respawn_error_code,
};
use crate::engine_lifecycle::{EngineShutdownOutcome, EngineSupervisor};
use crate::guards::ServiceMode;
use crate::kernel_control_service::{EventBus, snapshot_for_phase};

/// UI 生命周期信号（O3 强绑定）。
///
/// `true` = UI 存活（最小化到托盘仍存活——UI 进程与连接未断）；`false` = UI 彻底退出。
/// [`UiLifetime::on_ui_exited`] 由 P4 的 UI 传输层在连接掉线 / 窗口彻底关闭时调用。
#[derive(Clone)]
pub struct UiLifetime {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl UiLifetime {
    /// 构造：初始 `true`（UI 已连接/存活）。
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(true);
        Self { tx, rx }
    }

    /// UI 是否存活（当前信号值）。
    #[must_use]
    pub fn is_ui_alive(&self) -> bool {
        *self.rx.borrow()
    }

    /// UI 彻底退出（传输层掉线 / 窗口关闭）→ 信号翻 false → core 停机。
    pub fn on_ui_exited(&self) {
        let _ = self.tx.send(false);
    }

    /// 存活信号接收端（`CoreRuntime::run` 等待 UI 退出）。
    #[must_use]
    pub fn receiver(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }
}

impl Default for UiLifetime {
    fn default() -> Self {
        Self::new()
    }
}

/// core 停机结果（顺序契约的可观测事实）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreShutdownOutcome {
    /// engine 侧事实（D1 后 = 退出包已发 / 无 client；CleanExit/Terminated 仅崩溃
    /// 恢复/验证路径出现）。
    pub engine: EngineShutdownOutcome,
    /// composition 发出的业务 Stop 计数（唯一业务取消是 `exit()`——必须恰好 1）。
    pub stop_requests: u64,
    /// RPC waiter 取消计数（停机时取消待决 waiter——RPC cancel ≠ Stop）。
    pub rpc_waiter_cancellations: u64,
    /// 有界完整 teardown 是否已启动。
    pub teardown_started: bool,
}

/// 有序停机（D1 解耦 / 判据 8：UI 只等 core，不等 engine）。
///
/// 顺序（不可交换，测试钉死）：
/// 1. **停止接收 UI 请求**：取消待决 RPC waiter（`composition.on_rpc_waiter_cancel`——
///    记录取消，不改变业务状态；真正的业务取消只来自 `exit()`）。
/// 2. **engine 发退出包**：`EngineSupervisor::stop`——`StopTunnel` 业务停机（若在连）后
///    **即返**，不再有界等待 engine 退出/强制终止（engine 回 Idle 常驻，由退出包 / core
///    进程句柄 / 心跳（P2）三重保证随 core 退出）。
/// 3. **core 退出**：`composition.exit()`——幂等业务 Stop、关闭 admission、撤销 packet leg。
///
/// **R2 (a) CleanExit→synthesize 收敛保留在崩溃路径**：engine 异常死亡（状态流 EOF）时由
/// 状态转发器（`kernel_control_service::spawn_status_forwarder`）合成数据面侧加入收敛
/// Stopped/Idle；UI 关闭路径不等待 engine、不观测 CleanExit，故此处不再合成（core 即将
/// 退出，无收敛必要）。
///
/// 调用方须在进入本函数前 drop 服务任务 / 事件转发器（释放对共享 engine client 的引用）。
pub async fn shutdown_core(
    supervisor: Arc<Mutex<EngineSupervisor>>,
    composition: &Arc<Mutex<HostComposition>>,
) -> CoreShutdownOutcome {
    // 1. 取消待决 RPC waiter（RPC cancel ≠ 业务 Stop：只记录，不改业务状态）。
    {
        let mut comp = composition.lock().await;
        comp.on_rpc_waiter_cancel();
    }
    // 2. engine 发退出包（fire-and-forget；不等待 engine 退出）。按需拉起模型下
    //    supervisor 可能为空监督（无 engine 曾被拉起）→ `NoClient`（core 退出即完）。
    let supervisor = {
        let mut guard = supervisor.lock().await;
        std::mem::replace(&mut *guard, EngineSupervisor::empty())
    };
    let engine = supervisor.stop().await;
    // 3. composition.exit()（幂等业务 Stop）。
    let mut comp = composition.lock().await;
    comp.exit();
    let stop_requests = comp.stop_requests();
    let rpc_waiter_cancellations = comp.rpc_waiter_cancellations();
    let teardown_started = comp.teardown_started();
    CoreShutdownOutcome {
        engine,
        stop_requests,
        rpc_waiter_cancellations,
        teardown_started,
    }
}

/// core 运行期（进程级编排）。
///
/// 生命周期：**启动**（外部先 compose + spawn + connect + 挂接服务/转发器）→
/// [`CoreRuntime::run`] 运行（等 UI 退出 / 显式停机；监听 engine 掉线）→ 有序停机
/// （发退出包后即返）→ 返回 [`CoreShutdownOutcome`]（core 退出；engine 由三重保证随行）。
pub struct CoreRuntime {
    /// UI 生命周期信号（O3 强绑定）。
    ui: UiLifetime,
    /// 服务开关：`true` = 接收 UI 请求；停机置 `false`（P4 传输层据此拒绝新请求）。
    serving_tx: watch::Sender<bool>,
    serving_rx: watch::Receiver<bool>,
    /// 显式停机命令（UI 发 stop / 系统信号；`request_shutdown`）。
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    /// 共享 host composition（与 `KernelControlService` 共享同一 Arc）。
    composition: Arc<Mutex<HostComposition>>,
    /// engine 子进程监督句柄（与 `EngineProvisioner` 共享的互斥句柄——respawn/
    /// provision/停机三方互斥；按需拉起模型下可为空监督）。
    supervisor: Arc<Mutex<EngineSupervisor>>,
    /// engine 槽换点订阅（按需拉起模型：provision 换入新 engine 挂接新 liveness 后，
    /// run 循环据此重新获取掉线监听——启动时无 engine，无 liveness 可监听）。
    /// `None` = 未接线（测试）。
    slot_swaps: Option<watch::Receiver<u64>>,
    /// 共享维护形态（`KernelControlService::selected_mode_handle`；respawn 分流判据
    /// ——仅 oneshot 维护形态 respawn，2026-09-08 计划批 3）。`None` = 未接线（测试，
    /// 保持既有「总是 respawn」语义）。
    mode_gauge: Option<Arc<AtomicU8>>,
    /// KernelControl 服务任务（P4 传输层；停机先中止 = 停止接收 UI 请求）。
    serve_task: Option<tokio::task::JoinHandle<()>>,
    /// 事件转发器任务（持 engine client 引用；停机先中止释放引用）。
    forwarder_task: Option<tokio::task::JoinHandle<()>>,
    /// P5-b 统计转发器任务（持 engine client 引用；停机先中止释放引用）。
    stats_forwarder_task: Option<tokio::task::JoinHandle<()>>,
    /// R3 日志转发器任务（持 engine client 引用 + 日志聚合器引用；停机先中止释放引用）。
    logs_forwarder_task: Option<tokio::task::JoinHandle<()>>,
    /// C3a 自动重连 worker 任务（持服务克隆；停机先中止释放引用——否则 worker 永久
    /// 引用服务克隆）。
    reconnect_worker_task: Option<tokio::task::JoinHandle<()>>,
    /// P2 KeepAlive 心跳 tick 任务（持 engine client 引用；停机先中止释放引用——engine
    /// 侧据此刷新 `last_heartbeat`，15s 未收到即自清理+自退出）。
    keepalive_ticker_task: Option<tokio::task::JoinHandle<()>>,
    /// P3 崩溃自愈编排（engine 掉线 → respawn 全链路）；`None` = 不自愈（测试/降级）。
    crash_recovery: Option<CrashRecovery>,
    /// 2026-09-05 自愈上报 seam（阶段变化 → EventBus lane + 结构化日志）；`None` =
    /// 不上报（测试/降级）。
    self_heal_reporter: Option<Arc<dyn SelfHealReporter>>,
    /// 2026-09-05 自愈阶段显式发布的事件总线（`publish_self_heal_refresh`——自愈窗口
    /// 内没有自然事件，阶段变化必须显式驱动发布）；`None` = 不发布（测试/降级）。
    self_heal_events: Option<Arc<EventBus>>,
}

impl CoreRuntime {
    /// 构造运行期。
    #[must_use]
    pub fn new(
        ui: UiLifetime,
        composition: Arc<Mutex<HostComposition>>,
        supervisor: EngineSupervisor,
    ) -> Self {
        let (serving_tx, serving_rx) = watch::channel(true);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            ui,
            serving_tx,
            serving_rx,
            shutdown_tx,
            shutdown_rx,
            composition,
            supervisor: Arc::new(Mutex::new(supervisor)),
            slot_swaps: None,
            mode_gauge: None,
            serve_task: None,
            forwarder_task: None,
            stats_forwarder_task: None,
            logs_forwarder_task: None,
            reconnect_worker_task: None,
            keepalive_ticker_task: None,
            crash_recovery: None,
            self_heal_reporter: None,
            self_heal_events: None,
        }
    }

    /// 共享 supervisor 互斥句柄（`EngineProvisioner` 的 provision/retire 与本运行期
    /// 的 respawn/停机经它互斥）。
    #[must_use]
    pub fn supervisor_handle(&self) -> Arc<Mutex<EngineSupervisor>> {
        Arc::clone(&self.supervisor)
    }

    /// 接线 engine 槽换点订阅（provision 换入新 engine 后重新获取 liveness）。
    pub fn set_slot_swaps(&mut self, swaps: watch::Receiver<u64>) {
        self.slot_swaps = Some(swaps);
    }

    /// 接线共享维护形态（respawn 分流判据：仅 oneshot respawn）。
    pub fn set_mode_gauge(&mut self, gauge: Arc<AtomicU8>) {
        self.mode_gauge = Some(gauge);
    }

    /// UI 生命周期信号句柄。
    #[must_use]
    pub fn ui(&self) -> &UiLifetime {
        &self.ui
    }

    /// 共享 composition（对外驱动 `on_helper_link_terminal` / `exit` 或断言）。
    #[must_use]
    pub fn composition(&self) -> &Arc<Mutex<HostComposition>> {
        &self.composition
    }

    /// 服务开关接收端（P4 传输层据此在停机后拒绝新请求）。
    #[must_use]
    pub fn serving_receiver(&self) -> watch::Receiver<bool> {
        self.serving_rx.clone()
    }

    /// 挂接 KernelControl 服务任务（P4 传输层 serve future；停机先中止）。
    pub fn set_serve_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.serve_task = Some(task);
    }

    /// 挂接状态转发器任务（`KernelControlService::spawn_status_forwarder`；停机先中止）。
    pub fn set_forwarder_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.forwarder_task = Some(task);
    }

    /// 挂接统计转发器任务（`KernelControlService::spawn_stats_forwarder`，P5-b；停机先
    /// 中止——统计转发器同样持有 engine client Arc，须在 `shutdown_core` 前释放引用）。
    pub fn set_stats_forwarder_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.stats_forwarder_task = Some(task);
    }

    /// 挂接日志转发器任务（`KernelControlService::spawn_log_forwarder`，R3；停机先
    /// 中止——日志转发器持 engine client Arc 与日志聚合器 Arc，须在 `shutdown_core`
    /// 前释放引用。日志纯单向输出，中止无状态副作用）。
    pub fn set_logs_forwarder_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.logs_forwarder_task = Some(task);
    }

    /// 挂接自动重连 worker 任务（`KernelControlService::spawn_reconnect_worker`，C3a；
    /// 停机先中止——worker 持服务克隆，须在 `shutdown_core` 前释放引用，否则服务与
    /// 其引用的 engine client 不因停机释放）。
    pub fn set_reconnect_worker_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.reconnect_worker_task = Some(task);
    }

    /// 挂接 KeepAlive 心跳 tick 任务（P2：`KernelControlService::spawn_keepalive_ticker`；
    /// 停机先中止——ticker 持 engine client Arc，须在 `shutdown_core` 前释放引用。
    /// 中止后 engine 不再收到心跳；core 停机时 engine 由退出包/进程句柄随行退出）。
    pub fn set_keepalive_ticker_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.keepalive_ticker_task = Some(task);
    }

    /// 挂接崩溃自愈编排（P3：liveness 翻转 false → `CrashRecovery::run_respawn` 全链路；
    /// engine 死亡 ≠ core 死亡，core 自愈后继续服务 UI）。
    pub fn set_crash_recovery(&mut self, recovery: CrashRecovery) {
        self.crash_recovery = Some(recovery);
    }

    /// 挂接自愈上报 seam（2026-09-05 计划 §4.3：阶段变化 → EventBus lane + 结构化日志；
    /// 真实实现 = `EventBusSelfHealReporter`，测试用 recording fake）。
    pub fn set_self_heal_reporter(&mut self, reporter: Arc<dyn SelfHealReporter>) {
        self.self_heal_reporter = Some(reporter);
    }

    /// 挂接自愈阶段显式发布的事件总线（2026-09-05 计划 §4.3：阶段变化点以当前相位
    /// 快照经 `publish_self_heal_refresh` 发布——复用既有 `publish` 路径）。
    pub fn set_self_heal_events(&mut self, events: Arc<EventBus>) {
        self.self_heal_events = Some(events);
    }

    /// 显式请求停机（UI 发 stop / 系统信号）→ `run` 的 select 触发停机。
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// 运行 core 生命周期：监听 engine 掉线 → 等 UI 彻底退出（或显式停机）→ 有序停机。
    ///
    /// `self` 按值消费（engine 监督句柄所有权终结于停机）。
    ///
    /// **停机信号**：`ui_rx.changed()`（[`UiLifetime::on_ui_exited`]，由传输层的
    /// UI 进程退出监视触发）或 `shutdown_rx.changed()`（显式停机命令）。**不**把 serve
    /// 任务 resolve 当 UI 断开信号——tonic `serve_with_incoming` 对单元素传入流在 accept
    /// 后立即返回（流耗尽即 break，连接任务 detached 继续服务），其 JoinHandle 会假
    /// resolve；真实 UI 退出由进程监视感知（见 `kernel_control_transport`）。
    pub async fn run(mut self) -> CoreShutdownOutcome {
        // 运行期：等 UI 彻底退出（O3：进程退出 → on_ui_exited）或显式停机命令；同时监听
        // engine 掉线（liveness 翻 false）→ 崩溃自愈 respawn（P3：engine 死亡 ≠ core 死亡，
        // core 自愈后继续服务 UI）。
        let mut ui_rx = self.ui.receiver();
        let mut shutdown_rx = self.shutdown_rx.clone();
        // 按需拉起模型：启动时无 engine → 无 liveness 可监听；provision 换入新 engine
        //（supervisor 挂接新 liveness）后经 slot 换点通知重新获取。
        let mut liveness_rx = self.supervisor.lock().await.liveness();
        let mut slot_swaps = self.slot_swaps.take();
        let mode_gauge = self.mode_gauge.take();
        let crash_recovery = self.crash_recovery.take();

        loop {
            // engine 掉线监听 future：liveness 翻 false → 返回 true（触发 respawn）；
            // 无 client（无 liveness）→ pending（无 engine 可等）。
            let liveness_fired = liveness_rx.clone().map(|mut rx| {
                Box::pin(async move {
                    loop {
                        if !*rx.borrow() {
                            return true;
                        }
                        if rx.changed().await.is_err() {
                            return false;
                        }
                    }
                })
            });
            tokio::select! {
                _ = ui_rx.changed() => break,
                _ = shutdown_rx.changed() => break,
                // engine 槽换点（provision/respawn/service 路由）：liveness 尚未接线时
                // 尝试从 supervisor 获取新 engine 的掉线监听。
                _ = async {
                    if let Some(rx) = slot_swaps.as_mut() {
                        let _ = rx.changed().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    if liveness_rx.is_none() {
                        liveness_rx = self.supervisor.lock().await.liveness();
                    }
                }
                died = async {
                    if let Some(fut) = liveness_fired {
                        fut.await
                    } else {
                        std::future::pending::<()>().await;
                        false
                    }
                } => {
                    if died {
                        // 崩溃自愈按维护形态 + 业务在途分流（2026-09-08 计划批 3）：
                        // 1. service/auto 形态不 respawn——service engine 崩溃归 SCM
                        //    （SC_ACTION_RESTART），auto 无维护对象；
                        // 2. oneshot 形态但**业务不在途**（idle engine 自然死亡）同样
                        //    不 respawn——不为无用户业务手势的死亡弹 UAC；下一次
                        //    connect 的 provision（ensure_oneshot_engine）按需重拉。
                        // 未接线 gauge（测试）保持既有「总是 respawn」语义。
                        let mode_allows_respawn = match mode_gauge.as_ref().map(|g| {
                            ServiceMode::from_u8(g.load(Ordering::SeqCst))
                        }) {
                            None => true,
                            Some(ServiceMode::Oneshot) => true,
                            Some(_) => false,
                        };
                        let busy = {
                            let composition = self.composition.lock().await;
                            !matches!(
                                composition.phase(),
                                HostPhase::Idle | HostPhase::Stopped
                            )
                        };
                        if !mode_allows_respawn || !busy {
                            // 业务在途（如 service 连接中掉线）才做 composition 收敛；
                            // 闲置边界（如 oneshot 退役/idle 死亡引发的 liveness 翻转）
                            // 不重复 teardown，避免把 Idle/Stopped 压成 Reconciling
                            // 关闭 admission。
                            if busy {
                                self.composition.lock().await.on_helper_link_terminal();
                            }
                            liveness_rx = None;
                        } else if let Some(recovery) = &crash_recovery {
                            let mut supervisor = self.supervisor.lock().await;
                            let old_pid = supervisor.pid().unwrap_or(0);
                            // 2026-09-05 发布点 1（冻结表）：liveness 翻 false、进入
                            // respawn 前——`respawning` + 当前相位快照显式刷新。
                            Self::report_self_heal_refresh(
                                self.self_heal_reporter.as_ref(),
                                self.self_heal_events.as_ref(),
                                &self.composition,
                                SelfHealStage::Respawning,
                                old_pid,
                                0,
                                "",
                            )
                            .await;
                            match recovery.run_respawn(&mut *supervisor).await {
                                Ok(outcome) => {
                                    // respawn 成功：新 supervisor 带新 liveness（初始 true），
                                    // 继续等 UI 退出 / 下次掉线。
                                    liveness_rx = supervisor.liveness();
                                    tracing::info!(?outcome, "engine respawned (crash self-heal)");
                                    // 2026-09-05 发布点 2（冻结表）：`succeeded` + 重建后
                                    // Idle 快照。既有缺陷修复：composition 重建为 Idle 后
                                    // 无人发布快照（新 engine 状态流不回放旧状态），UI 停留
                                    // 「处理中」——此处显式刷新告诉 UI「自愈完成，可重新
                                    // 连接」。
                                    let RespawnOutcome::Respawned { old_pid, new_pid } = outcome;
                                    Self::report_self_heal_refresh(
                                        self.self_heal_reporter.as_ref(),
                                        self.self_heal_events.as_ref(),
                                        &self.composition,
                                        SelfHealStage::Succeeded,
                                        old_pid,
                                        new_pid,
                                        "",
                                    )
                                    .await;
                                }
                                Err(reason) => {
                                    // respawn 被阻/失败：回 Idle/报错（不自动重连）。停止
                                    // 掉线监听（无新 engine 可等）；用户再点连接走正常登录。
                                    liveness_rx = None;
                                    tracing::warn!(error = %reason, "engine respawn failed; user reconnect required");
                                    // 2026-09-05 发布点 3（冻结表）：`failed`（error_code
                                    // 按 §4.2 码表穷尽映射）+ 当前相位快照（teardown 已在
                                    // respawn 步骤 1 落地，composition 处 Reconciling）。
                                    Self::report_self_heal_refresh(
                                        self.self_heal_reporter.as_ref(),
                                        self.self_heal_events.as_ref(),
                                        &self.composition,
                                        SelfHealStage::Failed,
                                        old_pid,
                                        0,
                                        respawn_error_code(&reason),
                                    )
                                    .await;
                                }
                            }
                        } else {
                            // 无崩溃自愈编排（测试/降级）：teardown 兜底 + 停止掉线监听
                            // （避免死循环；保留旧语义的 on_helper_link_terminal）。
                            self.composition.lock().await.on_helper_link_terminal();
                            liveness_rx = None;
                        }
                    }
                }
            }
        }

        // 停止接收 UI 请求：置 serving=false + 中止服务任务（router 持有的服务引用
        // drop，停止接受新请求；服务实际释放 engine client 引用由连接任务终结完成——
        // UI 退出时连接任务已因管道 EOF 结束）。
        let _ = self.serving_tx.send(false);
        if let Some(task) = self.serve_task.take() {
            task.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), task).await;
        }

        // 中止并等待事件转发器实际结束（其 future drop 时释放持有的 engine client Arc）。
        // 契约要求进入 shutdown_core 前服务侧引用已释放——abort 是异步取消，光 abort 不
        // 保证 Arc 立即 drop，故有界等待其完成（转发器在 await 点被取消，很快结束）。
        if let Some(task) = self.forwarder_task.take() {
            task.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), task).await;
        }

        // 统计转发器（P5-b）同样持 engine client Arc：中止并等待其结束，确保进入
        // shutdown_core 前服务侧引用已释放（否则 engine 不因 PeerClosed 退出）。
        if let Some(task) = self.stats_forwarder_task.take() {
            task.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), task).await;
        }

        // 日志转发器（R3）持 engine client Arc + 日志聚合器 Arc：中止并等待其结束，
        // 确保进入 shutdown_core 前服务侧引用已释放。日志纯单向输出，中止无状态副作用。
        if let Some(task) = self.logs_forwarder_task.take() {
            task.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), task).await;
        }

        // 自动重连 worker（C3a）持服务克隆（间接持 engine client 引用）：中止并等待
        // 其结束，确保进入 shutdown_core 前服务侧引用已释放。
        if let Some(task) = self.reconnect_worker_task.take() {
            task.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), task).await;
        }

        // KeepAlive 心跳 tick 任务（P2）持 engine client Arc：中止并等待其结束，确保
        // 进入 shutdown_core 前服务侧引用已释放（此后 engine 不再收到心跳；core 停机
        // 由退出包/进程句柄保证 engine 随行退出）。
        if let Some(task) = self.keepalive_ticker_task.take() {
            task.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), task).await;
        }

        // 有序停机（supervisor 所有权移入；engine 发退出包后即返；composition.exit()）。
        shutdown_core(self.supervisor, &self.composition).await
    }

    /// 2026-09-05 自愈发布点统一落地（计划 §4.3，发布点表三行共用）：
    ///
    /// 1. 经 reporter 上报一次阶段变化（EventBus self_heal lane + `kernel.selfheal.*`
    ///    结构化日志）；
    /// 2. 以 composition **当前相位**组装快照经 `EventBus::publish_self_heal_refresh`
    ///    显式发布（复用既有 `publish(TRANSITION, ...)` 路径与 lane 附加链）——自愈窗口
    ///    内没有自然事件，不显式发布则订阅者拿不到阶段变化（§1.3 缺陷的机制根源）。
    ///
    /// reporter/events 任一未挂接（测试/降级）时跳过对应半边；相位快照从不伪造——
    /// 发布的是 composition 的真实相位（respawn 入口 = 进入 teardown 前；成功臂 =
    /// 重建后 Idle；失败臂 = teardown 后 Reconciling）。
    async fn report_self_heal_refresh(
        reporter: Option<&Arc<dyn SelfHealReporter>>,
        events: Option<&Arc<EventBus>>,
        composition: &Arc<Mutex<HostComposition>>,
        stage: SelfHealStage,
        old_pid: u32,
        new_pid: u32,
        error_code: &str,
    ) {
        if let Some(reporter) = reporter {
            reporter.report(stage, old_pid, new_pid, error_code);
        }
        if let Some(events) = events {
            let snapshot = {
                let composition = composition.lock().await;
                snapshot_for_phase(composition.phase(), &composition)
            };
            events.publish_self_heal_refresh(snapshot);
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试：UiLifetime 翻转 + shutdown_core 顺序契约 + CoreRuntime 触发路径
// （UI 退出 / 显式停机）。进程级 wait/terminate 用 fake child（占位 0 句柄——
// WaitForSingleObject(null) 返回 WAIT_FAILED → 有界等待失败 → Terminated，确定性）。
// ---------------------------------------------------------------------------
