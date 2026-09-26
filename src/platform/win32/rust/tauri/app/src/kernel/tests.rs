//! P4-b 集成测试：CoreClient 真实 tonic 调用 ↔ 进程内 mock `KernelControl` server。
//!
//! 用 localhost TCP 的 tonic server（mock 实现 `kernel_control_server::KernelControl`
//! trait，复用 Root workspace 的生成类型）验证 CoreClient 的 unary + streaming
//! 端到端：请求组装 → 发送 → 响应 wire→UI 映射。named-pipe transport 的真实拨号
//! 由 [`super::core_transport`] 的单测覆盖（同进程 local pipe）；core 二进制缺失
//! （P5 落 host main）使进程级 smoke 受环境阻塞——本文件提供语义层的最强可行验证。

#![cfg(test)]

use std::pin::Pin;
use std::sync::Arc;

use exv_vpn_wire::generated::kernel_control_server::{KernelControl, KernelControlServer};
use exv_vpn_wire::generated::{self as wire};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Channel, Endpoint, Server};
use tonic::{Request, Response, Status};

use super::client::{ConfigItem, ConnectCredentials, ConnectIntent, CoreClient, CoreHandle, CoreState};
use super::state::{OperationResult, RuntimeState};

/// mock `KernelControl` server：观测请求并回确定性响应（watch_events 用 mpsc 流）。
#[derive(Clone)]
struct MockKernel {
    /// watch_events 的待发事件流（测试注入 `mpsc::Receiver`；None = 立即 EOF）。
    watch_rx: Arc<tokio::sync::Mutex<Option<mpsc::Receiver<wire::RuntimeEvent>>>>,
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
    /// 观察送往 Core ConfigSet 边界的密码与派生记住标志；实际落盘由真实 Core 驱动验证。
    config_set_requests: Arc<tokio::sync::Mutex<Vec<wire::ConfigSetRequest>>>,
    /// `false` 模拟 core 拒绝了配置保存但 RPC 本身成功返回。
    config_set_ok: Arc<tokio::sync::Mutex<bool>>,
    /// 可注入的服务控制回复（用于验证安装失败不得伪造完成）。
    service_control_reply: Arc<tokio::sync::Mutex<wire::ServiceControlReply>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickStartCall {
    ConfigSet,
    ServiceControl,
}

impl MockKernel {
    fn new() -> Self {
        Self {
            watch_rx: Arc::new(tokio::sync::Mutex::new(None)),
            connects: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            connect_persist_metadata: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            connect_error: Arc::new(tokio::sync::Mutex::new(None)),
            stops: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            interactions: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            snapshot_reply: Arc::new(tokio::sync::Mutex::new(None)),
            logs_reply: Arc::new(tokio::sync::Mutex::new(None)),
            quick_start_calls: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            config_set_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            config_set_ok: Arc::new(tokio::sync::Mutex::new(true)),
            service_control_reply: Arc::new(tokio::sync::Mutex::new(wire::ServiceControlReply {
                service_status: None,
                ok: true,
                message: "mock service_control".to_string(),
            })),
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
            stats: None,
            proxy_tun: None,
            system_proxy: None,
            reconnect: None,
            self_heal: None,
            operation_id: vec![],

            service_status: None,
            mode: String::new(),
        })))
    }

    async fn watch_events(
        &self,
        _request: Request<wire::WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        let rx = self.watch_rx.lock().await.take();
        match rx {
            // `ReceiverStream` 产出 `RuntimeEvent`；服务契约要求 `Result<_, Status>`——
            // map 到 Ok 满足 tonic server-streaming 签名。
            Some(rx) => Ok(Response::new(Box::pin(ReceiverStream::new(rx).map(Ok)))),
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
        _request: Request<wire::ServiceControlRequest>,
    ) -> Result<Response<wire::ServiceControlReply>, Status> {
        self.quick_start_calls
            .lock()
            .await
            .push(QuickStartCall::ServiceControl);
        Ok(Response::new(
            self.service_control_reply.lock().await.clone(),
        ))
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
async fn serve_mock(mock: MockKernel) -> (MockKernel, Channel, tokio::task::JoinHandle<()>) {
    let addr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("local addr")
    };
    let server = Server::builder().add_service(KernelControlServer::new(mock.clone()));
    let handle = tokio::spawn(async move {
        let _ = server.serve(addr).await;
    });
    // `Endpoint::from_static` 需 `'static` 字面量；动态地址用 `Endpoint::new(Uri)`
    // （tonic 0.14 `new` 返回 `Result<Endpoint, _>`，unwrap 后再 connect）。
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
            ConnectIntent {
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
    // host 的 wire 意图 lookup_key.operation_id 一致——前端据此只让「当前用户操作」
    // 的事件驱动 UI。
    let op_id = reply
        .operation_id
        .expect("connect reply carries operation_id");
    assert_eq!(op_id.len(), 32, "operation_id 是 16 字节 UUID 的 hex 编码");
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
            ConnectIntent {
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
    // 注入带统计的 Idle 快照（host `GetSnapshot` 从 EventBus 统计 lane 附加）。
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
        proxy_tun: None,
        system_proxy: None,
        reconnect: None,
        self_heal: None,
        operation_id: vec![],

        service_status: None,
        mode: String::new(),
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
        stats: None,
        proxy_tun: None,
        system_proxy: None,
        reconnect: None,
        self_heal: Some(wire::SelfHealStatus {
            stage: "succeeded".to_string(),
            old_pid: 1111,
            new_pid: 2222,
            error_code: String::new(),
        }),
        operation_id: vec![],

        service_status: None,
        mode: String::new(),
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
    use tokio_stream::StreamExt;

    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let (tx, rx) = mpsc::channel::<wire::RuntimeEvent>(8);
    *mock.watch_rx.lock().await = Some(rx);

    let mut stream = CoreClient
        .open_watch_stream(&channel, 0)
        .await
        .expect("watch stream opens");

    // 向流中推一个 TRANSITION Connected 事件（必须 `.await` 使发送生效；裸 `let _ = send`
    // 会 drop future 而不发送）→ 真实 server-streaming 送达 → UI 映射。
    let sent = tx
        .send(wire::RuntimeEvent {
            monotonic_tick: 9,
            kind: wire::RuntimeEventKind::Transition as i32,
            snapshot: Some(wire::RuntimeSnapshot {
                state: Some(wire::runtime_snapshot::State::Connected(
                    wire::ConnectedState {
                        session: None,
                        session_established_at_ms: 0,
                    },
                )),
                stats: None,
                proxy_tun: None,
                system_proxy: None,
                reconnect: None,
                self_heal: None,
                operation_id: vec![0x11; 16],

                service_status: None,
                mode: String::new(),
            }),
            operation_id: vec![0x11; 16],
        })
        .await;
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

/// 快速入门必须先让 Core 成功保存配置，再独立安装服务；非空密码必须在此边界派生
/// remember_password=true，即使旧调用方仍传入 false。
#[tokio::test]
async fn quick_start_apply_saves_config_before_installing_service() {
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
    .expect("quick start succeeds");

    assert!(reply.ok);
    assert_eq!(
        *mock.quick_start_calls.lock().await,
        vec![QuickStartCall::ConfigSet, QuickStartCall::ServiceControl],
        "the service install may run only after ConfigSet"
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
        "nonempty quick-start password must override the obsolete false flag"
    );
    server_task.abort();
}

/// 前端 `DEFAULT_QUICK_START_CORE_DRAFT` 的完整键集必须被接受，并原样送达 Core。
///
/// 回归护栏：2026-09-21 用户可见的「快速入门提交失败，请重试。」源于 `90c2e1a44` 单侧把
/// `connection_mode` 加进前端草稿、`ALLOWED_CONFIG_KEYS` 未跟，`quick_start_apply` 在抵达
/// Core 之前整批拒绝——Core 日志里因此查不到任何提交记录。既有用例只喂 4 个最小键，
/// 覆盖不到前端真实键集，所以漏了。这里按前端真实键集提交：任何一侧再单独加键，
/// 都会先在这里失败。
#[tokio::test]
async fn quick_start_apply_accepts_the_full_frontend_draft_key_set() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    // 与 `product/quick-start.ts` 的 `DEFAULT_QUICK_START_CORE_DRAFT` 逐键一致。
    let items: Vec<ConfigItem> = [
        ("server", "vpn.example.test"),
        ("username", "student"),
        ("password", "one-time-password"),
        ("remember_password", "false"),
        ("connection_mode", "standard"),
        ("routes", "49.52.4.0/25,219.228.144.0/22"),
        ("user_agent", "AnyConnect Win_x86_64 4.10.05095"),
        ("mtu", "1290"),
        ("auto_reconnect", "false"),
        ("auto_reconnect_max_attempts", "0"),
        ("auto_reconnect_backoff", "false"),
    ]
    .into_iter()
    .map(|(key, value)| ConfigItem {
        key: key.to_string(),
        value: value.to_string(),
    })
    .collect();

    let reply = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items,
            install_service: false,
        },
    )
    .await
    .expect("前端草稿的完整键集必须被快速入门接受");

    assert!(reply.ok, "{}", reply.message);
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
            .any(|item| item.key == "connection_mode" && item.value == "standard"),
        "connection_mode 必须原样送达 Core，而不是被白名单挡在门外"
    );
    server_task.abort();
}

/// 留空密码也能完成快速入门，且必须显式清除旧密码，不能沿用旧调用方的 true 标志。
#[tokio::test]
async fn quick_start_apply_clears_password_despite_obsolete_remember_flag() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);
    let mut items = quick_start_items();
    items
        .iter_mut()
        .find(|item| item.key == "remember_password")
        .expect("quick-start fixture carries checkbox")
        .value = "true".to_string();
    items
        .iter_mut()
        .find(|item| item.key == "password")
        .expect("quick-start fixture carries password")
        .value
        .clear();

    let reply = super::quick_start::apply(
        &CoreClient,
        &state,
        super::quick_start::QuickStartApplyRequest {
            items,
            install_service: false,
        },
    )
    .await
    .expect("empty password is valid quick-start input");

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
            .any(|item| item.key == "remember_password" && item.value == "false"),
        "empty quick-start password must override the obsolete true flag"
    );
    assert!(
        config_set
            .items
            .iter()
            .any(|item| item.key == "password" && item.value.is_empty()),
        "an explicit empty password must clear Core's previously saved password"
    );
    server_task.abort();
}

/// 快速入门按原始密码是否为空派生保存标志，不得 trim 密码，也不依赖旧标志存在。
#[tokio::test]
async fn quick_start_apply_preserves_nonempty_password_bytes_without_remember_flag() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    let state = dialed_state(channel);

    for password in ["  synthetic password\t", " \t"] {
        let mut items = quick_start_items();
        items.retain(|item| item.key != "remember_password");
        items
            .iter_mut()
            .find(|item| item.key == "password")
            .expect("quick-start fixture carries password")
            .value = password.to_string();

        let reply = super::quick_start::apply(
            &CoreClient,
            &state,
            super::quick_start::QuickStartApplyRequest {
                items,
                install_service: false,
            },
        )
        .await
        .expect("a nonempty password must be accepted verbatim");
        assert!(reply.ok);
        let config_set = mock
            .config_set_requests
            .lock()
            .await
            .pop()
            .expect("ConfigSet observed");
        assert!(config_set
            .items
            .iter()
            .any(|item| item.key == "password" && item.value == password));
        assert!(config_set
            .items
            .iter()
            .any(|item| item.key == "remember_password" && item.value == "true"));
    }

    server_task.abort();
}

/// Core 的 ConfigSet 以 `ok=false` 表示保存未完成时，快速入门必须停止，不能执行
/// 服务安装这个后续副作用。
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
        "service install must not happen after a failed config save"
    );
    server_task.abort();
}

/// 服务安装是配置提交之后的独立动作；服务层明确失败时，前端必须拿到非成功回复，
/// 而不是被伪装成整个快速入门成功。
#[tokio::test]
async fn quick_start_apply_returns_service_failure_to_frontend() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    *mock.service_control_reply.lock().await = wire::ServiceControlReply {
        service_status: None,
        ok: false,
        message: "install denied".to_string(),
    };
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
    .expect("service failure is a displayable quick-start reply");

    assert!(!reply.ok);
    assert_eq!(reply.message, "install denied");
    assert_eq!(
        *mock.quick_start_calls.lock().await,
        vec![QuickStartCall::ConfigSet, QuickStartCall::ServiceControl]
    );
    server_task.abort();
}

/// 快速入门只接受批准的 Core 配置键；重复或未知字段必须在抵达 core 前拒绝，
/// 因而不会产生配置写入或服务安装副作用。
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
        "invalid input must not write config or install the service"
    );
    server_task.abort();
}

/// 未拨号（NotWired）时命令必须 fail closed（CoreUnreachable，不 panic）。
#[tokio::test]
async fn core_client_fails_closed_when_not_dialed() {
    let state = CoreState::default();
    assert!(state.channel().is_none());

    let err = CoreClient
        .connect(
            &state,
            ConnectIntent {
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
/// 原样回传，前端每轮续拉恰漏 seq 恰等的那一条（与 darwin 桥同构钉死）。
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

/// RPC 拒绝（mock 返回 Unauthorized）→ 非 transport 类错误映射为 Internal。
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

    let (_mock, _channel, server_task) = serve_mock(MockKernel::new()).await;
    // 换成拒绝 server：直接 serve Rejecting。
    server_task.abort();
    let addr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("local addr")
    };
    let server = Server::builder().add_service(KernelControlServer::new(Rejecting));
    let handle = tokio::spawn(async move {
        let _ = server.serve(addr).await;
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
            ConnectIntent {
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

/// 本地凭据不可用是连接恢复信号而非普通内部错误；错误边界不得序列化秘密。
#[tokio::test]
async fn maps_credential_required_status_without_secret() {
    let (mock, channel, server_task) = serve_mock(MockKernel::new()).await;
    *mock.connect_error.lock().await = Some((
        tonic::Code::FailedPrecondition,
        "credential_required|password_not_remembered".to_string(),
    ));
    let state = dialed_state(channel);

    let error = CoreClient
        .connect(
            &state,
            ConnectIntent {
                profile_ref: String::new(),
                credentials: None,
                secret_payload: None,
            },
        )
        .await
        .expect_err("credential recovery status must reject connect");

    let json = serde_json::to_value(error).expect("serialize app error");
    assert_eq!(json["kind"], "credential_required");
    assert_eq!(json["code"], "password_not_remembered");
    assert!(json.get("password").is_none());
    server_task.abort();
}
