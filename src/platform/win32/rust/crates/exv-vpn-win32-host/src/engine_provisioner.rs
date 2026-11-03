

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
use crate::engine_lifecycle::{EngineSlot, EngineSupervisor};
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
    /// **retire→install 序与 D12 管道名的过渡属性（T11）**：retire 先杀本 core 持有的
    /// oneshot（无论其持旧名还是新名——杀进程按 supervisor 句柄，与管道名无关），随后
    /// 服务绑定新名（安装用户 sid 段常量名）。**跨版本升级**（旧版本 engine 在跑）时
    /// 旧持有者持旧名（per-PID 名/裸常量服务名），与新名不同——首实例互不冲突，升级
    /// 窗口零互斥竞态；同名冲突只剩"同版本残留 engine"一种，由 engine 建管侧的
    /// bootstrap 有界重试 + 诚实失败收口（`grpc_transport::
    /// create_first_instance_with_bootstrap_retry`）。本函数不杀非本 core 持有的进程
    ///（孤儿 oneshot 由其自身的 core 句柄/心跳超时退场）。
    pub async fn retire_oneshot_engine(&self) {
        {
            let mut supervisor = self.supervisor.lock().await;
            if supervisor.pid().is_some() {
                supervisor.verify_exit(RETIRE_OLD_EXIT_WAIT_MS);
            }
        }
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
    }

    /// 槽内 engine 是否存活：`ensure_owner_lease` 幂等确认（detached 占位/死 client
    /// 返回 `ConnectionLost` → 不存活）。
    async fn slot_engine_alive(&self) -> bool {
        let current = self.slot.current().await;
        let mut guard = current.lock().await;
        guard.ensure_owner_lease().await.is_ok()
    }

    /// 拉起全链（见模块文档步骤 3-8）。调用方已持串行门且确认槽内无存活 engine。
    async fn provision(&self) -> Result<EnsureOutcome, Status> {
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
        // 提权拉起（本模型中唯一归属连接的 UAC）。
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
        // 有界等待就绪 + 连接。
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
            "kernel.engine.provisioned",
            &format!("oneshot engine provisioned on demand (pid={new_pid})"),
            &BTreeMap::new(),
        );
        Ok(EnsureOutcome::Provisioned)
    }

    /// 有界就绪等待（拨号+认证+keepalive 确认；上界 [`ONESHOT_READY_TIMEOUT`]）。
    /// 复用 respawn 的 [`RespawnClientConnector`] seam（生产实现内部已含拨号重试）。
    async fn connect_with_ready_wait(
        &self,
        pid: u32,
    ) -> Result<EngineConnection, String> {
        let start = Instant::now();
        loop {
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
