//! utun ↔ CSTP 双向数据泵（Engine 进程内；payload 不跨进程）。
//!
//! 实现为两个专用 OS 线程 + 阻塞 I/O：utun 的 kctl 套接字在 kqueue/AsyncFd 下
//! 的可读事件不可靠（实测永不触发），阻塞读是 utun 的标准用法。取消经
//! cancel 标志 + 上行 poll 有界等待 + 下行收包轮询；线程各自关闭 dup fd，保留原 utun。

// 泵线程为承载诊断与生命周期管理而展开；各线程以 OwnedFd 持有自己的描述符。
#![allow(clippy::too_many_lines)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::doc_markdown)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::packet::AF_INET_HEADER;
use crate::packet::probe::LatencyTap;
use exv_vpn_cstp::codec::{CSTP_PACKET_TYPE_DATA, Codec};

/// 双泵的运行句柄与字节计数（rx=CSTP→utun，tx=utun→CSTP）。
pub struct PumpSet {
    up_handle: std::thread::JoinHandle<()>,
    down_handle: std::thread::JoinHandle<()>,
    cancel: Arc<AtomicBool>,
    /// tx=utun→CSTP 累计字节。
    pub tx_bytes: Arc<AtomicU64>,
    /// rx=CSTP→utun 累计字节。
    pub rx_bytes: Arc<AtomicU64>,
}

impl PumpSet {
    /// 任一泵已退出（真实失败或对端通道关闭）。
    #[must_use]
    pub fn either_finished(&self) -> bool {
        self.up_handle.is_finished() || self.down_handle.is_finished()
    }

    /// 取消两泵并等待退出。
    pub fn shutdown(self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.up_handle.join();
        let _ = self.down_handle.join();
    }
}

/// 开发期诊断：首包与异常落盘（stderr 被服务代理置空）。
pub(crate) fn diag(message: &str) {
    use std::io::Write as _;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/private/tmp/exv-engine-pump.log")
    {
        let _ = writeln!(file, "[{now}] {message}");
    }
}

/// 从 utun fd dup 出两个独立描述符（引用同一打开文件，双泵各持其一）。
///
/// # Panics
///
/// dup 失败（root 进程正常路径不会）。
fn duplicate_utun_fds(fd: &OwnedFd) -> (OwnedFd, OwnedFd) {
    // SAFETY: dup(2) on a live fd.
    let first = unsafe { libc::dup(fd.as_raw_fd()) };
    assert!(first >= 0, "dup utun fd failed");
    // SAFETY: 成功 dup 的新描述符仅由本 OwnedFd 持有。
    let first = unsafe { OwnedFd::from_raw_fd(first) };
    let second = unsafe { libc::dup(fd.as_raw_fd()) };
    assert!(second >= 0, "dup utun fd failed");
    // SAFETY: 同上；若第二次 dup 失败，first 会随栈释放。
    (first, unsafe { OwnedFd::from_raw_fd(second) })
}

/// 启动双向泵。
///
/// `log_sink`（MAC-OBS-13 S1）接收泵生命周期/异常的结构化诊断事件（与既有
/// `diag` 落盘诊断并列；事件只含白名单字段，无凭据/证书材料）。按共享引用
/// 接收，`Arc` 的 clone 只发生给双泵线程。
///
/// `latency`（C2 延迟探测）= 下行回包匹配接线：下行线程在每个入站 payload 写
/// utun 之前调用（匹配在途 ping 的 echo reply 即 RTT 写注册表）；`None` = 本连接
/// 未接延迟探测（行为与 C2 之前一致）。探测节拍由管线保持循环驱动，不在泵内。
///
/// # Panics
///
/// 仅当 dup 失败（root 进程正常路径不会）。
pub fn start(
    utun_fd: &OwnedFd,
    write_channel: mpsc::UnboundedSender<Vec<u8>>,
    mut read_channel: mpsc::UnboundedReceiver<Vec<u8>>,
    log_sink: &Arc<crate::log_sink::LogSink>,
    latency: Option<LatencyTap>,
) -> PumpSet {
    let (up_fd, down_fd) = duplicate_utun_fds(utun_fd);
    let cancel = Arc::new(AtomicBool::new(false));
    let tx_bytes = Arc::new(AtomicU64::new(0));
    let rx_bytes = Arc::new(AtomicU64::new(0));

    // ---- 上行：utun 阻塞读 → CSTP Data 帧。----
    let up_cancel = Arc::clone(&cancel);
    let up_tx = Arc::clone(&tx_bytes);
    let up_sink = Arc::clone(log_sink);
    let up = std::thread::spawn(move || {
        let mut frame = vec![0_u8; 4 + 65_535];
        diag("up: pump started (threaded be)");
        up_sink.publish("info", "packet", "PUMP_STARTED", "up pump started", &[]);
        loop {
            if up_cancel.load(Ordering::Relaxed) {
                return;
            }
            // poll 有界等待取消；不从其他线程 close 正在读取的 FD，也不 shutdown
            // 底层 utun socket（自动重连需要复用原设备）。本线程是唯一上行读者。
            let mut ready = libc::pollfd {
                fd: up_fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: ready 指向有效的单个 pollfd，timeout 为 100 ms。
            let polled = unsafe { libc::poll(std::ptr::from_mut(&mut ready), 1, 100) };
            if up_cancel.load(Ordering::Relaxed) {
                return;
            }
            if polled == 0 {
                continue;
            }
            if polled < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                up_sink.publish(
                    "error",
                    "packet",
                    "PUMP_POLL_FAILED",
                    format!("up pump poll failed: {error}"),
                    &[],
                );
                return;
            }
            // SAFETY: poll 已报告可读/终止，本线程独占读取；OwnedFd 保持描述符存活。
            let n =
                unsafe { libc::read(up_fd.as_raw_fd(), frame.as_mut_ptr().cast(), frame.len()) };
            if n < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                diag(&format!("up: read failed: {error}"));
                up_sink.publish(
                    "error",
                    "packet",
                    "PUMP_READ_FAILED",
                    format!("up pump read failed: {error}"),
                    &[("direction", "up".to_owned())],
                );
                return;
            }
            let n = usize::try_from(n).unwrap_or(0);
            if n == 0 {
                diag("up: read returned 0 (device closed)");
                up_sink.publish(
                    "warn",
                    "packet",
                    "PUMP_DEVICE_CLOSED",
                    "up pump read returned 0 (device closed)",
                    &[("direction", "up".to_owned())],
                );
                return;
            }
            if n < 4 {
                continue;
            }
            // 实测 macOS utun 的 AF 头为大端。
            let family = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
            if family != u32::from_be_bytes(AF_INET_HEADER) {
                diag(&format!(
                    "up: non-AF_INET family={family:#x} head={:02x?}",
                    &frame[..4.min(n)]
                ));
                continue;
            }
            let payload = frame[4..n].to_vec();
            let Ok(bytes) = Codec::encode_raw(CSTP_PACKET_TYPE_DATA, &payload) else {
                continue;
            };
            if write_channel.send(bytes).is_err() {
                diag("up: write_channel closed");
                up_sink.publish(
                    "warn",
                    "packet",
                    "PUMP_CHANNEL_CLOSED",
                    "up pump write channel closed (CSTP task ended)",
                    &[("direction", "up".to_owned())],
                );
                return;
            }
            let previous =
                up_tx.fetch_add(u64::try_from(payload.len()).unwrap_or(0), Ordering::Relaxed);
            if (previous == 0 || previous < 4096) && payload.len() >= 20 {
                diag(&format!(
                    "up frame: {} bytes dst={}.{}.{}.{} proto={}",
                    payload.len(),
                    payload[16],
                    payload[17],
                    payload[18],
                    payload[19],
                    payload[9]
                ));
            }
        }
    });

    // ---- 下行：CSTP Data payload → utun 阻塞写。----
    let down_cancel = Arc::clone(&cancel);
    let down_rx = Arc::clone(&rx_bytes);
    let down_sink = Arc::clone(log_sink);
    // C2 延迟探测：回包匹配挂下行线程（对齐 win32 writer 线程匹配职责）。
    let down_latency = latency;
    let down = std::thread::spawn(move || {
        diag("down: pump started (threaded be)");
        loop {
            if down_cancel.load(Ordering::Relaxed) {
                return;
            }
            let payload = loop {
                if down_cancel.load(Ordering::Relaxed) {
                    return;
                }
                match read_channel.try_recv() {
                    Ok(payload) => break payload,
                    Err(mpsc::error::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        diag("down: read_channel closed");
                        down_sink.publish(
                            "warn",
                            "packet",
                            "PUMP_CHANNEL_CLOSED",
                            "down pump read channel closed (CSTP task ended)",
                            &[("direction", "down".to_owned())],
                        );
                        return;
                    }
                }
            };
            // C2 延迟探测：写 utun 之前匹配在途 ping 的 echo reply（不匹配零开销，
            // 入站包仍正常下行；对齐 win32 writer 线程职责）。
            if let Some(latency) = &down_latency {
                latency.on_inbound(&payload);
            }
            let mut buffer = Vec::with_capacity(4 + payload.len());
            buffer.extend_from_slice(&AF_INET_HEADER);
            buffer.extend_from_slice(&payload);
            // SAFETY: down_fd 是本泵独占的阻塞描述符。
            let written =
                unsafe { libc::write(down_fd.as_raw_fd(), buffer.as_ptr().cast(), buffer.len()) };
            if written < 0 {
                let write_error = io::Error::last_os_error();
                diag(&format!("down: write failed: {write_error}"));
                down_sink.publish(
                    "error",
                    "packet",
                    "PUMP_WRITE_FAILED",
                    format!("down pump write failed: {write_error}"),
                    &[("direction", "down".to_owned())],
                );
                return;
            }
            let previous =
                down_rx.fetch_add(u64::try_from(payload.len()).unwrap_or(0), Ordering::Relaxed);
            if (previous == 0 || previous < 4096) && payload.len() >= 20 {
                diag(&format!(
                    "down frame: {} bytes dst={}.{}.{}.{} proto={} src={}.{}.{}.{}",
                    payload.len(),
                    payload[16],
                    payload[17],
                    payload[18],
                    payload[19],
                    payload[9],
                    payload[12],
                    payload[13],
                    payload[14],
                    payload[15]
                ));
            }
        }
    });

    PumpSet {
        up_handle: up,
        down_handle: down,
        cancel,
        tx_bytes,
        rx_bytes,
    }
}
