//! WKWebView 同步 JSON 脚本的主线程执行原语；不建立认证等待循环。

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSError, NSString};
use objc2_web_kit::WKWebView;
use tokio::sync::oneshot;
use zeroize::{Zeroize, Zeroizing};

/// future 完成或取消时撤销操作；排队中的主线程闭包和迟到回调共享该标记。
struct OperationActive(Arc<AtomicBool>);

impl Drop for OperationActive {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 页面创建／回跳时 WKWebView.URL 可以为空；Wry 0.54.4 的同步 url() 会 unwrap。
/// 直接在主线程读取可空原生属性，缺失只表示本次未观察到位置。
pub(super) async fn current_url(
    window: &tauri::WebviewWindow,
) -> Result<Option<Zeroizing<String>>, String> {
    let active = Arc::new(AtomicBool::new(true));
    let _operation = OperationActive(Arc::clone(&active));
    let (sender, receiver) = oneshot::channel();
    window
        .with_webview(move |platform| {
            if !active.load(Ordering::Acquire) {
                return;
            }
            let Some(_main_thread) = MainThreadMarker::new() else {
                let _ = sender.send(Err("WKWebView 地址读取未进入主线程".to_owned()));
                return;
            };
            // SAFETY: 仅在 Tauri 主线程闭包内借用当前 WKWebView，原生对象不跨线程传递。
            let result = unsafe {
                platform
                    .inner()
                    .cast::<WKWebView>()
                    .as_ref()
                    .ok_or_else(|| "WKWebView 原生句柄不可用".to_owned())
                    .map(|webview| {
                        webview
                            .URL()
                            .and_then(|url| url.absoluteString())
                            .map(|url| Zeroizing::new(url.to_string()))
                    })
            };
            let _ = sender.send(result);
        })
        .map_err(|_| "无法安排 WKWebView 主线程地址读取".to_owned())?;
    receiver
        .await
        .map_err(|_| "WKWebView 地址读取回调已结束".to_owned())?
}

/// 脚本须同步返回 `JSON.stringify(...)` 字符串，不能返回 Promise。
/// NSError 只保留域/错误码，不读取可能带页面或脚本内容的 userInfo/描述。
pub(super) async fn evaluate_json(
    window: &tauri::WebviewWindow,
    script: Zeroizing<String>,
) -> Result<String, String> {
    let active = Arc::new(AtomicBool::new(true));
    let _operation = OperationActive(Arc::clone(&active));
    let (sender, receiver) = oneshot::channel();
    window
        .with_webview(move |platform| {
            if !active.load(Ordering::Acquire) {
                return;
            }
            // Tauri 2.10.3 保证 with_webview 闭包在主线程；仍检查线程再解引用原生指针。
            let Some(_main_thread) = MainThreadMarker::new() else {
                let _ = sender.send(Err("WKWebView 脚本执行未进入主线程".to_owned()));
                return;
            };
            // SAFETY: Tauri PlatformWebview::inner 在 macOS 上明确返回当前 WKWebView；
            // 指针只在本主线程闭包内借用，不跨线程或跨越闭包保存。
            let Some(webview) = (unsafe { platform.inner().cast::<WKWebView>().as_ref() }) else {
                let _ = sender.send(Err("WKWebView 原生句柄不可用".to_owned()));
                return;
            };
            let sender = RefCell::new(Some(sender));
            let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
                if !active.load(Ordering::Acquire) {
                    return;
                }
                let Some(sender) = sender.borrow_mut().take() else {
                    return;
                };
                // SAFETY: WebKit 的完成回调按 API 提供有效的可空 Objective-C 对象；
                // 仅在回调期间借用，不将原生对象放进 Send 通道。
                let result = unsafe {
                    if let Some(error) = error.as_ref() {
                        Err(format!(
                            "WKWebView JavaScript 执行失败（NSError domain={} code={}）",
                            error.domain(),
                            error.code()
                        ))
                    } else if let Some(value) = value.as_ref() {
                        value.downcast_ref::<NSString>().map_or_else(
                            || Err("WKWebView 脚本未返回 JSON 字符串".to_owned()),
                            |value| Ok(value.to_string()),
                        )
                    } else {
                        Err("WKWebView 脚本未返回结果".to_owned())
                    }
                };
                // 接收者取消与回调并发发生时，丢弃结果前清除返回的页面材料。
                if let Err(Ok(mut value)) = sender.send(result) {
                    value.zeroize();
                }
            });
            let script = NSString::from_str(&script);
            // SAFETY: 已确认主线程，webview 为 Tauri 所有的 WKWebView；NSString 和
            // block 在调用期间有效，WebKit 按异步 API 语义复制完成 block。回调只传 Rust 值。
            unsafe {
                webview.evaluateJavaScript_completionHandler(&script, Some(&callback));
            }
        })
        .map_err(|_| "无法安排 WKWebView 主线程脚本执行".to_owned())?;
    receiver
        .await
        .map_err(|_| "WKWebView 脚本回调已结束".to_owned())?
}
