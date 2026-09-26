
// 产品化（用户拍板 2026-08-19）：host 为 GUI 产品进程，隐藏控制台黑框。
// 注：workspace [profile.release] rustc-link-arg-bins=/SUBSYSTEM:WINDOWS 不生效
//（rustc 自身再发 /SUBSYSTEM:CONSOLE 覆盖），必须以本属性声明。
#![windows_subsystem = "windows"]

//! Win32 非特权 host（core 进程）真实入口（P5 补尾）。
//!
//! 本二进制由 Tauri 宿主（UI）spawn（P4-b `core_process.rs` 契约），执行
//! **UI → core → engine** 两跳进程架构中 core 的全部生命周期：
//!
//! 1. **CLI 解析**：`--control-pipe <name>`（`KernelControl` 服务管道，名按 UI PID
//!    唯一 `\\.\pipe\exv-core-<ui_pid>`）、`--ui-pid <pid>`（UI 进程 PID，身份记录）、
//!    `--ui-sid <sid>`（UI 用户 SID——管道 DACL + accept 后 peer 验证；缺省回退当前
//!    用户 SID，同用户拓扑下相等）。
//! 2. **compose（按需拉起模型，2026-09-08 计划）**：打开共享日志聚合器 → 组合
//!    composition（占位 engine 身份——真实身份随首次 provision/respawn 重建）→
//!    组装 `KernelControlService` + 空态 `EngineSupervisor` + detached engine 槽 →
//!    组装 `CoreRuntime`。**core 启动全程零 UAC、零特权子进程**；oneshot engine 由
//!    首次业务连接经 `EngineProvisioner` 提权拉起（`ShellExecuteExW(runas)` + 拓扑
//!    门禁：engine 必须 elevated），服务形态由 SCM 承载。
//! 3. **serve**：`serve_kernel_control_pipe` 建 UI 控制面管道（DACL = SYSTEM +
//!    UI SID）→ accept 一个 UI 连接 → 验证 UI peer（client pid + user SID +
//!    account name；并回填 provisioner 的 UI peer）→ gate 授权 → 拉起 engine
//!    事件/统计转发器 → 返回 [`UiKernelControlHandles`] 接进 `CoreRuntime`。
//! 4. **运行**：`CoreRuntime::run` 主循环——等 UI 彻底退出（O3 强绑定：UI **进程**退出
//!    → 传输层进程监视触发 `on_ui_exited` → 停机；tonic serve 任务不充当"UI 断开"
//!    信号——单元素流 accept 后即返回、连接任务 detached 继续服务）或显式停机；
//!    运行期监听 engine 掉线（liveness）→ 按维护形态分流（oneshot 才 respawn；
//!    service 形态归 SCM）→ 驱动 `composition.on_helper_link_terminal`
//!    （engine 死亡 ≠ core 死亡）。
//! 5. **停机**：`shutdown_core` 有序停机（先 RPC waiter 取消 → engine 发退出包
//!    （`StopTunnel` 业务停机）后即返 → `composition.exit()`）→ core 退出；engine 由
//!    退出包 / core 进程句柄 / 心跳（P2）三重保证随 core 退出（UI 只等 core）。
//!
//! **engine 不得遗留（O3）**：engine 子进程句柄归 `EngineSupervisor`/`EngineChild`
//! 所有，任何失败路径 drop 即强制终止已拉起的 engine。

use std::sync::Arc;

use exv_core::composition::compose_nonprivileged_host;
use exv_core::crash_recovery::{
    CrashRecovery, GrpcClientConnector, ProductSupervisorFactory, SystemResidueProbe,
};
use exv_core::engine_lifecycle::{EngineSlot, EngineSupervisor};
use exv_core::engine_provisioner::EngineProvisioner;
use exv_core::kernel_control_service::{EventBusSelfHealReporter, KernelControlService};
use exv_core::kernel_control_transport::{lookup_account_name, serve_kernel_control_pipe};
use exv_core::log_aggregator::LogAggregator;
use exv_core::process_lifecycle::ENGINE_ADAPTER_NAME;
use exv_core::shutdown::{CoreRuntime, UiLifetime};
use exv_vpn_win32_ipc::peer_auth::{VerifiedPipePeer, current_user_sid};

// R2 的 oneshot 就绪轮询常量（ONESHOT_READY_TIMEOUT / ONESHOT_READY_POLL /
// ONESHOT_KEEPALIVE_CONFIRM_TIMEOUT）随「启动即拉 engine」一并移入
// `engine_provisioner`（按需拉起路径的 provision 就绪等待）。

/// 启动参数用法说明（`--help` / 参数错误时打印）。
const USAGE: &str = "\
EXV core (exv-core)

用法: exv-core --control-pipe <name> --ui-pid <pid> [--ui-sid <sid>]

必需参数（P4-b core_process.rs 契约）:
  --control-pipe <name>  KernelControl 服务管道名（\\\\.\\pipe\\exv-core-<ui_pid>）
  --ui-pid <pid>         UI（Tauri 宿主）进程 PID
  --ui-sid <sid>         UI 进程用户 SID（缺省回退当前用户 SID；同用户拓扑下相等）
";

/// core 进程 CLI 参数（P4-b `core_process.rs` spawn 契约的 core 侧解析）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreArgs {
    /// `KernelControl` 服务管道名（`serve_kernel_control_pipe`；名按 UI PID 唯一）。
    pub control_pipe: String,
    /// UI 进程 PID（core 日志/身份记录）。
    pub ui_pid: u32,
    /// UI 进程用户 SID（管道 DACL + accept 后 peer 验证）；`None` = UI 未传
    /// （core 回退当前用户 SID，同用户拓扑下相等）。
    pub ui_sid: Option<String>,
}

impl CoreArgs {
    /// 解析 CLI 参数（`--control-pipe`/`--ui-pid`/`--ui-sid`）。
    ///
    /// 未知参数忽略（前向兼容）；`--help`/`-h` 返回 `Err`（打印用法）。`--ui-sid`
    /// 缺省允许（UI 可能省略——core 回退当前用户 SID）。
    ///
    /// # Errors
    /// 必需参数缺失 / `--ui-pid` 非数字 → 携带原因的字符串（同时作为 usage 提示）。
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut control_pipe: Option<String> = None;
        let mut ui_pid: Option<u32> = None;
        let mut ui_sid: Option<String> = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--control-pipe" => control_pipe = args.next(),
                "--ui-pid" => ui_pid = args.next().and_then(|v| v.parse::<u32>().ok()),
                "--ui-sid" => ui_sid = args.next(),
                "--help" | "-h" => return Err(USAGE.to_string()),
                _ => {} // 未知参数忽略（前向兼容）。
            }
        }
        let control_pipe = control_pipe
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "--control-pipe <name> is required".to_string())?;
        let ui_pid = ui_pid.ok_or_else(|| "--ui-pid <pid> is required (numeric)".to_string())?;
        let ui_sid = ui_sid.filter(|s| !s.is_empty());
        Ok(Self {
            control_pipe,
            ui_pid,
            ui_sid,
        })
    }

    /// 生效的 UI 用户 SID：显式 `--ui-sid` 优先；缺省回退当前进程用户 SID
    /// （core 与 UI 同用户拓扑，两者相等）。
    ///
    /// # Errors
    /// `--ui-sid` 缺失且当前用户 SID 无法解析 → 携带原因的字符串。
    pub fn effective_ui_sid(&self) -> Result<String, String> {
        if let Some(sid) = &self.ui_sid {
            return Ok(sid.clone());
        }
        current_user_sid()
            .ok_or_else(|| "--ui-sid missing and current user SID unresolvable".to_string())
    }
}

/// 构造 composition 绑定的 engine peer 身份（PID + 用户 SID + account name）。
///
/// engine 由 core 提权拉起（唯一特权进程），其身份经 gRPC 双向认证（pid+SID）核实；
/// account name 供 composition 的 helper 绑定语义与 gate 授权的身份事实使用（任何
/// 解析失败返回空串——fail closed 由 gate 在需要处执行）。
#[must_use]
pub fn engine_peer_for(pid: u32, user_sid: &str) -> VerifiedPipePeer {
    let account_name = lookup_account_name(user_sid);
    VerifiedPipePeer {
        process_id: pid,
        user_sid: user_sid.to_string(),
        logon_sid: None,
        account_name,
    }
}

/// core 进程主路径（可测单元：参数解析/compose 顺序；进程级由集成覆盖）。
///
/// 返回进程退出码：`0` = 正常有序停机；`1` = 启动失败（engine 拉起/连接/UI 控制面
/// 建立失败）。任何失败路径下已拉起的 engine 由 `EngineSupervisor` 的 Drop 兜底终止
/// （O3：engine 不得遗留）。
pub async fn run_core(args: CoreArgs) -> i32 {
    // 1. 生效的 UI SID（管道 DACL + peer 验证的输入）。
    let ui_sid = match args.effective_ui_sid() {
        Ok(sid) => sid,
        Err(e) => {
            eprintln!("core: {e}");
            return 1;
        }
    };

    // 2. 共享日志聚合器（product 日志通道 = 磁盘聚合文件；UI 经 logs.list 拉取）。
    let logs = match LogAggregator::open_default() {
        Ok(logs) => logs,
        Err(e) => {
            eprintln!("core: open aggregated log failed: {e}");
            return 1;
        }
    };
    let logs = Arc::new(logs);
    let _ = logs.append_core(
        "info",
        "core",
        "core.start.booting",
        &format!(
            "core booting: ui_pid={} control_pipe={}",
            args.ui_pid, args.control_pipe
        ),
        &std::collections::BTreeMap::new(),
    );

    // 3. （2026-09-08 计划批 1）core 启动**不再提权拉起 engine**。engine 槽以
    //    detached 占位构造、supervisor 空态——core 启动全程零 UAC；oneshot engine 由
    //    首次业务连接经 [`EngineProvisioner`] 按需提权拉起（UAC 归属该次连接），服务
    //    形态由 SCM 承载（存在性互斥：core 不为它拉任何常驻 engine）。
    let supervisor = EngineSupervisor::empty();
    let engine_slot = EngineSlot::new_detached();
    let user_sid = current_user_sid().unwrap_or_else(|| ui_sid.clone());
    let _ = logs.append_core(
        "info",
        "core",
        "core.start.lazy_engine",
        "core booting without engine; oneshot engine provisioned on first business request",
        &std::collections::BTreeMap::new(),
    );

    // 4. 组合：composition（占位 engine 身份）+ KernelControlService + CoreRuntime。
    //    占位 peer（pid 0）：composition 需要一份初始身份事实才能服务 UI 快照等只读
    //    路径；首次 provision/respawn 会以真实 engine peer 重建（rebind_composition）。
    let engine_peer = engine_peer_for(0, &user_sid);
    let composition = match compose_nonprivileged_host(&engine_peer) {
        Ok(composition) => composition,
        Err(e) => {
            let _ = logs.append_core(
                "error",
                "core",
                "core.compose.failed",
                &format!("composition failed: {e:?}"),
                &std::collections::BTreeMap::new(),
            );
            eprintln!("core: composition failed: {e:?}");
            return 1;
        }
    };
    // 确保 key.bin 存在：全新安装时 host 启动即初始化，避免 config_set 保存密码静默
    // 失败（password key unavailable）→ connect KeyMissing（dbg-auth 2026-08-23 根因 #1）。
    let _ = exv_vpn_win32_config::ExvConfig::ensure_key(&exv_vpn_win32_config::config_dir());
    // 服务持共享日志聚合器；main 侧保留一份 Arc 供停机结果落盘（UI 可观测全生命周期）。
    let (shared_composition, mut service) = KernelControlService::from_composition(
        composition,
        engine_slot.clone(),
        exv_vpn_win32_config::config_dir(),
        Arc::clone(&logs),
    );
    // UI 生命周期信号（O3 强绑定）：CoreRuntime 与 serve 传输层共享同一事实——传输层
    // 的 UI 进程退出监视触发 on_ui_exited，run 的 select 等待同一信号。
    let ui = UiLifetime::new();
    // 2026-09-05 host 自愈进展（计划 §4.3）：自愈阶段显式发布的事件总线（服务在 serve
    // 前先克隆一份——service 随后移入 serve_kernel_control_pipe）。
    let self_heal_events = service.events();
    let mut runtime = CoreRuntime::new(ui.clone(), Arc::clone(&shared_composition), supervisor);

    // 4.5 按需拉起编排接线（2026-09-08 计划批 2）：provisioner 与 CoreRuntime 共享
    //     supervisor 互斥句柄；维护形态句柄供 run 循环 respawn 分流（批 3）；槽换点
    //     通知用于 provision 后重新获取 liveness。
    let ui_peer_cell = Arc::new(std::sync::RwLock::new(None::<VerifiedPipePeer>));
    let provisioner = Arc::new(EngineProvisioner::new(
        runtime.supervisor_handle(),
        engine_slot.clone(),
        Arc::clone(&shared_composition),
        user_sid.clone(),
        Arc::clone(&logs),
        Arc::clone(&ui_peer_cell),
        Arc::new(ProductSupervisorFactory),
        Arc::new(GrpcClientConnector),
        Arc::new(SystemResidueProbe {
            adapter_name: ENGINE_ADAPTER_NAME.to_string(),
        }),
    ));
    service.set_engine_provisioner(Arc::clone(&provisioner));
    runtime.set_slot_swaps(engine_slot.subscribe_swaps());
    runtime.set_mode_gauge(service.selected_mode_handle());

    // 5. serve UI 控制面：accept UI → 验证 peer → gate 授权（并回填 provisioner 的
    //    UI peer）→ UI 进程退出监视 → 拉起事件/统计转发器。
    let handles = match serve_kernel_control_pipe(&args.control_pipe, &ui_sid, service, ui).await {
        Ok(handles) => handles,
        Err(e) => {
            eprintln!("core: serve kernel control pipe failed: {e}");
            return 1;
        }
    };
    // serve 内部已在验证 UI 后回填 provisioner 的 ui_peer；此处幂等兜底（双通道防窗口）。
    provisioner.set_ui_peer(handles.ui_peer.clone());
    runtime.set_serve_task(handles.serve_task);
    runtime.set_forwarder_task(handles.forwarder);
    runtime.set_stats_forwarder_task(handles.stats_forwarder);
    runtime.set_logs_forwarder_task(handles.logs_forwarder);
    // C3a 自动重连 worker（host 侧重连驱动；停机先中止释放服务克隆引用）。
    runtime.set_reconnect_worker_task(handles.reconnect_worker);
    // P2 有界存留：KeepAlive 心跳 tick（engine 侧 15s 未收到即自清理+自退出）。
    runtime.set_keepalive_ticker_task(handles.keepalive_ticker);

    // P3 崩溃自愈：engine 掉线（liveness 翻 false）→ respawn 全链路——新提权拉起 → 新
    // client → composition 身份重建（engine PID 变）→ 回 Idle；不自动重连（用户再点连接
    // 走正常登录）。
    let recovery = CrashRecovery::new(
        Arc::new(ProductSupervisorFactory),
        Arc::new(GrpcClientConnector),
        Arc::new(SystemResidueProbe {
            adapter_name: ENGINE_ADAPTER_NAME.to_string(),
        }),
        engine_slot.clone(),
        Arc::clone(&shared_composition),
        handles.ui_peer,
        user_sid,
    );
    runtime.set_crash_recovery(recovery);

    // 2026-09-05 host 自愈进展（计划 §4.3）：respawn 阶段变化 → EventBus self_heal
    // lane + kernel.selfheal.* 结构化日志 + 相位快照显式发布（自愈窗口内没有自然事件，
    // 不显式发布则 UI 停留「处理中」——§1.3 缺陷修复）。
    runtime.set_self_heal_reporter(Arc::new(EventBusSelfHealReporter::new(
        Arc::clone(&self_heal_events),
        Arc::clone(&logs),
    )));
    runtime.set_self_heal_events(self_heal_events);

    // 7. 运行：等 UI 退出 / 显式停机 → 有序停机（发退出包后即返；engine 由三重保证
    //    随 core 退出，UI 只等 core）。
    let outcome = runtime.run().await;
    let summary = format!(
        "engine={:?} stop_requests={} rpc_waiter_cancellations={} teardown_started={}",
        outcome.engine,
        outcome.stop_requests,
        outcome.rpc_waiter_cancellations,
        outcome.teardown_started,
    );
    eprintln!("core: shutdown complete: {summary}");
    let _ = logs.append_core(
        "info",
        "core",
        "core.shutdown.complete",
        &summary,
        &std::collections::BTreeMap::new(),
    );
    0
}

#[tokio::main]
async fn main() {
    let args = match CoreArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let code = run_core(args).await;
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// 单元测试：参数解析契约（纯）+ UI SID 回退 + engine peer 身份组装。
// 进程级（拉起 engine → serve 管道 → 连接 → 停机）需真实 engine bin + 提权，
// 由外部集成验证（见任务报告）。
// ---------------------------------------------------------------------------
