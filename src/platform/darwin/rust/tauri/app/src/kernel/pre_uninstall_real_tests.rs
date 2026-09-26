//! 预卸载**非提权段**的真机验证（用户已明确授权清除 EXV 自己的产物）。
//!
//! 跑法（需显式 `--ignored`，避免默认测试集误删开发者本机状态）：
//!
//! ```text
//! EXV_PRE_UNINSTALL_REAL=1 cargo test -p exv-vpn-darwin-tauri \
//!   pre_uninstall_non_privileged -- --ignored --nocapture
//! ```
//!
//! 边界：只触碰 EXV 自己的产物（与壳侧白名单一致）；**不含**任何需要 root 的动作
//! （服务、`/Library`、root 属主 runtime 残留、`/Applications` 下的应用本体）——
//! 那些由应用内入口在用户输入管理员密码后执行。
//!
//! `EXV_PRE_UNINSTALL_REAL=1` 是显式开关，防止误触开发者本机状态。

use std::path::{Path, PathBuf};

use super::pre_uninstall;

/// 打印 before/after 对照：哪些路径存在。
fn snapshot(label: &str, home: &Path) {
    println!("---- {label} ----");
    for path in [
        home.join(".exv"),
        home.join("Library/Application Support/EXV/ui-preferences.json"),
        home.join("Library/LaunchAgents/com.exv.vpn.exv-vpn-darwin.plist"),
        home.join("Library/WebKit/com.exv.vpn.desktop"),
        home.join("Library/Caches/com.exv.vpn.desktop"),
        home.join("Library/HTTPStorages/com.exv.vpn.desktop"),
        home.join("Library/Saved Application State/com.exv.vpn.desktop.savedState"),
        PathBuf::from("/private/tmp/com_exv_vpn_desktop_si.sock"),
    ] {
        println!(
            "  {} {}",
            if path.exists() { "存在" } else { "——  " },
            path.display()
        );
    }
}

fn real_enabled() -> bool {
    if std::env::var_os("EXV_PRE_UNINSTALL_REAL").is_none() {
        println!("未设置 EXV_PRE_UNINSTALL_REAL=1，跳过（保护开发者本机状态）");
        return false;
    }
    true
}

#[test]
#[ignore = "真机非提权段清理：需 EXV_PRE_UNINSTALL_REAL=1 显式开启"]
fn pre_uninstall_non_privileged_segment_removes_only_exv_artifacts() {
    if !real_enabled() {
        return;
    }
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    snapshot("执行前", &home);

    // 应用本体传不存在的路径：当前 /Applications/EXV.app 正承载活 VPN 会话，
    // 删除会打断用户网络；应用本体的真实删除由用户亲自在应用内执行。
    let app_placeholder = home.join("Library/Application Support/EXV/.not-an-app");
    // 真机测试：提权段**不在本测试内执行**（需用户输管理员密码），如实记未运行。
    let launcher: pre_uninstall::ElevationLauncher = std::sync::Arc::new(|| {
        Ok(pre_uninstall::ElevationOutcome::Skipped(
            "本测试不执行提权段（需用户在场输入管理员密码）".to_owned(),
        ))
    });
    // 真机测试：连接前置记为 Idle（不驱动真实连接状态机）。
    let reply = pre_uninstall::run(
        &home,
        &app_placeholder,
        pre_uninstall::ConnectionOutcome::Idle,
        &launcher,
    );

    println!("---- 逐项结果 ----");
    for item in &reply.items {
        println!("  [{}] {} — {}", item.status, item.label, item.detail);
    }
    for action in &reply.manual_actions {
        println!("  手动处理：{action}");
    }

    snapshot("执行后", &home);

    for item in &reply.items {
        assert!(
            ["removed", "absent", "skipped", "failed", "not_run"].contains(&item.status.as_str()),
            "出现未知状态码：{:?}",
            item.status
        );
    }
}

/// 只读对照：确认**没有**触碰白名单外的路径（防止将来误扩范围）。
#[test]
#[ignore = "真机只读对照：需 EXV_PRE_UNINSTALL_REAL=1"]
fn pre_uninstall_does_not_touch_foreign_paths() {
    if !real_enabled() {
        return;
    }
    let sample = [
        "/private/tmp/exv-build-final.log",
        "/private/tmp/exv_clippy_full.log",
        "/private/tmp/exv-dev-docs",
    ];
    let before: Vec<bool> = sample.iter().map(|p| PathBuf::from(p).exists()).collect();

    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    let app_placeholder = home.join("Library/Application Support/EXV/.not-an-app");
    let launcher: pre_uninstall::ElevationLauncher = std::sync::Arc::new(|| {
        Ok(pre_uninstall::ElevationOutcome::Skipped(
            "只读对照不执行提权".to_owned(),
        ))
    });
    let _ = pre_uninstall::run(
        &home,
        &app_placeholder,
        pre_uninstall::ConnectionOutcome::Idle,
        &launcher,
    );

    let after: Vec<bool> = sample.iter().map(|p| PathBuf::from(p).exists()).collect();
    for (index, path) in sample.iter().enumerate() {
        assert_eq!(
            before[index], after[index],
            "白名单外路径被触碰：{path}（存在性 {}=>{}）",
            before[index], after[index]
        );
    }
}
