//! 自管托盘（Shell_NotifyIcon）+ 托盘气泡通知。
//!
//! 为什么不用 `tauri::tray`：气泡通知（NIF_INFO）需要自持 NOTIFYICONDATA 做
//! NIM_MODIFY，tauri tray API 不暴露原始图标数据；自管还保证全应用只有一个
//! 托盘图标（lifecycle.rs 不再注册 tauri tray）。
//!
//! 行为对齐 C++ 壳（`webview2_host_win32.cpp`）：
//!   * 左键单击 = 显示窗口；
//!   * 右键菜单 = 显示 EXV / 退出（退出走 O3 停机 [`crate::lifecycle::notify_core_shutdown`]）；
//!   * 气泡通知 = `show_tray_notification(title, body)` 同款 NIM_MODIFY + NIF_INFO。
//!
//! 窗口操作（显示主窗口）与退出动作经 [`set_show_handler`] / [`set_quit_handler`]
//| 注入（lib.rs setup 提供 tauri 侧实现），本模块只管 Win32 交互面。

use std::sync::OnceLock;

use tauri::Manager;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, HICON, ICON_BIG, ICON_SMALL,
    ICONINFO, MF_SEPARATOR, MF_STRING, MSG, PostQuitMessage, RegisterClassW, SendMessageW,
    SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP,
    WM_SETICON, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

/// 托盘回调消息基址（WM_APP 区段，不与框架消息冲突；对齐 C++ 壳 `WM_APP + 0x42` 惯例）。
const TRAY_CALLBACK_MSG: u32 = WM_APP + 0x51;
/// 右键菜单项 id。
const MENU_SHOW: usize = 2001;
const MENU_QUIT: usize = 2002;
/// 托盘图标 id（单实例固定 1）。
const TRAY_ID: u32 = 1;

/// HICON/HWND 是裸句柄（`*mut c_void`，非 Send/Sync）；托盘状态只在安装线程写入、
/// 消息泵线程读取其 HWND 字段做 Shell 调用——Windows 句柄跨线程使用是合法的，
/// 这里用包装类型声明该契约。
#[derive(Clone, Copy)]
struct TrayHandles(*mut core::ffi::c_void);
unsafe impl Send for TrayHandles {}
unsafe impl Sync for TrayHandles {}

struct TrayState {
    /// 托盘消息窗口句柄（气泡修改需要）。
    hwnd: TrayHandles,
    /// 托盘图标句柄（退出时释放）。
    icon: TrayHandles,
}

static TRAY: OnceLock<TrayState> = OnceLock::new();
static ON_QUIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_SHOW: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// 注册「退出」回调（O3 停机入口；setup 阶段调用一次）。
pub fn set_quit_handler(handler: impl Fn() + Send + Sync + 'static) {
    let _ = ON_QUIT.set(Box::new(handler));
}

/// 注册「显示主窗口」回调（lib.rs setup 注入 tauri window.show + set_focus）。
pub fn set_show_handler(handler: impl Fn() + Send + Sync + 'static) {
    let _ = ON_SHOW.set(Box::new(handler));
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 把 source 截断写入定长 UTF-16 缓冲并保证 NUL 结尾。
fn fill_utf16_buf(buf: &mut [u16], source: &str) {
    let mut written = 0usize;
    for ch in source.chars() {
        let mut encoded = [0u16; 2];
        for unit in ch.encode_utf16(&mut encoded) {
            if written >= buf.len() - 1 {
                break;
            }
            buf[written] = *unit;
            written += 1;
        }
    }
    // 清零剩余部分（NUL 结尾由清零保证）。
    for slot in &mut buf[written..] {
        *slot = 0;
    }
}

extern "system" fn tray_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == TRAY_CALLBACK_MSG {
            let mouse = (lparam.0 & 0xFFFF) as u32;
            if mouse == WM_RBUTTONUP {
                show_context_menu(hwnd);
            } else if mouse == WM_LBUTTONUP {
                invoke_show();
            }
            return LRESULT(0);
        }
        if msg == WM_COMMAND {
            match (wparam.0 & 0xFFFF) as usize {
                id if id == MENU_SHOW => invoke_show(),
                id if id == MENU_QUIT => {
                    if let Some(quit) = ON_QUIT.get() {
                        quit();
                    }
                }
                _ => {}
            }
            return LRESULT(0);
        }
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, MENU_SHOW, w!("显示 EXV"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("退出"));
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // TrackPopupMenu 需要前台窗口才能正确收到菜单关闭消息。
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN,
            cursor.x,
            cursor.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
    }
}

fn invoke_show() {
    if let Some(show) = ON_SHOW.get() {
        show();
    }
}

/// 托盘图标渲染（纯像素处理，可单测）：保留完整源画布，按原始纵横比居中缩放，
/// 再按 alpha 预乘做面积平均（area-average）缩放到 32×32，最后转为 BGRA
/// （`CreateBitmap` 32bpp 期望的格式）。
///
/// 为什么这样做：
///   * **完整画布**：canonical PNG 中的透明留白是品牌图形纵横比的一部分；
///     按 alpha bbox 裁剪后再拉满正方形会把窄于画布的图形横向拉伸；
///   * **面积平均 + alpha 预乘**：256²→32² 是 8:1 大倍数缩小。面积平均等价于
///     高质量超采样，边缘像素按实际覆盖面积加权，抗锯齿自然保留；旧实现用
///     最近邻稀疏采样，会丢抗锯齿、白色细线（网络线/盾牌描边）断裂成噪点。
///     预乘避免半透明边缘出现黑/白描边（halo）。
///
/// 返回 None 表示源为空或全透明。
fn render_tray_bgra(rgba: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    render_icon_bgra(rgba, width, height, 32)
}

/// 把完整 RGBA 画布等比居中缩放为指定的方形 top-down BGRA。
/// `render_tray_bgra` 和运行时窗口大/小 HICON 共用这一像素真源。
fn render_icon_bgra(rgba: &[u8], width: usize, height: usize, size: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || size == 0 || rgba.len() < width * height * 4 {
        return None;
    }

    if !rgba.chunks_exact(4).any(|pixel| pixel[3] != 0) {
        return None; // 全透明
    }

    // 完整源画布按纵横比 fit 到 size×size，奇数留白时将多的 1px 放在右/下侧。
    let scale = (size as f64 / width as f64).min(size as f64 / height as f64);
    let target_width = ((width as f64 * scale).round() as usize).clamp(1, size);
    let target_height = ((height as f64 * scale).round() as usize).clamp(1, size);
    let offset_x = (size - target_width) / 2;
    let offset_y = (size - target_height) / 2;

    // 在 fit 矩形内做 alpha 预乘面积平均，矩形外保持透明。
    let mut top_down = vec![0u8; size * size * 4];
    for target_y in 0..target_height {
        let fy0 = target_y as f64 * height as f64 / target_height as f64;
        let fy1 = (target_y as f64 + 1.0) * height as f64 / target_height as f64;
        for target_x in 0..target_width {
            let fx0 = target_x as f64 * width as f64 / target_width as f64;
            let fx1 = (target_x as f64 + 1.0) * width as f64 / target_width as f64;
            let sx0 = fx0.floor() as usize;
            let sx1 = (fx1.ceil() as usize).min(width);
            let sy0 = fy0.floor() as usize;
            let sy1 = (fy1.ceil() as usize).min(height);
            let (mut pr, mut pg, mut pb, mut pa, mut area) =
                (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for sy in sy0..sy1 {
                let wy = ((sy as f64 + 1.0).min(fy1) - (sy as f64).max(fy0)).max(0.0);
                for sx in sx0..sx1 {
                    let wx = ((sx as f64 + 1.0).min(fx1) - (sx as f64).max(fx0)).max(0.0);
                    let w = wx * wy;
                    let i = (sy * width + sx) * 4;
                    let a = rgba[i + 3] as f64 / 255.0;
                    pr += rgba[i] as f64 * a * w;
                    pg += rgba[i + 1] as f64 * a * w;
                    pb += rgba[i + 2] as f64 * a * w;
                    pa += a * w;
                    area += w;
                }
            }
            let di = ((offset_y + target_y) * size + offset_x + target_x) * 4;
            if pa > 0.0 && area > 0.0 {
                let alpha = pa / area;
                top_down[di + 3] = (alpha * 255.0).round() as u8;
                top_down[di] = (pr / pa).round() as u8;
                top_down[di + 1] = (pg / pa).round() as u8;
                top_down[di + 2] = (pb / pa).round() as u8;
            }
        }
    }

    // 3) `CreateBitmap` 按传入的 top-down 扫描线解释颜色位图；只转换
    // RGBA→BGRA，绝不能额外翻转行序，否则托盘图标会垂直镜像。
    let mut bgra = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let si = (y * size + x) * 4;
            let di = (y * size + x) * 4;
            bgra[di] = top_down[si + 2];
            bgra[di + 1] = top_down[si + 1];
            bgra[di + 2] = top_down[si];
            bgra[di + 3] = top_down[si + 3];
        }
    }
    Some(bgra)
}

/// 返回一个全 0 的 1bpp AND 掩码。`CreateIconIndirect` 将该掩码按位解释为透明
/// 判据，因此绝不能把未初始化的 GDI bitmap 交给它：全 0 表示由 32bpp 色彩图（含
/// alpha）决定可见像素，避免 Shell 把整个图标当成透明。
fn opaque_and_mask(size: usize) -> Vec<u8> {
    let bytes_per_row = size.div_ceil(32) * 4;
    vec![0; bytes_per_row * size]
}

/// 把 RGBA 像素（tauri Image）转成 32×32 32bpp HICON（托盘图标）。
/// 像素处理见 [`render_tray_bgra`]（完整画布 + 等比居中 + alpha 预乘面积平均）。
fn hicon_from_rgba(image: &tauri::image::Image<'_>) -> Option<HICON> {
    let bgra = render_tray_bgra(
        image.rgba(),
        image.width() as usize,
        image.height() as usize,
    )?;
    hicon_from_bgra(&bgra, 32)
}

/// 从 canonical RGBA 画布生成指定方形尺寸的 32bpp HICON。
fn hicon_from_rgba_sized(image: &tauri::image::Image<'_>, size: i32) -> Option<HICON> {
    let size_usize = usize::try_from(size).ok().filter(|value| *value > 0)?;
    let bgra = render_icon_bgra(
        image.rgba(),
        image.width() as usize,
        image.height() as usize,
        size_usize,
    )?;
    hicon_from_bgra(&bgra, size)
}

fn hicon_from_bgra(bgra: &[u8], size: i32) -> Option<HICON> {
    let size_usize = usize::try_from(size).ok().filter(|value| *value > 0)?;
    if bgra.len() < size_usize * size_usize * 4 {
        return None;
    }
    let and_mask_pixels = opaque_and_mask(size_usize);

    unsafe {
        // `CreateBitmap(..., None)` 留下未初始化的 AND 掩码；Shell 会把其中的 1
        // 当透明像素，表现为偶发或全透明托盘图标。明确传全 0 让 32bpp 图标的 alpha
        // 成为唯一透明度来源。
        let and_mask = CreateBitmap(size, size, 1, 1, Some(and_mask_pixels.as_ptr().cast()));
        if and_mask.is_invalid() {
            return None;
        }
        let color = CreateBitmap(size, size, 1, 32, Some(bgra.as_ptr().cast()));
        if color.is_invalid() {
            let _ = DeleteObject(and_mask.into());
            return None;
        }
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: and_mask,
            hbmColor: color,
        };
        let handle = CreateIconIndirect(&info);
        let _ = DeleteObject(and_mask.into());
        let _ = DeleteObject(color.into());
        match handle {
            Ok(h) if !h.is_invalid() => Some(h),
            _ => None,
        }
    }
}

/// 同时设置 Win32 窗口的任务栏大图标和标题栏小图标。
///
/// Tauri/tao 的普通 `set_window_icon` 在 Windows 上只发送 `ICON_SMALL`；若
/// `ICON_BIG` 为空，Shell 可能回退到 16px 小图标并放大成任务栏图标。
fn set_window_icons(hwnd: HWND, big: HICON, small: HICON) {
    unsafe {
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(big.0 as isize)),
        );
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(small.0 as isize)),
        );
    }
}

struct WindowIconState {
    // 窗口持有引用期间必须保持 HICON 有效，随进程一并回收。
    big: TrayHandles,
    small: TrayHandles,
}

static WINDOW_ICONS: OnceLock<WindowIconState> = OnceLock::new();

/// 在主窗口可见前安装独立的 256px `ICON_BIG` 与 32px `ICON_SMALL`。
///
/// # Errors
/// 主窗口或 HWND 不可用、canonical PNG 无法解码/转换，或本进程已安装过窗口图标。
pub fn install_window_icons<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    if WINDOW_ICONS.get().is_some() {
        return Err("window icons already installed".into());
    }
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_owned())?;
    let tauri_hwnd = window.hwnd().map_err(|error| error.to_string())?;
    // Tauri 运行时返回 windows 0.61 HWND，本应用使用 windows 0.62；ABI 相同，
    // 显式经原始指针转接，不混用两个 crate 版本的 Rust 类型。
    let hwnd = HWND(tauri_hwnd.0);
    let image = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
        .map_err(|error| format!("canonical icon.png decode failed: {error}"))?;
    let big = hicon_from_rgba_sized(&image, 256)
        .ok_or_else(|| "256px taskbar HICON creation failed".to_owned())?;
    let small = match hicon_from_rgba_sized(&image, 32) {
        Some(icon) => icon,
        None => {
            unsafe {
                let _ = DestroyIcon(big);
            }
            return Err("32px window HICON creation failed".into());
        }
    };
    if WINDOW_ICONS
        .set(WindowIconState {
            big: TrayHandles(big.0),
            small: TrayHandles(small.0),
        })
        .is_err()
    {
        unsafe {
            let _ = DestroyIcon(big);
            let _ = DestroyIcon(small);
        }
        return Err("window icons already installed".to_owned());
    }
    let installed = WINDOW_ICONS.get().expect("window icons just installed");
    set_window_icons(hwnd, HICON(installed.big.0), HICON(installed.small.0));
    Ok(())
}

/// 安装自管托盘。失败返回 Err（调用方记录并降级为无托盘，行为同旧 tauri tray 缺图标分支）。
///
/// # Errors
/// 窗口类注册 / 消息窗口创建 / 图标渲染 / NIM_ADD 失败，或托盘已安装。
pub fn install_tray() -> Result<(), String> {
    static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

    unsafe {
        let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;

        let class_name = wide("EXV Tray Window");
        if CLASS_REGISTERED.get().is_none() {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(tray_proc),
                hInstance: module.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err("RegisterClassW failed".into());
            }
            let _ = CLASS_REGISTERED.set(());
        }

        let window_name = wide("EXV Tray");
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(window_name.as_ptr()),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW failed: {e}"))?;

        // 图标：显式用 app/icons/icon.png（256² 品牌红盾牌 logo）。不走
        // ExtractIconExW(exe, ...)——exe 内嵌 icon.ico 的默认帧在 16px 下会把
        // 白色盾牌糊成白块/白边（浅色任务栏白对白近不可见 = 「空/透明」），是
        // 托盘 icon 未就位的根因之一。这里保留完整 canonical 画布，等比居中后按
        // alpha 预乘面积平均缩放到 32×32；透明留白不裁剪，Windows 再下采样到托盘
        // 显示尺寸时仍保留正确的品牌比例与边缘质量。
        let embedded = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
            .ok()
            .and_then(|img| hicon_from_rgba(&img));
        let icon = match embedded {
            Some(handle) => handle,
            None => {
                return Err(
                    "no icon available for tray (embedded icon.png decode/render failed)".into(),
                );
            }
        };

        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON,
            uCallbackMessage: TRAY_CALLBACK_MSG,
            hIcon: icon,
            ..Default::default()
        };
        if !Shell_NotifyIconW(NIM_ADD, &mut nid).as_bool() {
            let _ = DestroyIcon(icon);
            return Err("Shell_NotifyIconW(NIM_ADD) failed".into());
        }

        if TRAY
            .set(TrayState {
                hwnd: TrayHandles(hwnd.0),
                icon: TrayHandles(icon.0),
            })
            .is_err()
        {
            return Err("tray already installed".into());
        }
    }

    // 消息泵线程：托盘交互事件在此分发；进程退出随线程终止（daemon 性质，不 join）。
    // HWND 跨线程传递经包装类型声明契约（见 TrayHandles 注释）。
    let pump_hwnd = TrayHandles(TRAY.get().map(|s| s.hwnd.0).unwrap_or(std::ptr::null_mut()));
    std::thread::Builder::new()
        .name("exv-tray".into())
        .spawn(move || unsafe {
            // 整体 move TrayHandles（Send 包装），块内再取裸句柄——
            // 直接写 `pump_hwnd.0` 会让闭包按字段精确捕获裸指针（非 Send）。
            let pump = pump_hwnd;
            let hwnd = HWND(pump.0);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, Some(hwnd), 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .map_err(|e| format!("tray thread spawn failed: {e}"))?;

    Ok(())
}

/// 卸载托盘（移除图标并释放句柄；进程退出前调用）。
pub fn remove_tray() {
    if let Some(state) = TRAY.get() {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: HWND(state.hwnd.0),
            uID: TRAY_ID,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid);
            let _ = DestroyIcon(HICON(state.icon.0));
        }
    }
}

/// 连接状态通知：优先系统 toast 弹窗（Windows 10/11 通知中心可见、有横幅），
/// 失败回退托盘气泡（NIF_INFO）。`tray_notify` Command 调用此函数。
pub fn notify(title: &str, body: &str) {
    if crate::toast::show_toast(title, body) {
        return;
    }
    tray_balloon(title, body);
}

/// 托盘气泡通知（NIM_MODIFY + NIF_INFO）。未安装托盘时静默丢弃。
fn tray_balloon(title: &str, body: &str) {
    let Some(state) = TRAY.get() else { return };
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(state.hwnd.0),
        uID: TRAY_ID,
        uFlags: NIF_INFO,
        ..Default::default()
    };
    fill_utf16_buf(&mut nid.szInfoTitle, title);
    fill_utf16_buf(&mut nid.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &mut nid);
    }
}

/// `tray_notify` Command：前端触发的托盘气泡（通断通知通道）。
#[tauri::command]
pub fn tray_notify(title: String, body: String) {
    notify(&title, &body);
}
