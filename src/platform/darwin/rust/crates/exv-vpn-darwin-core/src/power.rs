//! macOS 原生睡眠／唤醒通知。只观察，不阻止或延迟用户休眠。
//! 注册及清理顺序遵循 Apple QA1340；回调只采集事实，网络恢复交给异步 worker。

use std::ffi::c_void;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerEvent {
    WillSleep,
    DidWake,
}

pub(crate) struct PowerMonitor {
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for PowerMonitor {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type Port = *mut c_void;
type Callback = unsafe extern "C" fn(*mut c_void, u32, u32, *mut c_void);

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegisterForSystemPower(
        context: *mut c_void,
        port: *mut Port,
        callback: Callback,
        notifier: *mut u32,
    ) -> u32;
    fn IOAllowPowerChange(connection: u32, notification: isize) -> i32;
    fn IODeregisterForSystemPower(notifier: *mut u32) -> i32;
    fn IOServiceClose(connection: u32) -> i32;
    fn IONotificationPortGetRunLoopSource(port: Port) -> Port;
    fn IONotificationPortDestroy(port: Port);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: Port;
    fn CFRunLoopGetCurrent() -> Port;
    fn CFRunLoopAddSource(run_loop: Port, source: Port, mode: Port);
    fn CFRunLoopRemoveSource(run_loop: Port, source: Port, mode: Port);
    fn CFRunLoopRunInMode(mode: Port, seconds: f64, return_after_source: u8) -> i32;
}

const CAN_SLEEP: u32 = 0xe000_0270;
const WILL_SLEEP: u32 = 0xe000_0280;
const DID_WAKE: u32 = 0xe000_0300;

struct Context {
    connection: u32,
    callback: Box<dyn FnMut(PowerEvent) + Send>,
}

unsafe extern "C" fn callback(context: *mut c_void, _: u32, message: u32, argument: *mut c_void) {
    // 指针由监听线程的 Box 提供，直到注销和停止 run loop 后才释放；回调仅在该线程执行。
    let context = unsafe { &mut *context.cast::<Context>() };
    if message == WILL_SLEEP {
        (context.callback)(PowerEvent::WillSleep);
    } else if message == DID_WAKE {
        (context.callback)(PowerEvent::DidWake);
    }
    if message == CAN_SLEEP || message == WILL_SLEEP {
        // 必须确认这两种通知，否则系统会等待 30 秒；不调用取消休眠 API。
        unsafe {
            IOAllowPowerChange(context.connection, argument as isize);
        }
    }
}

pub(crate) fn observe(
    handler: impl FnMut(PowerEvent) + Send + 'static,
) -> Result<PowerMonitor, String> {
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopped);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("exv-power".into())
        .spawn(move || {
            let mut context = Box::new(Context {
                connection: 0,
                callback: Box::new(handler),
            });
            let mut port = std::ptr::null_mut();
            let mut notifier = 0;
            // 所有 IOKit/CF 对象都由此线程注册、使用并按逆序释放。
            unsafe {
                context.connection = IORegisterForSystemPower(
                    (&raw mut *context).cast(),
                    &raw mut port,
                    callback,
                    &raw mut notifier,
                );
                if context.connection == 0 {
                    let _ = ready_tx.send(false);
                    return;
                }
                let source = IONotificationPortGetRunLoopSource(port);
                let run_loop = CFRunLoopGetCurrent();
                CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
                let _ = ready_tx.send(true);
                while !stop.load(Ordering::Acquire) {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, 1);
                }
                CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
                IODeregisterForSystemPower(&raw mut notifier);
                IOServiceClose(context.connection);
                IONotificationPortDestroy(port);
            }
        })
        .map_err(|error| error.to_string())?;
    let monitor = PowerMonitor {
        stopped,
        thread: Some(thread),
    };
    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(true) => Ok(monitor),
        _ => Err("无法注册 macOS 睡眠与唤醒通知".into()),
    }
}

/// 核对系统当前主接口及同一地址族的可用地址，不将任意虚拟接口的旧地址当作网络恢复。
pub(crate) fn network_available() -> bool {
    let primary = crate::system_proxy::primary_network_interfaces();
    if primary.is_empty() {
        return false;
    }
    let mut list = std::ptr::null_mut();
    // getifaddrs 返回的链表只读访问，并在返回前统一释放。
    unsafe {
        if libc::getifaddrs(&raw mut list) != 0 {
            return false;
        }
        let mut cursor = list;
        let mut available = false;
        while !cursor.is_null() {
            let item = &*cursor;
            if !item.ifa_addr.is_null()
                && !item.ifa_name.is_null()
                && item.ifa_flags & (libc::IFF_UP as u32) != 0
                && item.ifa_flags & (libc::IFF_LOOPBACK as u32) == 0
            {
                let name = std::ffi::CStr::from_ptr(item.ifa_name).to_bytes();
                let family = i32::from((*item.ifa_addr).sa_family);
                if matches_primary_interface(&primary, name, family) {
                    if family == libc::AF_INET {
                        let address = &*item.ifa_addr.cast::<libc::sockaddr_in>();
                        let ip = std::net::Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes());
                        available |=
                            !ip.is_unspecified() && !ip.is_link_local() && !ip.is_loopback();
                    } else if family == libc::AF_INET6 {
                        let address = &*item.ifa_addr.cast::<libc::sockaddr_in6>();
                        let ip = std::net::Ipv6Addr::from(address.sin6_addr.s6_addr);
                        available |= !ip.is_unspecified()
                            && !ip.is_unicast_link_local()
                            && !ip.is_loopback();
                    }
                }
            }
            cursor = item.ifa_next;
        }
        libc::freeifaddrs(list);
        available
    }
}

fn matches_primary_interface(primary: &[(String, bool)], name: &[u8], family: i32) -> bool {
    // EXV 或其他代理的 utun 不能证明承载其连接的物理网络已恢复。
    !name.starts_with(b"utun")
        && !name.starts_with(b"awdl")
        && !name.starts_with(b"llw")
        && primary.iter().any(|(candidate, ipv6)| {
            candidate.as_bytes() == name
                && family == if *ipv6 { libc::AF_INET6 } else { libc::AF_INET }
        })
}
