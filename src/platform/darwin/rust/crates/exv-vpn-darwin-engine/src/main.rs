//! macOS 特权 Engine 进程入口。
//!
//! E4 只启动 bootstrap 与 Observe-only 控制面；连接、CSTP、`utun` 和网络资源事务仍由
//! 后续已批准任务接线。
//!
//! S7：进程启动/退出落固定路径文件日志（`file_log` 模块；杀进程/崩溃场景的取证
//! 底账——写失败静默降级，不阻断既有退出语义）。

/// 文件日志快捷入口（component 固定 `lifecycle`）。
fn lifecycle_log(level: &str, code: &str, message: &str, fields: &[(&str, String)]) {
    exv_vpn_darwin_engine::file_log::default_log().log(level, "lifecycle", code, message, fields);
}

fn main() {
    // 完整卸载内部动作先于日志初始化：无恢复记录时保持零新增产物。
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|arg| arg == "--internal-restore-auth-receiver") {
        let owner_uid = match arguments.as_slice() {
            [_, option, value] if option == "--owner-uid" => value.parse::<u32>().ok(),
            _ => None,
        };
        let Some(owner_uid) = owner_uid else {
            eprintln!("DARWIN_AUTH_RECEIVER_RESTORE_FAILED step=arguments detail=缺少有效owner_uid");
            std::process::exit(1);
        };
        if let Err(error) = exv_vpn_darwin_engine::restore_auth_receiver_for_uninstall(owner_uid) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let pid = std::process::id().to_string();
    lifecycle_log(
        "info",
        "ENGINE_PROCESS_START",
        "engine process starting",
        &[("pid", pid.clone())],
    );
    // 仅供正式 ServiceAgent 既有维护/卸载内部调用；不接收路径、设置或命令输入。
    if std::env::args().skip(1).eq(["--internal-cleanup-proxy-policy"]) {
        if unsafe { libc::geteuid() } != 0 {
            lifecycle_log(
                "error",
                "ENGINE_INTERNAL_CLEANUP",
                "internal proxy policy cleanup rejected: caller is not root",
                &[("pid", pid), ("outcome", "not_root".to_owned())],
            );
            std::process::exit(1);
        }
        match exv_vpn_darwin_engine::system_proxy::cleanup_installed() {
            Ok(status) => {
                eprintln!("proxy policy cleanup: {status:?}");
                lifecycle_log(
                    "info",
                    "ENGINE_INTERNAL_CLEANUP",
                    "internal proxy policy cleanup finished",
                    &[("pid", pid), ("outcome", "ok".to_owned())],
                );
            }
            Err(error) => {
                eprintln!("proxy policy cleanup failed: {error}");
                lifecycle_log(
                    "error",
                    "ENGINE_INTERNAL_CLEANUP",
                    "internal proxy policy cleanup failed",
                    &[("pid", pid), ("outcome", "error".to_owned())],
                );
                std::process::exit(1);
            }
        }
        return;
    }
    let outcome = exv_vpn_darwin_engine::run();
    match &outcome {
        Ok(()) => lifecycle_log(
            "info",
            "ENGINE_PROCESS_EXIT",
            "engine process finished",
            &[("pid", pid), ("outcome", "ok".to_owned())],
        ),
        Err(_) => lifecycle_log(
            "error",
            "ENGINE_PROCESS_EXIT",
            "engine process failed",
            &[("pid", pid), ("outcome", "error".to_owned())],
        ),
    }
    if outcome.is_err() {
        std::process::exit(1);
    }
}
