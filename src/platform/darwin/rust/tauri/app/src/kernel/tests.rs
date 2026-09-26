//! 集成测试：CoreClient 真实 tonic 调用 ↔ 进程内 mock `KernelControl` server
//!（win32 `kernel/tests.rs` 的 darwin 移植）。
//!
//! 用 localhost TCP 的 tonic server（mock 实现 `kernel_control_server::KernelControl`
//! trait，复用 Root workspace 的生成类型）验证 CoreClient 的 unary + streaming
//! 端到端：请求组装 → 发送 → 响应 wire→UI 映射。darwin 的真实 transport 是
//! authenticated UDS（`core_process.rs`，由其专属测试与真机 e2e 覆盖）；本文件
//! 提供语义层的最强可行验证（与 win32 同款基建）。

#![cfg(test)]

use std::pin::Pin;
use std::sync::Arc;

use exv_vpn_wire::generated::kernel_control_server::{KernelControl, KernelControlServer};
use exv_vpn_wire::generated::{self as wire};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Channel, Endpoint, Server};
use tonic::{Request, Response, Status};

use super::client::{
    ConfigItem, ConnectCredentials, ConnectIntent, CoreClient, CoreHandle, CoreState,
};
use super::state::{OperationResult, RuntimeState, ServiceControlAction};

/// mock `KernelControl` server：观测请求并回确定性响应（watch_events 用 broadcast 流）。
#[derive(Clone)]
struct MockKernel {
    /// watch_events 的事件源 sender（测试注入；None = 立即 EOF）。每次
    /// `watch_events` 调用从 broadcast sender `subscribe()` 派生新接收端——
    /// **可重复订阅 + fan-out**（W3-1/P1 S4：真实 core 支持多订阅/resume，mock 用
    /// broadcast 通道对齐多订阅语义；旧 mpsc `take()` 一次性接收端语义退役）。
    watch_tx: Arc<tokio::sync::Mutex<Option<broadcast::Sender<wire::RuntimeEvent>>>>,
    /// 已收到的 watch_events `resume_tick`（resume 循环测试断言用）。
    watch_resumes: Arc<tokio::sync::Mutex<Vec<u64>>>,
    /// 收到的 connect 请求（断言用）。
    connects: Arc<tokio::sync::Mutex<Vec<wire::ConnectRequest>>>,
    /// CoreConnect request 上的非秘密持久化选择；必须与 engine payload 隔离。
    connect_persist_metadata: Arc<tokio::sync::Mutex<Vec<Option<String>>>>,
    /// 注入 `connect` RPC 拒绝（凭据恢复等命令错误映射测试）。
    connect_error: Arc<tokio::sync::Mutex<Option<(tonic::Code, String)>>>,
    /// 收到的 stop 请求。
    stops: Arc<tokio::sync::Mutex<Vec<wire::StopRequest>>>,
    /// 收到的 interaction 应答。
    interactions: Arc<tokio::sync::Mutex<Vec<wire::InteractionResponse>>>,
    /// `get_snapshot` 的注入回复（`None` = 默认 Idle 快照；stats-wire 方案 A 测试
    /// 注入带统计的快照）。
    snapshot_reply: Arc<tokio::sync::Mutex<Option<wire::RuntimeSnapshot>>>,
    /// `logs_list` 的注入回复（`None` = 空页 `next_seq = 0`；W1-B/P9 桥游标
    /// 翻译测试注入带条目与 `next_seq` 的分片）。
    logs_reply: Arc<tokio::sync::Mutex<Option<wire::LogsListReply>>>,
    /// 快速入门编排的真实 core 边界观测：ConfigSet 与 ServiceControl 必须按批准顺序。
    quick_start_calls: Arc<tokio::sync::Mutex<Vec<QuickStartCall>>>,
    /// 收到的 `service_control` action oneof（W3-4/P4 v1 透传断言用）。
    service_control_actions:
        Arc<tokio::sync::Mutex<Vec<Option<wire::service_control_request::Action>>>>,
    /// `service_control` 的注入回复（`None` = typed 拒绝）。
    service_control_reply: Arc<tokio::sync::Mutex<Option<wire::ServiceControlReply>>>,
    /// `service_control` 的注入拒绝（`None`/`Some` 均优先于 reply；P4 v2 错误映射
    /// 测试注入服务路由前缀与稳定码）。
    service_control_error: Arc<tokio::sync::Mutex<Option<Status>>>,
    /// 实际送往 Core 的配置写请求，用于证明快速入门不会持久化输入密码。
    config_set_requests: Arc<tokio::sync::Mutex<Vec<wire::ConfigSetRequest>>>,
    /// `false` 模拟 core 拒绝了配置保存但 RPC 本身成功返回。
    config_set_ok: Arc<tokio::sync::Mutex<bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickStartCall {
    ConfigSet,
    ServiceControl,
}

impl MockKernel {
    fn new() -> Self {
        Self {
            watch_tx: Arc::new(tokio::sync::Mutex::new(None)),
            watch_resumes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            connects: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            connect_persist_metadata: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            connect_error: Arc::new(tokio::sync::Mutex::new(None)),
            stops: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            interactions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            snapshot_reply: Arc::new(tokio::sync::Mutex::new(None)),
            logs_reply: Arc::new(tokio::sync::Mutex::new(None)),
            quick_start_calls: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            service_control_actions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            service_control_reply: Arc::new(tokio::sync::Mutex::new(None)),
            service_control_error: Arc::new(tokio::sync::Mutex::new(None)),
            config_set_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            config_set_ok: Arc::new(tokio::sync::Mutex::new(true)),
        }
    }
}

type WatchStream = Pin<Box<dyn Stream<Item = Result<wire::RuntimeEvent, Status>> + Send>>;

#[tonic::async_trait]
impl KernelControl for MockKernel {
    type WatchEventsStream = WatchStream;

    async fn connect(
        &self,
        request: Request<wire::ConnectRequest>,
    ) -> Result<Response<wire::OperationReply>, Status> {
        self.connect_persist_metadata.lock().await.push(
            request
                .metadata()
                .get("x-exv-persist-credentials")
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned),
        );
        let req = request.into_inner();
        self.connects.lock().await.push(req);
        if let Some((code, message)) = self.connect_error.lock().await.clone() {
            return Err(Status::new(code, message));
        }
        Ok(Response::new(succeeded_reply()))
    }

    async fn logs_list(
        &self,
        _r: Request<wire::LogsListRequest>,
    ) -> Result<Response<wire::LogsListReply>, Status> {
        let injected = self.logs_reply.lock().await.clone();
        Ok(Response::new(injected.unwrap_or(wire::LogsListReply {
            entries: Vec::new(),
            next_seq: 0,
        })))
    }

    async fn logs_clear(
        &self,
        _r: Request<wire::LogsClearRequest>,
    ) -> Result<Response<wire::LogsClearReply>, Status> {
        Ok(Response::new(wire::LogsClearReply {
            cleared: true,
            removed_entries: 0,
        }))
    }

    async fn config_get(
        &self,
        _r: Request<wire::ConfigGetRequest>,
    ) -> Result<Response<wire::ConfigPayload>, Status> {
        Ok(Response::new(wire::ConfigPayload {
            items: Vec::new(),
            requires_quick_start: false,
        }))
    }

    async fn config_set(
        &self,
        request: Request<wire::ConfigSetRequest>,
    ) -> Result<Response<wire::ConfigReply>, Status> {
        self.config_set_requests
            .lock()
            .await
            .push(request.into_inner());
        self.quick_start_calls
            .lock()
            .await
            .push(QuickStartCall::ConfigSet);
        Ok(Response::new(wire::ConfigReply {
            ok: *self.config_set_ok.lock().await,
        }))
    }

    async fn respond_interaction(
        &self,
        request: Request<wire::InteractionResponse>,
    ) -> Result<Response<wire::OperationReply>, Status> {
        self.interactions.lock().await.push(request.into_inner());
        Ok(Response::new(succeeded_reply()))
    }

    async fn stop(
        &self,
        request: Request<wire::StopRequest>,
    ) -> Result<Response<wire::OperationReply>, Status> {
        self.stops.lock().await.push(request.into_inner());
        Ok(Response::new(succeeded_reply()))
    }

    async fn reconcile(
        &self,
        _request: Request<wire::ReconcileRequest>,
    ) -> Result<Response<wire::OperationReply>, Status> {
        Ok(Response::new(succeeded_reply()))
    }

    async fn get_operation(
        &self,
        _request: Request<wire::GetKernelOperationRequest>,
    ) -> Result<Response<wire::KernelOperationReply>, Status> {
        Ok(Response::new(wire::KernelOperationReply { state: None }))
    }

    async fn get_snapshot(
        &self,
        _request: Request<wire::SnapshotRequest>,
    ) -> Result<Response<wire::RuntimeSnapshot>, Status> {
        let injected = self.snapshot_reply.lock().await.clone();
        Ok(Response::new(injected.unwrap_or(wire::RuntimeSnapshot {
            state: Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
                last_cleanup: None,
            })),
            ..Default::default()
        })))
    }

    async fn watch_events(
        &self,
        request: Request<wire::WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        let resume_tick = request.into_inner().resume_tick;
        self.watch_resumes.lock().await.push(resume_tick);
        let tx = self.watch_tx.lock().await.clone();
        match tx {
            // 从共享 broadcast sender 派生新接收端：多次 watch_events 都得到活跃流
            //（可重开，对齐真实 core 的多订阅语义；lagged 在测试容量下不发生）。
            Some(tx) => Ok(Response::new(Box::pin(
                BroadcastStream::new(tx.subscribe())
                    .map(|item| item.map_err(|error| Status::internal(error.to_string()))),
            ))),
            None => {
                // 无事件源 → 立即 EOF（空流；测试可自行塞源）。
                Ok(Response::new(Box::pin(tokio_stream::iter(Vec::<
                    Result<wire::RuntimeEvent, Status>,
                >::new(
                )))))
            }
        }
    }

    async fn service_control(
        &self,
        request: Request<wire::ServiceControlRequest>,
    ) -> Result<Response<wire::ServiceControlReply>, Status> {
        self.service_control_actions
            .lock()
            .await
            .push(request.into_inner().action);
        self.quick_start_calls
            .lock()
            .await
            .push(QuickStartCall::ServiceControl);
        if let Some(error) = self.service_control_error.lock().await.clone() {
            return Err(error);
        }
        match self.service_control_reply.lock().await.clone() {
            Some(reply) => Ok(Response::new(reply)),
            // core 变更动作的真实错误面（P4 v2）：typed `failed_precondition` 携带
            // 稳定码（如用户取消提权；query 回复由测试注入）。
            None => Err(Status::failed_precondition(
                "DARWIN_CORE_SERVICE_ELEVATION_DENIED: 用户取消了管理员授权，服务操作未执行",
            )),
        }
    }
}

/// 确定性 succeeded `OperationReply`（receipt 带 effect_id + ownership_version）。
fn succeeded_reply() -> wire::OperationReply {
    wire::OperationReply {
        terminal: Some(wire::OperationTerminal {
            result: Some(wire::operation_terminal::Result::Succeeded(
                wire::MutationReceipt {
                    effect_id: vec![0x11; 16],
                    ownership_version: 5,
                    ..Default::default()
                },
            )),
        }),
    }
}

/// 起一个进程内 mock server，返回 (mock 句柄, channel)。
///
/// 监听 socket 先绑定再交给 server task（`serve_incoming`）：current_thread 测试
/// 运行时里 spawn 的任务要到首个 await 点才被调度，若让 server 自行 `serve(addr)`
/// 重绑端口会与客户端 connect 产生竞态（连接已被拒绝的旧端口）。
async fn serve_mock(mock: MockKernel) -> (MockKernel, Channel, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let server = Server::builder().add_service(KernelControlServer::new(mock.clone()));
    let handle = tokio::spawn(async move {
        let _ = server
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await;
    });
    // `Endpoint::from_static` 需 `'static` 字面量；动态地址用 `Endpoint::new(Uri)`
    //（tonic 0.14 `new` 返回 `Result<Endpoint, _>`，unwrap 后再 connect）。
    let uri = format!("http://{addr}")
        .parse::<http::Uri>()
        .expect("valid uri");
    let channel = Endpoint::new(uri)
        .expect("endpoint")
        .connect()
        .await
        .expect("connect to mock server");
    (mock, channel, handle)
}

/// 构造一个已拨号（Dialed）的 CoreState（测试注入 fake peer）。
fn dialed_state(channel: Channel) -> CoreState {
    CoreState {
        handle: std::sync::RwLock::new(CoreHandle::Dialed { channel }),
        last_snapshot: Default::default(),
        last_stats: Default::default(),
    }
}

/// Core 的控制通道不是启动期一次性常量：确认旧 core 已退出并重拉后，UI 必须能切换到
/// 新的已验证通道；常规刷新则只读取该状态，不扫描进程表。
#[tokio::test]
async fn core_state_replaces_dialed_handle_after_confirmed_recovery() {
    let (_mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = CoreState::default();

    assert!(state.channel().is_none(), "初始 core 状态应为已停止");
    state.replace_handle(CoreHandle::Dialed { channel });

    assert!(state.channel().is_some(), "恢复成功后应使用新的控制管道");
    state.mark_stopped();
    assert!(state.channel().is_none(), "管道断开后 UI 状态应为已停止");

    server_task.abort();
}

/// connect：真实 tonic 往返 → wire→UI 映射（succeeded + effect_id/epoch）。
#[tokio::test]
async fn core_client_connect_roundtrips_to_mock_server() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);
    let client = CoreClient;

    let reply = client
        .connect(
            &state,
            &ConnectIntent {
                profile_ref: "ecnu".to_string(),
                credentials: None,
                secret_payload: Some("ui-secret".to_string()),
            },
        )
        .await
        .expect("connect succeeds");

    let OperationResult::Succeeded {
        effect_id,
        authority_epoch,
    } = reply.result
    else {
        panic!("expected succeeded");
    };
    assert_eq!(effect_id, Some("11".repeat(16)));
    assert_eq!(authority_epoch, Some(5));

    // mock 观测到请求：well-formed intent（method=CONNECT、digest 32 字节、secret 透传）。
    let req = mock.connects.lock().await.pop().expect("connect observed");
    let intent = req.intent.expect("intent");
    let key = intent.lookup_key.as_ref().expect("key");
    assert_eq!(key.method, wire::OperationMethod::Connect as i32);
    assert_eq!(intent.request_digest.len(), 32);
    assert_eq!(req.secret_payload, b"ui-secret");

    // R4 事件关联：connect 命令必须回传 operation_id（hex16 = 32 字符），且与发给
    // core 的 wire 意图 lookup_key.operation_id 一致——前端据此只让「当前用户操作」
    // 的事件驱动 UI。
    let op_id = reply
        .operation_id
        .expect("connect reply carries operation_id");
    assert_eq!(
        op_id.len(),
        32,
        "operation_id 是 16 字节随机 id 的 hex 编码"
    );
    assert!(
        op_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "operation_id 必须是小写 hex：{op_id}"
    );
    assert_eq!(
        key.operation_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        op_id,
        "wire 意图的 operation_id 与回传前端的一致"
    );

    server_task.abort();
}

/// `persist` 是 host 的非秘密持久化选择：仅通过 Core RPC metadata 传递，不得写进
/// engine 使用的固定 `{version, username, password}` payload。
#[tokio::test]
async fn core_client_sends_persist_choice_only_as_nonsecret_metadata() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    CoreClient
        .connect(
            &state,
            &ConnectIntent {
                profile_ref: String::new(),
                credentials: Some(ConnectCredentials {
                    username: "alice".to_string(),
                    password: "secret".to_string(),
                    persist: true,
                }),
                secret_payload: None,
            },
        )
        .await
        .expect("connect succeeds");

    let metadata = mock.connect_persist_metadata.lock().await.pop();
    assert_eq!(metadata.flatten().as_deref(), Some("true"));
    let request = mock.connects.lock().await.pop().expect("connect observed");
    let payload: serde_json::Value =
        serde_json::from_slice(&request.secret_payload).expect("engine payload JSON");
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["username"], "alice");
    assert_eq!(payload["password"], "secret");
    assert!(payload.get("persist").is_none());

    server_task.abort();
}

/// stop：真实 tonic 往返（method=STOP）。
#[tokio::test]
async fn core_client_stop_roundtrips_to_mock_server() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    let reply = CoreClient.stop(&state).await.expect("stop succeeds");
    assert!(matches!(reply.result, OperationResult::Succeeded { .. }));

    let req = mock.stops.lock().await.pop().expect("stop observed");
    let intent = req.intent.expect("intent");
    assert_eq!(
        intent.lookup_key.as_ref().expect("key").method,
        wire::OperationMethod::Stop as i32
    );
    assert_eq!(intent.request_digest.len(), 32);

    server_task.abort();
}

/// snapshot：真实 tonic 往返 → wire Idle → UI Idle，并更新缓存。
#[tokio::test]
async fn core_client_snapshot_roundtrips_and_caches() {
    let (_mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    let snap = CoreClient
        .snapshot(&state)
        .await
        .expect("snapshot succeeds");
    assert!(matches!(snap.runtime, RuntimeState::Idle { .. }));
    // 缓存已更新。
    assert!(state.cached_snapshot().is_some());

    server_task.abort();
}

/// stats-wire 方案 A：GetSnapshot 携带统计 → snapshot 命令映射 `stats` 并更新缓存；
/// `stats` 命令从缓存读取（unary 拉取路径）。
#[tokio::test]
async fn core_client_snapshot_carries_stats_and_stats_command_reads_cache() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    // 注入带统计的 Idle 快照（core `GetSnapshot` 从统计投影附加）。
    *mock.snapshot_reply.lock().await = Some(wire::RuntimeSnapshot {
        state: Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
            last_cleanup: None,
        })),
        stats: Some(wire::RuntimeStats {
            rx_bytes: 1000,
            tx_bytes: 500,
            rx_rate_bps: 200,
            tx_rate_bps: 100,
            latency_ms: 7,
            phase: wire::StatsPhase::Connected as i32,
            engine_sequence: 3,
            sample_tick: 9,
        }),
        ..Default::default()
    });
    let state = dialed_state(channel);

    // snapshot 命令：wire stats → UI stats 镜像（字段映射 + phase 判别）。
    let snap = CoreClient
        .snapshot(&state)
        .await
        .expect("snapshot succeeds");
    let ui_stats = snap.stats.expect("snapshot carries stats");
    assert_eq!(ui_stats.rx_bytes, 1000);
    assert_eq!(ui_stats.tx_bytes, 500);
    assert_eq!(ui_stats.rx_rate_bps, 200);
    assert_eq!(ui_stats.tx_rate_bps, 100);
    assert_eq!(ui_stats.latency_ms, 7);
    assert_eq!(ui_stats.phase, super::stats::StatsPhase::Connected);
    assert_eq!(ui_stats.engine_sequence, 3);
    assert_eq!(ui_stats.sample_tick, 9);

    // stats 命令：从缓存读取同一份统计（无独立 RPC）。
    let cached = CoreClient.stats(&state).await.expect("stats from cache");
    assert_eq!(cached, ui_stats, "stats 命令读 snapshot 写入的缓存");

    server_task.abort();
}

/// EXV_UNFREEZE 2026-09-05（计划 §5.2）：携带 `self_heal` 的快照端到端——mock
/// GetSnapshot 注入自愈状态 → snapshot 命令 → UI 镜像字段逐一断言（respawn 成功态）。
#[tokio::test]
async fn core_client_snapshot_carries_self_heal_status() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    *mock.snapshot_reply.lock().await = Some(wire::RuntimeSnapshot {
        state: Some(wire::runtime_snapshot::State::Idle(wire::IdleState {
            last_cleanup: None,
        })),
        self_heal: Some(wire::SelfHealStatus {
            stage: "succeeded".to_string(),
            old_pid: 1111,
            new_pid: 2222,
            error_code: String::new(),
        }),
        ..Default::default()
    });
    let state = dialed_state(channel);

    let snap = CoreClient
        .snapshot(&state)
        .await
        .expect("snapshot succeeds");
    let self_heal = snap.self_heal.expect("snapshot carries self_heal");
    assert_eq!(self_heal.stage, "succeeded");
    assert_eq!(self_heal.old_pid, 1111);
    assert_eq!(self_heal.new_pid, 2222);
    assert_eq!(self_heal.error_code, "", "succeeded 态 error_code 为空");

    server_task.abort();
}

/// respond_interaction：真实 tonic 往返（interaction_id + payload 透传）。
#[tokio::test]
async fn core_client_respond_interaction_roundtrips() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    let reply = CoreClient
        .respond_interaction(&state, vec![0xAA; 16], b"answer".to_vec())
        .await
        .expect("respond succeeds");
    assert!(matches!(reply.result, OperationResult::Succeeded { .. }));

    let received = mock
        .interactions
        .lock()
        .await
        .pop()
        .expect("interaction observed");
    assert_eq!(received.interaction_id, vec![0xAA; 16]);
    assert_eq!(received.response_payload, b"answer");
    assert_eq!(
        received.runtime_epoch.len(),
        16,
        "epoch 必须 16 字节（core 校验）"
    );

    server_task.abort();
}

/// watch_events：真实 server-streaming → 事件映射（tick/kind/snapshot）。
#[tokio::test]
async fn core_client_watch_stream_maps_live_events() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let tx = broadcast::channel::<wire::RuntimeEvent>(8).0;
    *mock.watch_tx.lock().await = Some(tx.clone());

    let mut stream = CoreClient
        .open_watch_stream(&channel, 0)
        .await
        .expect("watch stream opens");

    // 向流中推一个 TRANSITION Connected 事件（broadcast send 同步；Err = 无接收端）
    // → 真实 server-streaming 送达 → UI 映射。
    let sent = tx.send(wire::RuntimeEvent {
        monotonic_tick: 9,
        kind: wire::RuntimeEventKind::Transition as i32,
        snapshot: Some(wire::RuntimeSnapshot {
            state: Some(wire::runtime_snapshot::State::Connected(
                wire::ConnectedState {
                    session: None,
                    session_established_at_ms: 0,
                },
            )),
            operation_id: vec![0x11; 16],
            ..Default::default()
        }),
        operation_id: vec![0x11; 16],
    });
    assert!(
        sent.is_ok(),
        "事件发送必须成功（receiver 在 server 流中存活）"
    );
    drop(tx);

    let ev = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
        .await
        .expect("event arrives")
        .expect("stream yields")
        .expect("event ok");
    // 流携带 wire 事件 → 经 wire→UI 映射断言 UI 视图。
    let ui = super::wire::event_from_wire(&ev);
    assert_eq!(ui.monotonic_tick, 9);
    assert_eq!(ui.kind, super::state::RuntimeEventKind::Transition);
    assert!(matches!(
        ui.snapshot.runtime,
        RuntimeState::Connected { .. }
    ));
    assert_eq!(ui.snapshot.monotonic_tick, 9, "UI snapshot 内嵌同一 tick");

    server_task.abort();
}

/// W3-1/P1 S4：resume 循环——首段流 EOF 后按 win32 语义以「本地已见最大 tick」
/// 重开订阅。mock `watch_events` 可重复调用（对齐真实 core 多订阅/resume）：第二次
/// `open_watch_stream` 成功且携带 resume 游标（= 首段已见最大 tick），重开后的流
/// 继续收到现场事件（真实 core 侧由 `EventBus::subscribe` 重放当前快照补齐断档，
/// 见 darwin-core `watch_events_resume_behind_replays_the_current_snapshot`）。
#[tokio::test]
async fn core_client_watch_stream_reopens_with_resume_tick_after_eof() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let tx = broadcast::channel::<wire::RuntimeEvent>(8).0;
    *mock.watch_tx.lock().await = Some(tx.clone());

    // 首段：resume=0，读到 tick 7 的事件。
    let mut first = CoreClient
        .open_watch_stream(&channel, 0)
        .await
        .expect("first watch stream opens");
    tx.send(wire::RuntimeEvent {
        monotonic_tick: 7,
        kind: wire::RuntimeEventKind::Snapshot as i32,
        snapshot: Some(wire::RuntimeSnapshot::default()),
        operation_id: Vec::new(),
    })
    .expect("send tick-7 event");
    let seen = tokio::time::timeout(std::time::Duration::from_secs(3), first.next())
        .await
        .expect("event arrives")
        .expect("stream yields")
        .expect("event ok");
    assert_eq!(seen.monotonic_tick, 7);

    // 首段 EOF：drop 测试侧 sender 并清空 mock 源（broadcast 通道全部 sender 释放
    // 即关闭，接收端观察到 EOF——模拟 core 进程死亡时的流终止）。
    drop(tx);
    *mock.watch_tx.lock().await = None;
    let eof = tokio::time::timeout(std::time::Duration::from_secs(3), first.next())
        .await
        .expect("eof resolves within bound");
    assert!(eof.is_none(), "首段流以 EOF 结束");

    // 重开：以已见最大 tick 为 resume 游标——重开成功且游标送达 core。
    let tx2 = broadcast::channel::<wire::RuntimeEvent>(8).0;
    *mock.watch_tx.lock().await = Some(tx2.clone());
    let mut second = CoreClient
        .open_watch_stream(&channel, 7)
        .await
        .expect("watch stream reopens after EOF (no session replacement)");
    assert_eq!(
        *mock.watch_resumes.lock().await,
        vec![0, 7],
        "重开携带 resume=本地已见最大 tick"
    );
    tx2.send(wire::RuntimeEvent {
        monotonic_tick: 8,
        kind: wire::RuntimeEventKind::Transition as i32,
        snapshot: Some(wire::RuntimeSnapshot::default()),
        operation_id: Vec::new(),
    })
    .expect("send tick-8 event into the reopened stream");
    let live = tokio::time::timeout(std::time::Duration::from_secs(3), second.next())
        .await
        .expect("reopened stream yields")
        .expect("live event exists")
        .expect("live event ok");
    assert_eq!(live.monotonic_tick, 8, "重开后的流继续收现场事件");

    server_task.abort();
}

fn quick_start_items() -> Vec<ConfigItem> {
    vec![
        ConfigItem {
            key: "server".to_string(),
            value: "vpn.example.test".to_string(),
        },
        ConfigItem {
            key: "username".to_string(),
            value: "student".to_string(),
        },
        ConfigItem {
            key: "password".to_string(),
            value: "one-time-password".to_string(),
        },
        ConfigItem {
            key: "remember_password".to_string(),
            value: "false".to_string(),
        },
    ]
}

/// 快速入门必须先让 Core 成功保存配置；darwin 无服务形态——`install_service=true`
/// 不得触碰 core 的 ServiceControl（darwin core 该 RPC 为 Unimplemented），配置
/// 保存成功 + 服务不可用以 `ok=false` 如实回报。
#[tokio::test]
async fn quick_start_apply_saves_config_and_reports_darwin_service_bypass() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    let reply = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items: quick_start_items(),
            install_service: true,
        },
    )
    .await
    .expect("quick start config path succeeds");

    // darwin 诚实语义：配置已真实保存，服务安装改由服务面板单独发起 → ok=false +
    // 可读 message。
    assert!(!reply.ok, "darwin 快速入门不代为安装服务，不得报告整体成功");
    assert!(reply.service_status.is_none());
    assert!(
        reply.message.contains("服务面板"),
        "回复必须指向服务面板：{}",
        reply.message
    );
    assert_eq!(
        *mock.quick_start_calls.lock().await,
        vec![QuickStartCall::ConfigSet],
        "darwin 旁路不得调用 core ServiceControl（Unimplemented）"
    );
    let config_set = mock
        .config_set_requests
        .lock()
        .await
        .pop()
        .expect("ConfigSet observed");
    assert!(
        config_set
            .items
            .iter()
            .any(|item| item.key == "password" && item.value == "one-time-password"),
        "quick-start must pass password to Core's encrypted-storage boundary"
    );
    assert!(
        config_set
            .items
            .iter()
            .any(|item| item.key == "remember_password" && item.value == "true"),
        "非空密码必须派生出 remember_password=true 交由 ConfigSet 加密保存"
    );
    server_task.abort();
}

/// 快速入门的「记住密码」由密码输入是否非空派生，不再读调用方的旧标志：
/// 非空密码即使旧标志为 false 也必须记住；密码留空（或遗漏 remember_password）
/// 必须派生成 false，交由 ConfigSet 清除旧密文。两项都必须进入 prepared，
/// 且不得与调用方输入的键重复。
#[tokio::test]
async fn quick_start_apply_derives_remember_password_from_password_input() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    let mut items = quick_start_items();
    items
        .iter_mut()
        .find(|item| item.key == "remember_password")
        .expect("quick-start fixture carries stale flag")
        .value = "false".to_string();

    let reply = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items,
            install_service: false,
        },
    )
    .await
    .expect("non-empty password is valid quick-start input");

    assert!(reply.ok);
    let config_set = mock
        .config_set_requests
        .lock()
        .await
        .pop()
        .expect("ConfigSet observed");
    assert!(
        config_set
            .items
            .iter()
            .any(|item| item.key == "remember_password" && item.value == "true"),
        "非空密码必须派生 remember_password=true，不能被旧标志覆盖"
    );
    assert!(
        config_set
            .items
            .iter()
            .any(|item| item.key == "password" && item.value == "one-time-password"),
        "password must reach Core's existing encrypted-storage boundary"
    );
    assert_eq!(
        config_set
            .items
            .iter()
            .filter(|item| item.key == "remember_password")
            .count(),
        1,
        "派生标志不得与调用方输入重复"
    );

    // 密码留空，且完全不含 remember_password：按留空处理并派生 false。
    let mut empty_items = quick_start_items();
    empty_items.retain(|item| item.key != "remember_password");
    empty_items
        .iter_mut()
        .find(|item| item.key == "password")
        .expect("quick-start fixture carries password")
        .value = String::new();

    let empty_reply = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items: empty_items,
            install_service: false,
        },
    )
    .await
    .expect("empty password is valid quick-start input");

    assert!(empty_reply.ok);
    let empty_set = mock
        .config_set_requests
        .lock()
        .await
        .pop()
        .expect("ConfigSet observed for empty password");
    assert!(
        empty_set
            .items
            .iter()
            .any(|item| item.key == "remember_password" && item.value == "false"),
        "留空密码必须派生 remember_password=false 以清除旧密文"
    );
    assert!(
        empty_set
            .items
            .iter()
            .any(|item| item.key == "password" && item.value.is_empty()),
        "留空密码仍须进入 Core 的清除边界"
    );
    server_task.abort();
}

/// Core 的 ConfigSet 以 `ok=false` 表示保存未完成时，快速入门必须停止，不能执行
/// 后续副作用。
#[tokio::test]
async fn quick_start_apply_does_not_install_when_config_save_is_unsuccessful() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    *mock.config_set_ok.lock().await = false;
    let state = dialed_state(channel);

    let error = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items: quick_start_items(),
            install_service: true,
        },
    )
    .await
    .expect_err("unsuccessful config save must fail quick start");

    assert!(matches!(error, super::error::AppError::Internal(_)));
    assert_eq!(
        *mock.quick_start_calls.lock().await,
        vec![QuickStartCall::ConfigSet],
        "no further side effects after a failed config save"
    );
    server_task.abort();
}

/// 快速入门只接受批准的 Core 配置键；重复或未知字段必须在抵达 core 前拒绝，
/// 因而不会产生配置写入副作用。
#[tokio::test]
async fn quick_start_apply_rejects_unknown_or_duplicate_config_keys_before_side_effects() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);
    let mut items = quick_start_items();
    items.push(ConfigItem {
        key: "unapproved".to_string(),
        value: "value".to_string(),
    });

    let error = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items,
            install_service: true,
        },
    )
    .await
    .expect_err("unknown configuration keys must be rejected");

    assert!(matches!(error, super::error::AppError::Internal(_)));
    let mut duplicate_items = quick_start_items();
    duplicate_items.push(ConfigItem {
        key: "server".to_string(),
        value: "duplicate.example.test".to_string(),
    });
    let duplicate_error = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items: duplicate_items,
            install_service: true,
        },
    )
    .await
    .expect_err("duplicate configuration keys must be rejected");
    assert!(matches!(
        duplicate_error,
        super::error::AppError::Internal(_)
    ));
    assert!(
        mock.quick_start_calls.lock().await.is_empty(),
        "invalid input must not write config or trigger side effects"
    );
    server_task.abort();
}

/// darwin `service_control` 透传 core（W3-4/P4 v1，旧旁路退役）：query 的 wire
/// oneof 到达 core、回复（含五态词汇 `health_state`）经 wire→UI 镜像到达前端；
/// core 变更动作的 typed 拒绝（CLI 指引）经 `map_status` 透传为 Internal。
#[tokio::test]
async fn service_control_query_passes_through_and_actions_map_core_rejections() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    // 注入 core query 探测结果（healthy 档；mock 只验证透传与映射，具体事实判据
    // 在 core `service_status` 模块的表驱动测试钉死）。
    *mock.service_control_reply.lock().await = Some(wire::ServiceControlReply {
        service_status: Some(wire::ServiceStatus {
            installed: true,
            state: "running".to_string(),
            binary_path:
                "/Library/Application Support/EXV/ServiceAgent/exv-vpn-darwin-service-agent"
                    .to_string(),
            health_state: "healthy".to_string(),
        }),
        ok: true,
        message: "darwin 服务代理探测完成（binary/socket/Status 三维度）".to_string(),
    });
    let state = dialed_state(channel);

    let reply = super::commands::service_control_via_core(&CoreClient, &state, ServiceControlAction::Query)
        .await
        .expect("query passes through to core");
    assert!(reply.ok, "query 透传 core 的受理事实");
    let status = reply
        .service_status
        .expect("query must map core service_status");
    assert_eq!(status.health_state.as_deref(), Some("healthy"));
    assert!(status.installed);
    assert_eq!(status.state, "running");
    assert!(status.binary_path.is_some());
    assert!(!reply.message.is_empty());
    assert!(
        matches!(
            mock.service_control_actions.lock().await.as_slice(),
            [Some(wire::service_control_request::Action::Query(_))]
        ),
        "UI action 必须以 wire oneof 透传到 core"
    );

    // 变更动作：mock core typed 拒绝（默认 = 提权取消稳定码）→ Internal 携带稳定码。
    *mock.service_control_reply.lock().await = None;
    *mock.service_control_error.lock().await = None;
    for action in [
        ServiceControlAction::Install,
        ServiceControlAction::Uninstall,
        ServiceControlAction::Start,
        ServiceControlAction::RotateKey,
    ] {
        let error = super::commands::service_control_via_core(&CoreClient, &state, action)
            .await
            .expect_err("变更动作透传 core 的 typed 拒绝");
        assert!(
            matches!(error, super::error::AppError::Internal(ref message) if message.contains("DARWIN_CORE_SERVICE_ELEVATION_DENIED")),
            "变更动作以 Internal + core 稳定码透传：{error:?}"
        );
    }
    assert_eq!(
        mock.service_control_actions.lock().await.len(),
        5,
        "全部 5 个 action 均到达 core"
    );

    // P4 v2 服务路由前缀 → typed 恢复 modal 变体（win32 同款分流）。
    *mock.service_control_error.lock().await = Some(Status::failed_precondition(
        "service_not_running|darwin 服务未安装，请先在服务面板安装服务",
    ));
    let not_running = super::commands::service_control_via_core(
        &CoreClient,
        &state,
        ServiceControlAction::Install,
    )
    .await
    .expect_err("prefix must map to the recovery kind");
    assert!(
        matches!(not_running, super::error::AppError::ServiceNotRunning(ref m) if m.contains("未安装")),
        "service_not_running 前缀映射恢复 modal 变体：{not_running:?}"
    );

    *mock.service_control_error.lock().await = Some(Status::failed_precondition(
        "service_connect_failed|DARWIN_CORE_ENGINE_ELEVATION_FAILED: 服务代理会话建立失败",
    ));
    let connect_failed = super::commands::service_control_via_core(
        &CoreClient,
        &state,
        ServiceControlAction::Start,
    )
    .await
    .expect_err("prefix must map to the recovery kind");
    assert!(
        matches!(connect_failed, super::error::AppError::ServiceConnectFailed(ref m) if m.contains("DARWIN_CORE_ENGINE_ELEVATION_FAILED")),
        "service_connect_failed 前缀映射恢复 modal 变体：{connect_failed:?}"
    );
    // P4 v2 状态拆分：未安装（无前缀、携带稳定码）→ ServiceNotInstalled（不弹恢复
    // modal，toast 安装指引）。
    *mock.service_control_error.lock().await = Some(Status::failed_precondition(
        "DARWIN_CORE_SERVICE_NOT_INSTALLED: 服务未安装：连接需要系统服务在位。请在连接页勾选「先安装服务再连接」，或到服务面板安装服务",
    ));
    let not_installed = super::commands::service_control_via_core(
        &CoreClient,
        &state,
        ServiceControlAction::Start,
    )
    .await
    .expect_err("stable code must map to the not-installed kind");
    assert!(
        matches!(not_installed, super::error::AppError::ServiceNotInstalled(ref m) if m.contains("先安装服务再连接")),
        "未安装稳定码映射独立 kind：{not_installed:?}"
    );
    server_task.abort();
}

/// darwin `tunnel_address` 实装判据（W2-A/P5）：注入伪接口事实验证纯判据四分支——
/// 单候选命中 / 零候选（无 IPv4、仅链路本地）None / 双候选歧义诚实 None /
/// fake-ip 上游 TUN 排除后命中。命令层无 State 无缓存可重入，仅 getifaddrs 枚举
/// 失败才报错；真机两态（未连接 None/已连接与 ifconfig 一致）验收挂用户窗口。
#[test]
fn tunnel_address_criteria_resolve_exv_utun_from_injected_facts() {
    use std::net::Ipv4Addr;

    fn fact(name: &str, addresses: &[Ipv4Addr]) -> (String, Vec<Ipv4Addr>) {
        (name.to_owned(), addresses.to_vec())
    }

    // 单候选命中：非 utun 接口（含持链路本地的 awdl0）不参与判定。
    let single = vec![
        fact("en0", &[Ipv4Addr::new(192, 168, 1, 2)]),
        fact("awdl0", &[Ipv4Addr::new(169, 254, 9, 9)]),
        fact("utun5", &[Ipv4Addr::new(10, 231, 0, 1)]),
    ];
    assert_eq!(
        super::adapter_address::first_ipv4_from_facts(&single),
        Some(Ipv4Addr::new(10, 231, 0, 1))
    );

    // 零候选：无 IPv4（E3）与仅持链路本地（E2）的 utun 均被排除。
    let zero = vec![
        fact("en0", &[Ipv4Addr::new(192, 168, 1, 2)]),
        fact("utun3", &[]),
        fact("utun4", &[Ipv4Addr::new(169, 254, 100, 116)]),
    ];
    assert_eq!(super::adapter_address::first_ipv4_from_facts(&zero), None);

    // 双候选：歧义诚实 None（不按枚举序猜）。
    let ambiguous = vec![
        fact("utun5", &[Ipv4Addr::new(10, 231, 0, 1)]),
        fact("utun6", &[Ipv4Addr::new(10, 232, 0, 1)]),
    ];
    assert_eq!(
        super::adapter_address::first_ipv4_from_facts(&ambiguous),
        None
    );

    // fake-ip 排除后命中：持 198.18.0.0/15 的上游代理 TUN（E1，Mihomo 共存形态）
    // 不误报，剩余唯一 EXV utun 胜出。
    let coexisting = vec![
        fact("utun1024", &[Ipv4Addr::new(198, 18, 0, 1)]),
        fact("utun7", &[Ipv4Addr::new(100, 64, 0, 1)]),
    ];
    assert_eq!(
        super::adapter_address::first_ipv4_from_facts(&coexisting),
        Some(Ipv4Addr::new(100, 64, 0, 1))
    );
}

/// darwin `trigger_latency_refresh` 旁路：v1 no-op 受理（周期探测并行车道实现）。
#[tokio::test]
async fn trigger_latency_refresh_is_an_accepted_noop() {
    assert!(super::commands::trigger_latency_refresh().await.is_ok());
}

/// darwin `open_external` 打开面约束：非 http(s) URL 必须在触碰 AppKit 前被拒绝
///（命令经 [`super::external_open`] seam；真实 LaunchServices 打开见该模块单测与
/// 真机验证——单测不触发真实浏览器）。
#[test]
fn open_external_validation_rejects_non_http_before_opening() {
    let mut opened = false;
    let error = super::external_open::open_with("file:///etc/passwd", |_| {
        opened = true;
        Ok(())
    })
    .expect_err("非 http(s) 必须被拒绝");
    assert!(!opened, "拒绝路径不得触碰真实打开器");
    assert!(matches!(error, error if error.contains("http")));
}

/// darwin `open_external` 打开面约束：合法 http(s) URL 原样交给打开器。
#[test]
fn open_external_forwards_valid_url_to_opener() {
    let mut seen = None;
    super::external_open::open_with("https://github.com/HiderWild/exv-ecnuvpn/", |url| {
        seen = Some(url.to_string());
        Ok(())
    })
    .expect("合法 URL 应交给打开器");
    assert_eq!(
        seen.as_deref(),
        Some("https://github.com/HiderWild/exv-ecnuvpn/")
    );
}

/// 未拨号（NotWired）时命令必须 fail closed（CoreUnreachable，不 panic）。
#[tokio::test]
async fn core_client_fails_closed_when_not_dialed() {
    let state = CoreState::default();
    assert!(state.channel().is_none());

    let err = CoreClient
        .connect(
            &state,
            &ConnectIntent {
                profile_ref: String::new(),
                credentials: None,
                secret_payload: None,
            },
        )
        .await
        .expect_err("must fail closed");
    assert!(matches!(err, super::error::AppError::CoreUnreachable(_)));
}

/// 未拨号时 logs/config 返回 CoreUnreachable（已真实接线，需 core 通道）；
/// stats 缓存无样本仍返回 typed NotWired（前端「暂无统计」，不视为失败）。
#[tokio::test]
async fn core_client_logs_config_require_dialed_stats_gap_not_wired() {
    let state = CoreState::default();
    let err = CoreClient
        .logs_list(&state, 0, 100)
        .await
        .expect_err("logs_list requires dialed core");
    assert!(matches!(err, super::error::AppError::CoreUnreachable(_)));

    let err = CoreClient
        .logs_clear(&state)
        .await
        .expect_err("logs_clear requires dialed core");
    assert!(matches!(err, super::error::AppError::CoreUnreachable(_)));

    let err = CoreClient
        .config_get(&state)
        .await
        .expect_err("config_get requires dialed core");
    assert!(matches!(err, super::error::AppError::CoreUnreachable(_)));

    let err = CoreClient
        .config_set(&state, Vec::new())
        .await
        .expect_err("config_set requires dialed core");
    assert!(matches!(err, super::error::AppError::CoreUnreachable(_)));

    // stats-wire 方案 A：stats 随 RuntimeSnapshot 携带（不再独立 RPC）；缓存尚无
    // 样本（未连接）→ typed NotWired 占位（前端「暂无统计」，不视为失败）。
    let err = CoreClient.stats(&state).await.expect_err("no stats yet");
    assert!(matches!(err, super::error::AppError::NotWired(_)));

    // NotWired 是 serde 序列化的（随 invoke 返回前端）——形状必须稳定。
    let json = serde_json::to_value(err).expect("serialize");
    assert_eq!(json["kind"], "not_wired");
}

/// W1-B（P9）：前端桥必须把 core 的 `next_seq`（末条 seq + 1）翻译成
/// last-seen 游标——core 增量过滤是严格 `seq > after_seq`，若把 `next_seq`
/// 原样回传，前端每轮续拉恰漏 seq 恰等的那一条。
#[tokio::test]
async fn core_client_logs_list_translates_next_seq_into_last_seen_cursor() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    // 伪 core：3 条（seq 8..=10），末条 seq = 10 → next_seq = 11。
    *mock.logs_reply.lock().await = Some(wire::LogsListReply {
        entries: vec![
            wire::LogEvent {
                message: "e8".to_owned(),
                ..Default::default()
            },
            wire::LogEvent {
                message: "e9".to_owned(),
                ..Default::default()
            },
            wire::LogEvent {
                message: "e10".to_owned(),
                ..Default::default()
            },
        ],
        next_seq: 11,
    });
    let state = dialed_state(channel);

    let chunk = CoreClient
        .logs_list(&state, 7, 100)
        .await
        .expect("logs_list succeeds");
    assert_eq!(chunk.events.len(), 3);
    assert_eq!(
        chunk.next_after_seq, 10,
        "next_after_seq = 末条 seq（last-seen），不是 core 的 next_seq"
    );
    assert!(!chunk.has_more);

    // 追平空页（core：next_seq = last_seq + 1）：游标钳制不回退（= 本次 after_seq）。
    *mock.logs_reply.lock().await = Some(wire::LogsListReply {
        entries: Vec::new(),
        next_seq: 11,
    });
    let caught = CoreClient
        .logs_list(&state, 10, 100)
        .await
        .expect("logs_list succeeds");
    assert!(caught.events.is_empty());
    assert_eq!(
        caught.next_after_seq, 10,
        "空页游标 = max(next_seq - 1, after_seq)，不回退"
    );

    server_task.abort();
}

/// RPC 拒绝（mock 返回 Unauthenticated）→ 非 transport 类错误映射为 Internal。
#[tokio::test]
async fn core_client_maps_business_rejection_to_internal() {
    struct Rejecting;
    #[tonic::async_trait]
    impl KernelControl for Rejecting {
        type WatchEventsStream = WatchStream;
        async fn connect(
            &self,
            _r: Request<wire::ConnectRequest>,
        ) -> Result<Response<wire::OperationReply>, Status> {
            Err(Status::unauthenticated("not authorized"))
        }
        async fn logs_list(
            &self,
            _r: Request<wire::LogsListRequest>,
        ) -> Result<Response<wire::LogsListReply>, Status> {
            Err(Status::unauthenticated("not authorized"))
        }
        async fn logs_clear(
            &self,
            _r: Request<wire::LogsClearRequest>,
        ) -> Result<Response<wire::LogsClearReply>, Status> {
            Err(Status::unauthenticated("not authorized"))
        }
        async fn config_get(
            &self,
            _r: Request<wire::ConfigGetRequest>,
        ) -> Result<Response<wire::ConfigPayload>, Status> {
            Err(Status::unauthenticated("not authorized"))
        }
        async fn config_set(
            &self,
            _r: Request<wire::ConfigSetRequest>,
        ) -> Result<Response<wire::ConfigReply>, Status> {
            Err(Status::unauthenticated("not authorized"))
        }
        async fn respond_interaction(
            &self,
            _r: Request<wire::InteractionResponse>,
        ) -> Result<Response<wire::OperationReply>, Status> {
            unreachable!()
        }
        async fn stop(
            &self,
            _r: Request<wire::StopRequest>,
        ) -> Result<Response<wire::OperationReply>, Status> {
            unreachable!()
        }
        async fn reconcile(
            &self,
            _r: Request<wire::ReconcileRequest>,
        ) -> Result<Response<wire::OperationReply>, Status> {
            unreachable!()
        }
        async fn get_operation(
            &self,
            _r: Request<wire::GetKernelOperationRequest>,
        ) -> Result<Response<wire::KernelOperationReply>, Status> {
            unreachable!()
        }
        async fn get_snapshot(
            &self,
            _r: Request<wire::SnapshotRequest>,
        ) -> Result<Response<wire::RuntimeSnapshot>, Status> {
            unreachable!()
        }
        async fn watch_events(
            &self,
            _r: Request<wire::WatchEventsRequest>,
        ) -> Result<Response<Self::WatchEventsStream>, Status> {
            unreachable!()
        }
        async fn service_control(
            &self,
            _r: Request<wire::ServiceControlRequest>,
        ) -> Result<Response<wire::ServiceControlReply>, Status> {
            unreachable!()
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let server = Server::builder().add_service(KernelControlServer::new(Rejecting));
    let handle = tokio::spawn(async move {
        let _ = server
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await;
    });
    let uri = format!("http://{addr}")
        .parse::<http::Uri>()
        .expect("valid uri");
    let channel = Endpoint::new(uri)
        .expect("endpoint")
        .connect()
        .await
        .expect("connect");
    let state = dialed_state(channel);

    let err = CoreClient
        .connect(
            &state,
            &ConnectIntent {
                profile_ref: String::new(),
                credentials: None,
                secret_payload: None,
            },
        )
        .await
        .expect_err("business rejection");
    assert!(
        matches!(err, super::error::AppError::Internal(_)),
        "非 transport 拒绝必须映射为 Internal（core 语义拒绝 ≠ 掉线），got {err:?}"
    );
    handle.abort();
}

/// darwin 凭据缺失稳定码 → typed CredentialRequired（前端凭据模态触发）；错误边界
/// 不得序列化秘密。
#[tokio::test]
async fn maps_darwin_credential_required_status_without_secret() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    *mock.connect_error.lock().await = Some((
        tonic::Code::FailedPrecondition,
        "DARWIN_CORE_CONNECT_CREDENTIALS_MISSING".to_string(),
    ));
    let state = dialed_state(channel);

    let error = CoreClient
        .connect(
            &state,
            &ConnectIntent {
                profile_ref: String::new(),
                credentials: None,
                secret_payload: None,
            },
        )
        .await
        .expect_err("credential recovery status must reject connect");

    let json = serde_json::to_value(error.clone()).expect("serialize app error");
    assert_eq!(json["kind"], "credential_required");
    assert_eq!(json["code"], "DARWIN_CORE_CONNECT_CREDENTIALS_MISSING");
    assert!(json.get("password").is_none());
    assert!(
        matches!(error, super::error::AppError::CredentialRequired { .. }),
        "typed 变体必须保留给前端凭据分流"
    );
    server_task.abort();
}
