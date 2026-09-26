
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use exv_engine::grpc_server::{HelperControlService, SHUTDOWN_TRIGGER_DELAY};
use exv_engine::grpc_transport::{accept_loop, serve_named_pipe, AcceptLoopOutcome, AcceptServe};
use exv_vpn_win32_ipc::peer_auth::current_user_sid;
use exv_vpn_wire::generated;
use generated::helper_control_client::HelperControlClient;
use generated::helper_control_server::HelperControlServer;
use hyper_util::rt::TokioIo;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tonic::codegen::http::Uri;
use tonic::codegen::Service;
use tonic::Request;

// ---------------------------------------------------------------------------
// Helpers（镜像 tests/grpc_server.rs 的已验证测试模式）
// ---------------------------------------------------------------------------

fn unique_pipe_name(tag: &str) -> String {
    format!(r"\\.\pipe\exv-shutdown-rpc-{tag}-{}", std::process::id())
}

/// A minimal test-only named-pipe connector (mirrors the product connector in
/// exv-core::grpc_transport).
struct TestPipeConnector {
    pipe: Option<NamedPipeClient>,
}

impl Service<Uri> for TestPipeConnector {
    type Response = TokioIo<NamedPipeClient>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let pipe = self.pipe.take().expect("single test connection");
        Box::pin(async move { Ok(TokioIo::new(pipe)) })
    }
}

/// Dial the pipe with bounded retry (engine startup race).
async fn dial_with_retry(name: &str) -> NamedPipeClient {
    for _ in 0..50 {
        if let Ok(pipe) = ClientOptions::new().open(name) {
            return pipe;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("engine pipe never became available: {name}");
}

/// service 语义 + 信号注入的 service（生产装配 = `run_service_main` 经 `run_engine`
/// 穿入同一 watch 的 Sender 克隆；测试直接注入）。返回 (service, stop_rx)。
fn service_with_signal() -> (HelperControlService, tokio::sync::watch::Receiver<bool>) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let service = HelperControlService::new()
        .in_service_mode()
        .with_shutdown_signal(tx);
    (service, rx)
}

/// 起真管道 server（oneshot 单 accept 版；service 语义由 service 自身决定，transport
/// 无关）并完成 client 拨号。
async fn spawn_server_and_connect(
    tag: &str,
    service: HelperControlService,
) -> (HelperControlClient<tonic::transport::Channel>, tokio::task::JoinHandle<()>) {
    let server = HelperControlServer::new(service);
    let name = unique_pipe_name(tag);
    let sid = current_user_sid().expect("current user sid");
    let pid = std::process::id();
    let server_task = tokio::spawn({
        let name = name.clone();
        let sid = sid.clone();
        async move {
            let _ = serve_named_pipe(&name, &sid, pid, server).await;
        }
    });
    let pipe = dial_with_retry(&name).await;
    let connector = TestPipeConnector { pipe: Some(pipe) };
    let channel = tonic::transport::Endpoint::from_static("http://engine.local")
        .connect_with_connector(connector)
        .await
        .expect("connect channel");
    (HelperControlClient::new(channel), server_task)
}

fn shutdown_request(reason: generated::ShutdownReason) -> Request<generated::ShutdownRequest> {
    Request::new(generated::ShutdownRequest {
        reason: reason as i32,
    })
}

// ---------------------------------------------------------------------------
// 端到端：真管道 ACCEPTED → 延迟触发既有 scm_stop watch → accept-loop Stopped。
// ---------------------------------------------------------------------------

/// 主路径端到端：`Shutdown(reason=UNINSTALL)` → `ACCEPTED` 回复 → 延迟窗口后 watch
/// 置位；watch 驱动的 accept-loop 以 `Stopped` 退出（与 SCM stop 同一下游，停机序列
/// 零改动）。
#[tokio::test]
async fn shutdown_accepted_arms_scm_stop_watch_and_stops_accept_loop() {
    let (service, stop_rx) = service_with_signal();
    let (mut client, server_task) = spawn_server_and_connect("accepted", service).await;

    let reply = client
        .shutdown(shutdown_request(generated::ShutdownReason::Uninstall))
        .await
        .expect("shutdown RPC answers")
        .into_inner();
    assert_eq!(
        reply.outcome,
        generated::ShutdownOutcome::Accepted as i32,
        "service 形态首次调用必须受理（ACCEPTED）"
    );

    // 延迟窗口后 watch 置位（回复先于触发的构造顺序由 paused 单测断言）。
    tokio::time::sleep(SHUTDOWN_TRIGGER_DELAY + Duration::from_millis(200)).await;
    assert!(
        *stop_rx.borrow(),
        "延迟触发后既有 scm_stop watch 必须被置位（与 SCM stop 同一根通道）"
    );

    // watch 置位 → accept-loop 以 Stopped 退出（service_lifecycle.rs 的既有判据）。
    struct BusyAcceptor;
    impl AcceptServe for BusyAcceptor {
        async fn serve_one(&mut self) -> Result<bool, String> {
            // 模拟真实 accept 阻塞（避免 busy-loop 饿死计时器驱动）。
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(true)
        }
    }
    let outcome = accept_loop(BusyAcceptor, stop_rx).await;
    assert_eq!(
        outcome,
        AcceptLoopOutcome::Stopped,
        "Shutdown 触发的 stop watch 必须与 SCM stop 一样让 accept-loop 返回 Stopped"
    );

    drop(client);
    server_task.abort();
}

/// 幂等端到端：停机窗口内第二次 `Shutdown` → `ALREADY_STOPPING`，且不重复触发
/// （engine 只退出一次；armed 置位后不再 spawn 新的延迟任务）。
#[tokio::test]
async fn shutdown_reentry_is_already_stopping_and_does_not_rearm() {
    let (service, stop_rx) = service_with_signal();
    let (mut client, server_task) = spawn_server_and_connect("idempotent", service).await;

    let first = client
        .shutdown(shutdown_request(generated::ShutdownReason::Upgrade))
        .await
        .expect("first shutdown answers")
        .into_inner();
    assert_eq!(first.outcome, generated::ShutdownOutcome::Accepted as i32);

    let second = client
        .shutdown(shutdown_request(generated::ShutdownReason::ServiceSwitch))
        .await
        .expect("second shutdown answers")
        .into_inner();
    assert_eq!(
        second.outcome,
        generated::ShutdownOutcome::AlreadyStopping as i32,
        "停机窗口内重复调用必须回 ALREADY_STOPPING（幂等）"
    );

    // 延迟窗口后恰好一次触发（watch 是根通道：置位事实只有一次；重复调用不产生
    // 第二个延迟任务—— armed swap 已挡住）。
    tokio::time::sleep(SHUTDOWN_TRIGGER_DELAY + Duration::from_millis(200)).await;
    assert!(*stop_rx.borrow(), "停机信号必须已触发");

    drop(client);
    server_task.abort();
}

/// 兜底回归（5.1 测试 3 的入口投影）：不经 Shutdown、SCM 控制处理器直写同一根
/// watch（生产写入端 = `run_service_main` 闭包）→ accept-loop 仍以 `Stopped` 退出
/// ——既有兜底路径不受本计划影响。
#[tokio::test]
async fn scm_stop_backstop_without_shutdown_still_stops_accept_loop() {
    struct BusyAcceptor;
    impl AcceptServe for BusyAcceptor {
        async fn serve_one(&mut self) -> Result<bool, String> {
            // 模拟真实 accept 阻塞（避免 busy-loop 饿死计时器驱动）。
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(true)
        }
    }
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(accept_loop(BusyAcceptor, stop_rx));
    tokio::time::sleep(Duration::from_millis(20)).await;
    stop_tx.send(true).expect("SCM stop send");
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("loop within timeout")
        .expect("task ok");
    assert_eq!(
        outcome,
        AcceptLoopOutcome::Stopped,
        "SCM stop 直写 watch 的兜底路径必须保持既有行为（Shutdown 不可达时的兜底）"
    );
}

// ---------------------------------------------------------------------------
// 拒绝面（传输层集成投影）：无 peer 认证通道的裸 handler 拒绝在 grpc_server.rs 单元
// 测试覆盖（shutdown_without_verified_peer_rejected_and_not_armed）；真管道拒绝路径
// （未认证连接）由 grpc_transport 的拦截器统一负责（既有测试），本文件不重复。
// ---------------------------------------------------------------------------
