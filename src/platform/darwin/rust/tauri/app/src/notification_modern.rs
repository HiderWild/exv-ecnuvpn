//! `UNUserNotificationCenter` 通知后端（macOS 10.14+ 的正统 API；本壳在 macOS >= 26 上使用）。
//!
//! ## 为什么需要第二条后端（2026-09-20 真机实证，macOS 26.5.2 arm64）
//!
//! 旧路径（`tauri-plugin-notification` 2.3.3 → `notify-rust` 4.11 → `mac-notification-sys`
//! 0.6.15）在 macOS 上走的是**已弃用的 `NSUserNotificationCenter`**：投递是 fire-and-forget，
//! 系统不投递时它也返回 `Ok`。真机探针（与本模块同 bundle 形态）实测：macOS 26 上
//! `send_notification` 返回 `Ok(None)` 但**零横幅**；同机 `UNUserNotificationCenter`
//! 在 bundle **带 ad-hoc 签名**后能正常弹授权并投递（未签名时系统直接回
//! `UNErrorDomain error 1 = notifications not allowed`）。因此本后端与打包侧的 ad-hoc 签名
//! 是一对，选择逻辑在 [`super::strategy_for`]。
//!
//! ## 线程纪律
//!
//! `UNUserNotificationCenter` 的用法要求在主线程（或有 run loop 的线程）发起；授权回调由
//! 系统在**任意队列**上调用，因此回调里只做纯 Rust 处理，真正的 ObjC 对象构建与投递再
//! 跳回主线程（`AppHandle::run_on_main_thread`）完成。delegate 被通知中心**弱引用**持有，
//! 必须由本进程持有：放在主线程的 thread_local 里。
//!
//! ## delegate 的作用
//!
//! 应用在前台时系统默认**不弹横幅**（只进通知中心）。用户点「连接」时应用正在前台，所以
//! 必须实现 `userNotificationCenter:willPresentNotification:withCompletionHandler:` 并显式
//! 要求 `Banner | List | Sound`，否则连接通知照样「看不见」。

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::AllocAnyThread as _;
use objc2::runtime::{Bool, ProtocolObject};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationRequest,
    UNNotificationPresentationOptions, UNNotificationSettings, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use tauri::AppHandle;

use crate::notification::DeliveryReport;

objc2::define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ExvNotificationDelegate"]
    #[ivars = ()]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        unsafe fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // 诊断：系统是否真的把「前台呈现」决策交给本 delegate（未实现它时前台不弹横幅）。
            crate::notification::record_delivery("modern", None, "will-present", "");
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }
    }
);

impl NotificationDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: `NSObject -init` 无前置条件，且 this 由 alloc 产出。
        unsafe { objc2::msg_send![super(this), init] }
    }
}

thread_local! {
    /// 通知中心弱引用 delegate：必须由本进程（主线程）持有，否则 delegate 被释放后前台
    /// 横幅又会静默消失。
    static DELEGATE: RefCell<Option<Retained<NotificationDelegate>>> = const { RefCell::new(None) };
}

/// 通知标识符序号（唯一化，避免相邻两条互相替换）。
static NOTIFICATION_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 在主线程向系统**提交**一条通知；授权未定时系统会弹授权窗，授权完成后在同一回调链里投递。
///
/// 返回 `Ok(())` 只表示「已提交给系统」——真实结果（投递成功/授权被拒/投递失败）经
/// `report` 在系统回调队列上异步落盘（见 `super::record_report`）。主线程派发失败或超时
/// 返回 `Err`，调用方据此回退旧后端。
pub(super) fn submit(
    app: &AppHandle,
    title: String,
    body: String,
    report: impl Fn(DeliveryReport) + Send + Sync + 'static,
) -> Result<(), String> {
    let report: Arc<dyn Fn(DeliveryReport) + Send + Sync> = Arc::new(report);
    let (ready_sender, ready_receiver) = mpsc::sync_channel::<Result<(), String>>(1);
    let main_app = app.clone();
    app.run_on_main_thread(move || {
        let submitted = dispatch(&main_app, &title, &body, report);
        let _ = ready_sender.send(submitted);
    })
    .map_err(|error| format!("main-thread dispatch failed: {error}"))?;
    ready_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| Err("main-thread dispatch timed out".to_owned()))
}

/// 必须在主线程执行：安装 delegate 并请求授权（回调里投递）。
fn dispatch(
    app: &AppHandle,
    title: &str,
    body: &str,
    report: Arc<dyn Fn(DeliveryReport) + Send + Sync>,
) -> Result<(), String> {
    // SAFETY: 主线程调用；该 API 只返回当前进程的通知中心单例。
    let center = UNUserNotificationCenter::currentNotificationCenter();
    DELEGATE.with(|slot| {
        if slot.borrow().is_none() {
            let delegate = NotificationDelegate::new();
            let protocol: &ProtocolObject<dyn UNUserNotificationCenterDelegate> =
                ProtocolObject::from_ref(&*delegate);
            center.setDelegate(Some(protocol));
            *slot.borrow_mut() = Some(delegate);
        }
    });

    let title = title.to_owned();
    let body = body.to_owned();
    let delivery_app = app.clone();
    let authorization_report = Arc::clone(&report);
    // 授权回调可能在任意队列：只做纯 Rust 处理，ObjC 构建与投递跳回主线程。
    let authorization = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        if !granted.as_bool() {
            let detail = if error.is_null() {
                "authorization not granted".to_owned()
            } else {
                // SAFETY: 非空 error 由系统在本回调内保证有效。
                unsafe { (*error).localizedDescription() }.to_string()
            };
            authorization_report(DeliveryReport::Denied(detail));
            return;
        }
        let request_app = delivery_app.clone();
        let request_title = title.clone();
        let request_body = body.clone();
        let request_report = Arc::clone(&authorization_report);
        if delivery_app
            .run_on_main_thread(move || {
                deliver_now(&request_app, &request_title, &request_body, request_report);
            })
            .is_err()
        {
            // 主线程已不可用（应用正在退出）；系统侧会随进程退出丢弃本条。
        }
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &authorization,
    );
    Ok(())
}

/// 读取并把 `UNNotificationSettings` 的关键数值落盘（诊断「接受了却不显示」类问题）。
///
/// 关注点：`alertStyle` 为 `None` 时系统**不弹横幅**（只进通知中心），而 `add` 仍返回成功；
/// 授权状态、通知中心/声音开关同理。数值映射见 `UNNotificationSettings` 的枚举定义
/// （0 = notSupported/None，1 = 对应最低档，2 = 上一档）。
fn report_notification_settings(center: &UNUserNotificationCenter) {
    let block = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
        // SAFETY: 回调参数由系统保证在本调用内有效。
        let settings = unsafe { settings.as_ref() };
        // 逐项数值：alertSetting=0 表示「不允许提醒」→ 系统接受请求但不弹横幅；
        // alertStyle=0（None）同理；scheduledDelivery=1 表示进了「定时摘要」（延后显示）。
        let summary = format!(
            "auth={} alert={} alertStyle={} center={} sound={} badge={} lock={} previews={} scheduled={} timeSensitive={} announcement={}",
            settings.authorizationStatus().0,
            settings.alertSetting().0,
            settings.alertStyle().0,
            settings.notificationCenterSetting().0,
            settings.soundSetting().0,
            settings.badgeSetting().0,
            settings.lockScreenSetting().0,
            settings.showPreviewsSetting().0,
            settings.scheduledDeliverySetting().0,
            settings.timeSensitiveSetting().0,
            settings.announcementSetting().0,
        );
        crate::notification::record_delivery("modern", None, "settings", &summary);
    });
    center.getNotificationSettingsWithCompletionHandler(&block);
}

/// 主线程投递：构建 content/request 并交给系统；结果经 `report` 异步回报。
fn deliver_now(
    app: &AppHandle,
    title: &str,
    body: &str,
    report: Arc<dyn Fn(DeliveryReport) + Send + Sync>,
) {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    report_notification_settings(&center);
    let content = UNMutableNotificationContent::new();
    // 字符串在此构建（NSString 跨线程安全，内容不可变）。
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    // 每条通知用唯一标识符：同一 identifier 的请求会**替换**通知中心里的前一条，
    // 连接/断开紧邻发生时用户就只能看见最后一条。
    let sequence = NOTIFICATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let identifier = NSString::from_str(&format!(
        "exv-notify-{}-{sequence}",
        std::process::id()
    ));
    // trigger=None 表示立即投递。
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &identifier,
        &content,
        None,
    );
    let _ = app;
    let completion_report = Arc::clone(&report);
    let completion = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            completion_report(DeliveryReport::Delivered);
        } else {
            // SAFETY: 非空 error 由系统在本回调内保证有效。
            let detail = unsafe { (*error).localizedDescription() }.to_string();
            completion_report(DeliveryReport::Failed(detail));
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&completion));
}
