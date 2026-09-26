
// 产品化（2026-08-19，随 host 一致）：engine 为特权产品进程，隐藏控制台黑框。
#![windows_subsystem = "windows"]

//! 特权 **engine** 进程二进制（生产控制面 = HelperControl gRPC，P1-b）。
//!
//! 被 core（普通权限，`exv-core`）经 `ShellExecuteExW(runas)` 提权拉起
//! （`engine_bin_path` 定位本二进制 = `exv-engine(.exe)`），是两进程架构中的
//! **唯一特权进程**（建 Wintun adapter / 写路由 / 网络设置 / 认证 / CSTP / 数据面）。
//!
//! 本二进制是 `exv-engine` crate（生产 engine 库）的真实入口：解析 core 传
//! 的控制面参数（对齐 `EngineControlGrpcClient::connect` 的管道 + 双向 peer 认证），
//! 组装 [`HelperControlService`]，经 `grpc_transport::serve_named_pipe` 建控制面
//! Named Pipe（DACL = SYSTEM + core 用户 SID）→ accept 一个 core 连接 → 双向认证 →
//! serve HelperControl gRPC。
//!
//! **进程生命周期**（D1 解耦：engine 生命周期 = core 生命周期，业务停机与进程退出解耦）：
//! - 业务停机：core 发 `StopTunnel` → [`HelperControlService`] 拆隧道 + publish Idle →
//!   engine 回 Idle **常驻**（进程不退出；再连直接复用，热启动免重复提权）。
//! - 进程退出两类触发 + 双保险：core **主动关停 / 崩溃 / kill**（D2 方案 A：
//!   `OpenProcess` + `WaitForSingleObject` 句柄 signaled → engine 随行——即时兜底）与
//!   **心跳超时**（P2 有界存留：core 存活但不发心跳 / 句柄路径故障 → `KeepAlive` 15s
//!   未收到 → 自清理 + 自退出——硬时间界兜底）。**两退出路径都无条件走完整序**
//!   （P3 折叠项：cancel → 有界等待 → teardown → post Idle → 300ms flush → 退）——
//!   进程句柄路径下 core 正常停机已完成业务 teardown（二次 teardown 幂等，`live.take`
//!   为 None → Ok），但 core 在 connect 中/已连接时被杀（无 StopTunnel 业务停机）必须
//!   由本路径兜底撤网卡/路由（判据 4：任意时刻被杀 0 路由残留）。
//!
//! 与 acceptance `exv-win32-engine`（legacy JSON frame 命令循环）的差异：本二进制是
//! 产品 engine，用 proto gRPC 控制面（wire 决策：gRPC 是唯一 wire 权威）；不接收
//! `--log-pipe`（产品日志通道是 gRPC `StreamLogs`，经 `HelperControlService` 的
//! `LogSink` 推送）。

use std::ffi::OsString;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use exv_engine::grpc_server::HelperControlService;
use exv_engine::grpc_transport::{
    serve_named_pipe, serve_named_pipe_loop, AcceptLoopOutcome, ControlPlaneReady,
};
use exv_engine::heartbeat::{
    heartbeat_shutdown, heartbeat_timeout_watcher, HEARTBEAT_TIMEOUT_MS,
};
use exv_engine::log_sink::{LogLevel, LogSink};
use exv_engine::service::{EngineExitForm, SERVICE_NAME};
use exv_engine::status::StatusPublisher;
use exv_engine::tunnel_runtime::TunnelRuntime;
use exv_vpn_win32_ipc::peer_auth::current_user_sid;
use exv_vpn_win32_ipc::service_key::read_service_psk;
use exv_vpn_wire::generated::helper_control_server::HelperControlServer;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_ACCESS_RIGHTS,
};
use windows_service::define_windows_service;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

/// SYNCHRONIZE（0x00100000）：等待 core 进程句柄退出所需的最小访问权（OpenProcess）。
const SYNCHRONIZE: PROCESS_ACCESS_RIGHTS = PROCESS_ACCESS_RIGHTS(0x0010_0000);

/// windows `HANDLE`（`*mut c_void`）非 `Send` 的等待封装：进程句柄是线程安全的句柄值，
/// 可跨线程 `WaitForSingleObject`——`unsafe impl Send` 仅用于把句柄移入 `spawn_blocking`
/// 等待线程（等待与关闭在同一线程串行完成，无并发关闭）。
///
/// 注意：必须经 [`WaitHandle::wait_exit_blocking`]（`self` 按值）完成等待——闭包若直接
/// 访问 `handle.0` 字段，Rust 2021 disjoint capture 会捕获裸 `HANDLE`（非 Send），绕过
/// 本 `Send` impl；方法调用强制捕获整个 `WaitHandle`。
struct WaitHandle(windows::Win32::Foundation::HANDLE);
// SAFETY: 进程句柄是独立句柄值；`WaitForSingleObject` 可在任意线程安全调用；句柄释放
// （`CloseHandle`）与等待由同一线程串行完成，无并发关闭。
unsafe impl Send for WaitHandle {}

impl WaitHandle {
    /// 阻塞等待进程句柄 signaled（INFINITE）后关闭句柄（本封装生命周期终结）。
    ///
    /// 调用方应将其置于 `spawn_blocking`/独立线程（阻塞调用，不占异步 worker）。
    fn wait_exit_blocking(self) {
        // SAFETY: self.0 是有效进程句柄；INFINITE（u32::MAX）等待其 signaled。
        unsafe {
            let _ = WaitForSingleObject(self.0, u32::MAX);
        }
        // SAFETY: self.0 使用后关闭。
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// engine 进程启动参数（core 侧 `engine_args_for_spawn` 传入；契约对齐 acceptance
/// `engine::parse_engine_args` 的必需/可选字段，但不含 `--log-pipe`——产品日志通道是
/// gRPC `StreamLogs`）。
#[derive(Debug, Clone)]
struct EngineArgs {
    /// 控制面 Named Pipe 名（core 连接的目标）。
    control_pipe: String,
    /// wintun.dll 路径（本 MVP 阶段由 engine 侧后续 slice 消费）。
    dll: PathBuf,
    /// durable journal 目录（`--journal-dir`；2026-09-05 账本计划起真正接线：系统代理
    /// 账本的 open_and_replay 与运行期记账都在此目录，host 传机器级稳定路径
    /// `%ProgramData%\ExvVpn\journal`，崩溃/重启后同路径可回放）。
    journal_dir: PathBuf,
    /// `Local\`-scoped authority mutex 名（后续 authority 组合使用）。
    authority_name: String,
    /// 声明的 core（host）进程 pid——engine 侧核对 client pid 必须等于它。
    host_pid: u32,
    /// 创建的 adapter 名称。
    adapter_name: String,
    /// core 进程显式传入的用户 SID——engine 用它建控制面管道 DACL（授权普通用户 core
    /// 连接）；缺省时回退 engine 本进程用户 SID（同用户拓扑下两者相等）。
    core_user_sid: Option<String>,
    /// 用户配置目录；服务形态不能依赖 LocalSystem 的 `%USERPROFILE%`。
    config_dir: PathBuf,
    /// 服务形态是否收到有效的显式 `--config-dir`（SCM 启动必须带该参数）。
    config_dir_explicit: bool,
}

/// 解析 engine bin 命令行参数（`--key value`；未知参数忽略，前向兼容）。
///
/// # Errors
/// 必需参数缺失（`--control-pipe`/`--dll`/`--host-pid`）或 `--host-pid` 非数字。
fn parse_engine_args() -> Result<EngineArgs, String> {
    let argv: Vec<String> = std::env::args().collect();
    parse_argv(&argv)
}

/// 纯参数提取（`argv[0]` 是程序名，与 `std::env::args()` 形状一致；测试直接注入）。
/// 不做必需性校验——由调用方（oneshot = [`parse_argv`]；service = [`parse_service_argv`]）
/// 按形态要求校验。
fn parse_fields(argv: &[String]) -> EngineArgs {
    let mut a = EngineArgs {
        control_pipe: String::new(),
        dll: PathBuf::new(),
        journal_dir: std::env::temp_dir().join("exv-engine-journal"),
        authority_name: String::new(),
        host_pid: 0,
        adapter_name: "ExvEngine".to_string(),
        core_user_sid: None,
        config_dir: exv_vpn_win32_config::config_dir(),
        config_dir_explicit: false,
    };
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--control-pipe" => a.control_pipe = argv.get(i + 1).cloned().unwrap_or_default(),
            "--dll" => a.dll = PathBuf::from(argv.get(i + 1).cloned().unwrap_or_default()),
            "--journal-dir" => {
                a.journal_dir = PathBuf::from(argv.get(i + 1).cloned().unwrap_or_default())
            }
            "--authority-name" => a.authority_name = argv.get(i + 1).cloned().unwrap_or_default(),
            "--host-pid" => {
                a.host_pid = argv
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
            }
            "--adapter-name" => a.adapter_name = argv.get(i + 1).cloned().unwrap_or_default(),
            "--user-sid" => a.core_user_sid = argv.get(i + 1).cloned(),
            "--config-dir" => {
                if let Some(value) = argv.get(i + 1) {
                    a.config_dir = PathBuf::from(value);
                    a.config_dir_explicit = !value.is_empty()
                        && !value.starts_with("--")
                        && a.config_dir.is_absolute();
                } else {
                    a.config_dir = PathBuf::new();
                    a.config_dir_explicit = false;
                }
            }
            _ => {} // 未知参数忽略（前向兼容）。
        }
        i += 1;
    }
    a
}

/// oneshot 参数校验（必需 `--control-pipe`/`--dll`/`--host-pid`；`--host-pid` 是 core 进程
/// 句柄等待的输入——oneshot 生命周期 = core 生命周期）。
fn parse_argv(argv: &[String]) -> Result<EngineArgs, String> {
    let a = parse_fields(argv);
    if a.control_pipe.is_empty() || a.dll.as_os_str().is_empty() || a.host_pid == 0 {
        return Err("engine args incomplete (need --control-pipe, --dll, --host-pid)".to_string());
    }
    Ok(a)
}

/// 服务形态参数校验（必需 `--control-pipe`/`--dll`/绝对 `--config-dir`；**不要求
/// `--host-pid`**——服务引擎无单一 core 可等，生命周期 = SCM）。ServiceMain 收到的启动
/// 参数（含 `--service` 本身，解析时忽略）经此校验。
fn parse_service_argv(argv: &[String]) -> Result<EngineArgs, String> {
    let a = parse_fields(argv);
    if a.control_pipe.is_empty()
        || a.dll.as_os_str().is_empty()
        || !a.config_dir_explicit
    {
        return Err(
            "service engine args incomplete (need --control-pipe, --dll, --config-dir absolute)"
                .to_string(),
        );
    }
    Ok(a)
}

/// 解析 SCM 服务入口参数。
///
/// Windows 的 `ServiceMain` 参数只包含 SCM 传给 `StartService` 的参数，
/// 不包含服务注册项 `BINARY_PATH_NAME` 中的启动参数；本服务的 `--control-pipe`
/// 和 `--dll` 正是注册项参数。因此优先读取当前进程命令行，只有它不完整时才
/// 回退到 `ServiceMain` 参数，兼容显式传参的 SCM 启动方式。
fn parse_service_argv_sources(
    service_argv: &[String],
    process_argv: &[String],
) -> Result<EngineArgs, String> {
    parse_service_argv(process_argv).or_else(|process_error| {
        parse_service_argv(service_argv).map_err(|_| process_error)
    })
}

/// 同步阻塞等待进程退出（`OpenProcess` fail-safe + `WaitForSingleObject` INFINITE）。
///
/// [`wait_core_process_exit`]（async oneshot 包装）与批量孤儿 watchdog（S2-C）共用的
/// **单一**进程等待实现——两处复用同一份代码，不复制。调用方应置于独立线程（阻塞调用）。
fn wait_for_process_exit_blocking(host_pid: u32) {
    // SAFETY: OpenProcess 打开同用户 core 进程句柄（SYNCHRONIZE 足以等待退出）。
    let handle = match unsafe { OpenProcess(SYNCHRONIZE, false, host_pid) } {
        Ok(handle) => handle,
        Err(_) => return, // core 已退出（进程不可打开）→ 立即返回。
    };
    WaitHandle(handle).wait_exit_blocking();
}

/// 等待 core（host）进程退出（崩溃/kill 路径）：进程句柄 signaled → engine 退出。
///
/// fail-safe：`OpenProcess` 失败（core 已退出）→ 立即返回。
async fn wait_core_process_exit(host_pid: u32) {
    // WaitForSingleObject 是阻塞调用，用普通 std::thread 承载（**非** tokio blocking
    // pool）。原因：心跳超时（hung-core）触发退出时，`await_core_process_exit` 的 select
    // 会丢弃本等待 future，但 blocking 线程仍卡在 INFINITE 上——若走 spawn_blocking，
    // runtime drop 会无限等它（BlockingPool::shutdown(None) 等所有 worker 退出），引擎
    // 完成 shutdown 后进程却退不出去。std::thread 不被 runtime drop 追踪：心跳路径下引擎
    // 能到达 `process::exit`（进程终止时该线程随进程消亡）；kill-core 路径下 core 退出 →
    // WaitForSingleObject 返回 → oneshot 通知本 future resolve（进程句柄即时兜底）。
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        wait_for_process_exit_blocking(host_pid);
        let _ = tx.send(());
    });
    let _ = rx.await;
}

/// engine 进程退出触发（P2 双保险：进程句柄即时兜底 + 心跳硬时间界）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EngineExitTrigger {
    /// core 进程句柄 signaled（正常关停随行 / 崩溃 / 强杀）——即时兜底。
    CoreProcessExited,
    /// 心跳超时（core 存活但不发心跳 = hung-core 检测；或进程句柄路径故障）——
    /// 硬时间界兜底。
    HeartbeatTimeout,
}

/// engine 主路径（可测单元：参数解析 + 生命周期选择逻辑；进程级由集成验证）。
///
/// D1 生命周期分叉由 [`EngineExitForm`] 驱动：
/// - **oneshot**：单 accept（`serve_named_pipe`）→ 等 core 进程句柄 / 心跳超时（双保险）
///   → [`shutdown_after_trigger`] 完整序。
/// - **service**：连续 accept-loop（`serve_named_pipe_loop`）→ SCM stop 信号 →
///   [`service_exit_cleanup`]（无心跳 watchdog、无 core-pid watch）。
///
/// `shutdown_tx`（2026-09-05 方案 B，增参穿入）：`run_service_main` 把**既有** scm_stop
/// watch 的 Sender 克隆传入（`Some`），`run_engine` 转交 `HelperControlService`
/// （`with_shutdown_signal`）——`Shutdown` RPC 受理后延迟触发与 SCM stop 相同的 watch，
/// 不新增第二根通道。`EngineExitForm` 定义于 `service.rs`（计划领地不可改），故经本
/// 参数穿入；oneshot 传 `None`（`Shutdown` 回 `NOT_APPLICABLE`）。
async fn run_engine(
    args: EngineArgs,
    form: EngineExitForm,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
) -> Result<i32, String> {
    // 控制面管道 DACL 的 core 用户 SID：显式 `--user-sid` 优先；缺省回退本进程用户 SID
    // （同用户拓扑下两者相等）。任一不可得 → fail closed。
    let core_sid = match args.core_user_sid.clone() {
        Some(sid) => sid,
        None => current_user_sid()
            .ok_or_else(|| "--user-sid missing and current user SID unresolvable".to_string())?,
    };

    // 组装生产 HelperControl gRPC 服务（产品日志 sink：raw 落盘机器级目录；
    // StreamLogs 推送由 LogSink 内部负责）。R1b：注入真实数据面运行时（登录/CSTP/
    // 特权初始化/数据面全在 engine 进程内）——wintun.dll、adapter 名与用户配置目录均来自
    // 启动参数。服务形态以 LocalSystem 运行，不能重新按自身 `%USERPROFILE%` 推导目录。
    // 产品日志 sink：raw 落盘机器级目录；StreamLogs 推送由 LogSink 内部负责。
    // 单独持有 Arc，供 service 退出路径（service_exit_cleanup）经 log.emit 记录
    // 退出 outcome——tracing 无 subscriber 会丢弃日志，log.emit 才是可达聚合器的通道。
    let log: Arc<LogSink> = Arc::new(LogSink::engine_default());
    // 2026-09-05 账本计划 J4：`args.journal_dir` 真正用起来——with_real_tunnel 在
    // serving 前执行系统代理账本 open_and_replay（§4.5 两形态共用钩子），回放后的
    // 句柄交 RealTunnelRuntime 供运行期记账（崩溃恢复的业务闭环入口）。
    let service = HelperControlService::with_real_tunnel(
        log.clone(),
        args.dll.clone(),
        args.adapter_name.clone(),
        args.config_dir.clone(),
        // 发起用户 SID：系统代理豁免写入该用户 HKU（engine 以服务身份运行）。
        Some(core_sid.clone()),
        args.journal_dir.clone(),
    );
    // S3/D7：service 形态启用 owner 断线释放（连接 EOF → 释放 owner + version++）。
    // oneshot 保持一次性 owner（host 握手后关流，owner 跨 stream 保持）。
    let service = if matches!(form, EngineExitForm::Service { .. }) {
        service.in_service_mode()
    } else {
        service
    };
    // 2026-09-05 方案 B：service 形态注入 Shutdown 触发信号（既有 scm_stop watch 的
    // Sender 克隆，见 `run_engine` 文档）；oneshot 不注入。
    let service = match (shutdown_tx, matches!(form, EngineExitForm::Service { .. })) {
        (Some(tx), true) => service.with_shutdown_signal(tx),
        _ => service,
    };
    let runtime = service.runtime_handle();
    let status = service.status_publisher();

    match form {
        EngineExitForm::Oneshot {
            core_handle,
            heartbeat_timeout,
        } => {
            // P2：仅 oneshot 提取心跳 watchdog 句柄（服务形态不启动心跳——SCM 管生死）。
            let heartbeat = service.heartbeat_watch();
            let server = HelperControlServer::new(service);

            // 建控制面管道 → accept core → 双向认证 → serve gRPC。
            serve_named_pipe(&args.control_pipe, &core_sid, core_handle, server).await?;

            // 生命周期（D1 解耦 + P2 双保险）：等 core 进程退出（进程句柄即时兜底）**或**
            // 心跳超时（硬时间界 + hung-core 检测）。StopTunnel 业务停机不触发自退。
            let process_wait = tokio::task::spawn(wait_core_process_exit(core_handle));
            let heartbeat_watch = heartbeat_timeout_watcher(
                heartbeat,
                Duration::from_millis(500),
                heartbeat_timeout,
            );
            let trigger = await_core_process_exit(process_wait, heartbeat_watch).await;
            // 完整序（含「关机瞬间 connect 中」）：teardown → post Idle → flush → 退。
            shutdown_after_trigger(trigger, runtime, status).await;
        }
        EngineExitForm::Service { scm_stop } => {
            let mut ready = ready_tx.map(ControlPlaneReady::new);
            // S3/D2：服务模式 PSK 是主认证机制——缺 PSK 文件 → 服务启动失败（fail
            // closed，不静默降级到无 PSK 服务）。exe 路径 + SYSTEM 的 SID 验证保留为
            // 传输层兜底；PSK 是共享秘密证明（D2）。PSK 读取失败路径不变（fail closed）。
            // 2026-09-05 撤销计划（4.2）：启动读取仅为 fail-closed 校验，**不再向
            // acceptor 传值**——accept 侧每 accept 现读文件（撤销即时生效），禁缓存
            // last-known-good。
            if let Err(e) = read_service_psk() {
                let error = format!("service PSK unavailable (reinstall the service): {e}");
                if let Some(ref mut ready) = ready {
                    let _ = ready.report_error(error.clone());
                }
                return Err(error);
            }
            // S3/Tier 2：控制面就绪的共享持久布尔——同一事实同时注入
            // HelperControlService（ServiceManage.query 读）与 ServiceAcceptor
            // （建管 + report_ready 成功后置 true）。ready oneshot 仍驱动 ServiceMain
            // 报 Running；AtomicBool 供 query 读，两者是同一事实的不同载体，不得互相替代。
            let control_plane_ready = Arc::new(AtomicBool::new(false));
            let service = service.with_service_self(Arc::clone(&control_plane_ready), true);
            let server = HelperControlServer::new(service);
            // 服务模式连续 accept（每次独立 verify + PSK 挑战 + 独立 owner 流）；SCM
            // stop → 退出清理。
            let outcome = serve_named_pipe_loop(
                &args.control_pipe,
                &core_sid,
                server,
                scm_stop,
                ready,
                control_plane_ready,
            )
            .await;
            service_exit_cleanup(outcome, runtime, status, log).await;
        }
    }
    Ok(0)
}

/// oneshot 主路径：解析 core 参数（必需 `--host-pid`）+ 组装 [`EngineExitForm::Oneshot`]。
async fn run_engine_oneshot() -> Result<i32, String> {
    let args = parse_engine_args()?;
    let form = EngineExitForm::Oneshot {
        core_handle: args.host_pid,
        heartbeat_timeout: HEARTBEAT_TIMEOUT_MS,
    };
    // oneshot 无 Shutdown 触发信号（生命周期随 core；`Shutdown` 回 NOT_APPLICABLE）。
    run_engine(args, form, None, None).await
}

/// 引擎 CLI 分派（main 的同步入口）：
///
/// - `--service-install` / `--service-uninstall` / `--service-start`：
///   SCM 管理子命令（本机 admin，runas 提权边界内；不新建特权二进制，D4）。
/// - `--service-batch`：批量提权模式（一次 runas 完成完整服务操作序列，1 次 UAC）。
/// - `--service-rotate-key`：轮换（撤销）服务 PSK（2026-09-05 撤销计划；覆盖写新
///   密钥，无需重启服务；stdout 输出 fingerprint 摘要）。
/// - `--service`：注册 SCM ServiceMain（阻塞直至服务停止；必须由 SCM 启动）。
/// - 其它：oneshot（默认）——core 生命周期。
fn dispatch(argv: &[String]) -> Result<i32, String> {
    if argv.iter().any(|a| a == "--service-install") {
        return exv_engine::service::install_service(argv).map(|()| 0);
    }
    if argv.iter().any(|a| a == "--service-uninstall") {
        return exv_engine::service::uninstall_service().map(|()| 0);
    }
    if argv.iter().any(|a| a == "--service-start") {
        return exv_engine::service::start_service().map(|()| 0);
    }
    if argv.iter().any(|a| a == "--service-batch") {
        return dispatch_service_batch(argv);
    }
    if wants_service_rotate_key(argv) {
        return dispatch_service_rotate_key();
    }
    if argv.iter().any(|a| a == "--service") {
        return run_service_dispatcher();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("build tokio runtime: {e}"))?;
    runtime.block_on(run_engine_oneshot())
}

/// 是否请求 `--service-rotate-key` 子命令（可测谓词；dispatch 据此分派）。
fn wants_service_rotate_key(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--service-rotate-key")
}

/// `--service-rotate-key` 分派（2026-09-05 撤销计划）：提权环境内直接轮换服务 PSK
///（`rotate_service_key` = `write_service_psk(current_user_sid)` 覆盖写新密钥，无需
/// 重启服务），stdout 输出新 key 的 fingerprint（`rotated fingerprint=<16hex>`，非
/// 秘密摘要）供运维审计。非提权执行 → Windows ACL 拒绝 → 诚实报错（非零退出）。
fn dispatch_service_rotate_key() -> Result<i32, String> {
    let fingerprint = exv_engine::service::rotate_service_key()?;
    println!("rotated fingerprint={fingerprint}");
    Ok(0)
}

/// 提取 `--key` 后的参数值；缺失或下一个 token 是另一个 `--` 选项 → `None`。
fn arg_value<'a>(argv: &'a [String], key: &str) -> Option<&'a str> {
    let pos = argv.iter().position(|a| a == key)?;
    let value = argv.get(pos + 1)?;
    if value.starts_with("--") {
        return None;
    }
    Some(value)
}

/// 批量孤儿 watchdog：后台线程阻塞监控 host 进程（复用 [`wait_for_process_exit_blocking`]，
/// 即 [`wait_core_process_exit`] 的同一实现）。host 中途退出且批量未完成 → 终止进程
/// 防孤儿。批量正常结束时调用方置位 `completed`，watchdog 见标志即静默返回；进程随后
/// 由 `dispatch_service_batch` 正常返回 + `process::exit` 终止，detached 线程随之消亡。
/// 决策逻辑在 lib `service_batch::orphan_watchdog_body`（可单测 seam）。
fn spawn_batch_orphan_watchdog(host_pid: u32, completed: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        exv_engine::service_batch::orphan_watchdog_body(
            || wait_for_process_exit_blocking(host_pid),
            &completed,
            || std::process::exit(1),
        );
    });
}

/// `--service-batch` 分派（S2-A）：解析 `--request <req-file> --result <res-file>
/// --host-pid <pid>`，读请求 → 执行完整服务操作序列 → 写结果 → 清理临时文件。
/// 任一步失败 → 非零退出码（host 按退出码 + 结果 JSON 判定）。
///
/// S2-C 孤儿 watchdog：`--host-pid` 接进批量路径——后台线程阻塞监控 host，host 中途
/// 退出 → 批量进程终止（防孤儿）。竞态处理：批量完成写 result 后进程即将退出，watchdog
/// 若恰在此刻返回不得误杀——`completed` 标志在批量结果返回后置位，watchdog 见标志即
/// no-op。
fn dispatch_service_batch(argv: &[String]) -> Result<i32, String> {
    let request = arg_value(argv, "--request")
        .ok_or_else(|| "--service-batch requires --request <req-file>".to_string())?;
    let result = arg_value(argv, "--result")
        .ok_or_else(|| "--service-batch requires --result <res-file>".to_string())?;
    let host_pid_str = arg_value(argv, "--host-pid")
        .ok_or_else(|| "--service-batch requires --host-pid <pid>".to_string())?;
    // `--host-pid` 先解析并校验（watchdog 的输入必须可靠，fail closed）。
    let host_pid: u32 = host_pid_str
        .parse()
        .map_err(|_| format!("--host-pid must be a numeric pid, got {host_pid_str}"))?;
    let request_path = PathBuf::from(request);
    let result_path = PathBuf::from(result);

    // 孤儿 watchdog（见函数注释）：批量进程运行期间监控 host。所有参数校验必须先于
    // watchdog 启动完成——失败路径不残留孤儿线程。
    let completed = Arc::new(AtomicBool::new(false));
    spawn_batch_orphan_watchdog(host_pid, completed.clone());

    let outcome = exv_engine::service_batch::run_batch_from_files(&request_path, &result_path);
    completed.store(true, Ordering::SeqCst);
    outcome.map(|()| 0).map_err(|e| format!("service-batch failed: {e}"))
}

/// 注册 SCM ServiceMain 并阻塞当前线程直至服务停止（`--service` 入口）。
///
/// 必须由 SCM 启动（`StartServiceCtrlDispatcherW` 要求服务控制分派器环境）；直接运行会
/// 失败（`ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`）——诚实报告该限制。
fn run_service_dispatcher() -> Result<i32, String> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| format!("service dispatcher start failed (must be launched by SCM): {e}"))?;
    Ok(0)
}

define_windows_service!(ffi_service_main, service_main_handler);

/// SCM ServiceMain 入口（`define_windows_service!` 生成的 FFI 回调委托至此；在 SCM
/// 派发的后台线程执行）。任何失败记日志并以非零码退（SCM 观察 Stopped + 退出码）。
fn service_main_handler(arguments: Vec<OsString>) {
    if let Err(e) = run_service_main(&arguments) {
        record_service_startup_error(&e);
        eprintln!("[engine-service] {e}");
        std::process::exit(1);
    }
}

/// Windows GUI subsystem 不提供可见 stderr；把服务启动失败记录到不含秘密的产品日志，
/// 便于区分 SCM/PSK/身份/控制管道初始化错误。
fn record_service_startup_error(error: &str) {
    let Some(program_data) = std::env::var_os("ProgramData") else {
        return;
    };
    let directory = PathBuf::from(program_data).join("ExvVpn").join("logs");
    if create_dir_all(&directory).is_err() {
        return;
    }
    let path = directory.join("engine-service-startup.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "service_startup_error: {error}");
}

/// 服务主路径（SCM 派发线程）：注册控制处理器 → 报告 StartPending → 等待控制面 ready
/// → 报告 Running → 运行 engine（service 形态）→ 报告 Stopped。
fn run_service_main(arguments: &[OsString]) -> Result<(), String> {
    let service_argv: Vec<String> = arguments
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    let process_argv: Vec<String> = std::env::args().collect();
    let args = parse_service_argv_sources(&service_argv, &process_argv)?;

    // 停止信号通道：控制处理器收到 SERVICE_CONTROL_STOP/SHUTDOWN → 置位 → accept-loop
    // 退出 → 清理。发送端随控制处理器存活（windows-service 在停止时释放回调）。
    // 2026-09-05 方案 B：`shutdown_tx` 是同一 watch 的额外克隆（控制处理器持原
    // Sender；克隆随 `run_engine` 穿入 `HelperControlService`，`Shutdown` RPC 受理后
    // 延迟 300ms 向同一 watch 发 `true`——"可编程 SCM stop"，停机序列零改动）。
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let shutdown_tx = stop_tx.clone();
    let status_handle = service_control_handler::register(SERVICE_NAME, move |control_event| {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                tracing::info!(?control_event, "service: SCM stop control received");
                let _ = stop_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })
    .map_err(|e| format!("register service control handler: {e}"))?;

    status_handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::StartPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 1,
            wait_hint: Duration::from_secs(15),
            process_id: None,
        })
        .map_err(|e| format!("report service start pending: {e}"))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("build tokio runtime: {e}"))?;
    let form = EngineExitForm::Service { scm_stop: stop_rx };
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let run = runtime.block_on(async {
        let mut run_future = Box::pin(run_engine(args, form, Some(ready_tx), Some(shutdown_tx)));
        tokio::select! {
            ready = ready_rx => {
                match ready {
                    Ok(Ok(())) => {
                        status_handle
                            .set_service_status(ServiceStatus {
                                service_type: ServiceType::OWN_PROCESS,
                                current_state: ServiceState::Running,
                                controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                                exit_code: ServiceExitCode::Win32(0),
                                checkpoint: 0,
                                wait_hint: Duration::default(),
                                process_id: None,
                            })
                            .map_err(|e| format!("report service running: {e}"))?;
                        run_future.await
                    }
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err("service readiness receiver dropped before Running".to_string()),
                }
            }
            result = &mut run_future => {
                match result {
                    Ok(code) => Err(format!("service exited before control-plane ready (code={code})")),
                    Err(error) => Err(error),
                }
            }
        }
    });

    let exit_code = if run.is_ok() { 0 } else { 1 };
    status_handle
        .set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(exit_code),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
        .map_err(|e| format!("report service stopped: {e}"))?;

    run.map(|_| ())
}

/// 服务模式退出清理（SCM stop → 退出清理**入口**接线）：与心跳路径共用同一完整序
/// （cancel → teardown → post Idle → 300ms flush）。服务形态不启动心跳 watchdog（无心跳
/// 自清理，`EngineExitForm::Service` 断言），清理由 SCM stop 显式触发。
///
/// 完整「0 网卡残留」断言归 S5 复核（本批只接退出清理入口；网卡清理由 S1.5 落地）。
async fn service_exit_cleanup(
    outcome: AcceptLoopOutcome,
    runtime: Arc<dyn TunnelRuntime>,
    status: Arc<StatusPublisher>,
    log: Arc<LogSink>,
) {
    // 退出原因经 log.emit 记录（可达聚合器/raw；tracing 无 subscriber 会丢）。
    let outcome_line = format!("{outcome:?}");
    log.emit(
        LogLevel::Info,
        "engine",
        "service.exit.outcome",
        "service engine exiting (accept loop returned)",
        &[("outcome", &outcome_line)],
    );
    tracing::info!(?outcome, "service engine exiting: full teardown sequence");
    heartbeat_shutdown(runtime, status).await;
    tokio::time::sleep(exv_engine::service::SERVICE_EXIT_FLUSH).await;
}

/// 生命周期触发后的 shutdown 路径（P2 完整序的可测单元，P3 双路径闭合）：
/// **cancel → 有界等待 → teardown → post Idle → flush → 退**。
///
/// **两退出路径无条件走同一完整序**（P3 折叠项[1]）：
/// - [`EngineExitTrigger::HeartbeatTimeout`]：自清理 + 自退出——cancel 在途组装 →
///   teardown 撤网卡/路由 → post Idle → 300ms flush。
/// - [`EngineExitTrigger::CoreProcessExited`]：core 正常关机已由 StopTunnel 完成业务
///   teardown + Idle → **二次 teardown 幂等**（[`RealTunnelRuntime::teardown`] 的
///   `live.take()` 为 None → Ok，no-op）→ post Idle（空 operation_id，host 侧空 id
///   过滤丢弃）→ 300ms flush。但 core 在 connect 中/已连接时被杀（无 StopTunnel 业务
///   停机）必须由本路径兜底撤网卡/路由——判据 4「任意时刻被杀 0 路由残留」的 engine
///   侧核心。
///
/// 300ms flush 给 detached serve task 一个有界窗口把终态（`Stopped` 回复 / Idle 收敛
/// 事件）写回 Named Pipe 后再退（本地 pipe，300ms 远超写回所需）；即便 Idle 仍因断线
/// 丢失，core 侧崩溃路径也有合成收敛双保险（P1 保留在崩溃路径）。调用方随后返回
/// （进程退出）。
async fn shutdown_after_trigger(
    trigger: EngineExitTrigger,
    runtime: std::sync::Arc<dyn TunnelRuntime>,
    status: std::sync::Arc<StatusPublisher>,
) {
    tracing::info!(?trigger, "engine exiting: full teardown sequence");
    // 双退出路径都走完整序（P3 折叠项）：teardown 幂等，无重复副作用。
    heartbeat_shutdown(runtime, status).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

/// engine 生命周期等待（D1 + P2 可测单元）：等两类退出触发之一——
/// core 进程句柄 signaled（[`EngineExitTrigger::CoreProcessExited`]）或心跳超时
/// （[`EngineExitTrigger::HeartbeatTimeout`]）。
///
/// - **业务停机与进程退出解耦**：StopTunnel 只做业务停机（拆隧道 + publish Idle），
///   engine 回 Idle **常驻**，不触发本等待——进程退出仅由上述两类信号触发。
/// - 300ms flush 由调用方在自清理（心跳路径 teardown→Idle）之后执行，保证顺序
///   teardown → Idle → flush → 退。
async fn await_core_process_exit(
    process_wait: tokio::task::JoinHandle<()>,
    heartbeat_timeout: tokio::task::JoinHandle<()>,
) -> EngineExitTrigger {
    tokio::select! {
        _ = process_wait => EngineExitTrigger::CoreProcessExited,
        _ = heartbeat_timeout => EngineExitTrigger::HeartbeatTimeout,
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let code = match dispatch(&argv) {
        Ok(code) => code,
        Err(e) => {
            record_service_startup_error(&format!("dispatch_error: {e}"));
            eprintln!("[engine] {e}");
            1
        }
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// 单元测试：参数解析契约（纯）+ 生命周期解耦语义（D1——StopTunnel 业务停机不触发
// 进程退出，engine 回 Idle 常驻；进程退出仅由 core 进程退出信号触发。不触碰真实
// 进程/管道，注入 JoinHandle 验证等待语义）。
// ---------------------------------------------------------------------------
