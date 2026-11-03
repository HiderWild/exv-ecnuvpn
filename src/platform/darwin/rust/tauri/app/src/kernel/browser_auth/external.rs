//! Darwin外部浏览器原语：沿用LaunchServices和EXV独立profile，认证循环在父模块。
use std::collections::HashSet;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

use super::*;
use exv_vpn_atrust::auth::browser_session::{
    BrowserSession, CookieSessionError, session_from_cookie_pairs,
};
use exv_vpn_atrust::browser_selection::{BrowserKind, Detection};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

/// 浏览器进程启动并写出 CDP 入口的等待上界（win32 同值 15s）。
const BROWSER_LAUNCH_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次 CDP 调用上界（win32 同值 10s）。
const CDP_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// 孤儿实例 SIGTERM 后的退出等待上界；超窗是有界失败（不无限循环）。
const ORPHAN_EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// 正常关闭（`Browser.close`）后等待进程退出的宽限；超窗由 kill-on-drop 兜底
/// （win32 同值 3s，给浏览器写资料目录的时间）。
const GRACEFUL_EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// 孤儿命令行核对的固定进程（只读 `-p <pid>`，不扫全表）。
const PS_PATH: &str = "/bin/ps";
/// 轮询间隙事件排空窗口：只吸收**已积压**的 target 事件（关窗在睡眠期间发生时下一轮
/// 即可见），不等新事件到达——静默连接上挂满 CDP_CALL_TIMEOUT 会让登录轮询退化成
/// ~11s 一轮。
const DRAIN_EVENTS_WINDOW: Duration = Duration::from_millis(250);

/// CDP 网络拦截清单（win32 同款）：阻止登录门户唤起官方 aTrust 客户端——
/// `atrust:` 深链与本地回调域 `localhost.sangfor.com.cn`，与内嵌窗拒绝非 HTTPS
/// 导航的防护对齐。
const BLOCKED_URLS: [&str; 2] = ["*://localhost.sangfor.com.cn:*/*", "atrust:*"];

/// 当前暂停Fetch请求队列的既有上限。
const NETWORK_FAILURE_LOG_CAP: usize = 32;

// ---- 支持的浏览器（识别面纯函数，可脱离 AppKit 单测） ----

/// 受支持的受控外部浏览器（Chrome/Edge；Safari 自动化通道已按用户裁定摘除，
/// 原生交接模式另行规划）。支持集与平台优先序以共享模块为准
/// （`exv_vpn_atrust::browser_selection::MACOS_SUPPORTED`），本枚举只承载识别与通道材料。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupportedBrowser {
    Chrome,
    Edge,
}

impl SupportedBrowser {
    /// LaunchServices bundle identifier（识别面；不按可执行文件名猜）。
    pub(crate) fn bundle_identifier(self) -> &'static str {
        match self {
            Self::Chrome => "com.google.Chrome",
            Self::Edge => "com.microsoft.edgemac",
        }
    }

    /// EXV 专用资料目录内的浏览器子目录名。
    fn profile_name(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
        }
    }

    /// bundle 根内的主程序相对路径（稳定布局约定；win32 已知安装目录同款语义）。
    fn executable_relative_path(self) -> &'static str {
        match self {
            Self::Chrome => "Contents/MacOS/Google Chrome",
            Self::Edge => "Contents/MacOS/Microsoft Edge",
        }
    }

    /// 前端可读的稳定标识（会话级选择/状态查询的 wire 词表）。
    pub(crate) fn wire_name(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
        }
    }

    /// 共享选择模块的种类（探测/阶梯经 `BrowserKind` 交流）。
    pub(crate) fn kind(self) -> BrowserKind {
        match self {
            Self::Chrome => BrowserKind::Chrome,
            Self::Edge => BrowserKind::Edge,
        }
    }

    /// 共享种类 → 本宿主浏览器；darwin 支持集外的种类（Safari，原生交接另行
    /// 规划）在此收敛为不可用——阶梯侧可用集不会给出，防御性回退 Chrome。
    pub(crate) fn from_kind(kind: BrowserKind) -> Self {
        match kind {
            BrowserKind::Chrome | BrowserKind::Safari => Self::Chrome,
            BrowserKind::Edge => Self::Edge,
        }
    }
}

/// LaunchServices 枚举顺序：与共享模块 darwin 支持集优先序（Chrome>Edge）
/// 同序；`browser_status.installed` wire 列表同此序。
pub(crate) const ENUMERATION_ORDER: [SupportedBrowser; 2] = [
    SupportedBrowser::Chrome,
    SupportedBrowser::Edge,
];

/// 前端 wire 词表（chrome/edge）→ 受支持浏览器；未知取值 `None`（不猜）。
pub(crate) fn browser_for_wire_name(wire: &str) -> Option<SupportedBrowser> {
    match wire {
        "chrome" => Some(SupportedBrowser::Chrome),
        "edge" => Some(SupportedBrowser::Edge),
        _ => None,
    }
}

/// bundle identifier → 受支持浏览器；未知/不支持返回 `None`（Firefox 等）。
pub(crate) fn browser_for_bundle_id(bundle_id: &str) -> Option<SupportedBrowser> {
    match bundle_id {
        "com.google.Chrome" => Some(SupportedBrowser::Chrome),
        "com.microsoft.edgemac" => Some(SupportedBrowser::Edge),
        _ => None,
    }
}

/// 一次登录将使用的浏览器与其主程序路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserChoice {
    pub(crate) browser: SupportedBrowser,
    pub(crate) executable: PathBuf,
}

/// bundle 根（`....app`）内解析主程序；布局不符或不存在返回 `None`。
fn executable_in_bundle(bundle: &Path, browser: SupportedBrowser) -> Option<PathBuf> {
    let executable = bundle.join(browser.executable_relative_path());
    executable.is_file().then_some(executable)
}

// ---- LaunchServices 发现（AppKit 主线程；接缝可注入测试） ----

/// 当前线程直接查询 LaunchServices（调用方保证主线程；无 NSApplication 事件循环
/// 依赖——同步查询 API，与 `external_open` 的主线程约束同源）。
pub(crate) mod launch_services {
    use super::{
        ENUMERATION_ORDER, BrowserChoice, SupportedBrowser, browser_for_bundle_id,
        executable_in_bundle,
    };
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSString, NSURL};

    /// 询问系统默认 https 处理方 App 的 bundle identifier（LaunchServices 机制）。
    fn default_browser_bundle_id() -> Option<String> {
        let probe = NSURL::URLWithString(&NSString::from_str("https://exv.invalid/"))?;
        let app_url = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&probe)?;
        let bundle = NSBundle::bundleWithURL(&app_url)?;
        let identifier = bundle.bundleIdentifier()?;
        Some(identifier.to_string())
    }

    /// 单浏览器 bundle 查询：LaunchServices 注册位置（含 `/Applications`、
    /// `~/Applications` 与系统卷）返回的 bundle 根内解析主程序。
    pub(crate) fn browser_choice(browser: SupportedBrowser) -> Option<BrowserChoice> {
        let app_urls = NSWorkspace::sharedWorkspace().URLsForApplicationsWithBundleIdentifier(
            &NSString::from_str(browser.bundle_identifier()),
        );
        let first = app_urls.firstObject()?;
        let path = PathBufFromNsUrl::path_from(&first)?;
        executable_in_bundle(&path, browser).map(|executable| BrowserChoice {
            browser,
            executable,
        })
    }

    /// 默认浏览器受支持时给出可执行选择；Firefox 等不支持或无默认 → `None`。
    pub(crate) fn default_browser() -> Option<BrowserChoice> {
        let identifier = default_browser_bundle_id()?;
        let browser = browser_for_bundle_id(&identifier)?;
        browser_choice(browser)
    }

    /// 枚举已安装的受支持浏览器（LaunchServices 注册位置；按共享支持集优先序
    /// Chrome → Edge 固定顺序，不按进程猜）。
    pub(crate) fn installed_browsers() -> Vec<BrowserChoice> {
        ENUMERATION_ORDER
            .iter()
            .filter_map(|browser| browser_choice(*browser))
            .collect()
    }

    /// NSURL → 文件系统路径（file URL；非 file URL 返回 `None`）。
    struct PathBufFromNsUrl;
    impl PathBufFromNsUrl {
        fn path_from(url: &NSURL) -> Option<std::path::PathBuf> {
            if !url.isFileURL() {
                return None;
            }
            let path = url.path()?;
            Some(std::path::PathBuf::from(path.to_string()))
        }
    }
}

/// 生产入口：在 tauri 主线程执行 LaunchServices 查询（AppKit 主线程约束，与
/// `external_open::open_in_default_browser` 同款派发形态）。
pub(crate) async fn discover_default_browser(
    app: &tauri::AppHandle,
) -> Result<Option<BrowserChoice>, String> {
    run_on_main_thread(app, launch_services::default_browser).await
}

/// 生产入口：枚举已安装的受支持浏览器（主线程约束同上）。
pub(crate) async fn discover_installed_browsers(
    app: &tauri::AppHandle,
) -> Result<Vec<BrowserChoice>, String> {
    run_on_main_thread(app, launch_services::installed_browsers).await
}

/// 生产入口：单浏览器主程序解析（缓存命中轻量复核与通道分派用）。`fn` 指针
/// 派发需按种类静态分列（`run_on_main_thread` 不携带捕获）。
pub(crate) async fn discover_browser_choice(
    app: &tauri::AppHandle,
    browser: SupportedBrowser,
) -> Result<Option<BrowserChoice>, String> {
    fn chrome() -> Option<BrowserChoice> {
        launch_services::browser_choice(SupportedBrowser::Chrome)
    }
    fn edge() -> Option<BrowserChoice> {
        launch_services::browser_choice(SupportedBrowser::Edge)
    }
    let query = match browser {
        SupportedBrowser::Chrome => chrome as fn() -> Option<BrowserChoice>,
        SupportedBrowser::Edge => edge,
    };
    run_on_main_thread(app, query).await
}

/// 探测事实 → 共享 [`Detection`]（纯函数，可单测）：`default`/`chosen` 只做支持集
/// 映射，可用性裁决由阶梯按可用集完成。
pub(crate) fn detection_from(
    installed: &[BrowserChoice],
    default: Option<&BrowserChoice>,
    chosen: Option<SupportedBrowser>,
) -> Detection {
    Detection {
        default: default.map(|choice| choice.browser.kind()),
        available: installed
            .iter()
            .map(|choice| choice.browser.kind())
            .collect(),
        chosen: chosen.map(SupportedBrowser::kind),
    }
}

/// 完整探测（缓存未命中路径）：LaunchServices 双查询（已安装枚举 + 默认浏览器）
/// → 共享 [`Detection`]。`chosen` 为会话内显式偏好（wire 兼容面，生产前端无写入方）。
pub(crate) async fn detect_browsers(
    app: &tauri::AppHandle,
    chosen: Option<SupportedBrowser>,
) -> Result<Detection, String> {
    let installed = discover_installed_browsers(app).await?;
    let default = discover_default_browser(app).await?;
    Ok(detection_from(&installed, default.as_ref(), chosen))
}

/// 缓存命中路径的轻量复核（B1 约束：二进制在位级别，禁止会话级探测）：
/// Chrome/Edge=主程序在位。
pub(crate) async fn browser_kind_available(
    app: &tauri::AppHandle,
    kind: BrowserKind,
) -> Result<bool, String> {
    let browser = SupportedBrowser::from_kind(kind);
    Ok(discover_browser_choice(app, browser).await?.is_some())
}

/// 主线程派发 + oneshot 回传（`external_open` 同款）。
async fn run_on_main_thread<T: Send + 'static>(
    app: &tauri::AppHandle,
    query: fn() -> T,
) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(query());
    })
    .map_err(|error| format!("LaunchServices 查询派发主线程失败：{error}"))?;
    rx.await
        .map_err(|_| "LaunchServices 查询未回传结果".to_string())
}

// ---- EXV 专用资料目录 ----

/// 资料目录根：`~/Library/Application Support/EXV/atrust-browser`（系统用户目录；
/// 不写死用户名）。根下按浏览器分子目录。这是用户认证数据，构建清理不得触碰。
pub(crate) fn profile_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "无法解析用户主目录".to_owned())?;
    Ok(home
        .join("Library")
        .join("Application Support")
        .join("EXV")
        .join("atrust-browser"))
}

/// 浏览器资料目录（根 + 浏览器子目录）。
fn browser_profile(root: &Path, browser: SupportedBrowser) -> PathBuf {
    root.join(browser.profile_name())
}

// ---- 孤儿实例回收（资料目录单例占用判别） ----

/// 读资料目录 `SingletonLock` 符号链接尾部的 PID（`<主机名>-<PID>`；主机名可含
/// 连字符，故从尾部解析）。无锁或形态不符 → `None`。
fn singleton_lock_pid(profile: &Path) -> Option<i32> {
    let target = std::fs::read_link(profile.join("SingletonLock")).ok()?;
    let name = target.to_str()?;
    let (_, pid) = name.rsplit_once('-')?;
    pid.parse::<i32>().ok()
}

/// 动作前核实可执行文件路径；查询或解码失败均为 None，不能据此裁定退出。
fn process_executable_path(pid: i32) -> Option<PathBuf> {
    #[repr(C)]
    struct PidPathBuffer {
        bytes: [u8; 4096],
    }
    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut u8, size: u32) -> i32;
    }
    let mut buffer = PidPathBuffer { bytes: [0; 4096] };
    // SAFETY: 传入本栈缓冲区指针与真实长度；查询失败时返回 0。
    let length = unsafe { proc_pidpath(pid, buffer.bytes.as_mut_ptr(), buffer.bytes.len() as u32) };
    if length <= 0 {
        return None;
    }
    let path = std::str::from_utf8(&buffer.bytes[..length as usize]).ok()?;
    Some(PathBuf::from(path))
}

/// 指定 PID 的命令行（固定 `/bin/ps -p <pid> -o command=`，只读该进程，不扫全表）。
/// 进程不存在或无输出 → `None`（调用方按「无法核实」处理，绝不猜）。
fn process_command_line(pid: i32) -> Option<String> {
    let output = std::process::Command::new(PS_PATH)
        .args(["-p", &pid.to_string(), "-o", "command="])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!line.is_empty()).then_some(line)
}

/// 资料目录是否仍被**活实例**占用：PID 存活 + 可执行文件与预期浏览器一致 + 命令行
/// 携带本资料目录（防 PID 复用误杀用户日常浏览器）。三重核实缺一不可。
fn live_dedicated_instance(profile: &Path, executable: &Path) -> Option<i32> {
    let pid = singleton_lock_pid(profile)?;
    let running = process_executable_path(pid)?;
    // 规范化后比较（如 /Applications 与 /System/Volumes/Data/... 前缀差异）。
    let same_executable = std::fs::canonicalize(&running)
        .ok()
        .zip(std::fs::canonicalize(executable).ok())
        .is_some_and(|(a, b)| a == b);
    if !same_executable {
        return None;
    }
    let user_data_arg = format!("--user-data-dir={}", profile.display());
    if !process_command_line(pid)?.contains(&user_data_arg) {
        return None;
    }
    Some(pid)
}

/// 只观察动作前已确认的一个专用实例，fd 随本次动作释放。
struct ProcessExitWatch {
    fd: OwnedFd,
    pid: i32,
}

fn process_io_error(context: &str, error: std::io::Error) -> String {
    match error.raw_os_error() {
        Some(errno) => format!("{context}：{error}（errno={errno}）"),
        None => format!("{context}：{error}"),
    }
}

fn process_event_error(context: &str, data: libc::intptr_t) -> String {
    match i32::try_from(data) {
        Ok(errno) => process_io_error(context, std::io::Error::from_raw_os_error(errno)),
        Err(_) => format!("{context}：原 kevent 错误 data={data} 超出 errno 表示范围"),
    }
}

/// true 为成功登记，false 仅为本次登记的原 ESRCH；回执不是退出事件。
fn process_exit_receipt(pid: i32, receipt: &libc::kevent) -> Result<bool, String> {
    if receipt.ident != pid as libc::uintptr_t
        || receipt.filter != libc::EVFILT_PROC
        || receipt.flags & libc::EV_ERROR == 0
    {
        return Err("登记上次 EXV 登录浏览器退出事件时未收到对应注册回执".into());
    }
    // packed kevent 的 guard 绑定会借用字段，先按值复制到正常对齐的局部量。
    let data = receipt.data;
    match data {
        0 => Ok(true),
        data if data == libc::ESRCH as libc::intptr_t => Ok(false),
        data => Err(process_event_error(
            "登记上次 EXV 登录浏览器退出事件失败",
            data,
        )),
    }
}

fn process_exit_event(pid: i32, event: &libc::kevent) -> Result<bool, String> {
    if event.flags & libc::EV_ERROR != 0 {
        // data=0 是成功回执，没有原 errno，且不构成退出。
        if event.data == 0 {
            return Ok(false);
        }
        return Err(process_event_error(
            "等待上次 EXV 登录浏览器退出事件失败",
            event.data,
        ));
    }
    Ok(event.ident == pid as libc::uintptr_t
        && event.filter == libc::EVFILT_PROC
        && event.fflags & libc::NOTE_EXIT != 0)
}

impl ProcessExitWatch {
    fn register(pid: i32) -> Result<Option<Self>, String> {
        // SAFETY: 创建本次独占的队列；成功 fd 立即交给 OwnedFd。
        let raw_fd = unsafe { libc::kqueue() };
        if raw_fd < 0 {
            let error = std::io::Error::last_os_error();
            return Err(process_io_error(
                "创建 EXV 登录浏览器退出事件队列失败",
                error,
            ));
        }
        // SAFETY: kqueue 返回新且有效的 fd，此后只由 OwnedFd 关闭。
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let change = libc::kevent {
            ident: pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT | libc::EV_RECEIPT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let mut receipt = std::mem::MaybeUninit::<libc::kevent>::uninit();
        let zero = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: 一项输入及一个有效输出槽；零 timeout，回执与退出读取分开。
        let count =
            unsafe { libc::kevent(fd.as_raw_fd(), &change, 1, receipt.as_mut_ptr(), 1, &zero) };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            return Err(process_io_error(
                "登记上次 EXV 登录浏览器退出事件失败",
                error,
            ));
        }
        if count != 1 {
            return Err("登记上次 EXV 登录浏览器退出事件时未返回注册回执".into());
        }
        // SAFETY: kevent 已确认写入一项；FFI 缓冲不跨异步等待。
        if !process_exit_receipt(pid, &unsafe { receipt.assume_init() })? {
            return Ok(None);
        }
        Ok(Some(Self { fd, pid }))
    }

    fn exited(&self) -> Result<bool, String> {
        let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
        let zero = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: 无变更、一个有效输出槽、零 timeout；不阻塞运行时线程。
        let count = unsafe {
            libc::kevent(
                self.fd.as_raw_fd(),
                std::ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                &zero,
            )
        };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            return Err(process_io_error(
                "读取上次 EXV 登录浏览器退出事件失败",
                error,
            ));
        }
        if count == 0 {
            return Ok(false);
        }
        // SAFETY: 本次已写入唯一输出槽，事件只在同步方法内消费。
        process_exit_event(self.pid, &unsafe { event.assume_init() })
    }
}

/// 只为本次已确认 PID 接合登记、原信号与退出等待；guard 不离开本次调用。
async fn reclaim_confirmed_instance_with<Watch>(
    register: impl FnOnce() -> Result<Option<Watch>, String>,
    terminate: impl FnOnce() -> std::io::Result<()>,
    mut exited: impl FnMut(&Watch) -> Result<bool, String>,
    budget: Duration,
) -> Result<(), String> {
    let Some(watch) = register()? else {
        return Ok(());
    };
    terminate().map_err(|error| {
        process_io_error("无法回收上次未关闭的 EXV 登录浏览器（SIGTERM）", error)
    })?;
    let deadline = tokio::time::Instant::now() + budget;
    while tokio::time::Instant::now() < deadline {
        if exited(&watch)? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("上次 EXV 登录浏览器未在期限内退出，请稍后重试".to_owned())
}

/// 仍只处置原识别出的专用实例；未确认时交浏览器原单例机制仲裁。
async fn reclaim_profile_instance(profile: &Path, executable: &Path) -> Result<(), String> {
    if let Some(pid) = live_dedicated_instance(profile, executable) {
        reclaim_confirmed_instance_with(
            || ProcessExitWatch::register(pid),
            || {
                // SAFETY: 仅原已确认 PID；退出订阅成功后才发送原 SIGTERM。
                if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
                    Ok(())
                } else {
                    // 必须在任何其他 syscall／释放 fd 前取得原错误所有权。
                    Err(std::io::Error::last_os_error())
                }
            },
            ProcessExitWatch::exited,
            ORPHAN_EXIT_TIMEOUT,
        )
        .await?;
    }
    Ok(())
}

// ---- 专用实例（connect-first 复用 + 取消感知收尾） ----

/// 手动登录的网关级持久浏览器目录（2026-10-26 S4a，对齐 win32 S4c 形态）：
/// `<root>/manual-persistent/<sha256(gateway)>/`，跨次登录复用，浏览器记住的
/// 密码/用户名与 SSO 会话随目录保留；EXV 不在登录尝试结束时删除该目录。
/// 哈希与 `manual-username-<sha>` 同源（[`super::gateway_hash`]）。
pub(super) fn manual_persistent_root(root: &Path, gateway: &str) -> Result<PathBuf, String> {
    let parent = root.join("manual-persistent");
    std::fs::create_dir_all(&parent)
        .map_err(|error| format!("创建手动登录持久目录失败：{error}"))?;
    let path = parent.join(super::gateway_hash(gateway));
    std::fs::create_dir_all(&path).map_err(|error| format!("创建手动登录持久目录失败：{error}"))?;
    Ok(path)
}

/// 本次使用的专用浏览器实例。`Spawned`=本次 spawn 的进程，Drop 走取消感知收尾
/// （有界等端口 → `Browser.close` 优雅关闭 → 有界等退出 → SIGKILL 仅最后兜底
/// 并记日志）；`Reused`=connect-first 复用的活实例，不拥有进程，Drop 无杀路径。
struct BrowserInstance {
    owner: InstanceOwner,
    profile: PathBuf,
}

enum InstanceOwner {
    Spawned {
        child: Child,
        spawned_at: std::time::Instant,
    },
    /// connect-first 复用；`pid` 为经三重核实可确认时的进程号（呈现前置用），
    /// 无法核实（如 ps 拒答）时为 `None`——只影响应用级前置，不影响复用。
    Reused { pid: Option<i32> },
}

impl BrowserInstance {
    fn spawned(profile: PathBuf, child: Child, spawned_at: std::time::Instant) -> Self {
        Self {
            owner: InstanceOwner::Spawned { child, spawned_at },
            profile,
        }
    }

    fn reused(profile: PathBuf, pid: Option<i32>) -> Self {
        Self {
            owner: InstanceOwner::Reused { pid },
            profile,
        }
    }

    /// 呈现前置用的进程号；复用实例未核实到进程时为 `None`（跳过应用级前置，
    /// 标签页前置仍生效）。
    fn presentation_pid(&self) -> Option<u32> {
        match &self.owner {
            InstanceOwner::Spawned { child, .. } => Some(child.id()),
            InstanceOwner::Reused { pid } => pid.and_then(|pid| u32::try_from(pid).ok()),
        }
    }

    fn try_wait_exited(&mut self) -> std::io::Result<bool> {
        match &mut self.owner {
            InstanceOwner::Spawned { child, .. } => child.try_wait().map(|status| status.is_some()),
            // 复用实例无进程可查；浏览器退出由 CDP 连接错误与 target 销毁事件
            // 判定（ensure_open），此处不得伪造退出。
            InstanceOwner::Reused { .. } => Ok(false),
        }
    }

    /// 正常收尾只据原 Child 退出；查询失败／超窗仍交原 Drop 兜底。
    async fn wait_exit(&mut self) -> Result<(), String> {
        if let InstanceOwner::Spawned { child, .. } = &mut self.owner {
            wait_child_exit_with(
                || child.try_wait().map(|status| status.is_some()),
                GRACEFUL_EXIT_TIMEOUT,
            )
            .await
        } else {
            Ok(())
        }
    }
}

impl Drop for BrowserInstance {
    fn drop(&mut self) {
        let owner = std::mem::replace(&mut self.owner, InstanceOwner::Reused { pid: None });
        let InstanceOwner::Spawned {
            mut child,
            spawned_at,
        } = owner
        else {
            return; // 复用实例不拥有进程，无杀路径。
        };
        // 已退出的进程只收割，不为已死实例起收尾线程。
        match child.try_wait() {
            Ok(Some(_)) => {
                let _ = child.wait();
                return;
            }
            Ok(None) => {}
            Err(error) => eprintln!("EXV 登录浏览器收尾前进程检查失败：{error}"),
        }
        let profile = std::mem::take(&mut self.profile);
        detach_graceful_reaper(profile, child, spawned_at);
    }
}

/// 取消感知收尾结果：优雅退出（含 `Browser.close` 后退出）与最后兜底强制结束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeardownOutcome {
    ExitedCleanly,
    KilledFallback,
}

/// 取消感知收尾核心：① 无端点（启动期取消）→ 有界等待端口文件出现；
/// ② 端点可用 → 连接并发送 `Browser.close` 优雅关闭；③ 有界等待进程退出；
/// ④ 仍存活 → SIGKILL 最后兜底并记日志。预算与宽限注入（生产=启动窗口剩余
/// 与 3s 宽限；测试用短预算验证顺序）。
async fn graceful_teardown(
    profile: PathBuf,
    child: Child,
    endpoint_budget: Duration,
    exit_grace: Duration,
) -> TeardownOutcome {
    let endpoint = wait_port_file_endpoint(&profile, endpoint_budget).await;
    if let Some(endpoint) = endpoint.as_deref()
        && let Ok((mut cdp, _)) = connect_browser_endpoint(endpoint).await
    {
        let _ = tokio::time::timeout(exit_grace, cdp.call("Browser.close", json!({}), None)).await;
    }
    let mut child = child;
    match wait_child_exit_with(
        || child.try_wait().map(|status| status.is_some()),
        exit_grace,
    )
    .await
    {
        Ok(()) => {
            let _ = child.wait();
            TeardownOutcome::ExitedCleanly
        }
        Err(error) => {
            eprintln!("EXV 登录浏览器优雅关闭未在宽限内完成（{error}），最后兜底强制结束进程");
            if let Err(kill_error) = child.kill() {
                eprintln!("EXV 登录浏览器强制结束失败：{kill_error}");
            }
            let _ = child.wait();
            TeardownOutcome::KilledFallback
        }
    }
}

/// 有界轮询资料目录的 `DevToolsActivePort` 直到解析出可用入口。
async fn wait_port_file_endpoint(profile: &Path, budget: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if let Some(endpoint) = endpoint_from_port_file(profile) {
            return Some(endpoint);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drop 是同步上下文；收尾的有界等待在分离线程的自建小运行时中执行，不占用
/// 异步运行时线程。线程派发失败（资源耗尽）时保留进程交由孤儿回收处置。
fn detach_graceful_reaper(profile: PathBuf, child: Child, spawned_at: std::time::Instant) {
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            eprintln!("EXV 登录浏览器收尾运行时创建失败，进程交由下次孤儿回收");
            return;
        };
        let remaining = BROWSER_LAUNCH_TIMEOUT.saturating_sub(spawned_at.elapsed());
        runtime.block_on(graceful_teardown(
            profile,
            child,
            remaining,
            GRACEFUL_EXIT_TIMEOUT,
        ));
    });
}

async fn wait_child_exit_with(
    mut exited: impl FnMut() -> std::io::Result<bool>,
    budget: Duration,
) -> Result<(), String> {
    tokio::time::timeout(budget, async {
        loop {
            if exited()
                .map_err(|error| process_io_error("等待 EXV 登录浏览器退出时检查进程失败", error))?
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|elapsed| format!("等待 EXV 登录浏览器退出超过正常关闭宽限：{elapsed}"))?
}

/// 启动参数（前五项=win32 已验证集合；末两项 `--disable-component-update`/
/// `--disable-sync` 为 2026-10-27 追加的 Chrome 自更新记账通知抑制尝试，与
/// win32 参数集同集、疗效待 E2E 实证；经进程参数 API 传递，不拼 shell）。
const BROWSER_ARGS: [&str; 7] = [
    "--remote-debugging-port=0",
    "--remote-debugging-address=127.0.0.1",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
];

/// connect-first 单次连接/核实预算（win32 同值 1s）。
const CONNECT_FIRST_TIMEOUT: Duration = Duration::from_secs(1);

/// 资料目录 `DevToolsActivePort` 文件解析出的浏览器级 WebSocket 入口；文件
/// 缺失或形态不符（端口 0、路径前缀不符）→ `None`，不猜端口。
fn endpoint_from_port_file(profile: &Path) -> Option<String> {
    let text = std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.parse::<u16>().ok()?;
    let path = lines.next()?;
    (port != 0 && path.starts_with("/devtools/browser/"))
        .then(|| format!("ws://127.0.0.1:{port}{path}"))
}

/// 连接既有浏览器级入口并核实可用性（`Browser.getVersion` 返回非空 UA，win32
/// 同款判定）。返回已就绪的 CDP 与端点；任何阶段失败返回阶段名，供诊断与
/// spawn 决策，不区分“残留文件”与“活实例握手失败”——都交 spawn 路径仲裁。
async fn connect_browser_endpoint(endpoint: &str) -> Result<(Cdp, String), &'static str> {
    let (socket, _) = tokio::time::timeout(
        CONNECT_FIRST_TIMEOUT,
        tokio_tungstenite::connect_async(endpoint),
    )
    .await
    .map_err(|_| "connect-timeout")?
    .map_err(|_| "connect")?;
    let mut cdp = Cdp {
        socket,
        sequence: 0,
        destroyed_targets: HashSet::new(),
        paused_requests: Vec::new(),
    };
    let version = tokio::time::timeout(
        CONNECT_FIRST_TIMEOUT,
        cdp.call("Browser.getVersion", json!({}), None),
    )
    .await
    .map_err(|_| "version-timeout")?
    .map_err(|_| "version")?;
    if !version["userAgent"]
        .as_str()
        .is_some_and(|agent| !agent.trim().is_empty())
    {
        return Err("version-shape");
    }
    Ok((cdp, endpoint.to_owned()))
}

/// connect-first 复用与启动（对齐 win32 `connect_or_launch_owned_until_typed`）：
/// ① 先尝试连接持久资料目录的既有 CDP 入口——活实例直接复用，**复用成功跳过
/// 孤儿回收**（进程不归本次所有，本次不发出任何信号）；
/// ② 连接失败才回收确实无法连接的残留专用实例（原 SIGTERM 路径，仅异常残留
/// 场景可达），随后 spawn 新实例并有界等待新入口就绪。
async fn connect_or_launch_with<F>(
    choice: &BrowserChoice,
    profile: &Path,
    budget: Duration,
    spawn: F,
) -> Result<(BrowserInstance, Cdp), String>
where
    F: FnOnce() -> Result<Child, String>,
{
    if let Some(endpoint) = endpoint_from_port_file(profile) {
        match connect_browser_endpoint(&endpoint).await {
            Ok((cdp, _)) => {
                let pid = live_dedicated_instance(profile, &choice.executable);
                eprintln!("EXV 登录浏览器 connect-first 复用既有实例 pid={pid:?}");
                return Ok((BrowserInstance::reused(profile.to_path_buf(), pid), cdp));
            }
            Err(stage) => {
                eprintln!("EXV 登录浏览器 connect-first 未命中（{stage}），转为重新启动");
            }
        }
    }
    reclaim_profile_instance(profile, &choice.executable).await?;
    std::fs::create_dir_all(profile).map_err(|_| "无法创建 EXV 浏览器配置目录".to_owned())?;
    let port_file = profile.join("DevToolsActivePort");
    match std::fs::remove_file(&port_file) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法准备浏览器调试入口".to_owned()),
    }
    let child = spawn()?;
    let mut instance =
        BrowserInstance::spawned(profile.to_path_buf(), child, std::time::Instant::now());
    let endpoint = tokio::time::timeout(budget, async {
        loop {
            // 启动失败（如单例被转发后立即退出/崩溃循环）在此尽早发现；这不是关窗
            // 判据（关窗判据是 CDP target 事件，见模块文档）。
            if instance
                .try_wait_exited()
                .map_err(|_| "无法检查浏览器状态".to_owned())?
            {
                return Err("登录浏览器启动后已退出".to_owned());
            }
            if let Some(endpoint) = endpoint_from_port_file(profile) {
                break Ok(endpoint);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "等待浏览器调试入口超时".to_owned())??;
    let (cdp, _) = connect_browser_endpoint(&endpoint)
        .await
        .map_err(|stage| format!("连接登录浏览器调试入口失败（{stage}）"))?;
    Ok((instance, cdp))
}

// ---- CDP 客户端 ----

/// CDP WebSocket 客户端：浏览器级连接 + 会话级调用。读取循环同时观测
/// `Target.targetDestroyed` 事件（关窗判据）。
struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    sequence: u64,
    destroyed_targets: HashSet<String>,
    /// 当前暂停请求，仅保存原Fetch处置需要的标识与命中结果。
    paused_requests: Vec<PausedRequest>,
}

/// 暂停请求的原处置材料，不保存完整URL。
#[derive(Debug, Clone)]
struct PausedRequest {
    request_id: String,
    session_id: Option<String>,
    blocked: bool,
}

/// URL 是否命中深链/本地回调拦截规则（与 [`BLOCKED_URLS`] 同一清单的语义判定）。
/// 只做前缀与主机名判断，不解析 URL、不保存。
fn url_hits_block_rule(url: &str) -> bool {
    if url.starts_with("atrust:") {
        return true;
    }
    // `*://localhost.sangfor.com.cn:*/*`：任意 scheme、任意端口的本地回调域。
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    rest.starts_with("localhost.sangfor.com.cn:")
        || rest == "localhost.sangfor.com.cn"
        || rest.starts_with("localhost.sangfor.com.cn/")
}

impl Cdp {
    /// 单次调用（10s 上界）；等待响应期间到达的 target 事件照常入账。
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        tokio::time::timeout(CDP_CALL_TIMEOUT, async {
            self.socket
                .send(Message::Text(request.to_string().into()))
                .await
                .map_err(|_| "浏览器调试连接已关闭".to_owned())?;
            while let Some(message) = self.socket.next().await {
                let message = message.map_err(|_| "浏览器调试连接读取失败".to_owned())?;
                if let Message::Text(text) = message {
                    let value: Value =
                        serde_json::from_str(&text).map_err(|_| "浏览器响应格式错误".to_owned())?;
                    self.observe_events(&value);
                    // 深链/拦截命中即断（不等主命令返回；CDP 支持并发命令）。
                    if !self.paused_requests.is_empty() {
                        self.settle_paused_requests().await;
                    }
                    if value["id"].as_u64() == Some(id) {
                        if value.get("error").is_some() {
                            if method == "Page.bringToFront" {
                                return Err(format!(
                                    "Page.bringToFront 原错误：{}",
                                    value["error"]
                                ));
                            }
                            return Err("浏览器命令执行失败".to_owned());
                        }
                        return Ok(value["result"].clone());
                    }
                }
            }
            Err("浏览器已退出".to_owned())
        })
        .await
        .map_err(|_| "浏览器命令超时".to_owned())?
    }

    /// 仅记录关窗与待处置Fetch原事件。
    fn observe_events(&mut self, value: &Value) {
        if let Some(target_id) = destroyed_target_id(value) {
            self.destroyed_targets.insert(target_id.to_owned());
        }
        if value.get("id").is_none()
            && value["method"] == "Fetch.requestPaused"
            && let Some(request_id) = value["params"]["requestId"].as_str()
            && self.paused_requests.len() < NETWORK_FAILURE_LOG_CAP
        {
            let url = value["params"]["request"]["url"]
                .as_str()
                .unwrap_or_default();
            self.paused_requests.push(PausedRequest {
                request_id: request_id.to_owned(),
                session_id: value["sessionId"].as_str().map(str::to_owned),
                blocked: url_hits_block_rule(url),
            });
        }
    }

    /// 处置全部暂停中的请求（fire-and-forget：只发送，不等待响应——CDP 支持并发
    /// 命令，响应由后续 call/drain 的 id 匹配自然丢弃）。命中拦截规则 →
    /// `Fetch.failRequest(BlockedByClient)`；其余 → `Fetch.continueRequest`（不能
    /// 让暂停请求悬死）。在任何读循环观察到暂停事件后立即调用，深链即发即断。
    async fn settle_paused_requests(&mut self) {
        let paused = std::mem::take(&mut self.paused_requests);
        for request in paused {
            let (method, reason) = if request.blocked {
                (
                    "Fetch.failRequest",
                    Some(json!({"errorReason": "BlockedByClient"})),
                )
            } else {
                ("Fetch.continueRequest", None)
            };
            let mut command = json!({"id": 0, "method": method, "params": {
                "requestId": request.request_id
            }});
            if let Some(reason) = reason {
                command["params"]["errorReason"] = reason["errorReason"].clone();
            }
            if let Some(session) = request.session_id {
                command["sessionId"] = json!(session);
            }
            self.sequence += 1;
            command["id"] = json!(self.sequence);
            let _ = self
                .socket
                .send(Message::Text(command.to_string().into()))
                .await;
        }
    }

    /// 登录标签页是否已被销毁（用户关窗/关标签的 CDP 判据）。
    fn target_destroyed(&self, target_id: &str) -> bool {
        self.destroyed_targets.contains(target_id)
    }

    /// 排空积压消息（轮询间隙到达的事件在此入账；短窗只吸收已积压事件，有界防挂）。
    /// 观察到暂停请求立即处置（含深链拦截）。
    async fn drain_events(&mut self) {
        let _ = tokio::time::timeout(DRAIN_EVENTS_WINDOW, async {
            while let Some(message) = self.socket.next().await {
                let Ok(Message::Text(text)) = message else {
                    continue;
                };
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    self.observe_events(&value);
                    if !self.paused_requests.is_empty() {
                        self.settle_paused_requests().await;
                    }
                }
            }
        })
        .await;
    }

    /// `Browser.close` 后有界等待对端关闭连接（复用实例无进程可等待时的收尾
    /// 判据）；期间到达的事件照常入账。
    async fn wait_closed(&mut self, budget: Duration) -> Result<(), String> {
        tokio::time::timeout(budget, async {
            while let Some(message) = self.socket.next().await {
                if let Ok(Message::Text(text)) = message
                    && let Ok(value) = serde_json::from_str::<Value>(&text)
                {
                    self.observe_events(&value);
                }
            }
        })
        .await
        .map_err(|elapsed| format!("等待 EXV 登录浏览器连接关闭超时：{elapsed}"))
    }
}

/// 消息是否为浏览器级 `Target.targetDestroyed` 事件；是则返回被销毁的 targetId。
/// 带 `id` 字段的是命令响应，不是事件。
fn destroyed_target_id(value: &Value) -> Option<&str> {
    if value.get("id").is_some() || value["method"] != "Target.targetDestroyed" {
        return None;
    }
    value["params"]["targetId"].as_str()
}

/// 打开登录标签页并完成会话装配（发现事件开启 + 网络启用 + 深链拦截）。
struct LoginTab {
    target_id: String,
    session_id: String,
    user_agent: String,
}

async fn open_login_tab(cdp: &mut Cdp) -> Result<LoginTab, String> {
    let version = cdp.call("Browser.getVersion", json!({}), None).await?;
    let user_agent = version["userAgent"]
        .as_str()
        .ok_or("浏览器未返回标识")?
        .to_owned();
    // 开启 target 发现：登录标签页销毁（关窗/关标签）事件从此可达——darwin 的
    // 关窗取消判据（win32 用子进程退出轮询，macOS 不可用）。
    cdp.call("Target.setDiscoverTargets", json!({"discover": true}), None)
        .await?;
    let target = cdp
        .call("Target.createTarget", json!({"url": "about:blank"}), None)
        .await?;
    let target_id = target["targetId"]
        .as_str()
        .ok_or("无法创建登录标签页")?
        .to_owned();
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            None,
        )
        .await?;
    let session_id = attached["sessionId"]
        .as_str()
        .ok_or("无法绑定登录标签页")?
        .to_owned();
    cdp.call("Network.enable", json!({}), Some(&session_id))
        .await?;
    // 深链/本地回调拦截（清单见 [`BLOCKED_URLS`]；阻止门户唤起官方 aTrust 客户端）。
    // 双层机制（B0 实证，见 evidence/2026-09-30-darwin-atrust-browser-b0-verification.md）：
    //   * `Network.setBlockedURLs`——win32 同款清单（字节对齐）；
    //   * `Fetch.enable` 请求拦截——命中清单的请求**暂停后由本模块显式
    //     `Fetch.failRequest(BlockedByClient)`**。实测（Chrome 142）setBlockedURLs
    //     对本会话请求未生效（glob 与子串两种形态均未拦截回环 fetch），Fetch 拦截
    //     是确定性执行层；pattern 只圈定拦截范围，未命中请求不经暂停、零延迟。
    cdp.call(
        "Network.setBlockedURLs",
        json!({"urls": BLOCKED_URLS}),
        Some(&session_id),
    )
    .await?;
    cdp.call(
        "Fetch.enable",
        json!({"patterns": [
            {"urlPattern": "atrust:*"},
            {"urlPattern": "*://localhost.sangfor.com.cn:*/*"},
            {"urlPattern": "*localhost.sangfor.com.cn*"}
        ]}),
        Some(&session_id),
    )
    .await?;
    Ok(LoginTab {
        target_id,
        session_id,
        user_agent,
    })
}

fn browser_pid(pid: u32) -> Result<libc::pid_t, String> {
    libc::pid_t::try_from(pid).map_err(|_| format!("登录浏览器 PID 无法表示：{pid}"))
}

fn activate_application(
    pid: libc::pid_t,
    activate: impl FnOnce(libc::pid_t) -> Option<bool>,
) -> Result<(), String> {
    match activate(pid) {
        Some(true) => Ok(()),
        Some(false) => Err(format!("登录浏览器 activate 原返回 false（PID {pid}）")),
        None => Err(format!("登录浏览器应用不存在（PID {pid}）")),
    }
}

fn activate_browser_on_main_thread(pid: libc::pid_t) -> Result<(), String> {
    let _main_thread = objc2::MainThreadMarker::new().ok_or("登录浏览器前置不在主线程")?;
    activate_application(pid, |pid| {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid).map(|application| {
            application.activateWithOptions(NSApplicationActivationOptions::empty())
        })
    })
}

async fn dispatch_browser_activation(
    pid: libc::pid_t,
    dispatch: impl FnOnce(Box<dyn FnOnce() + Send>) -> Result<(), String>,
    activate: impl FnOnce(libc::pid_t) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    dispatch(Box::new(move || {
        let _ = tx.send(activate(pid));
    }))?;
    rx.await
        .map_err(|_| "登录浏览器前置未回传结果".to_owned())?
}

/// 学校账户预填脚本注册（新鲜手动通道）：`Page.enable` 是脚本生效前提——真机
/// 验证（2026-10-26 Chrome 原生机制验收）缺省时 Chrome 受理注册并返回
/// identifier，但脚本对后续导航一律不执行，这是预填失效的根因。`runImmediately`
/// 与 F7 受控验证的外部浏览器注册形态一致；注册回执必须带 `identifier`，
/// 否则视为未生效。
async fn register_username_memory_script(
    cdp: &mut Cdp,
    session: &str,
    script: &str,
) -> Result<(), String> {
    cdp.call("Page.enable", json!({}), Some(session)).await?;
    let reply = cdp
        .call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": script, "runImmediately": true}),
            Some(session),
        )
        .await?;
    if reply
        .get("identifier")
        .is_some_and(|identifier| !identifier.is_null())
    {
        Ok(())
    } else {
        Err("浏览器未返回预填脚本标识".to_owned())
    }
}

async fn present_login_tab<F: Future<Output = Result<(), String>>>(
    cdp: &mut Cdp,
    session: &str,
    pid: Option<u32>,
    activate: impl FnOnce(libc::pid_t) -> F,
) -> Result<PresentationOutcome, String> {
    cdp.call("Page.bringToFront", json!({}), Some(session))
        .await?;
    // 复用实例未能核实进程号时跳过应用级前置；标签页前置已生效。
    if let Some(pid) = pid {
        activate(browser_pid(pid)?).await?;
    }
    Ok(PresentationOutcome::Visible)
}

fn browser_process_open(
    target_destroyed: bool,
    exited: impl FnOnce() -> std::io::Result<bool>,
) -> Result<(), String> {
    if target_destroyed
        || exited().map_err(|error| process_io_error("检查 EXV 登录浏览器状态失败", error))?
    {
        Err(CANCELLED.to_owned())
    } else {
        Ok(())
    }
}

pub(super) struct ExternalChannel {
    instance: BrowserInstance,
    cdp: Cdp,
    tab: LoginTab,
    app: Option<tauri::AppHandle>,
}
impl ExternalChannel {
    pub(super) async fn open(
        app: &tauri::AppHandle,
        choice: &BrowserChoice,
        intent: AuthIntent,
        gateway: &str,
    ) -> Result<Self, String> {
        let root = profile_root()?;
        // 手动登录使用网关级持久目录（跨次复用登录痕迹，2026-10-26 S4a）；自动
        // 通道沿用根下既有持久目录，形态不变。
        let profile = if intent.fresh() {
            browser_profile(&manual_persistent_root(&root, gateway)?, choice.browser)
        } else {
            browser_profile(&root, choice.browser)
        };
        let mut channel = Self::open_owned_profile(choice, &profile).await?;
        if intent.fresh() {
            let script = super::username_memory_script(
                super::remembered_username(&root, gateway).as_deref(),
            );
            // 注册在 open 内完成：编排器拿到通道后的首次 Page.navigate 之前，
            // 脚本对全部后续新文档（网关页 → 学校 SSO 跳转链）生效；失败不再
            // 静默吞掉（账户记忆是可选功能，失败只诊断）。
            if let Err(error) =
                register_username_memory_script(&mut channel.cdp, &channel.tab.session_id, &script)
                    .await
            {
                eprintln!("EXV 学校账户预填脚本注册失败：{error}");
            }
        }
        channel.app = Some(app.clone());
        Ok(channel)
    }

    async fn open_in_profile(choice: &BrowserChoice, root: &Path) -> Result<Self, String> {
        Self::open_owned_profile(choice, root).await
    }

    async fn open_owned_profile(choice: &BrowserChoice, profile: &Path) -> Result<Self, String> {
        let (instance, mut cdp) =
            connect_or_launch_with(choice, profile, BROWSER_LAUNCH_TIMEOUT, || {
                std::process::Command::new(&choice.executable)
                    .arg(format!("--user-data-dir={}", profile.display()))
                    .args(BROWSER_ARGS)
                    .arg("about:blank")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|_| "无法启动登录浏览器".to_owned())
            })
            .await?;
        let tab = open_login_tab(&mut cdp).await?;
        // 无 AppHandle 的私有构造仅供原生采集夹具；生产 open 必定补齐句柄。
        Ok(Self {
            instance,
            cdp,
            tab,
            app: None,
        })
    }

    fn ensure_open(&mut self) -> Result<(), String> {
        browser_process_open(self.cdp.target_destroyed(&self.tab.target_id), || {
            self.instance.try_wait_exited()
        })
    }

    async fn page_url(&mut self) -> Result<Option<String>, String> {
        let reply = self
            .cdp
            .call(
                "Runtime.evaluate",
                json!({"expression":"JSON.stringify(location.href)","returnByValue":true}),
                Some(&self.tab.session_id),
            )
            .await;
        self.ensure_open()?;
        Ok(reply
            .ok()
            .and_then(|reply| evaluation_string(&reply))
            .and_then(|json| serde_json::from_str(&json).ok()))
    }
}
impl ChannelTrait for ExternalChannel {
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            can_render: true,
            can_extract: true,
            can_inject: false,
            surface: PresentationSurface::AlwaysVisible,
        }
    }
    fn navigate<'a>(&'a mut self, url: &'a str) -> ChannelFuture<'a, ()> {
        Box::pin(async move {
            self.cdp
                .call(
                    "Page.navigate",
                    json!({"url":url}),
                    Some(&self.tab.session_id),
                )
                .await
                .map(|_| ())
        })
    }
    fn present(&mut self, _intent: PresentationIntent) -> ChannelFuture<'_, PresentationOutcome> {
        Box::pin(async move {
            self.ensure_open()?;
            let app = self.app.as_ref().ok_or("登录浏览器缺少主线程派发入口")?;
            present_login_tab(
                &mut self.cdp,
                &self.tab.session_id,
                self.instance.presentation_pid(),
                |pid| {
                    dispatch_browser_activation(
                        pid,
                        |job| {
                            app.run_on_main_thread(job)
                                .map_err(|error| format!("登录浏览器前置派发失败：{error}"))
                        },
                        activate_browser_on_main_thread,
                    )
                },
            )
            .await
        })
    }
    fn observe_page(&mut self, sequence: u64) -> ChannelFuture<'_, PageObservation> {
        Box::pin(async move {
            self.cdp.drain_events().await;
            self.ensure_open()?;
            let before = self.page_url().await?;
            let expression = page_facts::page_sample_script(sequence);
            let reply = self
                .cdp
                .call(
                    "Runtime.evaluate",
                    json!({"expression":expression,"returnByValue":true}),
                    Some(&self.tab.session_id),
                )
                .await;
            self.ensure_open()?;
            let sample = reply.ok().and_then(|reply| evaluation_string(&reply));
            let after = self.page_url().await?;
            Ok(page_facts::parse_sample(
                sample.as_deref(),
                sequence,
                before.as_deref(),
                after.as_deref(),
            ))
        })
    }
    fn probe_session<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, SessionStatus> {
        Box::pin(async move {
            self.cdp.drain_events().await;
            self.ensure_open()?;
            // 固定冻结网关；当前标签页可以停留在学校 SSO，探针不依赖该页 origin。
            let cookies = self
                .cdp
                .call(
                    "Network.getCookies",
                    json!({"urls":[gateway_cookie_request_url(context)]}),
                    Some(&self.tab.session_id),
                )
                .await;
            self.ensure_open()?;
            let Some(header) = cookies.ok().and_then(|reply| probe_cookie_header(&reply)) else {
                return Ok(SessionStatus::Unavailable);
            };
            let result = exv_vpn_darwin_core::atrust_session_probe::probe_auth_config(
                &context.gateway,
                &header,
                &self.tab.user_agent,
                Duration::from_secs(6),
                std::sync::Arc::new(exv_vpn_darwin_core::atrust_http_underlay::HostAuthUnderlay),
            )
            .await;
            Ok(match result {
                Ok(probe) if probe.is_login => SessionStatus::LoggedIn,
                Ok(probe) => SessionStatus::LoggedOut(probe.first_auth),
                Err(_) => SessionStatus::Unavailable,
            })
        })
    }
    fn extract<'a>(
        &'a mut self,
        context: &'a AuthContext,
    ) -> ChannelFuture<'a, Option<BrowserSession>> {
        Box::pin(async move {
            let cookies = self
                .cdp
                .call(
                    "Network.getCookies",
                    json!({"urls":[gateway_cookie_request_url(context)]}),
                    Some(&self.tab.session_id),
                )
                .await?;
            let pairs = cookies["cookies"]
                .as_array()
                .ok_or("浏览器Cookie响应格式错误")?
                .iter()
                .filter_map(|c| {
                    Some((
                        c["name"].as_str()?.to_owned(),
                        c["value"].as_str()?.to_owned(),
                    ))
                })
                .collect::<Vec<_>>();
            match session_from_cookie_pairs(&context.gateway, &self.tab.user_agent, pairs) {
                Ok(session) => Ok(Some(session)),
                Err(CookieSessionError::MissingSession) => Ok(None),
                Err(_) => Err("浏览器会话字段无效".into()),
            }
        })
    }
    fn observed_username(&mut self) -> ChannelFuture<'_, Option<String>> {
        Box::pin(async move {
            let reply = self
                .cdp
                .call(
                    "Network.getCookies",
                    json!({"urls":["https://sso.ecnu.edu.cn/login"]}),
                    Some(&self.tab.session_id),
                )
                .await?;
            Ok(reply["cookies"]
                .as_array()
                .and_then(|cookies| {
                    cookies
                        .iter()
                        .find(|cookie| cookie["name"] == super::USERNAME_COOKIE)
                })
                .and_then(|cookie| cookie["value"].as_str())
                .and_then(super::observed_username))
        })
    }
    fn cleanup(&mut self) -> ChannelFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.cdp.call("Browser.close", json!({}), None).await;
            if matches!(self.instance.owner, InstanceOwner::Reused { .. }) {
                // 复用实例不拥有进程：Browser.close 后以连接关闭为收尾判据。
                self.cdp.wait_closed(GRACEFUL_EXIT_TIMEOUT).await
            } else {
                self.instance.wait_exit().await
            }
        })
    }
}
/// 使用实际 authConfig 请求路径筛选 Cookie，包含 Path=/passport 的会话材料。
/// 仅接受冻结上下文，不读取当前学校标签页 URL。
fn gateway_cookie_request_url(context: &AuthContext) -> String {
    format!("{}{}", context.gateway, context.auth_path)
}

/// CDP returnByValue 的实际字符串；脚本异常没有页面事实。
fn evaluation_string(reply: &Value) -> Option<String> {
    if reply.get("exceptionDetails").is_some() {
        return None;
    }
    reply["result"]["value"].as_str().map(str::to_owned)
}

/// CDP 已按冻结 URL 选择 Cookie；仅生成本次固定网关探针的零化请求头。
fn probe_cookie_header(reply: &Value) -> Option<zeroize::Zeroizing<String>> {
    use exv_vpn_darwin_core::atrust_cas_facts::{apply_set_cookie_pair, cookie_header};
    let mut jar = std::collections::BTreeMap::new();
    for cookie in reply["cookies"].as_array()? {
        apply_set_cookie_pair(
            &mut jar,
            cookie["name"].as_str()?,
            cookie["value"].as_str()?,
        )
        .ok()?;
    }
    Some(cookie_header(&jar).unwrap_or_default())
}
