use std::{
    ffi::OsString,
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use exv_vpn_darwin_ipc::{
    path::{RuntimeDir, RuntimeOwner},
    ui_core_bootstrap::UI_CORE_BOOTSTRAP_V1_LEN,
};
use exv_vpn_wire::generated::{
    ConfigGetRequest, ConfigItem, ConfigSetRequest, LogsListRequest, SnapshotRequest,
    kernel_control_client::KernelControlClient,
    runtime_snapshot,
};

use super::core_process::{
    CoreLaunchArguments, CoreProcessError, ReapableChild, bundle_core_binary_at,
    start_core_session, wait_for_child_exit,
};
use super::client::ConnectIntent;
use super::wire::{connect_request, stop_request};

// ---- D1（L1）：正常退出路径必须回收 UI runtime 目录 ----

/// 正常退出（`lifecycle::notify_core_shutdown`）此前只关通道、`cleanup_empty` 仅在
/// 异常/会话替换路径被调用 → `/private/tmp/exv-vpn-*` 每次正常退出都残留。
///
/// 本测试以真实 `RuntimeDir` + 惰性 child 构造会话，走正常退出清理入口，断言目录消失。
#[test]
fn normal_exit_cleanup_removes_the_ui_runtime_directory() {
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create test runtime");
    let path = runtime.as_path().to_path_buf();
    assert!(
        path.is_dir(),
        "runtime directory exists before exit cleanup"
    );

    let session = super::core_process::CoreSession::test_session_with_runtime(runtime);
    session
        .shutdown_and_cleanup_runtime_blocking()
        .expect("normal exit cleanup of an empty runtime directory succeeds");

    assert!(
        !path.exists(),
        "normal exit must remove the UI runtime directory (L1 leak): {path:?}"
    );
}

/// `cleanup_empty()` 语义：非空即失败并**保留现场**（禁止 `remove_dir_all`）。
#[test]
fn normal_exit_cleanup_refuses_non_empty_runtime_and_keeps_the_file() {
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create test runtime");
    let path = runtime.as_path().to_path_buf();
    let stray = path.join("stray-user-data");
    fs::write(&stray, b"do not delete").expect("write stray file");

    let session = super::core_process::CoreSession::test_session_with_runtime(runtime);
    assert_eq!(
        session.shutdown_and_cleanup_runtime_blocking(),
        Err(CoreProcessError::CoreRuntimeCleanupRefused)
    );
    assert!(stray.is_file(), "non-empty runtime keeps its contents");
    assert!(path.is_dir(), "non-empty runtime directory is preserved");

    fs::remove_file(&stray).expect("remove stray file");
    fs::remove_dir(&path).expect("remove emptied test runtime");
}

/// 目录被替换为符号链接时拒绝删除（不跟随链接），并保留链接与链接目标。
#[test]
fn normal_exit_cleanup_refuses_symlink_swapped_runtime() {
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create test runtime");
    let path = runtime.as_path().to_path_buf();
    let target = std::env::temp_dir().join(format!("exv-d1-symlink-target-{}", std::process::id()));
    fs::create_dir_all(&target).expect("create symlink target directory");
    fs::remove_dir(&path).expect("remove empty runtime directory");
    std::os::unix::fs::symlink(&target, &path).expect("swap runtime path with a symlink");

    let session = super::core_process::CoreSession::test_session_with_runtime(runtime);
    assert_eq!(
        session.shutdown_and_cleanup_runtime_blocking(),
        Err(CoreProcessError::CoreRuntimeCleanupRefused)
    );
    assert!(
        fs::symlink_metadata(&path)
            .expect("runtime path still present")
            .file_type()
            .is_symlink(),
        "refused cleanup must not follow or remove the symlink"
    );
    assert!(target.is_dir(), "symlink target must be untouched");

    fs::remove_file(&path).expect("remove symlink");
    fs::remove_dir(&target).expect("remove symlink target");
}

/// 退出清理只取一次会话所有权：锁被占用或已取走后不再重复清理（幂等 + 无竞态）。
#[test]
fn shutdown_session_ownership_is_taken_at_most_once() {
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create test runtime");
    let path = runtime.as_path().to_path_buf();
    let container = super::client::CoreSession::default();
    *container.inner.try_lock().expect("empty session container") = Some(
        super::core_process::CoreSession::test_session_with_runtime(runtime),
    );

    let taken = container
        .take_inner_for_shutdown()
        .expect("first shutdown takes the session");
    assert!(
        container.take_inner_for_shutdown().is_none(),
        "second shutdown must not take the same session twice"
    );
    taken
        .shutdown_and_cleanup_runtime_blocking()
        .expect("taken session cleans its runtime");
    assert!(!path.exists());
}

struct FakeChild {
    exits_after_polls: usize,
    polls: usize,
    killed: bool,
}

/// ignored 子进程 smoke 必须把 Windows 同构配置限制在自己的临时目录，不得写入开发者的
/// `~/.exv` 设置。
struct SmokeConfigDir {
    previous: Option<OsString>,
    path: PathBuf,
}

impl SmokeConfigDir {
    fn create() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "exv-darwin-core-smoke-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create isolated smoke config directory");
        let previous = std::env::var_os("EXV_CONFIG_DIR");
        // SAFETY: this ignored smoke is run as one explicit process and restores the prior value in Drop.
        unsafe { std::env::set_var("EXV_CONFIG_DIR", &path) };
        Self { previous, path }
    }
}

impl Drop for SmokeConfigDir {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            // SAFETY: restores the value captured before this ignored smoke started.
            unsafe { std::env::set_var("EXV_CONFIG_DIR", previous) };
        } else {
            // SAFETY: restores the absent state captured before this ignored smoke started.
            unsafe { std::env::remove_var("EXV_CONFIG_DIR") };
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl ReapableChild for FakeChild {
    fn try_wait(&mut self) -> Result<bool, ()> {
        self.polls += 1;
        Ok(self.killed || self.polls > self.exits_after_polls)
    }

    fn kill(&mut self) -> Result<(), ()> {
        self.killed = true;
        Ok(())
    }
}

#[tokio::test]
async fn wait_seam_distinguishes_exit_from_deadline_without_launching_a_process() {
    let mut already_exited = FakeChild {
        exits_after_polls: 0,
        polls: 0,
        killed: false,
    };
    assert!(
        wait_for_child_exit(&mut already_exited, Duration::ZERO)
            .await
            .expect("read fake child")
    );

    let mut still_running = FakeChild {
        exits_after_polls: usize::MAX,
        polls: 0,
        killed: false,
    };
    assert!(
        !wait_for_child_exit(&mut still_running, Duration::ZERO)
            .await
            .expect("read fake child")
    );
    still_running.kill().expect("kill fake child");
    assert!(
        wait_for_child_exit(&mut still_running, Duration::ZERO)
            .await
            .expect("reap fake child")
    );
}

#[test]
fn fixed_launch_arguments_are_exactly_the_core_contract() {
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create test runtime");
    let socket = runtime.engine_socket_path().expect("derive test socket");
    let arguments = CoreLaunchArguments::new(42, RuntimeOwner::current().uid(), socket)
        .expect("construct fixed arguments");

    let tokens = arguments.tokens();
    assert_eq!(tokens.len(), 6);
    assert_eq!(tokens[0], "--ui-pid");
    assert_eq!(tokens[1], "42");
    assert_eq!(tokens[2], "--ui-uid");
    assert_eq!(tokens[4], "--ui-socket");
    assert!(tokens[5].starts_with("/private/tmp/"));
    assert_eq!(UI_CORE_BOOTSTRAP_V1_LEN, 40);

    runtime.cleanup_empty().expect("clean empty test runtime");
}

#[test]
fn fixed_launch_arguments_reject_zero_or_out_of_range_identity() {
    let runtime = RuntimeDir::create(RuntimeOwner::current()).expect("create test runtime");
    let socket = runtime.engine_socket_path().expect("derive test socket");
    let uid = RuntimeOwner::current().uid();

    assert_eq!(
        CoreLaunchArguments::new(0, uid, socket.clone()),
        Err(CoreProcessError::CoreBootstrapEarlyExit)
    );
    assert_eq!(
        CoreLaunchArguments::new(i32::MAX as u32 + 1, uid, socket),
        Err(CoreProcessError::CoreBootstrapEarlyExit)
    );

    runtime.cleanup_empty().expect("clean empty test runtime");
}

// ---- MAC-PACKAGE-12：无签名 `.app` bundle 布局的 Core resolver 布局判定 ----

/// 构造真实 bundle 形状的临时目录（含产品名空格），返回 launcher exe 路径。
fn fake_bundle_launcher(tag: &str) -> std::path::PathBuf {
    let macos = std::env::temp_dir()
        .join(format!("exv-pkg12-core-test-{}-{tag}", std::process::id()))
        .join("EXV VPN.app")
        .join("Contents")
        .join("MacOS");
    fs::create_dir_all(&macos).expect("create fake bundle MacOS directory");
    let launcher = macos.join("exv-vpn-darwin-tauri");
    fs::write(&launcher, b"launcher").expect("write fake launcher");
    launcher
}

fn cleanup_dir(path: &std::path::Path) {
    let _ = fs::remove_dir_all(
        path.ancestors()
            .nth(3)
            .expect("exe sits at <tmp>/<bundle>/Contents/MacOS/<exe>"),
    );
}

#[test]
fn bundle_layout_resolves_core_next_to_the_launcher() {
    use std::os::unix::fs::PermissionsExt;

    let launcher = fake_bundle_launcher("resolve");
    let core = launcher
        .parent()
        .expect("MacOS directory")
        .join("exv-vpn-darwin-core");
    fs::write(&core, b"core").expect("write fake core");
    fs::set_permissions(&core, fs::Permissions::from_mode(0o755)).expect("chmod core");

    let resolved = bundle_core_binary_at(&launcher).expect("bundle layout resolves core");
    assert_eq!(resolved, core);

    cleanup_dir(&launcher);
}

#[test]
fn bundle_layout_requires_an_executable_plain_core_file() {
    let launcher = fake_bundle_launcher("not-executable");
    let core = launcher
        .parent()
        .expect("MacOS directory")
        .join("exv-vpn-darwin-core");
    fs::write(&core, b"core").expect("write fake core");

    assert!(
        bundle_core_binary_at(&launcher).is_none(),
        "non-executable core must not resolve"
    );

    // 可执行目录同样拒绝（必须是普通文件）。
    fs::remove_file(&core).expect("remove plain file");
    fs::create_dir(&core).expect("replace with directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&core, fs::Permissions::from_mode(0o755)).expect("chmod directory");
    }
    assert!(
        bundle_core_binary_at(&launcher).is_none(),
        "directory core must not resolve"
    );

    cleanup_dir(&launcher);
}

#[test]
fn bundle_layout_requires_the_contents_macos_shape() {
    // 缺 `Contents/MacOS` 双层形状（例如裸 target/debug launcher）不得命中。
    let flat =
        std::env::temp_dir().join(format!("exv-pkg12-core-test-{}-flat", std::process::id()));
    fs::create_dir_all(&flat).expect("create flat directory");
    let launcher = flat.join("exv-vpn-darwin-tauri");
    fs::write(&launcher, b"launcher").expect("write fake launcher");
    let core = flat.join("exv-vpn-darwin-core");
    fs::write(&core, b"core").expect("write fake core");

    assert!(bundle_core_binary_at(&launcher).is_none());

    // 只有单层 MacOS、缺 Contents 父层同样不命中。
    let lone_macos = std::env::temp_dir().join(format!(
        "exv-pkg12-core-test-{}-macos-only",
        std::process::id()
    ));
    fs::create_dir_all(&lone_macos).expect("create lone MacOS directory");
    let lone_launcher = lone_macos.join("exv-vpn-darwin-tauri");
    fs::write(&lone_launcher, b"launcher").expect("write fake launcher");
    fs::write(lone_macos.join("exv-vpn-darwin-core"), b"core").expect("write fake core");

    assert!(bundle_core_binary_at(&lone_launcher).is_none());

    let _ = fs::remove_dir_all(&flat);
    let _ = fs::remove_dir_all(&lone_macos);
}

#[test]
fn bundle_layout_without_core_falls_back_to_none() {
    let launcher = fake_bundle_launcher("missing-core");
    assert!(
        bundle_core_binary_at(&launcher).is_none(),
        "bundle shape without a core binary must return None"
    );

    cleanup_dir(&launcher);
}

/// 真实的普通用户 Tauri→Core 子进程业务流。
///
/// 该测试使用 launcher 规定的固定 `target/debug/exv-vpn-darwin-core`，所以必须先在
/// 同一 Darwin Rust target 构建 Core binary；常规 crate 测试不会隐式构建另一个产品 binary。
#[tokio::test]
#[ignore = "先构建固定 target/debug/exv-vpn-darwin-core，再运行这个真实普通用户 smoke"]
async fn ordinary_user_core_child_serves_config_idle_and_reaps() {
    let _config_dir = SmokeConfigDir::create();
    let session = start_core_session()
        .await
        .expect("fixed Core child starts and authenticates over the UI UDS");

    let result = async {
        let mut client = KernelControlClient::new(session.channel());

        let initial = client
            .config_get(ConfigGetRequest {})
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert_eq!(initial.items.len(), 10);
        assert!(
            initial
                .items
                .iter()
                .any(|item| item.key == "password" && item.value.is_empty())
        );

        let reply = client
            .config_set(ConfigSetRequest {
                items: vec![
                    ConfigItem {
                        key: "server".to_owned(),
                        value: "vpn.example.edu.cn".to_owned(),
                    },
                    ConfigItem {
                        key: "username".to_owned(),
                        value: "ordinary-user-smoke".to_owned(),
                    },
                    ConfigItem {
                        key: "routes".to_owned(),
                        value: "10.0.0.0/8".to_owned(),
                    },
                    ConfigItem {
                        key: "mtu".to_owned(),
                        value: "000576".to_owned(),
                    },
                    ConfigItem {
                        key: "user_agent".to_owned(),
                        value: "EXV VPN macOS smoke".to_owned(),
                    },
                ],
            })
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(reply.ok);

        let updated = client
            .config_get(ConfigGetRequest {})
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert_eq!(
            updated
                .items
                .iter()
                .map(|item| (item.key.as_str(), item.value.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("server", "vpn.example.edu.cn"),
                ("username", "ordinary-user-smoke"),
                ("password", ""),
                ("remember_password", "false"),
                ("routes", "10.0.0.0/8"),
                ("user_agent", "EXV VPN macOS smoke"),
                ("mtu", "576"),
                ("auto_reconnect", "false"),
                ("auto_reconnect_max_attempts", "0"),
                ("auto_reconnect_backoff", "false"),
            ]
        );

        let snapshot = client
            .get_snapshot(SnapshotRequest {
                runtime_epoch: Vec::new(),
            })
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(
            matches!(
                snapshot.state,
                Some(runtime_snapshot::State::Idle(_))
            ),
            "无连接的真实快照必须回退 Idle，不得伪造连接态"
        );
        Ok::<(), String>(())
    }
    .await;

    let reaped = session.close_and_reap().await;
    result.expect("Core ConfigGet/ConfigSet/GetSnapshot smoke succeeds");
    reaped.expect("Core child exits and its UI runtime is cleaned after channel close");
}

/// MAC-OBS-13 S2 真实 Core smoke：起真实 Core 子进程，`logs_list` 在**无 Engine 会话、
/// 无任何 VPN 连接**的情况下经同一认证 channel 返回 Core 聚合日志的真实分页。语义覆盖：
/// 空库（`next_seq=1`）、`config_set` 真实节点入库后尾部拉取（条目不带 seq）、增量游标
/// 续拉空页（`next_seq = last_seq + 1`）、`filter` 透传无匹配空页。
#[tokio::test]
#[ignore = "先构建固定 target/debug/exv-vpn-darwin-core，再运行这个真实普通用户 smoke"]
async fn ordinary_user_core_child_serves_logs_list_without_an_engine_session() {
    let _config_dir = SmokeConfigDir::create();
    let session = start_core_session()
        .await
        .expect("fixed Core child starts and authenticates over the UI UDS");

    let result = async {
        let mut client = KernelControlClient::new(session.channel());

        let empty = client
            .logs_list(LogsListRequest {
                after_seq: 0,
                limit: 500,
                filter: String::new(),
            })
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(
            empty.entries.is_empty(),
            "隔离配置目录下新 Core 的聚合日志应为空库"
        );
        assert_eq!(empty.next_seq, 1, "空库游标 = last_seq + 1 = 1");

        client
            .config_set(ConfigSetRequest {
                items: vec![ConfigItem {
                    key: "username".to_owned(),
                    value: "logs-list-smoke".to_owned(),
                }],
            })
            .await
            .map_err(|error| error.to_string())?;
        // config_set 的真实节点入库（CONFIG_SET）使聚合日志出现首条真实条目。
        let tail = client
            .logs_list(LogsListRequest {
                after_seq: 0,
                limit: 500,
                filter: String::new(),
            })
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert_eq!(tail.entries.len(), 1, "初始拉取返回最近条目");
        let event = tail.entries.first().expect("config entry");
        assert_eq!(event.level, "info");
        assert_eq!(event.component, "core");
        assert_eq!(event.code, "CONFIG_SET");
        assert_eq!(event.message, "configuration updated");
        assert_eq!(tail.next_seq, 2, "游标 = 最后返回条目 seq + 1");

        let incremental = client
            .logs_list(LogsListRequest {
                after_seq: tail.next_seq,
                limit: 500,
                filter: String::new(),
            })
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(incremental.entries.is_empty(), "游标已追平应返回空页");
        assert_eq!(incremental.next_seq, 2, "空页游标保持 last_seq + 1 稳健");

        let filtered = client
            .logs_list(LogsListRequest {
                after_seq: 0,
                limit: 500,
                filter: "logs-list-smoke-no-match-xyz".to_owned(),
            })
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(filtered.entries.is_empty(), "filter 无匹配返回空页");

        Ok::<(), String>(())
    }
    .await;

    let reaped = session.close_and_reap().await;
    result.expect("Core LogsList smoke succeeds without an engine session");
    reaped.expect("Core child exits and its UI runtime is cleaned after channel close");
}

/// 真实 Core 子进程的业务流：正常退出路径（L1）必须回收 UI runtime 目录。
///
/// 与上面两个 ignored smoke 的区别：不调用 `close_and_reap`（那是异常/会话替换路径，
/// 会 kill 子进程），而是走 [`CoreSession::shutdown_and_cleanup_runtime_blocking`]——
/// 即 `lifecycle::notify_core_shutdown` 在 `app.exit(0)` 之前使用的同一条清理路径：
/// 关 stdin、释放唯一 channel 让 Core 经认证连接 EOF 有序停机，等它自己退出后
/// `cleanup_empty()`。
///
/// 用多线程 runtime：清理本身是同步有界等待（会阻塞一个 worker），而 tonic channel
/// 的关闭要由 runtime 任务驱动——current-thread runtime 上阻塞会让连接永不关闭、
/// Core 永不退出（这正是生产侧 `lifecycle.rs` 把清理放独立线程的原因）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "先构建固定 target/debug/exv-vpn-darwin-core，再运行这个真实普通用户 smoke"]
async fn ordinary_user_core_child_runtime_is_removed_on_the_normal_exit_path() {
    let _config_dir = SmokeConfigDir::create();
    let session = start_core_session()
        .await
        .expect("fixed Core child starts and authenticates over the UI UDS");

    let runtime_path = session.runtime_path_for_test();
    assert!(
        runtime_path.is_dir(),
        "运行期 UI runtime 目录必须存在：{runtime_path:?}"
    );

    session.shutdown_and_cleanup_runtime_blocking().expect(
        "normal exit cleanup: Core exits after connection EOF and the empty runtime is removed",
    );

    assert!(
        !runtime_path.exists(),
        "正常退出后 UI runtime 目录必须消失（L1 泄漏判据）：{runtime_path:?}"
    );
}

/// 真实桌面链路验收：Tauri 的 Core 子进程经 macOS 授权启动固定 Engine，并在同一已认证
/// 本地通道上完成 `ApplyTunnel` pending 与 `StopTunnel`。本阶段不进入 CSTP、utun 或网络。
///
/// 这是显式的人工授权验收，因此不会在常规测试中自动运行。它必须在共享固定 target 中先
/// 构建 Core、Engine 和 Tauri，再以 `--ignored --nocapture` 单独执行。
#[tokio::test]
#[ignore = "需要 macOS 管理员授权；验证桌面应用→Core→Engine→ApplyTunnel 的完整本地链路"]
async fn ordinary_user_core_child_starts_engine_applies_pending_and_stops() {
    let _config_dir = SmokeConfigDir::create();
    let session = start_core_session()
        .await
        .expect("fixed Core child starts and authenticates over the UI UDS");

    let result = async {
        let mut client = KernelControlClient::new(session.channel());

        let reply = client
            .config_set(ConfigSetRequest {
                items: vec![
                    ConfigItem {
                        key: "username".to_owned(),
                        value: "ordinary-user-engine-smoke".to_owned(),
                    },
                    ConfigItem {
                        key: "password".to_owned(),
                        value: "ordinary-user-engine-password".to_owned(),
                    },
                    ConfigItem {
                        key: "remember_password".to_owned(),
                        value: "true".to_owned(),
                    },
                ],
            })
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(reply.ok);

        let apply = client
            .connect(
                connect_request(&ConnectIntent::default(), vec![0xAA; 16])
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(apply.terminal.is_none());

        let stop = client
            .stop(stop_request(vec![0xBB; 16]).map_err(|error| error.to_string())?)
            .await
            .map_err(|error| error.to_string())?
            .into_inner();
        assert!(stop.terminal.is_none());
        Ok::<(), String>(())
    }
    .await;

    let reaped = session.close_and_reap().await;
    result.expect("Core starts Engine and receives ApplyTunnel/StopTunnel replies");
    reaped.expect("Core child and its UI runtime are reaped after the Engine session closes");
}
