
//! runtime-metrics 统计链路回归（RT-TRANSPORT-05 / RT-INTEROP-06）共享 harness。
//!
//! 拓扑（单进程、真实组件、无外网、无账号；计划 §5.2）：真实
//! `HelperControlService`（engine，持真实 `StatsPublisher`）经真实 DACL + 双向
//! peer 认证 named pipe 提供控制面；core 侧真实 `EngineControlGrpcClient` 装入
//! `EngineSlot`，由真实 `KernelControlService` 消费。测试专用注入**只**走同进程
//! 持有的 `registry()` 引用（`record_tx`/`record_rx`/`set_phase`），不得新增任何
//! "任意写统计"的 RPC、命令、配置开关或未认证 pipe。
//!
//! engine 任务跑在**专用 runtime** 上：`EngineServe::kill` 以 runtime 关停回收全部
//! engine 任务——包括 tonic `serve_with_incoming` 分离 spawn 的连接 task（abort
//! serve future 不关闭 pipe，见 `grpc_interop.rs` 尾注）——从而产生真实的管道断裂。

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use exv_core::composition::compose_nonprivileged_host;
use exv_core::engine_lifecycle::EngineSlot;
use exv_core::grpc_control::KernelEngineControl;
use exv_core::kernel_control_service::KernelControlService;
use exv_core::log_aggregator::LogAggregator;
use exv_engine::grpc_server::HelperControlService;
use exv_engine::grpc_transport::{create_control_pipe_server, serve_named_pipe};
use exv_engine::stats::StatsRegistry;
use exv_vpn_win32_ipc::peer_auth::{VerifiedPipePeer, current_user_sid};
use exv_vpn_win32_resource::proxy_tun::ProxyTunDetection;
use exv_vpn_wire::generated::helper_control_server::HelperControlServer;
use exv_vpn_wire::generated::kernel_control_server::KernelControl;
use exv_vpn_wire::generated as wire;

/// 唯一 pipe 名（同进程多测试并行互不干扰）。
pub fn unique_pipe_name(tag: &str) -> String {
    format!(r"\\.\pipe\exv-runtime-metrics-{tag}-{}", std::process::id())
}

/// 已验 helper 身份（同进程互操作：server pid == 本进程 pid、SID == 当前用户）。
pub fn fake_peer() -> VerifiedPipePeer {
    VerifiedPipePeer {
        process_id: 4242,
        user_sid: "S-1-5-21-3980489076-1253412212-3874560562-1002".to_string(),
        logon_sid: Some("S-1-5-5-0-323470".to_string()),
        account_name: "EXV VPN Helper".to_string(),
    }
}

/// 确定性 SCM 状态源：服务未安装（测试不触真实 SCM）。
pub struct NotInstalledStatusSource;

impl exv_core::service_status::ServiceStatusSource for NotInstalledStatusSource {
    fn query_raw(
        &self,
        _service_name: &str,
    ) -> Result<exv_core::service_status::RawServiceQuery, String> {
        Ok(exv_core::service_status::RawServiceQuery {
            installed: false,
            raw_state: 0,
            binary_path: None,
        })
    }
}

/// 确定性探针注入的真实 `KernelControlService`（tempdir LogAggregator 由调用方
/// 持有同一 Arc 供扫码；proxy TUN / 系统代理 / SCM 全部确定性，不触真实 Win32 状态）。
pub fn hermetic_service(
    engine: Arc<tokio::sync::Mutex<dyn KernelEngineControl>>,
    logs: Arc<LogAggregator>,
) -> KernelControlService {
    let composition =
        compose_nonprivileged_host(&fake_peer()).expect("compose nonprivileged host");
    let composition = Arc::new(tokio::sync::Mutex::new(composition));
    KernelControlService::new(composition, EngineSlot::new(engine), PathBuf::new(), logs)
        .with_proxy_tun_probe(Arc::new(|| {
            Ok(ProxyTunDetection {
                detected: false,
                adapters: vec![],
            })
        }))
        .with_system_proxy_probe(Arc::new(|| Ok(wire::SystemProxyDetection::default())))
        .with_service_status_source(Arc::new(NotInstalledStatusSource))
}

/// tempdir 日志聚合器（调用方持有 Arc 扫码；证据不落仓库、不落用户目录）。
pub fn tempdir_logs() -> (Arc<LogAggregator>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("logs tempdir");
    let logs = Arc::new(LogAggregator::open(&dir.path().join("svc.jsonl")).expect("open logs"));
    (logs, dir)
}

/// 真实 engine `HelperControlService` 的 named pipe 承载（专用 runtime）。
pub struct EngineServe {
    name: String,
    sid: String,
    pid: u32,
    rt: Option<tokio::runtime::Runtime>,
    registry: Arc<StatsRegistry>,
}

impl EngineServe {
    /// 启动真实 engine 控制面（真实 DACL + first-instance + reject-remote；
    /// server 侧验 core pid+SID，client 侧验 engine pid+SID）。
    pub fn start(name: &str) -> Self {
        let sid = current_user_sid().expect("current user sid");
        let pid = std::process::id();
        let rt = tokio::runtime::Runtime::new().expect("engine runtime");
        let service = HelperControlService::new();
        let registry = service.stats_publisher().registry().clone();
        let server = HelperControlServer::new(service);
        let pipe = name.to_string();
        let serve_sid = sid.clone();
        rt.spawn(async move {
            let _ = serve_named_pipe(&pipe, &serve_sid, pid, server).await;
        });
        Self {
            name: name.to_string(),
            sid,
            pid,
            rt: Some(rt),
            registry,
        }
    }

    /// engine 进程 id（client 侧 peer 验证期望值）。
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// engine 期望用户 SID（client 侧 peer 验证期望值）。
    pub fn sid(&self) -> &str {
        &self.sid
    }

    /// 数据面统计注册表（唯一的合法注入点：record_tx/record_rx/set_phase）。
    pub fn registry(&self) -> &Arc<StatsRegistry> {
        &self.registry
    }

    /// 强制断流：关停 engine 专用 runtime → 全部 engine 任务（含 tonic 分离的连接
    /// task）被回收 → pipe 关闭 → 客户端读端断裂（EOF/传输错误）。非阻塞关停
    ///（`shutdown_background`）——本 harness 生命周期在 async 测试上下文中，禁止
    /// 在该上下文阻塞（`shutdown_timeout`/Drop 同理会 panic）。
    pub fn kill(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }

    /// 以**同一 pipe 名**重新 serve（先等旧实例——含客户端残留句柄——全部释放，
    /// 再以新 runtime 拉起全新 `HelperControlService`；统计注册表清零重启）。
    pub fn restart(&mut self) {
        self.kill();
        let deadlined = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            match create_control_pipe_server(&self.name, &self.sid, true, 1) {
                // 名称可建 = 旧实例已全部释放；立即弃置探针实例（名称重新空闲）。
                Ok(_probe) => break,
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadlined,
                        "pipe name never released: {}",
                        self.name
                    );
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
        let rt = tokio::runtime::Runtime::new().expect("engine runtime (restart)");
        let service = HelperControlService::new();
        self.registry = service.stats_publisher().registry().clone();
        let server = HelperControlServer::new(service);
        let pipe = self.name.clone();
        let serve_sid = self.sid.clone();
        let pid = self.pid;
        rt.spawn(async move {
            let _ = serve_named_pipe(&pipe, &serve_sid, pid, server).await;
        });
        self.rt = Some(rt);
    }
}

impl Drop for EngineServe {
    /// 兜底关停（非阻塞——测试断言失败提前返回时也不得在 async 上下文阻塞）。
    fn drop(&mut self) {
        self.kill();
    }
}

/// Connected 快照 fixture（测试侧发布到 EventBus，使 `publish_stats` 为每个样本铸造
/// 递增 tick——生产中该角色由状态转发器的 Connected 快照承担；`subscribe_stats` 的
/// live 过滤以 `sample_tick > resume_tick` 判定，无 tick 样本不可达）。
pub fn connected_snapshot_fixture() -> wire::RuntimeSnapshot {
    let mut connected = wire::ConnectedState::default();
    connected.session_established_at_ms = 1;
    wire::RuntimeSnapshot {
        state: Some(wire::runtime_snapshot::State::Connected(connected)),
        stats: None,
        proxy_tun: None,
        system_proxy: None,
        reconnect: None,
        // 2026-09-05 wire 解冻新增字段（self_heal = 16）的机械占位；fixture 不携带自愈上下文。
        self_heal: None,
        operation_id: Vec::new(),
        service_status: None,
        mode: String::new(),
    }
}

/// §4.1 冻结诊断码表 v1（白名单断言的事实源；与执行计划逐字对齐，表驱动）。
pub const FROZEN_DIAG_CODES: &[&str] = &[
    "kernel.stats.subscribed",
    "kernel.stats.first_sample",
    "kernel.stats.subscribe_failed",
    "kernel.stats.stream_error",
    "kernel.stats.stream_ended",
    "kernel.stats.first_sample_timeout",
    "kernel.stats.slot_swapped",
    "kernel.stats.phase_unspecified",
    "kernel.stats.timestamp_regression",
    "kernel.stats.counter_regression",
    "kernel.session.adopted",
];

/// warn 级码（I4 零流量诚实性：码 3-6、8-10 不得出现）。
pub const FROZEN_WARN_CODES: &[&str] = &[
    "kernel.stats.subscribe_failed",
    "kernel.stats.stream_error",
    "kernel.stats.stream_ended",
    "kernel.stats.first_sample_timeout",
    "kernel.stats.phase_unspecified",
    "kernel.stats.timestamp_regression",
    "kernel.stats.counter_regression",
];

/// `error_kind` 冻结枚举（码 3/4 的取值域）。
pub const FROZEN_ERROR_KINDS: &[&str] = &[
    "transport",
    "rpc_unauthenticated",
    "rpc_permission_denied",
    "rpc_unavailable",
    "rpc_deadline",
    "connection_lost",
    "rpc_other",
];

/// 各码的 fields 白名单（§4.1 逐码对齐；表驱动，不允许复制粘贴漂移）。
pub fn field_whitelist(code: &str) -> &'static [&'static str] {
    match code {
        "kernel.stats.subscribed" => &["attempt", "backoff_ms"],
        "kernel.stats.first_sample" => &["phase", "engine_sequence"],
        "kernel.stats.subscribe_failed" => &["error_kind", "attempt", "backoff_ms"],
        "kernel.stats.stream_error" => &["error_kind", "sample_count"],
        "kernel.stats.stream_ended" => &["sample_count"],
        "kernel.stats.first_sample_timeout" => &["wait_ms"],
        "kernel.stats.slot_swapped" => &[],
        "kernel.stats.phase_unspecified" => &["engine_sequence"],
        "kernel.stats.timestamp_regression" => &["engine_sequence"],
        "kernel.stats.counter_regression" => &["engine_sequence"],
        "kernel.session.adopted" => &["source"],
        _ => &[],
    }
}

/// 扫描聚合日志中的 core/kernel 诊断条目（logs.list(0, 0) 语义）。
pub fn scan_kernel_diag(logs: &LogAggregator) -> Vec<exv_core::log_aggregator::LogEntry> {
    logs.list(0, 0)
        .expect("list logs")
        .entries
        .into_iter()
        .filter(|entry| entry.source == "core" && entry.component == "kernel")
        .collect()
}

/// 冻结白名单断言：扫到的全部码 ∈ 冻结表、code 非空、fields 键 ⊆ 该码白名单；
/// 码 3/4 的 error_kind ∈ 冻结枚举。出现未知码即失败（防将来加码不走契约）。
pub fn assert_diag_whitelist(entries: &[exv_core::log_aggregator::LogEntry]) {
    for entry in entries {
        assert!(!entry.code.is_empty(), "诊断码非空");
        assert!(
            FROZEN_DIAG_CODES.contains(&entry.code.as_str()),
            "未知诊断码：{}（冻结表之外不得出现）",
            entry.code
        );
        let whitelist = field_whitelist(&entry.code);
        for key in entry.fields.keys() {
            assert!(
                whitelist.contains(&key.as_str()),
                "码 {} 的 fields 键 {} 越白名单",
                entry.code,
                key
            );
        }
        if let Some(kind) = entry.fields.get("error_kind") {
            assert!(
                FROZEN_ERROR_KINDS.contains(&kind.as_str()),
                "码 {} 的 error_kind {} 不在冻结枚举",
                entry.code,
                kind
            );
        }
    }
}
