
//! engine 兜底善后（T5，计划 §2.4 删除权 / §2.5 core 崩溃响应 / §2.9 缺口②）。
//!
//! 「core 不在场且不会再驱动善后」时的删除权行使 + 断开路径的验证收口，全部经
//! [`AftermathDriver`]：
//!
//! - **情形①（`on_core_exit=teardown` 的 core 死亡）**：service 形态
//!   `MaintainOwnerLease` 流 EOF（既有 owner 释放路径之后）→
//!   [`AftermathDriver::on_owner_stream_eof`] 复核 core PID 存活 → 确死且策略
//!   teardown → [`AftermathDriver::abandon_session`]。
//! - **情形① keep（无头续存）**：确死且策略 keep → 只释放 owner（既有语义，由
//!   调用侧 EOF 块完成）+ 登记 [`HeadlessState`]，**不拆隧道**、痕保持 Live——
//!   拉起归 T10、采纳归 T8（本阶段只定义内部状态供其查询）。
//! - **情形④（connecting 态 core 死亡）**：确死时痕仍在 connecting（组装未完成/
//!   在途）→ 取消 + teardown 部分状态 → teardown 善后收口。
//! - **情形②（无头 R1 终态）/ 情形③（崩溃环终态）**：重连与拉起归 T6a/T10；
//!   终态钩子 = [`AftermathDriver::abandon_session`]（本模块实现并被单测，调用方
//!   后续接入）。
//! - **断开路径（core 删除权前提"善后已验证"）**：`StopTunnel` 的 disconnect 完成
//!   后 [`AftermathDriver::verify_disconnect_aftermath`] 复验（预检探针 + `trace`
//!   hint）→ 干净：痕 Aftermath → 删 + `verified=true`（Idle 终态事件携带，
//!   `EXV_UNFREEZE` tag 7，core/UI 可清除本地会话状态）；残留：痕留 Aftermath +
//!   `verified=false`（下次连接预检阻断）。
//! - **oneshot 随行退出（T10 前过渡）**：engine 进程随 core 退出（main.rs 既有
//!   生命周期）时 [`AftermathDriver::on_engine_exit`] 收口——keep 的无头续存在
//!   oneshot 形态依赖 T10（拉起底质 + 心跳退出动作策略化），退出即终局。
//!
//! # 痕迹即证据（计划 §2.5/§2.9）
//!
//! 每次善后都写终态摘要事件：`aftermath.completed`（验证干净，痕已删）/
//! `aftermath.residual`（残留未决，痕留 Aftermath），携带稳定 reason id
//! （[`reason`]）与残留 id（[`residual_reasons`]）。摘要在删痕**之前**落日志。
//!
//! # 与预检的关系
//!
//! 善后验证 = [`crate::preflight::run_preflight_for`]（trace 作 hint；其清理阶段
//! 会顺带清除 teardown 漏掉的自有残留——清理执行器即 T3 的复用）。评估口径与
//! 连接门的 `blocking_reasons` 不同：外来 NRPT 冲突是**活系统事实**而非本会话
//! 残留（不阻断善后完成，下次连接预检自会看见）；D12 设计内保留的网卡（Paused
//! 态 engine 仍持有 adapter 句柄）不算残留（[`residual_reasons`] 的
//! `expected_retained_adapters` 豁免）。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_wire::generated;
use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

use crate::connect_gate::ConnectGate;
use crate::connection_trace::{
    TraceEngineMode, TraceOnCoreExit, TraceReadOutcome, TraceRecord, TraceState,
};
use crate::heartbeat::ASSEMBLY_CANCEL_WAIT;
use crate::log_sink::{LogLevel, LogSink};
use crate::preflight::{
    self, CleanupOutcome, DomainKind, DomainStatus, FindingPolicy, PreflightInputs,
    PreflightReport,
};
use crate::status::{StatusEvent, StatusPublisher};
use crate::stats::StatsPublisher;
use crate::tunnel_runtime::TunnelRuntime;

/// 在途组装取消的轮询周期（镜像 `stop_tunnel` / `heartbeat_shutdown` 的 50ms）。
const ASSEMBLY_CANCEL_POLL: Duration = Duration::from_millis(50);

/// 善后验证墙钟上界：超时按"未验证"处理（痕留 Aftermath，绝不因验证卡死而吊住
/// 断开/退出路径）。典型干净路径 ≪ 此值（PS 冷启动 ~1s + 各域毫秒级）。
const VERIFY_BOUND: Duration = Duration::from_secs(8);

/// 崩溃环上限（§2.5 缺口②：engine 拉起的 core 在窗口内第 N 次复死 → 判定不可
/// 恢复放弃拉起；计划建议 3，可调常量）。
pub const CRASH_LOOP_MAX_RELAUNCHES: u32 = 3;

/// 崩溃环窗口（§2.5：复死时刻距窗口锚点超过该值 → 窗口重置；计划建议 60s）。
pub const CRASH_LOOP_WINDOW_MS: u64 = 60_000;

/// 受理期记录的 core 会话事实（T10：拉起选会话的稳定事实源——core 活着时
/// `ProcessIdToSessionId` 必可查得；EOF 时 core 已死不可再查，故在受理点记录）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreSessionFacts {
    /// 受理本次连接的 core PID（传输层已验证 peer）。
    pub core_pid: u32,
    /// core 所在终端会话 id（查询失败 = None → 拉起回退活动控制台会话）。
    pub session_id: Option<u32>,
    /// core 背后的 UI PID（trace 同源事实）。
    pub ui_pid: u32,
}

/// 善后终态摘要的稳定 reason id（§2.4 删除权各情形 + 断开路径 + oneshot 过渡）。
pub mod reason {
    /// 情形①：core 死亡 + `on_core_exit=teardown`。
    pub const CORE_EXIT_TEARDOWN: &str = "core_exit_teardown";
    /// 情形④：connecting 态 core 死亡（组装未完成/在途即死）。
    pub const CONNECTING_ABORT: &str = "connecting_abort";
    /// 情形②：无头续存中 R1 重连终态失败（T6a 调用）。
    pub const R1_TERMINAL: &str = "r1_terminal";
    /// 情形③：core 崩溃环终态（T10 在 N 次拉起复死后调用，§2.9 缺口②）。
    pub const CRASH_LOOP: &str = "crash_loop";
    /// 断开路径：core 正常 `StopTunnel` 后的善后验证。
    pub const NORMAL_DISCONNECT: &str = "normal_disconnect";
    /// oneshot 随行退出（T10 前过渡：keep 无头续存不可用，退出即终局）。
    pub const ONESHOT_ENGINE_EXIT: &str = "oneshot_engine_exit";
}

/// core PID 存活性谓词（生产 = `OpenProcess` 探测；测试注入假实现）。
pub type CoreLiveness = Arc<dyn Fn(u32) -> bool + Send + Sync>;

/// 善后验证器：以遗留 trace 为 hint 跑一轮预检（生产 = `run_preflight_for`）。
pub type AftermathVerifier = Arc<
    dyn Fn(Option<TraceRecord>) -> Pin<Box<dyn Future<Output = PreflightReport> + Send>>
        + Send
        + Sync,
>;

/// 无头会话记录（keep 策略下 core 死亡后的 engine 内部状态；T8 采纳/T10 拉起查询）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessSession {
    /// 死去的 core PID（传输层已验证 peer；采纳的新 core 不得与它混同）。
    pub core_pid: u32,
    /// 进入无头态的 Unix 毫秒时刻。
    pub since_ms: u64,
    /// 当时的连接策略（恒 Keep；留字段防未来策略扩型）。
    pub policy: TraceOnCoreExit,
    /// T10：core 背后的 UI PID（trace 受理事实）——拉起参数 `--ui-pid` 与 core
    /// UI 侧管道名的来源；0 = 旧 core 未上报（UI 已亡场景，core 照常拉起，新 UI
    /// 走 T9 自注册）。
    pub ui_pid: u32,
    /// T10：发起用户 SID（受理事实）——拉起参数 `--ui-sid` 的来源；None = 载体
    /// 无 SID（拉起降级失败，见 [`AftermathDriver::relaunch_after_keep`]）。
    pub user_sid: Option<String>,
}

/// 一次成功拉起的在册记录（[`crate::core_relaunch::SpawnedCore`] 的账本条目）。
pub use crate::core_relaunch::SpawnedCore;

/// T10 拉起账本：跨复死累积的崩溃环计数 + 拉起记录（与 [`HeadlessState`] 同驻，
/// 会话终局 / 新受理连接时整体归零——不另造平行状态机）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RelaunchLedger {
    /// 窗口内已观察到的「engine 拉起的 core 复死」次数。
    pub crash_count: u32,
    /// 窗口锚点（窗口内首次复死时刻；None = 尚无复死）。
    pub window_started_ms: Option<u64>,
    /// 已拉起 core 的记录（pid + 拉起时刻）——复死判定与 `relaunched_by_engine`
    /// 匹配的共用材料（任务契约：pid/时间窗）。终态上限内最多
    /// [`CRASH_LOOP_MAX_RELAUNCHES`] 条，无界增长不可能。
    pub spawned: Vec<SpawnedCore>,
    /// 崩溃环终态标志（复死达上限后置位——此后不再拉起，直至账本归零）。
    pub terminal: bool,
}

/// 崩溃环判定结局（[`crash_loop_step`] 纯函数产出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashLoopDecision {
    /// 允许继续拉起（本次复死已计入窗口；`crash_count` 为窗口内当前计数）。
    Relaunch { crash_count: u32 },
    /// 崩溃环终态（窗口内复死达 [`CRASH_LOOP_MAX_RELAUNCHES`]——放弃拉起转善后）。
    Terminal { crash_count: u32 },
}

/// 崩溃环判定（纯函数，可单测——时钟由参数注入）：EOF 确死 pid 进来，按账本判
/// 下一步（§2.5 缺口②：N 次/T 秒窗口内复死 → 终态）。
///
/// - EOF pid **不在**拉起记录中（初次死亡——UI 拉的原 core）：不计入，返回
///   `Relaunch { crash_count }`（原计数不动，engine 首次拉起）。
/// - 在记录中（engine 拉起的 core 复死）：计入窗口——距窗口锚点超过
///   `window_ms` → 窗口重置（count=1、锚点=now）；计数达 `max_relaunches` →
///   `Terminal`（账本 `terminal` 置位）。
pub(crate) fn crash_loop_step(
    ledger: &mut RelaunchLedger,
    eof_pid: u32,
    now_ms: u64,
    max_relaunches: u32,
    window_ms: u64,
) -> CrashLoopDecision {
    let engine_spawned = ledger.spawned.iter().any(|entry| entry.pid == eof_pid);
    if !engine_spawned {
        // 初次死亡：不动窗口计数（R1 首拉起点）。
        return CrashLoopDecision::Relaunch {
            crash_count: ledger.crash_count,
        };
    }
    // 复死：窗口过期 → 重置锚点与计数（环已断，重新观察）。
    let in_window = ledger
        .window_started_ms
        .is_some_and(|start| now_ms.saturating_sub(start) <= window_ms);
    if !in_window {
        ledger.crash_count = 0;
        ledger.window_started_ms = Some(now_ms);
    } else if ledger.window_started_ms.is_none() {
        ledger.window_started_ms = Some(now_ms);
    }
    ledger.crash_count = ledger.crash_count.saturating_add(1);
    if ledger.crash_count >= max_relaunches {
        ledger.terminal = true;
        return CrashLoopDecision::Terminal {
            crash_count: ledger.crash_count,
        };
    }
    CrashLoopDecision::Relaunch {
        crash_count: ledger.crash_count,
    }
}

/// 无头态载体：单一活跃会话（每用户单 engine 单连接的既有拓扑）+ 同驻拉起账本。
#[derive(Default)]
pub struct HeadlessState {
    inner: Mutex<HeadlessInner>,
}

/// 会话记录与拉起账本同驻一体（一个无头情景的全部状态）。
#[derive(Default)]
struct HeadlessInner {
    /// 当前无头会话（None = 无 / 已被终态收口清除）。
    session: Option<HeadlessSession>,
    /// T10 拉起账本（跨复死累积；`clear_all` / 新受理连接归零）。
    relaunch: RelaunchLedger,
}

impl HeadlessState {
    /// 当前无头会话（None = 无 / 已被终态收口清除）。
    #[must_use]
    pub fn current(&self) -> Option<HeadlessSession> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session
            .clone()
    }

    /// 拉起账本快照（观测/测试）。
    #[must_use]
    pub fn relaunch_ledger(&self) -> RelaunchLedger {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relaunch
            .clone()
    }

    /// T10 `relaunched_by_engine` 匹配（ObserveOwnedState 消费）：对端 core pid
    /// 是否在 engine 拉起记录中（transport 已验证 pid + 拉起记录双向事实）。
    #[must_use]
    pub fn relaunched_pid_matches(&self, pid: u32) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relaunch
            .spawned
            .iter()
            .any(|entry| entry.pid == pid)
    }

    /// 进入无头态（替换任何先前记录）。
    fn enter(&self, session: HeadlessSession) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session = Some(session);
    }

    /// 会话终局全清（无头会话 + 拉起账本一起归零——崩溃环终态 / 善后收口）。
    fn clear_all(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.session = None;
        inner.relaunch = RelaunchLedger::default();
    }

    /// 新受理连接的崩溃环归零（core 回归在场——环已断，新会话新起点）。
    /// 拉起记录一并清（新连接的 core 是 UI 拉的，旧记录无匹配价值）。
    fn reset_relaunch_ledger(&self) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relaunch = RelaunchLedger::default();
    }

    /// 拉起账本锁内变更（EOF 分叉的崩溃环判定 + 拉起记录写入）。
    fn with_ledger<R>(&self, mutate: impl FnOnce(&mut RelaunchLedger) -> R) -> R {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mutate(&mut inner.relaunch)
    }
}

/// service 形态按 peer PID 计数的在途 `MaintainOwnerLease` 流量规（EOF 细则）。
///
/// EOF on one connection ≠ core death：engine 服务形态对多客户端连续 accept，同
/// core PID 可能同时存在多根控制流。善后分叉只在「同 PID **最后一根**流结束」时
/// 评估（配合 PID 存活复核，见 [`AftermathDriver::on_owner_stream_eof`]）。
#[derive(Default)]
pub struct LeaseStreamGauge {
    by_pid: Mutex<HashMap<u32, usize>>,
}

impl LeaseStreamGauge {
    /// 建一个空量规。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一根新流（lease 流打开即计，无论握手结局）。
    pub fn register(&self, pid: u32) {
        *self
            .by_pid
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(pid)
            .or_insert(0) += 1;
    }

    /// 注销一根流；返回该 PID 剩余在途流数（归零时条目移除）。
    pub fn release(&self, pid: u32) -> usize {
        let mut map = self
            .by_pid
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(count) = map.get_mut(&pid) else {
            return 0;
        };
        *count -= 1;
        if *count == 0 {
            map.remove(&pid);
            0
        } else {
            *count
        }
    }

    /// 某 PID 当前在途流数（观测/测试）。
    #[must_use]
    pub fn count(&self, pid: u32) -> usize {
        self.by_pid
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&pid)
            .copied()
            .unwrap_or(0)
    }
}

/// 一次善后收口的结局（断开路径把 `verified` 上 wire；其余路径进日志摘要）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AftermathOutcome {
    /// 触发本次善后的稳定 reason id（[`reason`]）。
    pub reason: &'static str,
    /// 善后是否已验证干净（干净 = 本会话无未决残留）。
    pub verified: bool,
    /// 残留稳定 id 清单（未验证时非空；[`residual_reasons`] 产）。
    pub residual_reasons: Vec<&'static str>,
    /// 痕是否已删除（只在验证通过后为 true；无痕会话 = false——无事可删）。
    pub trace_deleted: bool,
    /// 验证是否整体超时（按未验证处理；痕留 Aftermath 待下次预检/善后裁定）。
    pub timed_out: bool,
}

/// engine 兜底善后驱动（持有全部接缝：载体 / 运行时 / 日志 / 状态 / 统计 /
/// 存活性谓词 / 验证器 / core 拉起器）。
pub struct AftermathDriver {
    /// 连接门（痕载体 + 用户 SID + 命名空间源）。None = 测试/未接线（无痕保护，
    /// 分叉降级为纯 teardown + 探针验证，不触碰痕）。
    gate: Option<Arc<ConnectGate>>,
    /// 数据面运行时（teardown / 状态判定）。
    runtime: Arc<dyn TunnelRuntime>,
    /// 结构化日志（终态摘要事件）。
    log: Arc<LogSink>,
    /// 连接状态通道（Idle 终态事件携带验证结论）。
    status: Option<Arc<StatusPublisher>>,
    /// 统计相位（teardown 类善后完成后回 Idle）。
    stats: Option<Arc<StatsPublisher>>,
    /// 无头态（keep 分叉登记 + T10 拉起账本同驻；T8/T10 查询）。
    headless: Arc<HeadlessState>,
    /// core PID 存活性谓词。
    liveness: CoreLiveness,
    /// 善后验证器。
    verifier: AftermathVerifier,
    /// T10 core 拉起器接缝：Some = 拉起可用（service 形态生产装配
    /// `wts_spawner`，测试装配 fake）；None = 未装配（旧构造调用方 / 测试默认
    /// ——keep 分叉维持 T5 无头续存现状，不拉起）。
    relaunch_spawner: Option<crate::core_relaunch::CoreSpawner>,
    /// 受理期 core 会话事实（T10：拉起选会话的稳定事实源；单一活跃会话拓扑 →
    /// 单槽，最新受理覆盖）。
    core_session: Mutex<Option<CoreSessionFacts>>,
}

impl AftermathDriver {
    /// 全接缝构造（生产接缝经 [`open_process_liveness`] / [`preflight_verifier`] /
    /// [`crate::core_relaunch::wts_spawner`] 组装；测试注入假 liveness / 假
    /// verifier / fake spawner）。`relaunch_spawner = None` = 不拉起（T5 行为）。
    #[must_use]
    #[allow(clippy::similar_names, clippy::too_many_arguments)] // status/stats 是两个既有发布器；接缝按计划逐参数注入
    pub fn new(
        gate: Option<Arc<ConnectGate>>,
        runtime: Arc<dyn TunnelRuntime>,
        log: Arc<LogSink>,
        status: Option<Arc<StatusPublisher>>,
        stats: Option<Arc<StatsPublisher>>,
        liveness: CoreLiveness,
        verifier: AftermathVerifier,
        relaunch_spawner: Option<crate::core_relaunch::CoreSpawner>,
    ) -> Self {
        Self {
            gate,
            runtime,
            log,
            status,
            stats,
            headless: Arc::new(HeadlessState::default()),
            liveness,
            verifier,
            relaunch_spawner,
            core_session: Mutex::new(None),
        }
    }

    /// 无头态句柄（T8 采纳 / T10 拉起查询）。
    #[must_use]
    pub fn headless(&self) -> Arc<HeadlessState> {
        Arc::clone(&self.headless)
    }

    /// T10：受理期记录 core 会话事实（`admit_through_gate` 放行路径调用——core
    /// 活着时查 `ProcessIdToSessionId`，EOF 时不可再查）。单槽覆盖（最新受理）。
    pub fn note_core_session(&self, core_pid: u32, ui_pid: u32) {
        let facts = CoreSessionFacts {
            core_pid,
            session_id: crate::core_relaunch::process_session_id(core_pid),
            ui_pid,
        };
        if facts.session_id.is_none() {
            // 活 core 查询失败属异常（进程刚死/权限边界）——记录在案，拉起时回退
            // 活动控制台会话。
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "relaunch.session_unresolved",
                "受理期未能解析 core 会话 id（拉起时回退活动控制台会话）",
                &[("core_pid", &core_pid.to_string())],
            );
        }
        *self
            .core_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(facts);
    }

    /// T10：core 回归在场（新受理连接）——崩溃环账本归零（环已断，新会话新起点）。
    /// 由 `admit_through_gate` 的非跳过路径（新受理/自留重建；采纳跳过路径不清
    /// ——采纳的 core 本就是拉起记录内进程，其死亡仍按复死计）。
    pub fn note_core_returned(&self) {
        self.headless.reset_relaunch_ledger();
    }

    /// 受理期 core 会话事实快照（观测；拉起编排内消费同一事实源）。
    #[must_use]
    pub fn core_session_facts(&self) -> Option<CoreSessionFacts> {
        *self
            .core_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 情形①/④ 的 EOF 分叉（service 形态 lease 流 EOF、既有 owner 释放之后调用）。
    ///
    /// 判定序（层层收窄，任何一层不成立即维持既有行为不动隧道）：
    /// 1. `same_pid_streams_remaining > 0`：同 core PID 仍有其他控制流——EOF 只是
    ///    多连接中的一条断开，不是 core 死亡（EOF 细则）。
    /// 2. PID 存活复核：EOF 的 peer 仍活着 = 优雅断开/换代（新 core 将接管）——
    ///    既有"释放 owner 等新 owner"语义不变。
    /// 3. 痕主核对：痕 `core_pid` ≠ EOF peer pid（换代/另一会话的痕）——保守不动。
    /// 4. 痕状态 × 策略分叉：connecting → teardown 善后（情形④）；Live+teardown →
    ///    teardown 善后（情形①）；Live+keep → 无头续存（登记 [`HeadlessState`]，
    ///    隧道/状态/统计照跑）；痕已 Aftermath/损坏 → 不重复驱动（损坏痕留给
    ///    预检裁定）。
    ///
    /// # PID 复用注意
    ///
    /// 存活复核是 `OpenProcess(SYNCHRONIZE)+WaitForSingleObject(0)` 的 PID 级探测：
    /// EOF→复核窗口内 PID 被无关新进程复用时会被误判"存活"（保守方向——不拆隧道，
    /// 误判只是漏走一次分叉，下次 EOF/预检仍会兜底）；痕主核对把误判面进一步
    /// 收窄到「同数值 PID 复用且恰为本会话痕主」。完整的进程身份（启动时间/句柄
    /// 钉住）留给 T10 拉起机制（它天然持有进程句柄）。
    pub async fn on_owner_stream_eof(&self, eof_pid: u32, same_pid_streams_remaining: usize) {
        if same_pid_streams_remaining > 0 {
            self.log.emit(
                LogLevel::Debug,
                "engine",
                "aftermath.skipped",
                "同 core PID 仍有其他控制流在途，EOF 不视为 core 死亡",
                &[
                    ("reason", "peer_streams_remaining"),
                    ("core_pid", &eof_pid.to_string()),
                    ("remaining", &same_pid_streams_remaining.to_string()),
                ],
            );
            return;
        }
        if (self.liveness)(eof_pid) {
            self.log.emit(
                LogLevel::Debug,
                "engine",
                "aftermath.skipped",
                "core 存活（优雅断开/换代），维持既有释放等接管语义",
                &[("reason", "core_alive"), ("core_pid", &eof_pid.to_string())],
            );
            return;
        }
        // core 确死：读痕定分叉。载体不可用（gate 未接线/无痕保护会话）→ 纯
        // teardown + 验证收口（core 死亡后无人驱动善后，隧道不能无人认领地泄漏）。
        let current = self.gate.as_ref().map(|gate| {
            gate.read_current_trace(&self.log)
                .unwrap_or(TraceReadOutcome::Absent)
        });
        match current {
            None => {
                self.abandon_session(reason::CORE_EXIT_TEARDOWN).await;
            }
            Some(TraceReadOutcome::Present(record)) => {
                self.fork_present_record(record, eof_pid).await;
            }
            Some(TraceReadOutcome::Corrupt) => {
                // 损坏 = 善后未竟的**预检信号**：痕本身不触碰（store 拒改写；删除
                // 会消灭预检信号，绝不做）。但 core 确死时活隧道不能无人认领地泄漏，
                // 且留隧道会让下次连接撞上"痕在→预检→网卡 Manual"死局——teardown +
                // 无 hint 验证照做（settle 对损坏痕无痕可记/删，摘要在案）。
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "aftermath.skipped",
                    "痕损坏（善后未竟的预检信号）：痕留给预检裁定，活隧道按 legacy teardown 兜底",
                    &[("reason", "trace_corrupt"), ("core_pid", &eof_pid.to_string())],
                );
                self.abandon_session(reason::CORE_EXIT_TEARDOWN).await;
            }
            Some(TraceReadOutcome::Absent) => {
                // core 死亡 + 无痕（已收口过的会话/载体竞态）——teardown 兜底 +
                // 验证照做（无痕可删；隧道若仍在则由本路径收口）。
                self.abandon_session(reason::CORE_EXIT_TEARDOWN).await;
            }
        }
    }

    /// 痕主核对 + 痕状态×策略分叉（[`Self::on_owner_stream_eof`] 的 Present 分支）。
    ///
    /// 情形④：connecting 态 → teardown 善后（keep/teardown 都无活隧道可保）。
    /// 情形①：Live+teardown → teardown 善后；Live+keep → 无头续存 + **T10 拉起**
    /// （service 形态；owner 已由既有 EOF 块释放，隧道/状态/统计照跑、痕保持
    /// Live，采纳归 T8）。痕已 Aftermath → 不重复驱动。
    ///
    /// 痕主核对（T10 修订）：EOF peer ≠ 痕主 core_pid 且**不在 engine 拉起记录**
    /// 中才保守不动——engine 自己拉起的 core（拉起记录 pid 匹配）是第一手事实，
    /// 其复死直接延续本会话分叉（采纳路径不换痕主，否则复死永远撞痕主不符而
    /// 挡掉崩溃环判定）。
    async fn fork_present_record(&self, record: TraceRecord, eof_pid: u32) {
        let engine_spawned = self.headless.relaunched_pid_matches(eof_pid);
        if record.core_pid != eof_pid && !engine_spawned {
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "aftermath.skipped",
                "EOF peer 与痕主 core PID 不一致（换代/另一会话的痕），保守不驱动善后",
                &[
                    ("reason", "core_pid_mismatch"),
                    ("eof_pid", &eof_pid.to_string()),
                    ("trace_core_pid", &record.core_pid.to_string()),
                ],
            );
            return;
        }
        match (record.state, record.on_core_exit) {
            (TraceState::Connecting, _) => {
                self.abandon_session(reason::CONNECTING_ABORT).await;
            }
            (TraceState::Live, TraceOnCoreExit::Teardown) => {
                self.abandon_session(reason::CORE_EXIT_TEARDOWN).await;
            }
            (TraceState::Live, TraceOnCoreExit::Keep) => {
                self.headless.enter(HeadlessSession {
                    core_pid: eof_pid,
                    since_ms: unix_now_ms(),
                    policy: TraceOnCoreExit::Keep,
                    ui_pid: record.ui_pid,
                    user_sid: self
                        .gate
                        .as_ref()
                        .and_then(|gate| gate.user_sid_ref().map(str::to_owned)),
                });
                self.log.emit(
                    LogLevel::Info,
                    "engine",
                    "aftermath.headless.entered",
                    "core 死亡且策略为 keep：owner 已释放，隧道无头续存（痕保持 Live；拉起归 T10、采纳归 T8）",
                    &[
                        ("reason", "core_exit_keep"),
                        ("core_pid", &eof_pid.to_string()),
                        ("engine_mode", &format!("{:?}", record.engine_mode)),
                    ],
                );
                // T10 连接态主路径（D5）：keep 不止无头续存——engine 拉起新 core
                // （成功后新 core 经 T8 采纳握手自行接管）。
                self.relaunch_after_keep(eof_pid, &record).await;
            }
            (TraceState::Aftermath, _) => {
                self.log.emit(
                    LogLevel::Debug,
                    "engine",
                    "aftermath.skipped",
                    "痕已在善后态（其他路径正在/已经收口），不重复驱动",
                    &[
                        ("reason", "already_aftermath"),
                        ("core_pid", &eof_pid.to_string()),
                    ],
                );
            }
        }
    }

    /// T10 keep 分支的拉起编排（§2.5 连接态主路径 + 缺口②崩溃环终态）：
    ///
    /// 1. **形态判别**：按痕记录的 `engine_mode`——oneshot 提权 engine 直接 spawn
    ///    会把管理员令牌遗传给 core（违反「core 永不特权」），拉起底质（explorer
    ///    令牌复制 + `CreateProcessWithTokenW`）后置分期，keep 维持 T5 无头续存
    ///    现状（等 R2/UI 路径，计划 §2.5 拉起底质②）。
    /// 2. **崩溃环判定**（[`crash_loop_step`] 纯函数）：窗口内复死达上限 → 放弃
    ///    拉起，`abandon_session(CRASH_LOOP)` 收口（teardown 善后 + 删痕 + 摘要）。
    /// 3. **拉起**：构造参数（exe 发现 + `--control-pipe`/`--ui-pid`/`--ui-sid`，
    ///    缺口① ui_pid 传递链）→ spawner（`spawn_blocking` 承载阻塞 Win32）→
    ///    记录入账本。失败只记日志不终态（拉起失败 ≠ 崩溃环；无头续存与 R1
    ///    自愈照常，UI 侧 L1 拉起仍是备用路径）。
    async fn relaunch_after_keep(&self, eof_pid: u32, record: &TraceRecord) {
        // 形态判别（受理时落痕的形态事实）。
        if record.engine_mode != TraceEngineMode::Service {
            // TODO(T10-oneshot, 计划 §2.5 拉起底质②)：oneshot 提权 engine 需 explorer
            //   令牌复制 + CreateProcessWithTokenW 才能拉非提权 core——后置分期；
            //   在此之前 keep 在 oneshot = T5 无头续存现状（R2/UI 路径接管）。
            self.log.emit(
                LogLevel::Debug,
                "engine",
                "relaunch.skipped_oneshot",
                "oneshot 形态不拉起 core（提权 spawn 会遗传管理员令牌，违反 core 永不特权；底质后置，计划 §2.5）",
                &[("core_pid", &eof_pid.to_string())],
            );
            return;
        }
        // 拉起器未装配（旧构造/测试默认）——维持 T5 无头续存，不拉起。
        let Some(spawner) = self.relaunch_spawner.clone() else {
            self.log.emit(
                LogLevel::Debug,
                "engine",
                "relaunch.skipped_no_spawner",
                "core 拉起器未装配，无头续存维持 T5 现状",
                &[("core_pid", &eof_pid.to_string())],
            );
            return;
        };
        // 崩溃环判定（纯函数；真实时钟）。
        let decision = self.headless.with_ledger(|ledger| {
            crash_loop_step(
                ledger,
                eof_pid,
                unix_now_ms(),
                CRASH_LOOP_MAX_RELAUNCHES,
                CRASH_LOOP_WINDOW_MS,
            )
        });
        let CrashLoopDecision::Relaunch { crash_count } = decision else {
            let count = match decision {
                CrashLoopDecision::Terminal { crash_count } => crash_count,
                CrashLoopDecision::Relaunch { .. } => unreachable!("matched above"),
            };
            // 缺口②终态：放弃拉起 → 转善后（teardown 语义 + 删痕 + 摘要留日志）。
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "relaunch.crash_loop_terminal",
                "core 崩溃环终态：窗口内复死达上限，放弃拉起并转善后（teardown+验证+删痕）",
                &[
                    ("crash_count", &count.to_string()),
                    ("max_relaunches", &CRASH_LOOP_MAX_RELAUNCHES.to_string()),
                    ("window_ms", &CRASH_LOOP_WINDOW_MS.to_string()),
                    ("core_pid", &eof_pid.to_string()),
                ],
            );
            self.abandon_session(reason::CRASH_LOOP).await;
            return;
        };
        // 拉起参数（缺口①：ui_pid 传递链——trace 受理事实）。
        let Some(user_sid) = self
            .gate
            .as_ref()
            .and_then(|gate| gate.user_sid_ref().map(str::to_owned))
        else {
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "relaunch.failed",
                "发起用户 SID 缺失，无法构造拉起参数（无头续存维持；UI 侧 L1 仍是备用路径）",
                &[("reason", "user_sid_missing"), ("core_pid", &eof_pid.to_string())],
            );
            return;
        };
        let Some(exe) = crate::core_relaunch::core_exe_path() else {
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "relaunch.failed",
                "engine 同目录未发现 exv-core 可执行，无法拉起（无头续存维持；UI 侧 L1 仍是备用路径）",
                &[("reason", "core_exe_not_found"), ("core_pid", &eof_pid.to_string())],
            );
            return;
        };
        // 会话 id：受理期记录优先（core 原会话——多用户/断开场景比活动控制台稳）。
        let session_id = self
            .core_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .filter(|facts| facts.core_pid == eof_pid)
            .and_then(|facts| facts.session_id);
        let request = crate::core_relaunch::CoreRelaunchRequest {
            args: crate::core_relaunch::core_relaunch_args(&user_sid, record.ui_pid),
            exe,
            user_sid,
            ui_pid: record.ui_pid,
            session_id,
        };
        self.log.emit(
            LogLevel::Info,
            "engine",
            "relaunch.attempted",
            "无头续存期拉起新 core（用户令牌，core 永不特权；拉起后经 T8 采纳握手接管）",
            &[
                ("dead_core_pid", &eof_pid.to_string()),
                ("ui_pid", &record.ui_pid.to_string()),
                ("crash_count", &crash_count.to_string()),
                ("session_id_known", &session_id.is_some().to_string()),
            ],
        );
        let outcome = tokio::task::spawn_blocking(move || spawner(request))
            .await
            .map_err(|e| format!("spawner join: {e}"))
            .and_then(|result| result);
        match outcome {
            Ok(spawned) => {
                self.headless.with_ledger(|ledger| {
                    ledger.spawned.push(spawned);
                });
                self.log.emit(
                    LogLevel::Info,
                    "engine",
                    "relaunch.succeeded",
                    "新 core 已拉起（等待其经 T8 采纳握手认领会话）",
                    &[
                        ("pid", &spawned.pid.to_string()),
                        ("launched_at_ms", &spawned.launched_at_ms.to_string()),
                        ("ui_pid", &record.ui_pid.to_string()),
                    ],
                );
            }
            Err(detail) => {
                self.log.emit(
                    LogLevel::Warn,
                    "engine",
                    "relaunch.failed",
                    "core 拉起失败（无头续存与 R1 自愈维持；UI 侧 L1 仍是备用路径）",
                    &[("detail", &detail), ("core_pid", &eof_pid.to_string())],
                );
            }
        }
    }

    /// 会话终态善后（§2.4 删除权②③的终态钩子——T6a/T10 调用；①④复用同一主体）：
    /// cancel 在途组装 → teardown → 痕 Aftermath → 预检验证 → 终态摘要 →（干净）删痕
    /// → Idle 终态事件（携带验证结论）。
    ///
    /// `reason` 用 [`reason`] 的稳定 id（崩溃环 = [`reason::CRASH_LOOP`]，
    /// R1 终态 = [`reason::R1_TERMINAL`]；core 死亡/connecting 中止/oneshot 退出
    /// 经各自入口复用本主体）。
    pub async fn abandon_session(&self, reason: &'static str) -> AftermathOutcome {
        let record = self.current_record();
        // 1. teardown：cancel 在途组装（有界等待段边界自清理）→ teardown（幂等）。
        if let Err(error) = self.teardown_with_cancel().await {
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "aftermath.teardown_failed",
                "teardown 硬失败（验证将如实上报残留；adapter 由退出清理兜底）",
                &[("reason", reason), ("detail", &error)],
            );
        }
        // 2. 验证收口（Aftermath 标记 → 验证 → 摘要 → 删痕）。
        let outcome = self.settle(reason, record, &[]).await;
        // 3. Idle 终态事件（空 operation_id——终态无外部操作关联，镜像
        //    heartbeat_shutdown 的发布形态；携带验证结论 EXV_UNFREEZE tag 7）。
        if let Some(status) = &self.status {
            status.publish(StatusEvent::idle_with_aftermath(
                Vec::new(),
                outcome.verified,
            ));
        }
        outcome
    }

    /// 断开路径善后验证（core 删除权前提"善后已验证"；`StopTunnel` 的 disconnect
    /// 完成后调用，**不再 teardown**——减负断开已按 D13 完成）。
    ///
    /// 干净 → 痕 Aftermath → 删 + `verified=true`（调用方挂到 stop 的 Idle 终态
    /// 事件上 wire）；残留/超时 → 痕留 Aftermath + `verified=false`（下次连接的
    /// 连接门预检阻断）。
    ///
    /// D12 网卡豁免：disconnect 成功后 engine 处于 Paused 且持有 adapter 句柄——
    /// 痕记录的 `adapter_names` 是**设计内存活**，验证时豁免（teardown 关网卡的
    /// 语义留给退出/终态路径）。
    pub async fn verify_disconnect_aftermath(&self) -> AftermathOutcome {
        let record = self.current_record();
        let expected_retained: Vec<String> = if self.runtime.is_paused() {
            record
                .as_ref()
                .map(|record| record.adapter_names.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.settle(reason::NORMAL_DISCONNECT, record, &expected_retained)
            .await
    }

    /// oneshot 随行退出收口（main.rs 在 `shutdown_after_trigger` 的 teardown 之后
    /// 调用；T10 前过渡——keep 无头续存在 oneshot 形态依赖拉起/心跳策略化解耦）。
    ///
    /// teardown 已由退出序列完成，此处只做：痕 Aftermath → 复验（无网卡豁免——
    /// 退出 teardown 已关 adapter，stop 路径留下的残留可能已收敛）→ 摘要 → 删痕。
    /// 不发布状态事件（`heartbeat_shutdown` 已发 Idle；进程即将退出）。
    pub async fn on_engine_exit(&self) -> AftermathOutcome {
        let record = self.current_record();
        self.settle(reason::ONESHOT_ENGINE_EXIT, record, &[]).await
    }

    /// 善后收口主体：痕 → Aftermath → 有界验证 → 终态摘要 →（干净）删痕 →
    /// stats 回 Idle → 清无头态。
    async fn settle(
        &self,
        reason: &'static str,
        record: Option<TraceRecord>,
        expected_retained_adapters: &[String],
    ) -> AftermathOutcome {
        let started = Instant::now();
        // 痕 → Aftermath（验证前落——Aftermath 即"善后进行中/未竟"的信号；失败只
        // 告警，验证继续，残留由验证如实上报）。
        if let Some(gate) = self.gate.as_ref()
            && record.is_some()
        {
            gate.mark_aftermath(&self.log);
        }
        // 有界验证（超时按未验证；痕留 Aftermath 待下次预检/善后裁定）。验证器
        // 拿记录克隆（settle 后续仍要据 `record` 判删痕）。
        let (verified, residual, timed_out) = match tokio::time::timeout(
            VERIFY_BOUND,
            (self.verifier)(record.clone()),
        )
        .await
        {
            Ok(report) => {
                let residual = residual_reasons(&report, expected_retained_adapters);
                (residual.is_empty(), residual, false)
            }
            Err(_) => (false, vec!["verify_timeout"], true),
        };
        // 终态摘要（痕迹即证据；在删痕**之前**落——痕删除是收口的最后一步动作，
        // 摘要先于动作成案）。`has_trace` 字段区分"删痕"与"无痕可删"（gate 未接线
        // /损坏痕留给预检——后者绝不删）。
        let has_trace = record.is_some();
        if verified {
            self.log.emit(
                LogLevel::Info,
                "engine",
                "aftermath.completed",
                "善后完成并复验干净（core/UI 可清除本地会话状态）",
                &[
                    ("reason", reason),
                    ("has_trace", &has_trace.to_string()),
                    ("elapsed_ms", &started.elapsed().as_millis().to_string()),
                ],
            );
        } else {
            self.log.emit(
                LogLevel::Warn,
                "engine",
                "aftermath.residual",
                "善后完成但复验存在未决残留，痕留 Aftermath（下次连接预检将阻断）",
                &[
                    ("reason", reason),
                    ("residual_ids", &residual.join(",")),
                    ("timed_out", &timed_out.to_string()),
                    ("elapsed_ms", &started.elapsed().as_millis().to_string()),
                ],
            );
        }
        // 删痕 LAST（只在验证通过后；无痕会话/损坏痕 = false——无事可删）。
        let trace_deleted = verified
            && has_trace
            && self
                .gate
                .as_ref()
                .is_some_and(|gate| gate.delete_aftermath_trace(&self.log));
        // 会话终局：清无头态与拉起账本（T10）+ stats 回 Idle（teardown 类善后
        // 完成后引擎无连接）。
        self.headless.clear_all();
        if let Some(stats) = &self.stats {
            stats.registry().set_phase(generated::StatsPhase::Idle);
        }
        AftermathOutcome {
            reason,
            verified,
            residual_reasons: residual,
            trace_deleted,
            timed_out,
        }
    }

    /// 当前痕记录（Present 才 Some；损坏/缺失/载体不可用 → None，原因已按需记录）。
    fn current_record(&self) -> Option<TraceRecord> {
        self.gate.as_ref().and_then(|gate| {
            match gate.read_current_trace(&self.log) {
                Some(TraceReadOutcome::Present(record)) => Some(record),
                // 损坏 = 预检信号：善后路径不覆盖不删除，交预检裁定。
                Some(TraceReadOutcome::Corrupt) => {
                    self.log.emit(
                        LogLevel::Warn,
                        "engine",
                        "aftermath.skipped",
                        "痕损坏（善后未竟的预检信号），善后不触碰痕，交下次连接预检裁定",
                        &[("reason", "trace_corrupt")],
                    );
                    None
                }
                Some(TraceReadOutcome::Absent) | None => None,
            }
        })
    }

    /// cancel → 有界等待在途组装段边界自清理（5s/50ms，镜像 `stop_tunnel`）→ teardown。
    async fn teardown_with_cancel(&self) -> Result<(), String> {
        self.runtime.cancel();
        let runtime = Arc::clone(&self.runtime);
        tokio::task::spawn_blocking(move || {
            let polls = u64::try_from(
                ASSEMBLY_CANCEL_WAIT.as_millis() / ASSEMBLY_CANCEL_POLL.as_millis(),
            )
            .unwrap_or(100);
            for _ in 0..polls {
                if !runtime.is_assembling() {
                    break;
                }
                std::thread::sleep(ASSEMBLY_CANCEL_POLL);
            }
            runtime.teardown()
        })
        .await
        .map_err(|e| format!("teardown join: {e}"))?
    }
}

/// 生产接缝构造：`OpenProcess` 存活性谓词（`SYNCHRONIZE` + 0ms 探测等待；与
/// main.rs core 句柄等待同一底质）。
///
/// 打不开 = 不在场（死亡复核宁可保守触发分叉——engine 为 SYSTEM/提权用户，对
/// 用户态 core 进程恒可打开，打不开的实际原因几乎必然是进程已退出）。
#[must_use]
pub fn open_process_liveness() -> CoreLiveness {
    Arc::new(|pid: u32| {
        // SAFETY: 以 SYNCHRONIZE 打开目标 PID 的进程句柄；句柄归本次探测所有。
        let Ok(handle) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
            return false;
        };
        // SAFETY: 0ms 探测等待——句柄 signaled = 进程已退出；WAIT_TIMEOUT = 存活。
        let waited = unsafe { WaitForSingleObject(handle, 0) };
        // SAFETY: 句柄归本次探测所有，恰好关闭一次。
        unsafe {
            let _ = CloseHandle(handle);
        }
        waited == WAIT_TIMEOUT
    })
}

/// 生产接缝构造：预检验证器（`run_preflight_for` 包装；`user_sid`/命名空间取自门）。
#[must_use]
pub fn preflight_verifier(
    user_sid: Option<String>,
    campus_namespaces: Vec<String>,
) -> AftermathVerifier {
    Arc::new(move |hint| {
        let user_sid = user_sid.clone();
        let namespaces = campus_namespaces.clone();
        Box::pin(async move {
            preflight::run_preflight_for(&PreflightInputs {
                record_hint: hint.as_ref(),
                user_sid: user_sid.as_deref(),
                campus_namespaces: &namespaces,
            })
            .await
        })
    })
}

/// 善后残留评估：预检报告 + 会话语境 → 未决残留稳定 id（空 = 验证干净）。
///
/// 与连接门 `blocking_reasons` 的口径差异（见模块级文档）：
/// - **外来 NRPT 冲突不算残留**（他牌活系统事实，非本会话残留；下次连接预检
///   自会看见并阻断）；
/// - **网卡域**：命中 `expected_retained_adapters`（D12 设计内保留，Paused 态
///   engine 持有句柄）的发现豁免，其余 `adapter_orphan`；
/// - **探测 Timeout/Error 域**：无法宣称验证干净 → `verify_degraded`（连接门
///   不据此拦连接，但善后"已验证"必须诚实）；
/// - 清理失败/未收敛照算（`cleanup_failed` / `cleanup_unresolved`）。
#[must_use]
pub fn residual_reasons(
    report: &PreflightReport,
    expected_retained_adapters: &[String],
) -> Vec<&'static str> {
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut push = |reason: &'static str| {
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    };
    for domain in &report.domains {
        if domain.domain == DomainKind::Adapter {
            // 每条发现必须命中保留清单（detail 含「网卡 <名>（」——由本 crate 的
            // adapter_findings 格式化产出，格式契约由单测钉住）。
            let unexpected = domain.findings.iter().any(|finding| {
                !expected_retained_adapters
                    .iter()
                    .any(|name| finding.detail.contains(&format!("网卡 {name}（")))
            });
            if unexpected {
                push("adapter_orphan");
            }
            continue;
        }
        if domain
            .findings
            .iter()
            .any(|finding| finding.policy == FindingPolicy::Manual)
        {
            // 非网卡域的 Manual（当前域清单只有网卡产 Manual；防御未来扩型）。
            push("adapter_manual");
        }
        let had_cleanup = domain
            .findings
            .iter()
            .any(|finding| finding.policy == FindingPolicy::Cleanup);
        if had_cleanup && domain.post_cleanup_status == Some(DomainStatus::Findings) {
            push("cleanup_unresolved");
        }
        if matches!(
            domain.status,
            DomainStatus::Timeout | DomainStatus::Error(_)
        ) {
            push("verify_degraded");
        }
    }
    for action in &report.cleanup {
        if matches!(action.outcome, CleanupOutcome::Failed(_)) {
            push("cleanup_failed");
        }
    }
    reasons
}

/// Unix 毫秒（时钟回拨等异常退化为 0；与 `connection_trace` 同款约定）。
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------
// 单元测试：temp 目录真 store + FakeTunnelRuntime + 假 liveness/verifier。
// 预检本体行为在 preflight.rs；这里只测善后策略分叉与收口时序。
// ---------------------------------------------------------------------------
