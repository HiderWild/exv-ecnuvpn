//! O1局部预算/真实UDS连接用例；不提权、不launch产品、不触正式端点。

use std::{cell::Cell, io, os::unix::fs::PermissionsExt, task::Poll, time::Duration};

use exv_vpn_darwin_ipc::{
    auth::{AUTH_KEY_LEN, AuthKey},
    path::SocketPath,
    peer::ExpectedPeer,
};
use tokio::{
    net::{UnixListener, UnixStream},
    time::Instant,
};

use super::{EngineLifecycleError, dial_engine_until_bootstrap_deadline_with};
use crate::engine_client::{ObserveEndpoint, ObserveError, authenticate_connected_engine};

#[tokio::test(start_paused = true)]
async fn repeated_enoent_expires_at_original_deadline_without_pending_or_extra_dial() {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(45);
    let calls = Cell::new(0);
    let error = dial_engine_until_bootstrap_deadline_with(deadline, || {
        calls.set(calls.get() + 1);
        std::future::ready(Err::<(), _>(io::Error::from_raw_os_error(libc::ENOENT)))
    })
    .await
    .unwrap_err();
    assert_eq!(calls.get(), 3, "0/20/40ms发起，45ms到期不再dial");
    assert_eq!(Instant::now(), deadline);
    match &error {
        EngineLifecycleError::BootstrapConnectDeadline {
            last_connect: Some(last),
            connect_pending: false,
        } => {
            assert_eq!(last.raw_os_error(), Some(libc::ENOENT));
            assert!(error.to_string().contains(&last.to_string()));
        }
        other => panic!("必须保最后真实IO与sleep到期事实: {other:?}"),
    }
    assert!(error.to_string().contains("budget=30s"));
    let source = std::error::Error::source(&error).unwrap();
    assert_eq!(
        source.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(libc::ENOENT)
    );
}

#[tokio::test(start_paused = true)]
async fn connection_refused_retries_only_until_first_success() {
    let started = Instant::now();
    let calls = Cell::new(0);
    let stream =
        dial_engine_until_bootstrap_deadline_with(started + Duration::from_secs(1), || {
            calls.set(calls.get() + 1);
            std::future::ready(if calls.get() == 1 {
                Err(io::Error::from_raw_os_error(libc::ECONNREFUSED))
            } else {
                Ok(7)
            })
        })
        .await
        .unwrap();
    assert_eq!(stream, 7);
    assert_eq!(calls.get(), 2);
    assert_eq!(Instant::now() - started, Duration::from_millis(20));
}

#[tokio::test(start_paused = true)]
async fn other_io_and_kind_without_original_errno_return_immediately() {
    for original in [
        io::Error::from_raw_os_error(libc::EACCES),
        io::Error::from_raw_os_error(libc::EINTR),
        io::Error::from_raw_os_error(libc::ETIMEDOUT),
        io::Error::new(io::ErrorKind::NotFound, "original error without OS errno"),
    ] {
        let started = Instant::now();
        let errno = original.raw_os_error();
        let text = original.to_string();
        let mut next = Some(original);
        let calls = Cell::new(0);
        let error =
            dial_engine_until_bootstrap_deadline_with(started + Duration::from_secs(1), || {
                calls.set(calls.get() + 1);
                std::future::ready(Err::<(), _>(next.take().expect("不得重试其他原错")))
            })
            .await
            .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_eq!(Instant::now(), started, "没有额外sleep");
        match error {
            EngineLifecycleError::Observe(ObserveError::Connect(source)) => {
                assert_eq!(source.raw_os_error(), errno);
                assert_eq!(source.to_string(), text);
            }
            other => panic!("原connect错误不能被折叠: {other:?}"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn first_pending_dial_expires_without_inventing_os_error() {
    let deadline = Instant::now() + Duration::from_millis(45);
    let calls = Cell::new(0);
    let error = dial_engine_until_bootstrap_deadline_with(deadline, || {
        calls.set(calls.get() + 1);
        std::future::pending::<io::Result<()>>()
    })
    .await
    .unwrap_err();
    assert_eq!(calls.get(), 1);
    assert_eq!(Instant::now(), deadline);
    assert!(matches!(
        error,
        EngineLifecycleError::BootstrapConnectDeadline {
            last_connect: None,
            connect_pending: true,
        }
    ));
    assert!(std::error::Error::source(&error).is_none());
    assert!(!error.to_string().contains("errno="));
}

#[tokio::test(start_paused = true)]
async fn error_then_pending_preserves_last_real_error_and_same_deadline() {
    let deadline = Instant::now() + Duration::from_millis(45);
    let calls = Cell::new(0);
    let error = dial_engine_until_bootstrap_deadline_with(deadline, || {
        calls.set(calls.get() + 1);
        let attempt = calls.get();
        std::future::poll_fn(move |_| {
            if attempt == 1 {
                Poll::Ready(Err::<(), _>(io::Error::from_raw_os_error(
                    libc::ECONNREFUSED,
                )))
            } else {
                Poll::Pending
            }
        })
    })
    .await
    .unwrap_err();
    assert_eq!(calls.get(), 2);
    assert_eq!(Instant::now(), deadline, "不能为pending dial重置预算");
    match error {
        EngineLifecycleError::BootstrapConnectDeadline {
            last_connect: Some(last),
            connect_pending: true,
        } => {
            assert_eq!(last.raw_os_error(), Some(libc::ECONNREFUSED));
        }
        other => panic!("必须分开保最后实错与当前pending: {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn expired_budget_does_not_start_a_dial() {
    let calls = Cell::new(0);
    let error = dial_engine_until_bootstrap_deadline_with(Instant::now(), || {
        calls.set(calls.get() + 1);
        std::future::ready(Ok(()))
    })
    .await
    .unwrap_err();
    assert_eq!(calls.get(), 0);
    assert!(matches!(
        error,
        EngineLifecycleError::BootstrapConnectDeadline {
            last_connect: None,
            connect_pending: false,
        }
    ));
}

#[tokio::test]
async fn delayed_real_listener_is_reached_by_original_dial() {
    let directory = tempfile::Builder::new()
        .prefix("exvd")
        .tempdir_in("/private/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = SocketPath::new(directory.path().join("c.sock")).unwrap();
    let bind_path = path.clone();
    let listener_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let listener = UnixListener::bind(bind_path.as_path()).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        (listener, stream)
    });
    let calls = Cell::new(0);
    let stream =
        dial_engine_until_bootstrap_deadline_with(Instant::now() + Duration::from_secs(2), || {
            calls.set(calls.get() + 1);
            UnixStream::connect(path.as_path())
        })
        .await
        .unwrap();
    assert!(calls.get() > 1, "先遇真实ENOENT，再实际连上延迟发布的端点");
    let (listener, peer) = listener_task.await.unwrap();
    let extra_connection_ready =
        std::future::poll_fn(|context| Poll::Ready(listener.poll_accept(context).is_ready())).await;
    assert!(!extra_connection_ready, "没有第二条成功连接");
    drop((stream, peer, listener));
}

#[tokio::test]
async fn delayed_atomic_service_publication_is_reached_within_original_dial_deadline() {
    use std::os::unix::fs::MetadataExt;

    use exv_vpn_darwin_ipc::service_listener::{ServiceListener, ServiceListenerConfig};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let directory = tempfile::Builder::new()
        .prefix("exvsp")
        .tempdir_in("/private/tmp")
        .unwrap();
    // IPC并发用例会短暂设置进程umask；测试目录须保持可遍历。
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = SocketPath::new(directory.path().join("service.sock")).unwrap();
    let publish_path = path.clone();
    // SAFETY: geteuid只读取当前测试进程身份；测试不切换uid或提权。
    let owner_uid = unsafe { libc::geteuid() };
    let deadline = Instant::now() + Duration::from_secs(2);
    let (first_missing, wait_first_missing) = tokio::sync::oneshot::channel();
    let publisher = tokio::spawn(async move {
        tokio::time::timeout_at(deadline, wait_first_missing).await.unwrap().unwrap();
        let listener = ServiceListener::bind(ServiceListenerConfig::new(publish_path.clone(), owner_uid))
            .expect("publish production service listener after the first actual ENOENT");
        let metadata = std::fs::symlink_metadata(publish_path.as_path()).unwrap();
        assert_eq!(metadata.uid(), owner_uid);
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        let connection = tokio::time::timeout_at(deadline, listener.accept_uid_gated())
            .await.unwrap().unwrap();
        assert_eq!(connection.verified_peer().uid(), owner_uid);
        let mut peer = connection.into_authenticated_unix_stream();
        let mut request = [0; 4];
        tokio::time::timeout_at(deadline, peer.read_exact(&mut request)).await.unwrap().unwrap();
        assert_eq!(&request, b"core");
        peer.write_all(b"ready").await.unwrap();
        drop(peer);
        listener.cleanup().unwrap();
    });

    let calls = Cell::new(0);
    let mut first_missing = Some(first_missing);
    let mut stream = dial_engine_until_bootstrap_deadline_with(deadline, || {
        calls.set(calls.get() + 1);
        let notify = first_missing.take();
        let path = path.clone();
        async move {
            let result = UnixStream::connect(path.as_path()).await;
            if let Some(notify) = notify {
                assert_eq!(result.as_ref().unwrap_err().raw_os_error(), Some(libc::ENOENT));
                notify.send(()).unwrap();
            }
            result
        }
    }).await.unwrap();
    stream.write_all(b"core").await.unwrap();
    let mut reply = [0; 5];
    tokio::time::timeout_at(deadline, stream.read_exact(&mut reply)).await.unwrap().unwrap();
    assert_eq!(&reply, b"ready");
    publisher.await.unwrap();
    assert!(Instant::now() < deadline, "发布、原dial和uid门在同一原期限内完成");
    assert!(calls.get() >= 2, "首次真实ENOENT后等待同一个生产端点发布");
    assert!(!path.as_path().exists(), "正式端点按原identity完成cleanup");
}

#[tokio::test]
async fn connected_stream_authentication_crosses_dial_deadline_without_redial() {
    let directory = tempfile::Builder::new()
        .prefix("exva")
        .tempdir_in("/private/tmp")
        .unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = SocketPath::new(directory.path().join("c.sock")).unwrap();
    let listener = UnixListener::bind(path.as_path()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    let peer_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // 原连接成功后，直到连接预算已过才关闭；认证保原自身期限。
        tokio::time::sleep_until(deadline + Duration::from_millis(30)).await;
        drop(stream);
    });
    let calls = Cell::new(0);
    let stream = dial_engine_until_bootstrap_deadline_with(deadline, || {
        calls.set(calls.get() + 1);
        UnixStream::connect(path.as_path())
    })
    .await
    .unwrap();
    let error = authenticate_connected_engine(
        ObserveEndpoint::new(
            path,
            ExpectedPeer::current_process(),
            AuthKey::from_bytes([0x51; AUTH_KEY_LEN]),
        ),
        stream,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, ObserveError::Transport(_)),
        "认证原错误不能变为dial期限或重试: {error:?}"
    );
    assert!(Instant::now() >= deadline, "认证没有被连接deadline提前取消");
    assert_eq!(calls.get(), 1, "首次成功stream之后不重dial/重认证");
    peer_task.await.unwrap();
}
