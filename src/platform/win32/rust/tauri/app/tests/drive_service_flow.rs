//! 进程级服务流驱动（自测，模拟 UI 行为）。
//!
//! spawn 真实 exv-core → 拨号 KernelControl 管道 → 按 UI 顺序驱动：
//!   ServiceControl(Install) → Connect → Snapshot 轮询 → LogsList。
//! 打印每个后端响应 + 最近日志，供无 GUI 的根因定位。
//!
//! 运行（需真实 core bin + 提权，UAC 静默自动通过）：
//!   cargo test -p exv-ui --test drive_service_flow -- --ignored --nocapture

use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use exv_ui::kernel::{
    client::{
        ConfigItem, ConnectCredentials, ConnectIntent, CoreClient, CoreHandle, CoreState,
        dial_core,
    },
    error::AppError,
    quick_start::{QuickStartApplyRequest, apply as apply_quick_start},
    state::OperationResult,
};
use exv_ui::kernel::core_process::{
    core_args_for_spawn, core_bin_path, core_control_pipe_name, spawn_core,
};
use exv_ui::kernel::core_transport::current_user_sid;
use exv_ui::kernel::state::ServiceControlAction;

/// 临时改写本测试进程及其子进程可见的配置目录；Drop 后恢复调用前环境。
///
/// 该 guard 只供本文件中 `--exact` 的单个无服务驱动测试进程使用，不可与其他依赖
/// `EXV_CONFIG_DIR` 的测试并发运行。
struct ConfigDirGuard {
    previous: Option<OsString>,
    path: PathBuf,
}

impl ConfigDirGuard {
    fn empty_isolated() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "exv-quick-start-driver-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create isolated EXV_CONFIG_DIR");
        let previous = std::env::var_os("EXV_CONFIG_DIR");
        // SAFETY: 该 ignored 驱动按 --exact 单独运行；core 子进程在设置后才创建。
        unsafe { std::env::set_var("EXV_CONFIG_DIR", &path) };
        Self { previous, path }
    }
}

impl Drop for ConfigDirGuard {
    fn drop(&mut self) {
        // SAFETY: 与构造时相同，此单测试进程退出前恢复原值。
        unsafe {
            if let Some(previous) = &self.previous {
                std::env::set_var("EXV_CONFIG_DIR", previous);
            } else {
                std::env::remove_var("EXV_CONFIG_DIR");
            }
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 真实 Core 驱动在断言失败时也必须终止子进程，避免遗留控制管道或特权 Engine。
struct CoreChildGuard(Child);

impl Drop for CoreChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 驱动专用的 Core 启动边界：每个配置解析 fallback 都锁定到临时目录。
///
/// 不能仅依赖测试进程环境变量的隐式继承：真实 Core 后续还会拉起 Engine。显式传入
/// EXV_CONFIG_DIR、USERPROFILE、HOME 且切换 child working directory，保证即使下游
/// 回退路径生效，也不可能把 `config.json` 写回源码工作目录。
fn spawn_driver_core(exe: &std::path::Path, config_dir: &std::path::Path) -> Child {
    Command::new(exe)
        .args(core_args_for_spawn())
        .env("EXV_CONFIG_DIR", config_dir)
        .env("USERPROFILE", config_dir)
        .env("HOME", config_dir)
        // 驱动进程读取它们后，只能由当前 CoreClient 请求编码为 one-shot payload；真实
        // Core 及其后续可能提权的 Engine 均不得从环境继承测试凭据。
        .env_remove("EXV_DRIVER_USERNAME")
        .env_remove("EXV_DRIVER_PASSWORD")
        .current_dir(config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn real exv-core with isolated config environment")
}

/// 无服务纵向驱动必须使用实际构建出的 Core，而不是 `cargo test` 默认的 deps sibling。
///
/// 常规 build 输出位于 Rust 根 workspace 的 `target/debug/exv-core.exe`。外部编排也可
/// 通过 `CARGO_BIN_EXE_exv-core`（Cargo 的连字符原名）显式指定同一二进制。两者都缺失时
/// 立即给出可复现的前置命令，绝不让 `spawn_core` 以模糊的 os error 2 失败。
fn real_core_bin_for_driver() -> PathBuf {
    let from_cargo = std::env::var_os("CARGO_BIN_EXE_exv-core")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    let rust_workspace_output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/exv-core.exe");
    let fallback = rust_workspace_output.is_file().then_some(rust_workspace_output);

    from_cargo.or(fallback).unwrap_or_else(|| {
        panic!(
            "real exv-core binary is required; run from src/platform/win32/rust: cargo build -p exv-core -p exv-engine"
        )
    })
}

/// 真实凭据恢复驱动只从进程环境读取人工提供的测试账户，绝不把其打印、写入证据或
/// 嵌入源码。该 ignored 测试的执行人负责在本机注入这两个变量。
fn credential_driver_input() -> ConnectCredentials {
    let username = std::env::var("EXV_DRIVER_USERNAME")
        .expect("EXV_DRIVER_USERNAME is required for the credential recovery driver");
    let password = std::env::var("EXV_DRIVER_PASSWORD")
        .expect("EXV_DRIVER_PASSWORD is required for the credential recovery driver");
    assert!(!username.trim().is_empty(), "credential driver username must not be empty");
    assert!(!password.is_empty(), "credential driver password must not be empty");
    ConnectCredentials {
        username,
        password,
        persist: false,
    }
}

/// 连接 RPC 的正常返回仍可能携带业务终态失败。凭据恢复流只能接受异步已受理或
/// 同步成功；这里不等待或断言 VPN 数据面已连接。
fn credential_operation_is_accepted(result: &OperationResult) -> bool {
    matches!(result, OperationResult::Pending | OperationResult::Succeeded { .. })
}

/// 保持断言信息不含任意结果详情，以免未来失败错误携带敏感认证原文时被测试输出回显。
fn assert_credential_operation_accepted(result: &OperationResult, context: &str) {
    assert!(
        credential_operation_is_accepted(result),
        "{context}: credential recovery connect returned a terminal business failure"
    );
}

#[test]
fn credential_operation_acceptance_rejects_terminal_business_failure() {
    assert!(credential_operation_is_accepted(&OperationResult::Pending));
    assert!(credential_operation_is_accepted(&OperationResult::Succeeded {
        effect_id: None,
        authority_epoch: None,
    }));
    assert!(!credential_operation_is_accepted(&OperationResult::Failed {
        error: None,
    }));
}

/// 每个真实 Core 会话在任何 connect 前只允许执行一次只读 Query。服务一旦已安装，
/// 本驱动立即失败，不会尝试安装、启动、卸载或修复机器服务。
async fn assert_service_not_installed(client: &CoreClient, state: &CoreState) {
    let query = client
        .service_control(state, ServiceControlAction::Query)
        .await
        .expect("read-only service query before credential flow");
    let status = query
        .service_status
        .expect("service query must include service status");
    assert!(
        !status.installed,
        "credential recovery driver requires an uninstalled service and will not mutate it"
    );
}

/// 建立一个真实 CoreClient/Named Pipe 会话。该函数不调用 Tauri command / WebView invoke，
/// 但与其后端等价地走 CoreClient → 认证 Named Pipe → Host KernelControl 路由。
async fn dial_driver_core(child: &CoreChildGuard) -> CoreState {
    let pid = child.0.id();
    let pipe = core_control_pipe_name();
    let sid = current_user_sid().expect("current user SID");
    let (channel, _core_peer) = dial_core(&pipe, pid, &sid)
        .await
        .expect("dial real KernelControl core");
    // CoreHandle::Dialed 现仅持 `channel`（验证后的对端信息经 channel 内建在连接上，
    // CorePeer 不再入 handle）。
    CoreState {
        handle: std::sync::RwLock::new(CoreHandle::Dialed { channel }),
        ..Default::default()
    }
}

/// `config.json` 永远不得含本次提交的明文密码；一次性路径还必须保持空密码和
/// `remember_password=false`，持久化路径则必须产生非空密文。
fn read_driver_config(config_dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(config_dir.join("config.json"))
            .expect("read credential driver config.json"),
    )
    .expect("credential driver config.json must be JSON")
}

#[tokio::test]
#[ignore = "需要真实 core/engine、未安装服务和 EXV_DRIVER_USERNAME/EXV_DRIVER_PASSWORD；手动纵向验证"]
async fn drive_credential_recovery_without_service() {
    let config_dir = ConfigDirGuard::empty_isolated();
    let exe = real_core_bin_for_driver();
    let client = CoreClient;
    let credentials = credential_driver_input();

    // 会话一：健康合法、但未记住密码的默认配置必须以可恢复的稳定类别拒绝。
    {
        let child = CoreChildGuard(spawn_driver_core(&exe, &config_dir.path));
        let state = dial_driver_core(&child).await;
        assert_service_not_installed(&client, &state).await;
        let first = client.config_get(&state).await.expect("bootstrap legal default config");
        assert!(
            !first.requires_quick_start || first.items.iter().any(|item| item.key == "password"),
            "bootstrap must yield a readable legal config"
        );

        let error = client
            .connect(
                &state,
                ConnectIntent {
                    profile_ref: String::new(),
                    credentials: None,
                    secret_payload: None,
                },
            )
            .await
            .expect_err("legal config without remembered password must request credentials");
        assert!(matches!(
            error,
            AppError::CredentialRequired { code: Some(ref code), .. }
                if code == "password_not_remembered"
        ));

        // 会话一的恢复：one-shot 凭据必须成功受理，却只能写用户名，不能落盘密码。
        let one_shot_reply = client
            .connect(
                &state,
                ConnectIntent {
                    profile_ref: String::new(),
                    credentials: Some(credentials.clone()),
                    secret_payload: None,
                },
            )
            .await
            .expect("one-shot credential connect accepted by real host route");
        assert_credential_operation_accepted(
            &one_shot_reply.result,
            "one-shot credential recovery",
        );
        let one_shot_config = read_driver_config(&config_dir.path);
        assert!(
            one_shot_config["username"].as_str() == Some(credentials.username.as_str()),
            "one-shot path must update the stored username"
        );
        assert!(
            one_shot_config["password"].as_str() == Some(""),
            "one-shot path must not create a password ciphertext"
        );
        assert!(
            one_shot_config["remember_password"].as_bool() == Some(false),
            "one-shot path must keep remember_password disabled"
        );
        assert!(
            !std::fs::read_to_string(config_dir.path.join("config.json"))
                .expect("read one-shot config text")
                .contains(&credentials.password),
            "one-shot password must never be written as plaintext"
        );

        // 仍然只走连接路由：停止 Core 子进程由 guard 完成，不调用任何服务控制写操作。
    }

    // 会话二：持久化选择产生密文；随后结束整个 Core，以验证下一个真实 Core 无 payload
    // 也能复用保存的凭据。
    {
        let child = CoreChildGuard(spawn_driver_core(&exe, &config_dir.path));
        let state = dial_driver_core(&child).await;
        assert_service_not_installed(&client, &state).await;
        let mut remembered = credentials.clone();
        remembered.persist = true;
        let remembered_reply = client
            .connect(
                &state,
                ConnectIntent {
                    profile_ref: String::new(),
                    credentials: Some(remembered),
                    secret_payload: None,
                },
            )
            .await
            .expect("remembered credential connect accepted by real host route");
        assert_credential_operation_accepted(
            &remembered_reply.result,
            "remembered credential recovery",
        );
        let persisted_config = read_driver_config(&config_dir.path);
        let ciphertext = persisted_config["password"]
            .as_str()
            .expect("persisted password is a string");
        assert!(!ciphertext.is_empty(), "remembered password must be encrypted");
        assert!(
            ciphertext != credentials.password,
            "remembered password must not be stored as plaintext"
        );
        assert!(
            persisted_config["remember_password"].as_bool() == Some(true),
            "remembered password path must enable remember_password"
        );
        assert!(
            !std::fs::read_to_string(config_dir.path.join("config.json"))
                .expect("read persisted config text")
                .contains(&credentials.password),
            "remembered password must never be written as plaintext"
        );
    }

    // 会话三：重新拉起真实 Core 后不提交 payload，必须仍可通过已保存密文受理连接。
    {
        let child = CoreChildGuard(spawn_driver_core(&exe, &config_dir.path));
        let state = dial_driver_core(&child).await;
        assert_service_not_installed(&client, &state).await;
        let restart_reply = client
            .connect(
                &state,
                ConnectIntent {
                    profile_ref: String::new(),
                    credentials: None,
                    secret_payload: None,
                },
            )
            .await
            .expect("restarted real Core accepts connect using remembered credentials only");
        assert_credential_operation_accepted(
            &restart_reply.result,
            "remembered credential restart",
        );
    }
}

#[tokio::test]
#[ignore = "需要真实 exv-core/exv-engine 二进制；不安装服务的手动纵向验证"]
async fn drive_quick_start_config_flow_without_service() {
    let config_dir = ConfigDirGuard::empty_isolated();
    let exe = real_core_bin_for_driver();
    let child = CoreChildGuard(spawn_driver_core(&exe, &config_dir.path));
    let pid = child.0.id();
    let pipe = core_control_pipe_name();
    let sid = current_user_sid().expect("current user SID");
    let (channel, _core_peer) = dial_core(&pipe, pid, &sid)
        .await
        .expect("dial real KernelControl core");
    let state = CoreState {
        handle: std::sync::RwLock::new(CoreHandle::Dialed { channel }),
        ..Default::default()
    };
    let client = CoreClient;

    // 首次读取必须由真实 Core ConfigGet 触发 bootstrap，而非 mock 或前端推断。
    let first = client.config_get(&state).await.expect("real config_get bootstrap");
    assert!(first.requires_quick_start, "empty config directory must require quick start");
    let config_path = config_dir.path.join("config.json");
    let initial_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&config_path).expect("bootstrap writes config.json"),
    )
    .expect("bootstrap config is JSON");
    let initial_keys = initial_json
        .as_object()
        .expect("bootstrap config is object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let expected_keys = [
        "server",
        "username",
        "password",
        "remember_password",
        "routes",
        "useragent",
        "mtu",
        "auto_reconnect",
        "auto_reconnect_max_attempts",
        "auto_reconnect_backoff",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    // 2026-09-08 计划 I6：网关 IP 字段退役——初始化写出的完整配置键集**不含**
    // `server_bypass_ips`（写路径结构性禁止网关 IP 字段）。
    assert!(
        !initial_keys.contains("server_bypass_ips"),
        "初始化不得写入网关 IP 字段：{initial_keys:?}"
    );
    assert_eq!(initial_keys, expected_keys, "bootstrap writes the complete known config");

    // install_service=false 是本纵向测试的硬边界：不允许任何 ServiceControl 调用。
    let reply = apply_quick_start(
        &client,
        &state,
        QuickStartApplyRequest {
            items: vec![
                ConfigItem { key: "server".into(), value: "vpn-cn.ecnu.edu.cn".into() },
                ConfigItem { key: "username".into(), value: "quick-start-driver".into() },
                ConfigItem { key: "password".into(), value: "not-a-real-password".into() },
                // 旧客户端仍可能发 false；快速入门应按非空密码自动保存。
                ConfigItem { key: "remember_password".into(), value: "false".into() },
                ConfigItem { key: "routes".into(), value: "10.20.0.0/16,10.21.0.0/16".into() },
                ConfigItem { key: "user_agent".into(), value: "EXV quick-start driver".into() },
                ConfigItem { key: "mtu".into(), value: "1290".into() },
                ConfigItem { key: "auto_reconnect".into(), value: "false".into() },
                ConfigItem { key: "auto_reconnect_max_attempts".into(), value: "0".into() },
            ],
            install_service: false,
        },
    )
    .await
    .expect("quick_start_apply writes through real Core ConfigSet");
    assert!(reply.ok);
    assert!(reply.service_status.is_none(), "install_service=false must not contact ServiceControl");

    let second = client.config_get(&state).await.expect("real config_get after apply");
    assert!(!second.requires_quick_start);
    assert!(second.items.iter().any(|item| item.key == "username" && item.value == "quick-start-driver"));
    assert!(second.items.iter().any(|item| item.key == "remember_password" && item.value == "true"));
    assert!(second.items.iter().any(|item| item.key == "routes" && item.value == "10.20.0.0/16,10.21.0.0/16"));

    let final_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&config_path).expect("read final config.json"),
    )
    .expect("final config is JSON");
    let final_keys = final_json
        .as_object()
        .expect("final config is object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(final_keys, expected_keys, "submission leaves only known config fields");

    // 真实 Core + 独立配置目录的边界集成，不冒充原生桌面或 VPN E2E。
    // 每轮都先保存新密码，再留空清除，确认旧 remember 标志不能反转新规则。
    for round in 1..=3 {
        let fixture = format!("  quick-start-fixture-{round}  ");
        for (password, stale_remember, expected_remember) in [
            (fixture.as_str(), "false", true),
            ("", "true", false),
        ] {
            let reply = apply_quick_start(&client, &state, QuickStartApplyRequest {
                items: vec![
                    ConfigItem { key: "server".into(), value: "vpn-cn.ecnu.edu.cn".into() },
                    ConfigItem { key: "username".into(), value: "quick-start-driver".into() },
                    ConfigItem { key: "password".into(), value: password.into() },
                    ConfigItem { key: "remember_password".into(), value: stale_remember.into() },
                ],
                install_service: false,
            }).await.expect("real Quick Start submission");
            assert!(reply.ok, "round {round}: submission must succeed");
            let stored = read_driver_config(&config_dir.path);
            assert_eq!(stored["remember_password"].as_bool(), Some(expected_remember));
            let cipher = stored["password"].as_str().expect("stored password string");
            if expected_remember {
                assert!(!cipher.is_empty(), "round {round}: encrypted password must be saved");
                assert!(cipher != password, "round {round}: stored password must not be plaintext");
            } else {
                assert!(cipher.is_empty(), "round {round}: blank input must clear the old password");
            }
            let visible = client.config_get(&state).await.expect("real Core readback");
            assert!(!visible.requires_quick_start);
            assert!(visible.items.iter().any(|item| item.key == "remember_password" && item.value == expected_remember.to_string()));
            assert!(visible.items.iter().filter(|item| item.key == "password").all(|item| item.value.is_empty()));
        }
    }
}

#[tokio::test]
#[ignore = "需要真实 core 二进制 + 提权；手动运行"]
async fn drive_service_flow() {
    // 0. 真实 UI 环境下 core 有完整 USERPROFILE/ProgramData；本驱动可能从 bash 继承
    //    缺失环境（bash 用 HOME 替代 USERPROFILE、无 ProgramData）→ 补上，忠实复现 UI。
    //    core 的 config_dir 依赖 USERPROFILE；read_service_psk 依赖 ProgramData。
    if std::env::var_os("USERPROFILE").is_none() {
        // 本机开发 profile 固定为 C:\Users\user（dev-only 驱动；若换机需改）。
        // SAFETY: 测试进程私有 env 修改，仅本驱动生效；thread-safe（edition 2024 要求 unsafe）。
        unsafe { std::env::set_var("USERPROFILE", r"C:\Users\user") };
        eprintln!("NOTE: USERPROFILE was missing (bash env); set to C:\\Users\\user");
    }
    if std::env::var_os("ProgramData").is_none() {
        // SAFETY: 同上。
        unsafe { std::env::set_var("ProgramData", r"C:\ProgramData") };
    }

    // 1. spawn 真实 core（非特权；core 自行提权拉 engine / 服务批量）。
    let exe = core_bin_path().expect("core bin not found (build exv-core first)");
    let mut child = spawn_core(&exe).expect("spawn core");
    let pid = child.pid();
    let pipe = core_control_pipe_name();
    let sid = current_user_sid().expect("current user SID");
    eprintln!("== CORE pid={pid} pipe={pipe} sid={sid}");

    // 2. 拨号 + 验证 core server（同用户拓扑）。
    let (channel, core_peer) = dial_core(&pipe, pid, &sid).await.expect("dial core");
    eprintln!("== CORE dialed peer pid={}", core_peer.process_id);
    let state = CoreState {
        handle: std::sync::RwLock::new(CoreHandle::Dialed { channel }),
        ..Default::default()
    };
    let client = CoreClient;

    // 3. 服务安装（UI「安装服务后连接」的第一步）。
    eprintln!("== service_control(install) ==");
    match client.service_control(&state, ServiceControlAction::Install).await {
        Ok(r) => eprintln!(
            "INSTALL ok={} message={:?} service_status={:?}",
            r.ok, r.message, r.service_status
        ),
        Err(e) => eprintln!("INSTALL error: {e:?}"),
    }
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 4. 服务查询（复现「UI 显示运行中但 SCM 未运行」）。
    eprintln!("== service_control(query) ==");
    match client.service_control(&state, ServiceControlAction::Query).await {
        Ok(r) => eprintln!(
            "QUERY ok={} message={:?} service_status={:?}",
            r.ok, r.message, r.service_status
        ),
        Err(e) => eprintln!("QUERY error: {e:?}"),
    }

    // 5. 连接（UI 点击连接）。
    eprintln!("== connect ==");
    let intent = ConnectIntent {
        profile_ref: String::new(),
        credentials: None,
        secret_payload: None,
    };
    match client.connect(&state, intent).await {
        Ok(r) => eprintln!("CONNECT op={:?}", r.operation_id),
        Err(e) => eprintln!("CONNECT error: {e:?}"),
    }

    // 6. Snapshot 轮询（观察状态推进/卡点）。
    for i in 0..8 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match client.snapshot(&state).await {
            Ok(s) => eprintln!(
                "SNAP[{i}] runtime={:?} mode={:?} op={:?} svc={:?}",
                s.runtime, s.mode, s.operation_id, s.service_status
            ),
            Err(e) => eprintln!("SNAP[{i}] error: {e:?}"),
        }
    }

    // 7. 断开（stop）→ 立即重连（复现「断开后立刻点连接」的竞态）。
    eprintln!("== stop ==");
    match client.stop(&state).await {
        Ok(r) => eprintln!("STOP op={:?}", r.operation_id),
        Err(e) => eprintln!("STOP error: {e:?}"),
    }
    tokio::time::sleep(Duration::from_millis(300)).await; // 极短间隔，放大竞态
    eprintln!("== reconnect (immediately after stop) ==");
    let intent2 = ConnectIntent {
        profile_ref: String::new(),
        credentials: None,
        secret_payload: None,
    };
    match client.connect(&state, intent2).await {
        Ok(r) => eprintln!("RECONNECT op={:?}", r.operation_id),
        Err(e) => eprintln!("RECONNECT error: {e:?}"),
    }
    for i in 0..5 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match client.snapshot(&state).await {
            Ok(s) => eprintln!("SNAP2[{i}] runtime={:?} svc={:?}", s.runtime, s.service_status),
            Err(e) => eprintln!("SNAP2[{i}] error: {e:?}"),
        }
    }

    // 7. 最近日志（解析后端到底在做什么）。
    eprintln!("== logs_list ==");
    match client.logs_list(&state, 0, 120).await {
        Ok(chunk) => {
            for l in chunk.events.iter().take(80) {
                eprintln!("LOG [{:?}] {}", l.level, l.message);
            }
        }
        Err(e) => eprintln!("LOGS error: {e:?}"),
    }

    child.terminate();
    eprintln!("== DONE ==");
}
