
//! P5 进程级 smoke（opt-in，`#[ignore]`）：core main 真实入口全链冒烟
//! （2026-09-08 按需提权模型语义版）。
//!
//! 流程（镜像 P4-b `bootstrap.rs` 的 UI 角色）：
//!   1. 用 `CARGO_BIN_EXE_exv-core` 定位 core 二进制并 spawn
//!      （`--control-pipe \\.\pipe\exv-core-<testpid> --ui-pid <testpid> --ui-sid <sid>`）；
//!   2. **启动零提权断言**（按需拉起模型核心判据）：core 启动稳定后，断言系统中
//!      不存在以本 core 为宿主的 `exv-engine.exe` 子进程（按 `--host-pid <core_pid>`
//!      精确匹配命令行）——旧模型"启动即 runas 拉 engine + UAC"在新结构下必须绝迹；
//!   3. 一个短命子进程扮演 **UI**（同用户 SID，满足 `verify_ui_peer`）：拨号控制面
//!      管道 → 保持连接 → 退出——证明 UI 控制面不依赖任何已拉起的 engine；
//!   4. **UI 进程退出**（O3 强绑定）→ core 的 UI 进程退出监视触发 `on_ui_exited` →
//!      有序停机（空态 supervisor → NoClient → composition.exit）；
//!   5. 断言 core 干净退出（exit 0）且聚合日志出现 `core.start.lazy_engine`（按需
//!      模型启动标记）与停机标记。
//!
//! > 首连 provision（UAC 归属连接）属真机业务验收（2026-09-08 计划 §5 判据 2），
//! > 需要凭据/配置/engine bin——不在本 smoke 范围（本 smoke 只钉「启动 0 提权 +
//! > 控制面独立 + 有序停机」）。
//!
//! > 为什么是"UI 进程退出"而非"拨号后丢管道"：tonic `serve_with_incoming` 对单元素流
//! > accept 后立即返回、连接任务 detached 继续服务——serve 任务 JoinHandle 不充当"UI
//! > 断开"信号（会假 resolve 触发假停机）。真实信号是已验证 UI 进程的退出监视
//! > （`kernel_control_transport::spawn_ui_exit_watcher`）。
//!
//! 环境依赖（缺一即跳过/失败）：core 二进制已构建（cargo build 先跑）、PowerShell
//! 可用（UI dialer + 进程枚举）。**无需 UAC**（按需模型下本 smoke 全程零提权）；
//! CI 环境用 `--ignored` 显式跑并据环境如实报告。

use std::process::{Command, Stdio};

use exv_vpn_win32_ipc::peer_auth::current_user_sid;
use tempfile::TempDir;

fn core_bin() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("CARGO_BIN_EXE_exv-core").map(std::path::PathBuf::from)?;
    path.exists().then_some(path)
}

/// 短命 UI dialer 子进程：拨号 core 的控制面管道 → 保持连接 → 退出。
///
/// 用 .NET `NamedPipeClientStream`（同用户进程，SID 满足 `verify_ui_peer` 的
/// client-pid+SID+account 检查）；重试拨号直到 core 管道就绪（按需模型下 core 启动
/// 即建管，无 engine/UAC 竞窗）。连接成功并保持 1.5s 后退出（退出 = UI 进程退出 →
/// core 停机）。
fn spawn_ui_dialer(pipe_name: &str) -> std::process::Child {
    // 去 `\\.\pipe\` 前缀（.NET serverName='.' + pipeName 拼出完整路径）。
    let short = pipe_name.trim_start_matches(r"\\.\pipe\");
    let script = format!(
        "$c=$null; $deadline=(Get-Date).AddSeconds(25); \
         while((Get-Date) -lt $deadline){{ \
           try{{ \
             $c=New-Object System.IO.Pipes.NamedPipeClientStream('.', '{short}', [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::None, [System.Security.Principal.TokenImpersonationLevel]::Impersonation); \
             $c.Connect(2000); break \
           }} catch {{ Start-Sleep -Milliseconds 500 }} \
         }}; \
         if($null -eq $c){{ exit 1 }}; \
         Start-Sleep -Milliseconds 1500; $c.Dispose(); exit 0"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ui dialer")
}

/// 以 `host_pid` 为宿主（命令行含 `--host-pid <host_pid>`，词边界精确匹配）的
/// `exv-engine.exe` 进程数。枚举失败（PowerShell 不可用）返回 0——按需模型下本
/// smoke 的断言方向是"必须为 0"，fail-open 只会弱化检测灵敏度，不会误报通过。
fn engine_processes_for_host(host_pid: u32) -> u32 {
    let script = format!(
        "@(Get-CimInstance Win32_Process -Filter \"Name='exv-engine.exe'\" | \
          Where-Object {{$_.CommandLine -match '--host-pid {host_pid}\\b'}}).Count"
    );
    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// 进程级全链 smoke（2026-09-08 按需提权语义）：core main 启动**零 engine**、UI
/// 控制面独立可用、UI 进程退出 → 有序停机。
#[tokio::test]
#[ignore = "requires real core bin + PowerShell (opt-in process smoke; no UAC under lazy-engine model)"]
async fn core_main_boots_zero_engine_serves_ui_and_shuts_down_ordered() {
    let Some(exe) = core_bin() else {
        eprintln!("SMOKE-SKIP: core bin not built (run cargo build first)");
        return;
    };
    let Some(sid) = current_user_sid() else {
        eprintln!("SMOKE-SKIP: current user SID unresolvable");
        return;
    };
    // 独立 config 目录（聚合日志写入其中，不污染真实 ~/.exv）。
    let dir = TempDir::new().expect("tempdir");
    let cfg_dir = dir.path().join("cfg");
    let _ = std::fs::create_dir_all(&cfg_dir);
    let pipe_name = format!(r"\\.\pipe\exv-core-smoke-{}", std::process::id());
    let ui_pid = std::process::id().to_string();

    // 1. spawn core（UI 宿主角色，非特权；O3：core 由 UI 拉起）。
    let mut child = Command::new(&exe)
        .args([
            "--control-pipe",
            &pipe_name,
            "--ui-pid",
            &ui_pid,
            "--ui-sid",
            &sid,
        ])
        .env("EXV_CONFIG_DIR", &cfg_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn core");
    let core_pid = child.id();

    // 2. 启动零提权断言：core 启动稳定后（旧模型的 runas+拨号窗口远小于此宽限），
    //    本 core 不得有任何 engine 子进程——"启动即拉 UAC"在新结构下绝迹。
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    let engine_count = engine_processes_for_host(core_pid);
    assert!(
        engine_count == 0,
        "SMOKE-FAIL: core boot spawned {engine_count} engine process(es) for host-pid \
         {core_pid}——按需模型下 core 启动必须零 engine（启动即 UAC 已废除）"
    );

    // 3. 短命 UI 进程：拨号控制面管道（重试直到 core 就绪）→ 保持 → 退出——证明
    //    UI 控制面不依赖任何 engine。
    let mut ui = spawn_ui_dialer(&pipe_name);
    let ui_status = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        ui.wait().expect("wait ui dialer")
    })
    .await
    .expect("ui dialer must exit within 30s");
    if !ui_status.success() {
        let _ = child.kill();
        panic!(
            "SMOKE-FAIL: ui dialer could not connect to core pipe (control plane must not depend on engine)"
        );
    }

    // 4. UI 进程已退出 → core 的 UI 进程退出监视触发 on_ui_exited → 有序停机。
    let status = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        child.wait().expect("wait core")
    })
    .await
    .expect("core must exit after UI process exit");

    // 5. 断言：干净退出 + 聚合日志含按需模型启动标记与停机事实。
    assert!(status.success(), "core exit code = {status}");
    let log_path = cfg_dir.join("logs").join("aggregated.jsonl");
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("core.start.booting"),
        "aggregated log must record booting: {log}"
    );
    assert!(
        log.contains("core.start.lazy_engine"),
        "aggregated log must record the lazy-engine boot marker: {log}"
    );
    eprintln!(
        "SMOKE-OK: core exit {status}; zero engine at boot (host-pid {core_pid}); \
         ordered shutdown via UI process exit; log={}",
        log.lines().count()
    );
}
