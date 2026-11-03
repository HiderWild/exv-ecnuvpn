

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, watch};
use tonic::Status;

use exv_vpn_win32_ipc::peer_auth::VerifiedPipePeer;
use exv_vpn_wire::generated::KeepAliveRequest;

use crate::composition::HostComposition;
use crate::crash_recovery::{
    ResidueProbe, RespawnClientConnector, RespawnSupervisorFactory, assert_no_residue,
    engine_peer_for, rebind_composition,
};
use crate::engine_lifecycle::{EngineShutdownOutcome, EngineSlot, EngineSupervisor};
use crate::grpc_control::KernelEngineControl;
use crate::log_aggregator::LogAggregator;

/// 旧 engine 回收的有界等待上界（与 respawn 路径 `RESPAWN_OLD_EXIT_WAIT_MS` 同源）。
const RETIRE_OLD_EXIT_WAIT_MS: u32 = 3000;
/// provision 就绪轮询上界（原生判据 = 拨号+认证成功 或 keepalive 回复；engine
/// 冷启动/UAC 慢不因首次拨号失败而终止 provision）。
const ONESHOT_READY_TIMEOUT: Duration = Duration::from_secs(15);
/// provision 就绪轮询间隔。
const ONESHOT_READY_POLL: Duration = Duration::from_millis(500);
/// keepalive 确认单次超时（拨号+认证成功后确认 gRPC 服务实际响应）。
const ONESHOT_KEEPALIVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// ensure 结果：复用既有 engine（零 UAC）或本次按需拉起（1 次 UAC）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// 槽内 engine 存活，直接复用（热启动）。
    Reused,
    /// 本次 provision 提权拉起了新 engine。
    Provisioned,
}

/// oneshot engine 按需拉起编排（见模块文档）。
pub struct EngineProvisioner {
    /// 共享 supervisor（与 `CoreRuntime` 共持——provision/respawn/停机三方互斥）。
    supervisor: Arc<Mutex<EngineSupervisor>>,
    /// 共享 engine 控制面槽（换点即服务/转发器/ticker 重指向）。
    slot: EngineSlot,
    /// 共享 composition（provision 成功后身份重建）。
    composition: Arc<Mutex<HostComposition>>,
    /// core 进程用户 SID（engine peer 组装与 client 连接）。
    user_sid: String,
    /// 共享日志聚合器。
    logs: Arc<LogAggregator>,
    /// 提权拉起工厂（生产 = `EngineSupervisor::spawn_product`；测试注入 fake）。
    factory: Arc<dyn RespawnSupervisorFactory>,
    /// engine 控制面连接器（生产 = gRPC Named Pipe；测试注入 fake）。
    connector: Arc<dyn RespawnClientConnector>,
    /// 拉起前 0 残留探测（respawn 同款硬断言输入）。
    residue: Arc<dyn ResidueProbe>,
    /// 已验证的 UI peer（composition 重建后 gate 重授权的身份事实）。core serve
    /// 接受 UI 连接并验证 peer 后回填；provision 前为 `None`（fail closed）。
    ui_peer: Arc<std::sync::RwLock<Option<VerifiedPipePeer>>>,
    /// provision 串行门（并发 connect 只拉起一次）。
    gate: Arc<tokio::sync::Mutex<()>>,
}

/// [`EngineProvisioner::new`] 的依赖束（clippy 债务清偿 2026-09-27：9 个位置参数
/// 收敛为结构体——provisioner 的依赖本就是一组共享句柄 + 三个接缝，按名传值）。
pub struct EngineProvisionerDeps {
    /// 共享 supervisor（与 `CoreRuntime` 共持——provision/respawn/停机三方互斥）。
    pub supervisor: Arc<Mutex<EngineSupervisor>>,
    /// 共享 engine 控制面槽（换点即服务/转发器/ticker 重指向）。
    pub slot: EngineSlot,
    /// 共享 composition（provision 成功后身份重建）。
    pub composition: Arc<Mutex<HostComposition>>,
    /// core 进程用户 SID（engine peer 组装与 client 连接）。
    pub user_sid: String,
    /// 共享日志聚合器。
    pub logs: Arc<LogAggregator>,
    /// 已验证 UI peer cell（serve 回填；provision 前 `None` = fail closed）。
    pub ui_peer: Arc<std::sync::RwLock<Option<VerifiedPipePeer>>>,
    /// 提权拉起工厂（生产 = `EngineSupervisor::spawn_product`；测试注入 fake）。
    pub factory: Arc<dyn RespawnSupervisorFactory>,
    /// engine 控制面连接器（生产 = gRPC Named Pipe；测试注入 fake）。
    pub connector: Arc<dyn RespawnClientConnector>,
    /// 拉起前 0 残留探测（respawn 同款硬断言输入）。
    pub residue: Arc<dyn ResidueProbe>,
}

/// 连接产物：engine 控制面 + 存活 watch（与 `EngineSlot` 槽内元素同形态）。
type EngineConnection = (Arc<Mutex<dyn KernelEngineControl>>, watch::Receiver<bool>);

impl EngineProvisioner {
    /// 生产构造（真实 factory/connector/residue；`ui_peer` cell 由 serve 回填）。
    #[must_use]
    pub fn new(deps: EngineProvisionerDeps) -> Self {
        Self {
            supervisor: deps.supervisor,
            slot: deps.slot,
            composition: deps.composition,
            user_sid: deps.user_sid,
            logs: deps.logs,
            ui_peer: deps.ui_peer,
            factory: deps.factory,
            connector: deps.connector,
            residue: deps.residue,
            gate: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// 回填已验证的 UI peer（serve 接受并验证 UI 连接后调用；幂等）。
    pub fn set_ui_peer(&self, peer: VerifiedPipePeer) {
        let _ = self.ui_peer.write().unwrap().replace(peer);
    }

    /// 确保槽内存在存活的 oneshot engine（connect 路由 Oneshot 分支的唯一入口）。
    ///
    /// 复用优先（零 UAC）；无 engine / engine 已死 → 按需提权拉起（1 次 UAC，归属
    /// 本次业务连接）。并发 connect 经串行门收敛为单次 provision。
    ///
    /// # Errors
    /// [`Status::failed_precondition`]（`engine_provision_failed|` 前缀）：UI peer 未
    /// 建立 / 残留断言失败 / 提权拉起失败（含用户取消 UAC）/ 就绪超时 / 身份重建
    /// 失败。失败不触碰 composition——connect 的状态回滚由
    /// `apply_connect_failed_for_route` 收敛，用户可立即重试。
    pub async fn ensure_oneshot_engine(&self) -> Result<EnsureOutcome, Status> {
        if self.slot_engine_alive().await {
            return Ok(EnsureOutcome::Reused);
        }
        let _serial = self.gate.lock().await;
        // 串行门内二次检查：并发的第一个 connect 可能已完成 provision。
        if self.slot_engine_alive().await {
            return Ok(EnsureOutcome::Reused);
        }
        self.provision().await
    }

    /// 退役 oneshot engine（服务 install 成功后的存在性互斥边界）：
    /// 终止 supervisor 持有的 oneshot 子进程（业务停机已由 transition stop 完成，
    /// 此处只收进程）+ 槽换回 detached 占位。幂等。
    ///
    /// **收走真值与槽位处置（P0-1 随附，计划 v6.1 §8.2 缺口④扩项——mode 翻转
    /// 饿死收口）**：提权 oneshot 子进程对非提权 core 的 `TerminateProcess` 可能
    /// 被拒（完整性级别）——此时子进程**很可能仍存活**。仅当收走已确认
    /// （干净退出/终止确认/本就无 child）才换槽 detached；未确认时**保留槽内
    /// engine client**，供失败路径回滚 mode 后心跳继续喂给（调用方
    /// `restore_oneshot_keepalive_if_engine_alive`），避免「retire 名义收走 +
    /// 实际饿死 15s」的隐式退场（2026-09-28 Install 流案）。D12 管道互斥不受
    /// 槽位影响（互斥在 engine 进程侧的管道首实例，不在 core 槽）。
    ///
    /// **retire→install 序与 D12 管道名的过渡属性（T11）**：retire 先杀本 core 持有的
    /// oneshot（无论其持旧名还是新名——杀进程按 supervisor 句柄，与管道名无关），随后
    /// 服务绑定新名（安装用户 sid 段常量名）。**跨版本升级**（旧版本 engine 在跑）时
    /// 旧持有者持旧名（per-PID 名/裸常量服务名），与新名不同——首实例互不冲突，升级
    /// 窗口零互斥竞态；同名冲突只剩"同版本残留 engine"一种，由 engine 建管侧的
    /// bootstrap 有界重试 + 诚实失败收口（`grpc_transport::
    /// create_first_instance_with_bootstrap_retry`）。本函数不杀非本 core 持有的进程
    ///（孤儿 oneshot 由其自身的 core 句柄/心跳超时退场）。
    pub async fn retire_oneshot_engine(&self) {
        let confirmed_gone = {
            let mut supervisor = self.supervisor.lock().await;
            match supervisor.pid() {
                None => true, // 无 child 可收（从未拉起/已收走）。
                Some(_) => !matches!(
                    supervisor.verify_exit(RETIRE_OLD_EXIT_WAIT_MS),
                    EngineShutdownOutcome::TerminateUnconfirmed
                ),
            }
        };
        if confirmed_gone {
            let swapped = self.slot.swap_detached().await;
            let _ = self.logs.append_core(
                "info",
                "kernel",
                "kernel.engine.retired",
                &format!(
                    "oneshot engine retired (service form is now authoritative); slot detached (swapped={swapped})"
                ),
                &BTreeMap::new(),
            );
        } else {
            // 收走未确认：槽位保留（心跳可续喂；engine 随 core 退出或被下次重试
            // 收走）。已从 supervisor 视野移除（句柄已消费）——存活判定走槽内
            // client 的 lease 探测。
            let _ = self.logs.append_core(
                "warn",
                "kernel",
                "kernel.engine.retire_unconfirmed",
                "oneshot engine retire could not confirm exit (elevated child terminate refused?); \
                 slot kept for heartbeat continuity; engine exits with core or on next retire retry",
                &BTreeMap::new(),
            );
        }
    }

    /// 槽内 engine 是否存活：`ensure_owner_lease` 幂等确认（detached 占位/死 client
    /// 返回 `ConnectionLost` → 不存活）。
    async fn slot_engine_alive(&self) -> bool {
        let current = self.slot.current().await;
        let mut guard = current.lock().await;
        guard.ensure_owner_lease().await.is_ok()
    }

    /// 拉起全链（见模块文档步骤 3-8）。调用方已持串行门且确认槽内无存活 engine。
    ///
    /// **分阶段打点（计划 v6.1 第二批诊断，P1-5 合并交付）**：12.8s 提权链此前只有
    /// 起止两条日志，无法细分 UAC 等待/残留探测/冷启动/拨号等待——每阶段落
    /// `kernel.engine.provision.stage`（stage + elapsed_ms），供 40s 窗口机制分解。
    async fn provision(&self) -> Result<EnsureOutcome, Status> {
        let t_provision = Instant::now();
        let stage = |name: &str, fields: &[(&str, String)]| {
            let mut pairs: BTreeMap<String, String> = BTreeMap::from([
                ("stage".to_string(), name.to_string()),
                ("elapsed_ms".to_string(), t_provision.elapsed().as_millis().to_string()),
            ]);
            for (key, value) in fields {
                pairs.insert((*key).to_string(), value.clone());
            }
            pairs
        };
        let Some(ui_peer) = self.ui_peer.read().unwrap().clone() else {
            return Err(Status::failed_precondition(
                "engine_provision_failed|ui peer not established; cannot provision engine",
            ));
        };
        let mut supervisor = self.supervisor.lock().await;
        // T6b（R2 oneshot 门控开启）并发守卫：liveness respawn 路径与本 provisioner 不
        // 共享串行门、仅共享 supervisor 互斥——等待本锁期间对方可能已完成 respawn 并把
        // 新 engine 换入槽。锁内复核槽内存活即让位（复用，零 UAC），不得回收/覆盖他人
        // 刚拉起的新 engine（verify_exit 会误杀新进程）。
        if self.slot_engine_alive().await {
            let _ = self.logs.append_core(
                "info",
                "kernel",
                "kernel.engine.provision_yielded",
                "another path already swapped in a live engine while provision waited on the supervisor lock; reusing it",
                &BTreeMap::new(),
            );
            return Ok(EnsureOutcome::Reused);
        }
        // 回收残留旧 child（此前 engine 死亡未被 respawn 路径回收）。
        if supervisor.pid().is_some() {
            supervisor.verify_exit(RETIRE_OLD_EXIT_WAIT_MS);
        }
        // 拉起前 0 adapter/0 路由硬断言（respawn 同款：旧 engine 崩溃可能遗留残留）。
        let report = self.residue.probe().map_err(|e| {
            Status::failed_precondition(format!("engine_provision_failed|residue probe: {e}"))
        })?;
        let _ = self.logs.append_core(
            "info",
            "kernel",
            "kernel.engine.provision.stage",
            "residue probe done before spawn",
            &stage("residue_probed", &[]),
        );
        if let Err(e) = assert_no_residue(&report) {
            let _ = self.logs.append_core(
                "error",
                "kernel",
                "kernel.engine.provision_residue_blocked",
                &format!("engine provision blocked by residue: {e}"),
                &BTreeMap::new(),
            );
            return Err(Status::failed_precondition(format!(
                "engine_provision_failed|{e}"
            )));
        }
        // 提权拉起（本模型中唯一归属连接的 UAC）。elapsed 覆盖 UAC 弹窗等待 + 进程
        // spawn（12.8s 链的主要疑点段——Defender 首扫也在此窗）。
        let t_spawn = Instant::now();
        let mut new_supervisor = self.factory.spawn_supervisor().map_err(|e| {
            let _ = self.logs.append_core(
                "error",
                "kernel",
                "kernel.engine.provision_spawn_failed",
                &format!("engine provision spawn failed: {e}"),
                &BTreeMap::new(),
            );
            Status::failed_precondition(format!("engine_provision_failed|engine spawn: {e}"))
        })?;
        let new_pid = new_supervisor.pid().unwrap_or(0);
        let _ = self.logs.append_core(
            "info",
            "kernel",
            "kernel.engine.provision.stage",
            "elevated spawn returned (UAC wait + process creation)",
            &stage(
                "spawned",
                &[("pid", new_pid.to_string()), ("spawn_ms", t_spawn.elapsed().as_millis().to_string())],
            ),
        );
        // 有界等待就绪 + 连接。
        let t_ready = Instant::now();
        let (engine, liveness) = match self.connect_with_ready_wait(new_pid).await {
            Ok(pair) => pair,
            Err(detail) => {
                new_supervisor.terminate();
                let _ = self.logs.append_core(
                    "error",
                    "kernel",
                    "kernel.engine.provision_connect_failed",
                    &format!("engine control connect timed out: {detail}"),
                    &BTreeMap::new(),
                );
                return Err(Status::failed_precondition(format!(
                    "engine_provision_failed|engine connect: {detail}"
                )));
            }
        };
        let _ = self.logs.append_core(
            "info",
            "kernel",
            "kernel.engine.provision.stage",
            "engine control plane ready (dial + auth + keepalive confirmed)",
            &stage("ready", &[("pid", new_pid.to_string()), ("ready_wait_ms", t_ready.elapsed().as_millis().to_string())]),
        );
        // composition 身份重建（绑定新 peer；admission 重开；gate 重授权）。provision
        // 可能发生在 connect 在途（受理后路由内拉起）——重建前捕获在途操作上下文，
        // 重建后恢复（相位 Connecting + operation_id），使 engine 终态事件照常驱动
        // 相态（R3-C2 的在途登记过滤也有据可依）。respawn 路径不经过此处（teardown
        // 后无在途 connect），语义不受影响。
        let inflight_connect = {
            let composition = self.composition.lock().await;
            if composition.phase() == crate::composition::HostPhase::Connecting {
                Some(composition.operation_id())
            } else {
                None
            }
        };
        let new_peer = engine_peer_for(new_pid, &self.user_sid);
        if let Err(e) = rebind_composition(&self.composition, &new_peer, &ui_peer).await {
            new_supervisor.terminate();
            let _ = self.logs.append_core(
                "error",
                "kernel",
                "kernel.engine.provision_rebind_failed",
                &format!("composition rebind failed: {e}"),
                &BTreeMap::new(),
            );
            return Err(Status::failed_precondition(format!(
                "engine_provision_failed|composition rebind: {e}"
            )));
        }
        if let Some(operation_id) = inflight_connect {
            let mut composition = self.composition.lock().await;
            composition.apply(crate::composition::HostEvent::Connect);
            if let Some(operation_id) = operation_id {
                composition.set_operation_id(operation_id);
            }
        }
        // 换槽 + 挂接（转发器/ticker 经 swap watch 自动重指向；run loop 经 supervisor
        // 挂接的新 liveness 恢复掉线监听）。
        self.slot.swap(Arc::clone(&engine)).await;
        new_supervisor.attach_client(engine, liveness);
        *supervisor = new_supervisor;
        let _ = self.logs.append_core(
            "info",
            "kernel",
            "kernel.engine.provision.stage",
            "composition rebound + engine slot swapped",
            &stage("swapped", &[("pid", new_pid.to_string())]),
        );
        let _ = self.logs.append_core(
            "info",
            "kernel",
            "kernel.engine.provisioned",
            &format!("oneshot engine provisioned on demand (pid={new_pid})"),
            &BTreeMap::from([(
                "total_ms".to_string(),
                t_provision.elapsed().as_millis().to_string(),
            )]),
        );
        Ok(EnsureOutcome::Provisioned)
    }

    /// 有界就绪等待（拨号+认证+keepalive 确认；上界 [`ONESHOT_READY_TIMEOUT`]）。
    /// 复用 respawn 的 [`RespawnClientConnector`] seam（生产实现内部已含拨号重试）。
    ///
    /// 每次失败尝试落 debug 级 `kernel.engine.provision.ready_attempt`（诊断打点，
    /// P1-5：拨号等待段的可细分性；默认 info 级日志不放大重试噪音）。
    async fn connect_with_ready_wait(
        &self,
        pid: u32,
    ) -> Result<EngineConnection, String> {
        let start = Instant::now();
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let outcome: Result<EngineConnection, String> =
                match self.connector.connect(pid, &self.user_sid).await {
                Ok((engine, liveness)) => {
                    let confirmed = {
                        let mut guard = engine.lock().await;
                        tokio::time::timeout(
                            ONESHOT_KEEPALIVE_CONFIRM_TIMEOUT,
                            guard.keep_alive(KeepAliveRequest { monotonic_tick: 0 }),
                        )
                        .await
                    };
                    match confirmed {
                        Ok(Ok(_)) => Ok((engine, liveness)),
                        Ok(Err(e)) => Err(format!("keepalive confirm rejected: {e:?}")),
                        Err(_) => Err("keepalive confirm timeout".to_string()),
                    }
                }
                Err(e) => Err(format!("{e:?}")),
            };
            match outcome {
                Ok(pair) => return Ok(pair),
                Err(detail) => {
                    let _ = self.logs.append_core(
                        "debug",
                        "kernel",
                        "kernel.engine.provision.ready_attempt",
                        &format!("engine ready-wait attempt failed: {detail}"),
                        &BTreeMap::from([
                            ("pid".to_string(), pid.to_string()),
                            ("attempt".to_string(), attempt.to_string()),
                            ("elapsed_ms".to_string(), start.elapsed().as_millis().to_string()),
                        ]),
                    );
                    if start.elapsed() >= ONESHOT_READY_TIMEOUT {
                        return Err(detail);
                    }
                    tokio::time::sleep(ONESHOT_READY_POLL).await;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 单元测试：复用优先（零 UAC）/ 失败不触碰 composition（可立即重试）/ 成功换槽重建。
// ---------------------------------------------------------------------------
