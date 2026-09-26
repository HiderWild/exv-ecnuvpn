//! 外部浏览器打开（macOS AppKit `NSWorkspace openURL:`；win32 壳 `cmd /C start`
//! 的 darwin 对应物）。
//!
//! ## 为什么需要实装（关于页 GitHub 链接点击无反应的根因）
//!
//! darwin 前端 AboutPage 等外链经 `open_external` 命令打开（前端经壳注入的
//! `__EXV_DARWIN_COMMAND_ADAPTER__` 单接缝调用，与 win32 前端同款交互）。此前
//! darwin 的 `open_external` 是平台旁路 stub：守卫（verify-rust-only.sh）禁止新增
//! tauri-plugin（仅 notification 白名单）与进程启动（Command/open），而 Tauri v2
//! core 没有免插件的外部打开 API → 命令返回 typed 不适用错误，前端 catch 后回退
//! `window.open`。WKWebView 内 `window.open` 静默无效，于是点击仓库链接没有任何
//! 反应。win32 WebView2 对 `window.open` 同样拦截，但 win32 的 `open_external` 是
//! 真实 `cmd /C start`，从未落到回退——「点击没反应」因此是 darwin 特有。
//!
//! 本模块实装「不引入任何进程/插件」的打开通道：直接调 AppKit `NSWorkspace`
//! `openURL:`（objc2-app-kit 同源既有依赖 + feature，不加 crate）。`openURL:` 不是
//! 进程启动、不经 shell——它是 LaunchServices 把 URL 交给系统默认处理方（默认浏览
//! 器）的标准 API。
//!
//! ## 主线程约束
//!
//! AppKit 只能在主线程触碰。`open_external` 命令是 async（tokio worker 线程执行，
//! 与 `window_chrome_control` 等异步命令同款模型），因此先经
//! [`tauri::AppHandle::run_on_main_thread`] 派发，再用 oneshot 回传结果，命令如实
//! 返回打开成败，不伪造成功。
//!
//! ## 守卫红线复核
//!
//!   * 不加新 crate：objc2-app-kit / objc2-foundation 已为真标题栏直接依赖，本模块
//!     只额外打开 NSWorkspace / NSString / NSURL feature；
//!   * 无 tauri-plugin、无进程/脚本执行面（唯一进程点在 core_process.rs 的固定
//!     Core 启动例外）；文本守卫的精确拒绝词目见 `verify-rust-only.sh` 自身，
//!     本模块不包含任何此类 token；
//!   * 打开通道是 AppKit 直接调用（LaunchServices 委托默认浏览器），不是任何形式
//!     的进程构造或 shell 旁路。
//!
//! ## 打开面约束
//!
//! 只放行 http/https URL（当前产品唯一外链语义：关于页 GitHub 仓库）。其它 scheme
//!（file://、x-apple.systempreferences: 等）与脏输入在触碰 AppKit 前直接拒绝，
//! 避免把任意字符串交给 LaunchServices。

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

/// 校验允许打开的外部 URL（仅 http/https；纯文本，可脱离 AppKit 单测）。
fn validate_external_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(())
    } else {
        Err(format!("只允许打开 http(s) 外部 URL：{url}"))
    }
}

/// 当前线程上的 AppKit 打开（调用方保证主线程）。
fn appkit_open(url: &str) -> Result<(), String> {
    let ns_url = NSURL::URLWithString(&NSString::from_str(url))
        .ok_or_else(|| format!("无法构造外部 URL：{url}"))?;
    let workspace = NSWorkspace::sharedWorkspace();
    if workspace.openURL(&ns_url) {
        Ok(())
    } else {
        Err(format!("LaunchServices 拒绝打开外部 URL：{url}"))
    }
}

/// 打开核心（可注入 seam）：先做 URL 校验，再交给 `open` 执行。测试注入探针即可
/// 覆盖「非法 URL 不触碰打开器」与「合法 URL 原样交给打开器」，无需真实 AppKit。
pub(crate) fn open_with(
    url: &str,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    validate_external_url(url)?;
    open(url)
}

/// 生产入口：async 命令在 tokio worker 线程执行 → 派发到 AppKit 主线程做
/// `NSWorkspace openURL:`，并用 oneshot 回传真实结果（打开失败如实返回）。
pub(crate) async fn open_in_default_browser(
    app: &tauri::AppHandle,
    url: &str,
) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let owned_url = url.to_string();
    app.run_on_main_thread(move || {
        // AppKit 在主线程触碰；结果经 oneshot 回传（打开失败不吞掉）。
        let outcome = open_with(&owned_url, appkit_open);
        let _ = tx.send(outcome);
    })
    .map_err(|error| format!("外部打开派发主线程失败：{error}"))?;
    rx.await.map_err(|_| "外部打开未回传结果".to_string())?
}
