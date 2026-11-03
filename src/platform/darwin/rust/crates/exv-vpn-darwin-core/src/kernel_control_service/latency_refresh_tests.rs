// 受控 Unix socket pair 上的真实 tonic 转发；不启动 Engine 进程、不访问外部网络。
mod latency_refresh_tests {
    use super::*;
    use exv_vpn_wire::generated::helper_control_server::{HelperControl, HelperControlServer};
    use std::task::{Context, Poll};
    use tokio::net::UnixStream;
    use tonic::Response;

    #[derive(Clone)]
    struct PairConnector(Arc<Mutex<Option<UnixStream>>>);
    impl tonic::codegen::Service<http::Uri> for PairConnector {
        type Response = hyper_util::rt::TokioIo<UnixStream>;
        type Error = std::io::Error;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;
        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, _: http::Uri) -> Self::Future {
            std::future::ready(
                self.0
                    .lock()
                    .unwrap()
                    .take()
                    .map(hyper_util::rt::TokioIo::new)
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotConnected)),
            )
        }
    }

    #[derive(Clone)]
    struct LatencyService {
        requests: Arc<Mutex<Vec<Vec<u8>>>>,
        reply: Arc<Mutex<Result<bool, Status>>>,
        block: Arc<std::sync::atomic::AtomicBool>,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    #[tonic::async_trait]
    impl HelperControl for LatencyService {
        async fn auth_receiver_control(&self, _request: tonic::Request<wire::AuthReceiverControlRequest>) -> Result<tonic::Response<wire::AuthReceiverControlReply>, tonic::Status> {
            Err(tonic::Status::unimplemented("official_auth_receiver/test fixture"))
        }

        type MaintainOwnerLeaseStream =
            tokio_stream::Empty<Result<wire::HelperLeaseMessage, Status>>;
        type StreamLogsStream = tokio_stream::Empty<Result<wire::LogEvent, Status>>;
        type StreamStatsStream = tokio_stream::Empty<Result<wire::StatsEvent, Status>>;
        type StreamConnectStatusStream =
            tokio_stream::Empty<Result<wire::ConnectStatusEvent, Status>>;
        async fn trigger_latency_refresh(
            &self,
            request: Request<wire::HelperLatencyRefreshRequest>,
        ) -> Result<Response<wire::HelperLatencyRefreshReply>, Status> {
            self.requests
                .lock()
                .unwrap()
                .push(request.into_inner().operation_id);
            self.started.notify_one();
            if self.block.load(Ordering::SeqCst) {
                self.release.notified().await;
            }
            let accepted = self.reply.lock().unwrap().clone()?;
            Ok(Response::new(wire::HelperLatencyRefreshReply { accepted }))
        }
        async fn maintain_owner_lease(
            &self,
            _: Request<tonic::Streaming<wire::HostLeaseMessage>>,
        ) -> Result<Response<Self::MaintainOwnerLeaseStream>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn observe_owned_state(
            &self,
            _: Request<wire::ObserveOwnedStateRequest>,
        ) -> Result<Response<wire::ObserveOwnedStateReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn acquire_lease(
            &self,
            _: Request<wire::AcquireLeaseRequest>,
        ) -> Result<Response<wire::AcquireLeaseReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn apply_tunnel(
            &self,
            _: Request<wire::ApplyTunnelRequest>,
        ) -> Result<Response<wire::ApplyTunnelReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn stop_tunnel(
            &self,
            _: Request<wire::StopTunnelRequest>,
        ) -> Result<Response<wire::StopTunnelReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn get_operation(
            &self,
            _: Request<wire::GetOperationRequest>,
        ) -> Result<Response<wire::GetOperationReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn release_lease(
            &self,
            _: Request<wire::ReleaseLeaseRequest>,
        ) -> Result<Response<wire::ReleaseLeaseReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn stream_logs(
            &self,
            _: Request<wire::StreamLogsRequest>,
        ) -> Result<Response<Self::StreamLogsStream>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn stream_stats(
            &self,
            _: Request<wire::StreamStatsRequest>,
        ) -> Result<Response<Self::StreamStatsStream>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn stream_connect_status(
            &self,
            _: Request<wire::StreamConnectStatusRequest>,
        ) -> Result<Response<Self::StreamConnectStatusStream>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn keep_alive(
            &self,
            _: Request<wire::KeepAliveRequest>,
        ) -> Result<Response<wire::KeepAliveReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn service_manage(
            &self,
            _: Request<wire::ServiceManageRequest>,
        ) -> Result<Response<wire::ServiceManageReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
        async fn shutdown(
            &self,
            _: Request<wire::ShutdownRequest>,
        ) -> Result<Response<wire::ShutdownReply>, Status> {
            Err(Status::unimplemented("latency-only fixture"))
        }
    }
    struct ServerGuard {
        task: tokio::task::JoinHandle<()>,
        _incoming: tokio::sync::mpsc::UnboundedSender<Result<UnixStream, std::io::Error>>,
    }
    impl Drop for ServerGuard {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn live_latency_fixture() -> (LiveServiceFixture, LatencyService, ServerGuard) {
        let fixture = live_service_fixture();
        let remote = LatencyService {
            requests: Arc::new(Mutex::new(Vec::new())),
            reply: Arc::new(Mutex::new(Ok(true))),
            block: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            started: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        };
        let (client, server) = UnixStream::pair().unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(Ok(server)).unwrap();
        let service = remote.clone();
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(HelperControlServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                .await
                .unwrap();
        });
        let channel = tonic::transport::Endpoint::from_static("http://latency-fixture.invalid")
            .connect_with_connector(PairConnector(Arc::new(Mutex::new(Some(client)))))
            .await
            .unwrap();
        let live = fixture.service.live_connection.as_ref().unwrap();
        *live.session.lock().await = Some(super::super::LiveEngineSession::Live(
            super::super::EngineControlSession::fixture_channel(channel),
        ));
        admit_latency(&fixture, "cisco", 0x41);
        (
            fixture,
            remote,
            ServerGuard {
                task,
                _incoming: tx,
            },
        )
    }
    fn admit_latency(fixture: &LiveServiceFixture, protocol: &str, id: u8) {
        let live = fixture.service.live_connection.as_ref().unwrap();
        let operation_id = vec![id; 16];
        let generation = live.reconnect_generation.advance();
        live.reconnect_generation
            .install(
                generation,
                &operation_id,
                Some(wire::SessionIdentity {
                    protocol: protocol.into(),
                    server: "fixture.invalid".into(),
                    username: "fixture".into(),
                }),
                |_| (),
            )
            .unwrap();
        live.publish(wire::RuntimeSnapshot {
            state: Some(wire::runtime_snapshot::State::Connected(
                wire::ConnectedState::default(),
            )),
            operation_id,
            ..Default::default()
        });
    }
    fn request(id: u8) -> Request<wire::KernelLatencyRefreshRequest> {
        Request::new(wire::KernelLatencyRefreshRequest {
            operation_id: vec![id; 16],
        })
    }

    #[tokio::test]
    async fn core_latency_really_forwards_operation_and_propagates_rejection() {
        let (fixture, remote, _guard) = live_latency_fixture().await;
        let reply = KernelControl::trigger_latency_refresh(&fixture.service, request(0x41))
            .await
            .unwrap()
            .into_inner();
        assert!(reply.accepted);
        assert_eq!(*remote.requests.lock().unwrap(), vec![vec![0x41; 16]]);
        *remote.reply.lock().unwrap() = Ok(false);
        assert_eq!(
            KernelControl::trigger_latency_refresh(&fixture.service, request(0x41))
                .await
                .unwrap_err()
                .code(),
            Code::FailedPrecondition
        );
        *remote.reply.lock().unwrap() = Err(Status::unimplemented("old Engine endpoint"));
        assert_eq!(
            KernelControl::trigger_latency_refresh(&fixture.service, request(0x41))
                .await
                .unwrap_err()
                .code(),
            Code::Unimplemented
        );
        assert_eq!(
            fixture
                .service
                .events
                .current_snapshot()
                .unwrap()
                .operation_id,
            vec![0x41; 16]
        );
    }

    #[tokio::test]
    async fn core_latency_rejects_old_operation_atrust_and_disconnected_without_forwarding() {
        let (fixture, remote, _guard) = live_latency_fixture().await;
        assert_eq!(
            KernelControl::trigger_latency_refresh(&fixture.service, request(0x42))
                .await
                .unwrap_err()
                .code(),
            Code::FailedPrecondition
        );
        admit_latency(&fixture, "atrust", 0x42);
        assert_eq!(
            KernelControl::trigger_latency_refresh(&fixture.service, request(0x42))
                .await
                .unwrap_err()
                .code(),
            Code::Unimplemented
        );
        assert!(matches!(
            fixture.service.events.current_snapshot().unwrap().state,
            Some(runtime_snapshot::State::Connected(_))
        ));
        let live = fixture.service.live_connection.as_ref().unwrap();
        live.reconnect_generation.retire();
        live.publish(wire::RuntimeSnapshot {
            state: Some(runtime_snapshot::State::Idle(wire::IdleState::default())),
            ..Default::default()
        });
        assert_eq!(
            KernelControl::trigger_latency_refresh(&fixture.service, request(0x42))
                .await
                .unwrap_err()
                .code(),
            Code::FailedPrecondition
        );
        assert!(remote.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn core_latency_releases_session_lock_and_drops_stale_acceptance() {
        let (fixture, remote, _guard) = live_latency_fixture().await;
        remote.block.store(true, Ordering::SeqCst);
        let call = KernelControl::trigger_latency_refresh(&fixture.service, request(0x41));
        tokio::pin!(call);
        tokio::select! {
            result = &mut call => panic!("must await controlled Engine reply: {result:?}"),
            started = tokio::time::timeout(Duration::from_secs(2), remote.started.notified()) => { started.expect("Engine receives refresh within bound"); }
        }
        let live = fixture.service.live_connection.as_ref().unwrap();
        assert!(live.session.try_lock().is_ok(), "RPC await 必须在组合锁外");
        admit_latency(&fixture, "cisco", 0x42);
        remote.release.notify_one();
        assert_eq!(call.await.unwrap_err().code(), Code::FailedPrecondition);
        assert_eq!(
            fixture
                .service
                .events
                .current_snapshot()
                .unwrap()
                .operation_id,
            vec![0x42; 16]
        );
    }
}
