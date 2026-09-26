//! 系统通知（MAC-SHELL-17 S3；win32 `toast.rs` 的 macOS 重写）。
//!
//! ## 平台现实：两条 API 代际，选错就是「静默什么都不显示」（2026-09-20 真机实测）
//!
//! macOS 上第三方通知有两条互斥的实现代际，行为差异极大：
//!
//! * **旧路径 `NSUserNotificationCenter`**（`tauri-plugin-notification` 2.3.3 的 macOS 后端
//!   实际用的就是它：`notify-rust` → `mac-notification-sys` 0.6.15，与本文档早先写的
//!   「UNUserNotificationCenter」不符）：投递是 fire-and-forget，**系统不投递时同样返回
//!   `Ok`**。真机探针（同 bundle 形态）实测：macOS 26.5.2 上 `send_notification` 返回
//!   `Ok(None)` 且零横幅——用户看到的现象就是「授权也开了，通知就是不弹」。
//! * **新路径 `UNUserNotificationCenter`**（macOS 10.14+ 的正统 API，本壳经 objc2 自实现，
//!   见 [`crate::notification_modern`]）：macOS 26 上要求 app bundle **至少带 ad-hoc 签名**
//!   ——未签名时系统直接拒绝（`UNErrorDomain error 1 = notifications not allowed`，真机
//!   实测），签名后正常弹授权并投递。打包侧（`scripts/package-unsigned.sh`）因此对 bundle
//!   做 ad-hoc 签名，与本模块的策略选择是一对。
//!
//! 于是本模块**按系统主版本选后端**（[`strategy_for`]）：macOS >= [`MODERN_BACKEND_MIN_MAJOR`]
//! 用新路径；低版本用旧路径（旧系统上它可用且无需签名）；系统版本读不到时先试新路径
//! （它能如实报错，失败再回退旧路径）。
//!
//! ## 两态语义（保留）
//!
//! 通知要求 `.app bundle`。裸 debug 二进制（`bundle.active = false` 的验证形态）→
//! **完全不触碰任何通知路径**，如实返回 `delivered=false` + 稳定降级原因。
//!
//! ## 可观测性（2026-09-20 新增）
//!
//! GUI 启动的 app 没有可见 stderr（`open` 启动时 stdout/stderr 进 /dev/null），原先
//! 「降级原因只写 stderr」等于没有诊断。现在每次投递都追加一行到
//! `~/Library/Application Support/EXV/notification.log`：策略、系统主版本、结果与有界细节
//! （**不含标题/正文**，避免把用户内容写进诊断文件）。

use std::path::Path;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::notification_modern;

/// `delivered=false` 的稳定降级原因：未打包二进制（debug 壳预期态）。
pub(crate) const DEGRADED_UNBUNDLED: &str = "notification-unavailable-unbundled";
/// `delivered=false` 的稳定降级原因：bundled 但旧路径发送失败。
pub(crate) const DEGRADED_SEND_FAILED: &str = "notification-send-failed";
/// `delivered=false` 的稳定降级原因：新路径提交失败且旧路径回退也失败。
pub(crate) const DEGRADED_MODERN_FAILED: &str = "notification-modern-backend-failed";

/// 需要新后端（`UNUserNotificationCenter`）的 macOS 主版本下限。
///
/// 依据：macOS 26 真机上旧路径零投递（探针实证），新路径在 ad-hoc 签名后可用。
pub(crate) const MODERN_BACKEND_MIN_MAJOR: u32 = 26;

/// 通知投递后端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotificationStrategy {
    /// `UNUserNotificationCenter`（macOS >= 26；要求 bundle 带 ad-hoc 签名）。
    Modern,
    /// 插件旧路径（`NSUserNotificationCenter`；旧系统上可用，无需签名）。
    Legacy,
}

impl NotificationStrategy {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Modern => "modern",
            Self::Legacy => "legacy",
        }
    }
}

/// 一次投递的真实结果（新后端在系统回调里异步回报，用于落盘诊断）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DeliveryReport {
    /// 系统已投递（`addNotificationRequest` 无错误返回）。
    Delivered,
    /// 授权被拒或授权调用报错（细节为系统本地化描述，已截断）。
    Denied(String),
    /// 授权通过但投递失败。
    Failed(String),
}

// ---- 能力探测（纯函数，单测覆盖）----

/// 系统通知能力（由进程可执行文件路径推导）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotificationCapability {
    /// 运行在 `.app` bundle 内：通知可用面。
    Bundled,
    /// 裸二进制（debug 壳）：必须降级，不得触碰任何通知路径。
    Unbundled,
}

/// 由可执行文件路径判断通知能力：路径存在 `<name>.app/Contents/` 祖先段即为
/// bundled（macOS bundle 形状 `X.app/Contents/MacOS/<bin>`；通知只要求 Info.plist
/// 所在的 `.app` 包，故只校验 `Contents` 段与其 `.app` 父目录）。
#[must_use]
pub(crate) fn capability_for_exe(exe: &Path) -> NotificationCapability {
    let mut ancestors = exe.ancestors().peekable();
    while let Some(dir) = ancestors.next() {
        if dir.file_name().is_some_and(|name| name == "Contents")
            && ancestors
                .peek()
                .and_then(|bundle| bundle.file_name())
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(".app"))
        {
            return NotificationCapability::Bundled;
        }
    }
    NotificationCapability::Unbundled
}

/// 当前进程的通知能力（生产入口；`current_exe` 失败按最严格处理：视为 unbundled）。
fn current_capability() -> NotificationCapability {
    std::env::current_exe().map_or_else(
        |_| NotificationCapability::Unbundled,
        |exe| capability_for_exe(&exe),
    )
}

// ---- 系统版本识别（纯解析 + sysctl seam）----

/// 解析 `kern.osproductversion` 的主版本（`"26.5.2"` → `26`；非数字/空 → `None`）。
#[must_use]
pub(crate) fn parse_major_version(raw: &str) -> Option<u32> {
    raw.trim().split('.').next()?.parse::<u32>().ok()
}

/// 读取 `kern.osproductversion`（失败返回 `None`，调用方按未知版本处理）。
fn read_osproductversion() -> Option<String> {
    let name = c"kern.osproductversion";
    let mut size: libc::size_t = 0;
    // SAFETY: 标准两段式 sysctl——先取长度，再按该长度读入缓冲区；两次调用之间长度
    // 变化只会让第二次返回失败，此时如实返回 None。
    unsafe {
        if libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
            || size == 0
        {
            return None;
        }
        let mut buffer = vec![0_u8; size];
        if libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buffer.truncate(size.saturating_sub(1));
        String::from_utf8(buffer).ok()
    }
}

/// 当前系统主版本（`None` = 读不到）。
#[must_use]
pub(crate) fn os_major_version() -> Option<u32> {
    read_osproductversion().as_deref().and_then(parse_major_version)
}

/// 版本 + 能力 → 后端选择（纯函数，表驱动单测）。
///
/// `None` 表示「不投递」（未打包二进制）。版本读不到时选新路径：它能如实报错，失败会
/// 由 [`notify`] 回退旧路径；反之在 macOS 26 上选旧路径只会静默不投递、无从发现。
#[must_use]
pub(crate) fn strategy_for(
    major: Option<u32>,
    capability: NotificationCapability,
) -> Option<NotificationStrategy> {
    match capability {
        NotificationCapability::Unbundled => None,
        NotificationCapability::Bundled => Some(match major {
            Some(major) if major >= MODERN_BACKEND_MIN_MAJOR => NotificationStrategy::Modern,
            Some(_) => NotificationStrategy::Legacy,
            None => NotificationStrategy::Modern,
        }),
    }
}

// ---- 投递结果与诊断落盘 ----

/// `tray_notify` 的结果：前端据此决定是否应用内 toast 回退。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellNotifyOutcomeDto {
    delivered: bool,
    /// `delivered=false` 时的稳定降级原因；成功为 None。
    degraded_reason: Option<&'static str>,
}

impl ShellNotifyOutcomeDto {
    const fn submitted() -> Self {
        Self {
            delivered: true,
            degraded_reason: None,
        }
    }

    const fn degraded(reason: &'static str) -> Self {
        Self {
            delivered: false,
            degraded_reason: Some(reason),
        }
    }
}

/// 诊断文件叶名（`~/Library/Application Support/EXV/notification.log`）。
const NOTIFICATION_LOG_LEAF: &str = "notification.log";

/// 追加一行诊断（时间戳用 UNIX 秒，可排序；细节截断到 200 字符；**不写标题/正文**）。
pub(crate) fn record_delivery(strategy: &str, major: Option<u32>, outcome: &str, detail: &str) {
    let Some(dir) = crate::ui_prefs::ui_state_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let major = major.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let detail = detail
        .chars()
        .take(200)
        .collect::<String>()
        .replace(['\n', '\r'], " ");
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(" detail={detail}")
    };
    let line = format!("{seconds} strategy={strategy} os_major={major} outcome={outcome}{suffix}\n");
    let path = dir.join(NOTIFICATION_LOG_LEAF);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            file.write_all(line.as_bytes())
        });
}

/// 把新后端的异步结果落盘（在系统回调队列上被调用）。
fn report_to_log(major: Option<u32>, report: DeliveryReport) {
    match report {
        DeliveryReport::Delivered => record_delivery("modern", major, "delivered", ""),
        DeliveryReport::Denied(detail) => record_delivery("modern", major, "denied", &detail),
        DeliveryReport::Failed(detail) => record_delivery("modern", major, "failed", &detail),
    }
}

// ---- 发送编排 ----

/// 旧路径（插件）：`show()` 只表示已交给系统（fire-and-forget），如实记为 `submitted`。
fn legacy(app: &AppHandle, title: &str, body: &str, major: Option<u32>) -> ShellNotifyOutcomeDto {
    match app.notification().builder().title(title).body(body).show() {
        Ok(()) => {
            record_delivery("legacy", major, "submitted", "");
            ShellNotifyOutcomeDto::submitted()
        }
        Err(error) => {
            let detail = error.to_string();
            record_delivery("legacy", major, "failed", &detail);
            #[allow(clippy::print_stderr)] // debug 壳 stderr 是辅助诊断通道
            {
                eprintln!("exv.notification: legacy send failed (degraded): {detail}");
            }
            ShellNotifyOutcomeDto::degraded(DEGRADED_SEND_FAILED)
        }
    }
}

/// 生产发送路径：按系统版本选后端；新后端提交失败时回退旧路径。
fn notify(app: &AppHandle, title: &str, body: &str) -> ShellNotifyOutcomeDto {
    let capability = current_capability();
    let major = os_major_version();
    let Some(strategy) = strategy_for(major, capability) else {
        // 态 B（debug 壳预期态）：如实降级，不触碰任何通知路径。
        record_delivery("none", major, "degraded", DEGRADED_UNBUNDLED);
        return ShellNotifyOutcomeDto::degraded(DEGRADED_UNBUNDLED);
    };
    match strategy {
        NotificationStrategy::Legacy => legacy(app, title, body, major),
        NotificationStrategy::Modern => {
            let report = move |report: DeliveryReport| report_to_log(major, report);
            match notification_modern::submit(app, title.to_owned(), body.to_owned(), report) {
                Ok(()) => {
                    // 已提交给系统；真实结果（delivered/denied/failed）由异步回调落盘。
                    record_delivery("modern", major, "submitted", "");
                    ShellNotifyOutcomeDto::submitted()
                }
                Err(error) => {
                    record_delivery("modern", major, "submit-failed", &error);
                    let fallback = legacy(app, title, body, major);
                    if fallback.delivered {
                        fallback
                    } else {
                        ShellNotifyOutcomeDto::degraded(DEGRADED_MODERN_FAILED)
                    }
                }
            }
        }
    }
}

/// `tray_notify` Command（win32 前端 `tray_notify` 的 darwin 对应入口）。
///
/// win32 前端期待 void 回复（失败仅 console.error），降级结果不回传、不伪造成功；
/// 诊断细节落 `notification.log` 与 stderr。
#[allow(clippy::needless_pass_by_value)] // Tauri command 宏的固定签名形状
#[tauri::command]
pub(crate) fn tray_notify(app: AppHandle, title: String, body: String) {
    let outcome = notify(&app, &title, &body);
    if let Some(reason) = outcome.degraded_reason {
        #[allow(clippy::print_stderr)]
        {
            eprintln!("exv.notification: tray_notify degraded: {reason}");
        }
    }
}
