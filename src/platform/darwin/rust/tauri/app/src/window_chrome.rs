//! Darwin 窗口壳（win32 `window_chrome.rs` 的 darwin 适配）。
//!
//! 窗口配置 `titleBarStyle: Overlay`：系统标题栏带消失，网页侧 `product-titlebar`
//! （34px）即**真标题栏**；系统红绿灯三键浮于其左侧（系统层，网页之上），win32 的
//! 自绘 `NativeWindowControls` 在 darwin 无对应物。本模块负责：
//!   * **原生拖动条**（win32 `WM_NCHITTEST`→`HTCAPTION`「拖动区交还系统」的 darwin 对应
//!     物）：标题栏中段盖一块透明 `NSView`（`TitlebarDragStripView`），`mouseDown:`
//!     直接进入系统 `performDrag:` 移动循环——零网页/IPC 介入，与原生标题栏同响应；
//!     左侧让位红绿灯三键，右侧让位网页控件区（win32 `FrameMetrics` 同款几何预约）；
//!   * 模式尺寸切换（advanced ↔ minimal 的逻辑尺寸与最小尺寸约束）；
//!   * `window_chrome_control`（minimize/maximize/close=hide——供极简视图/快捷路径
//!     调用；原生三键的关闭仍走 lifecycle 的 `close_preference` 决策）；
//!   * `window_set_visible`（纯显隐原语：连接后最小化到托盘 / 托盘唤出）。
//!
//! 不触碰 decorations（保持原生红绿灯与圆角窗口），不新增 tauri-plugin，全部经
//! tauri window API（Rust 侧调用，不经 JS 权限面）。

use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, ClassType};
use objc2_app_kit::{NSEvent, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use tauri::{LogicalSize, Manager, Runtime, State, WebviewWindow};

const ADVANCED_MIN_WIDTH: f64 = 860.0;
const ADVANCED_MIN_HEIGHT: f64 = 600.0;
const ADVANCED_DEFAULT_WIDTH: f64 = 1180.0;
const ADVANCED_DEFAULT_HEIGHT: f64 = 760.0;
const MINIMAL_WIDTH: f64 = 328.0;
const MINIMAL_HEIGHT: f64 = 136.0;

/// 网页标题栏高度（CSS `--titlebar-height: 34px` 的壳侧镜像；win32
/// `TITLEBAR_LOGICAL_PX` 同值）。
const TITLEBAR_PX: f64 = 34.0;
/// 左侧系统红绿灯三键预留（按钮簇约 70px + 间距余量；AppKit 逻辑像素）。
const TRAFFIC_LIGHTS_PX: f64 = 78.0;
/// 完整模式右侧网页控件区预留：主题三联 + 模式二联合计约 280px，保留余量避免
/// DPI 缩放或字体回退时把网页控件盖进拖动条（win32 `WEB_CONTROL_LOGICAL_PX`
/// 同值同理——两平台是同一组控件）。
const ADVANCED_WEB_CONTROL_PX: f64 = 288.0;
/// 极简模式 icon-only 两控件约 138px + 右缘 8px 距离 + 余量。
const MINIMAL_WEB_CONTROL_PX: f64 = 168.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowMode {
    Advanced,
    Minimal,
}

impl WindowMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "advanced" => Ok(Self::Advanced),
            "minimal" => Ok(Self::Minimal),
            other => Err(format!("不支持的窗口模式：{other}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeWindowControl {
    Minimize,
    Maximize,
    Close,
}

impl NativeWindowControl {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "minimize" => Ok(Self::Minimize),
            "maximize" => Ok(Self::Maximize),
            "close" => Ok(Self::Close),
            other => Err(format!("不支持的窗口操作：{other}")),
        }
    }
}

/// 标题栏拖动条几何（AppKit 左下原点坐标系，逻辑像素 = 网页 CSS px）。
///
/// 纯几何部分不依赖真实 NSWindow（win32 `hit_test` 同款设计），便于宿主外复核。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DragStripFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 计算标题栏中段拖动条的 frame：左侧让位红绿灯三键、右侧让位网页控件区、
/// 垂直贴窗口顶缘（y = 高度 - 标题栏高）。窗口过窄放不下时返回 `None`（拖动条
/// 隐藏，拖动退回红绿灯之外的系统标题栏边缘行为）。
pub(crate) fn drag_strip_frame(
    window_width: f64,
    window_height: f64,
    mode: WindowMode,
) -> Option<DragStripFrame> {
    let web_control = match mode {
        WindowMode::Advanced => ADVANCED_WEB_CONTROL_PX,
        WindowMode::Minimal => MINIMAL_WEB_CONTROL_PX,
    };
    let width = window_width - TRAFFIC_LIGHTS_PX - web_control;
    if width <= 0.0 {
        return None;
    }
    Some(DragStripFrame {
        x: TRAFFIC_LIGHTS_PX,
        y: window_height - TITLEBAR_PX,
        width,
        height: TITLEBAR_PX,
    })
}

// 标题栏原生拖动条：透明 NSView，`mouseDown:` 直接进入系统窗口拖动移动循环
// （原生标题栏同款路径，等同 win32 HTCAPTION 交还系统——事件不经过
// WebView/IPC）。`acceptsFirstMouse:` 让未聚焦窗口的首次点击即可拖动。
//
// 拖动实现用 AppKit 公开 API `-[NSWindow performWindowDragWithEvent:]`
//（10.11+，objc2-app-kit 已生成类型化方法）。**不用**裸 `msg_send!`
// `performDrag:`：实测裸选择器在 tao `sendEvent` 的不可 unwind 上下文里
// panic→abort（点击拖动区 100% 闪退，崩溃栈在
// `tao::platform_impl::platform::window::send_event` 内）。
define_class!(
    #[unsafe(super(NSView))]
    pub(crate) struct TitlebarDragStripView;

    impl TitlebarDragStripView {
        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: &NSEvent) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let Some(window) = self.as_super().window() else {
                return;
            };
            window.performWindowDragWithEvent(event);
        }
    }
);

/// 主线程专属 `Retained<NSView>` 的 Send 包装：视图仅在主线程创建与触碰
/// （`install` 于 setup 主线程；几何写入只把裸指针值派发到主线程借用），包装
/// 只为满足 managed State 与锁的线程安全约束。
struct MainThreadStripView(Retained<TitlebarDragStripView>);

unsafe impl Send for MainThreadStripView {}

/// 壳侧窗口模式状态（advanced 期尺寸记忆 + 最大化记忆；win32 同款）。
pub(crate) struct WindowChromeState {
    mode: Mutex<WindowMode>,
    advanced_size: Mutex<LogicalSize<f64>>,
    advanced_was_maximized: Mutex<bool>,
    /// 标题栏原生拖动条（`install` 注入；测试态为 None 时同步为 no-op）。
    strip: Mutex<Option<MainThreadStripView>>,
}

impl WindowChromeState {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            mode: Mutex::new(WindowMode::Advanced),
            advanced_size: Mutex::new(LogicalSize::new(
                ADVANCED_DEFAULT_WIDTH,
                ADVANCED_DEFAULT_HEIGHT,
            )),
            advanced_was_maximized: Mutex::new(false),
            strip: Mutex::new(None),
        }
    }

    /// 把拖动条几何同步到当前窗口尺寸与模式（Resized 事件与模式切换后调用；
    /// `AppKit` 调用全部经 `run_on_main_thread`，本函数可在任意线程）。
    fn sync_strip<R: Runtime>(&self, window: &WebviewWindow<R>) -> Result<(), String> {
        let guard = self.strip.lock().map_err(|_| "拖动条状态锁已损坏")?;
        // 状态持有的 Retained 保证视图存活整个应用生命周期；闭包只携带裸指针值
        // （usize 天然 Send），主线程内仅借用——绕开 RFC 2229 精确捕获把
        // `Retained<NSView>` 本体拽进捕获集的 Send 问题。
        let Some(strip) = guard.as_ref() else {
            return Ok(());
        };
        let view_ptr = std::ptr::from_ref::<NSView>(strip.0.as_super()) as usize;
        drop(guard);
        let factor = window
            .scale_factor()
            .map_err(|error| format!("读取窗口缩放比例失败：{error}"))?;
        let physical = window
            .inner_size()
            .map_err(|error| format!("读取窗口尺寸失败：{error}"))?;
        let mode = *self.mode.lock().map_err(|_| "窗口状态锁已损坏")?;
        let frame = drag_strip_frame(
            f64::from(physical.width) / factor,
            f64::from(physical.height) / factor,
            mode,
        );
        window
            .run_on_main_thread(move || {
                // SAFETY: 指针来自 state 长期持有的视图（同对象强引用计数 ≥1），
                // run_on_main_thread 保证在主线程触碰（AppKit 要求）。
                let view = unsafe { &*(view_ptr as *const NSView) };
                match frame {
                    Some(frame) => {
                        view.setHidden(false);
                        view.setFrame(NSRect::new(
                            NSPoint::new(frame.x, frame.y),
                            NSSize::new(frame.width, frame.height),
                        ));
                    }
                    None => view.setHidden(true),
                }
            })
            .map_err(|error| format!("派发拖动条几何失败：{error}"))
    }

    /// 应用模式切换的尺寸约束（win32 `apply_mode` 的 darwin 版：不动 decorations）。
    fn apply_mode<R: Runtime>(
        &self,
        window: &WebviewWindow<R>,
        requested: WindowMode,
    ) -> Result<(), String> {
        let current = *self.mode.lock().map_err(|_| "窗口状态锁已损坏")?;
        if current == requested {
            return Ok(());
        }

        match (current, requested) {
            (WindowMode::Advanced, WindowMode::Minimal) => {
                let factor = window
                    .scale_factor()
                    .map_err(|error| format!("读取窗口缩放比例失败：{error}"))?;
                let physical = window
                    .inner_size()
                    .map_err(|error| format!("读取窗口尺寸失败：{error}"))?;
                *self
                    .advanced_size
                    .lock()
                    .map_err(|_| "窗口尺寸锁已损坏")? = LogicalSize::new(
                    f64::from(physical.width) / factor,
                    f64::from(physical.height) / factor,
                );
                *self
                    .advanced_was_maximized
                    .lock()
                    .map_err(|_| "窗口最大化状态锁已损坏")? = window
                    .is_maximized()
                    .map_err(|error| format!("读取窗口最大化状态失败：{error}"))?;
                if *self
                    .advanced_was_maximized
                    .lock()
                    .map_err(|_| "窗口最大化状态锁已损坏")?
                {
                    window
                        .unmaximize()
                        .map_err(|error| format!("退出最大化失败：{error}"))?;
                }
                window
                    .set_resizable(false)
                    .map_err(|error| format!("锁定极简窗口尺寸失败：{error}"))?;
                let minimal = LogicalSize::new(MINIMAL_WIDTH, MINIMAL_HEIGHT);
                window
                    .set_min_size(Some(minimal))
                    .map_err(|error| format!("设置极简最小尺寸失败：{error}"))?;
                window
                    .set_max_size(Some(minimal))
                    .map_err(|error| format!("设置极简最大尺寸失败：{error}"))?;
                window
                    .set_size(minimal)
                    .map_err(|error| format!("设置极简窗口尺寸失败：{error}"))?;
            }
            (WindowMode::Minimal, WindowMode::Advanced) => {
                window
                    .set_resizable(true)
                    .map_err(|error| format!("恢复完整窗口缩放失败：{error}"))?;
                window
                    .set_max_size::<LogicalSize<f64>>(None)
                    .map_err(|error| format!("移除完整窗口最大尺寸失败：{error}"))?;
                window
                    .set_min_size(Some(LogicalSize::new(
                        ADVANCED_MIN_WIDTH,
                        ADVANCED_MIN_HEIGHT,
                    )))
                    .map_err(|error| format!("恢复完整窗口最小尺寸失败：{error}"))?;
                let size = *self
                    .advanced_size
                    .lock()
                    .map_err(|_| "窗口尺寸锁已损坏")?;
                window
                    .set_size(size)
                    .map_err(|error| format!("恢复完整窗口尺寸失败：{error}"))?;
                if *self
                    .advanced_was_maximized
                    .lock()
                    .map_err(|_| "窗口最大化状态锁已损坏")?
                {
                    window
                        .maximize()
                        .map_err(|error| format!("恢复窗口最大化失败：{error}"))?;
                }
            }
            _ => unreachable!("窗口模式只允许在完整和极简之间切换"),
        }

        *self.mode.lock().map_err(|_| "窗口状态锁已损坏")? = requested;
        // 模式切换改变了右侧网页控件区预约 → 立即重算拖动条几何
        //（随后的 Resized 事件还会再同步一次，这里保证确定性时序）。
        self.sync_strip(window)?;
        Ok(())
    }
}

impl Default for WindowChromeState {
    fn default() -> Self {
        Self::new()
    }
}

/// 窗口模式切换（advanced 1180×760/min 860×600 ↔ minimal 328×136；不动装饰）。
#[tauri::command]
pub(crate) async fn window_chrome_set_mode<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, WindowChromeState>,
    mode: String,
) -> Result<(), String> {
    state.apply_mode(&window, WindowMode::parse(&mode)?)
}

/// 窗口动作（win32 自绘三键的 Rust 通道；darwin 无自绘三键，供程序化路径调用）。
///
/// `close` 保持 win32 语义 = 隐藏到托盘（不携带关闭决策；原生红绿灯的关闭仍由
/// lifecycle 的 close_preference 处理）。
#[tauri::command]
pub(crate) async fn window_chrome_control<R: Runtime>(
    window: WebviewWindow<R>,
    control: String,
) -> Result<(), String> {
    match NativeWindowControl::parse(&control)? {
        NativeWindowControl::Minimize => window
            .minimize()
            .map_err(|error| format!("窗口最小化失败：{error}")),
        NativeWindowControl::Maximize => {
            let maximized = window
                .is_maximized()
                .map_err(|error| format!("读取窗口最大化状态失败：{error}"))?;
            if maximized {
                window
                    .unmaximize()
                    .map_err(|error| format!("窗口还原失败：{error}"))
            } else {
                window
                    .maximize()
                    .map_err(|error| format!("窗口最大化失败：{error}"))
            }
        }
        NativeWindowControl::Close => window
            .hide()
            .map_err(|error| format!("窗口隐藏失败：{error}")),
    }
}

/// 窗口显隐（前端 lifecycle-effects 的 hide-window 效果链终点：连接后最小化到
/// 托盘 / 静默启动后托盘唤出）。
///
/// 与 `window_chrome_control(Close)` 的区别：这是纯显隐原语，不携带关闭语义——
/// close_preference 的决策在调用方（Rust setup/lifecycle）完成，前端只表达目标状态。
/// 旧 `shell_hide_main_window` 命令保留（同一 hide 执行点；win32 前端经
/// `window_set_visible` 走本命令）。
#[tauri::command]
pub(crate) async fn window_set_visible<R: Runtime>(
    window: WebviewWindow<R>,
    visible: bool,
) -> Result<(), String> {
    if visible {
        window
            .show()
            .map_err(|error| format!("窗口显示失败：{error}"))?;
        window
            .set_focus()
            .map_err(|error| format!("窗口聚焦失败：{error}"))
    } else {
        window
            .hide()
            .map_err(|error| format!("窗口隐藏失败：{error}"))
    }
}

/// 在主窗口上安装标题栏原生拖动条（main.rs `.setup` 挂载；必须在主线程）。
///
/// 拖动条加在窗口 contentView（wry 的 WryWebViewParent，含 WKWebView）之上、
/// 系统标题栏容器（红绿灯三键）之下：盖住网页标题栏中段，点击直达系统
/// `performDrag:`；红绿灯与右侧网页控件不受影响。
pub(crate) fn install<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("main") else {
        return Err(tauri::Error::WindowNotFound);
    };
    let state = app.state::<WindowChromeState>();
    // install 触碰 AppKit（alloc/addSubview），必须断言在主线程。
    if MainThreadMarker::new().is_none() {
        return Err(tauri::Error::Io(std::io::Error::other(
            "标题栏拖动条必须在主线程安装",
        )));
    }
    let ns_window_ptr = window.ns_window()?;
    let ns_window: Retained<objc2_app_kit::NSWindow> =
        unsafe { Retained::retain(ns_window_ptr.cast()) }.ok_or_else(|| {
            tauri::Error::Io(std::io::Error::other("主窗口 NSWindow 句柄无效"))
        })?;
    let content_view = ns_window.contentView().ok_or_else(|| {
        tauri::Error::Io(std::io::Error::other("主窗口缺少 contentView"))
    })?;
    // 初始零尺寸；真实几何随后的 sync_strip 统一写入（复用同一几何真源）。
    // `new` = alloc+init（NSView init 零 frame；本类无自定义 ivars，无需
    // set_ivars）。
    let strip: Retained<TitlebarDragStripView> =
        unsafe { msg_send![TitlebarDragStripView::class(), new] };
    content_view.addSubview(strip.as_super());
    *state
        .strip
        .lock()
        .map_err(|_| tauri::Error::Io(std::io::Error::other("拖动条状态锁已损坏")))? =
        Some(MainThreadStripView(strip));
    state
        .sync_strip(&window)
        .map_err(|error| tauri::Error::Io(std::io::Error::other(error)))?;
    Ok(())
}

/// `on_window_event` 的拖动条几何同步接线（main.rs Builder 挂载）：窗口尺寸
/// 变化（缩放/zoom/全屏/极简切换）后重算拖动条 frame。
#[allow(clippy::print_stderr)] // debug 壳 stderr 是唯一诊断通道
pub(crate) fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::Resized(_) = event
        && window.label() == "main"
        && let Some(webview) = window.app_handle().get_webview_window("main")
    {
        // 窗口创建期的首个 Resized 可能先于 `.manage`/`install` 生效；此时既无
        // state 也无拖动条，直接跳过（install 后首次 sync 由 setup 显式完成）。
        // 用 try_state：`.state::<>()` 在未托管时 panic，而窗口事件回调运行在
        // tao 的 Objective-C 上下文（send_event 不可 unwind）——panic 会直接
        // abort 整个应用（实测启动即崩）。
        let Some(state) = window.app_handle().try_state::<WindowChromeState>() else {
            return;
        };
        if let Err(error) = state.sync_strip(&webview) {
            eprintln!("[window-chrome] 同步标题栏拖动条几何失败：{error}");
        }
    }
}

// ---- 纯函数/状态测试（模式解析 + 状态机初始值）----
