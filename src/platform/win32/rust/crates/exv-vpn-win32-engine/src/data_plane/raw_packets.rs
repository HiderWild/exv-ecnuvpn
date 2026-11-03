// 原始 IP 通道由产品 assemble 的 aTrust 分支调用；Cisco 仍用 spawn_data_plane。
// 接线状态见 docs/research/atrust/t07-runtime-assembly.md；真实宿主流尚待验收。

use super::{
    Arc, AtomicBool, AtomicU64, DataPlaneAux, DataPlaneSpawnError, DataPlaneWorkerGuard, Duration,
    EngineDataPlane, EngineDataPlaneThreads, Mutex, Ordering, WAIT_FAILED, WaitForSingleObject,
    count_reader_upload, count_writer_download, mpsc, wait_for_data_plane_activation,
};

/// 原始包桥接的累计观测；快照各字段独立读取，不表示原子事务。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RawPacketSnapshot {
    pub overflow_packets: u64,
    pub overflow_bytes: u64,
    pub full_wait_count: u64,
    pub full_wait_us: u64,
    pub empty_wait_count: u64,
    pub empty_wait_us: u64,
}

#[derive(Default)]
pub(super) struct RawPacketStats {
    overflow_packets: AtomicU64,
    overflow_bytes: AtomicU64,
    full_wait_count: AtomicU64,
    full_wait_us: AtomicU64,
    empty_wait_count: AtomicU64,
    empty_wait_us: AtomicU64,
}

impl RawPacketStats {
    pub(super) fn snapshot(&self) -> RawPacketSnapshot {
        RawPacketSnapshot {
            overflow_packets: self.overflow_packets.load(Ordering::Relaxed),
            overflow_bytes: self.overflow_bytes.load(Ordering::Relaxed),
            full_wait_count: self.full_wait_count.load(Ordering::Relaxed),
            full_wait_us: self.full_wait_us.load(Ordering::Relaxed),
            empty_wait_count: self.empty_wait_count.load(Ordering::Relaxed),
            empty_wait_us: self.empty_wait_us.load(Ordering::Relaxed),
        }
    }

    fn record_send_error(&self, code: u32, len: usize) -> bool {
        if code != windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW.0 {
            return false;
        }
        self.overflow_packets.fetch_add(1, Ordering::Relaxed);
        self.overflow_bytes.fetch_add(len as u64, Ordering::Relaxed);
        true
    }

    fn sleep(&self, full: bool) {
        let started = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(1));
        let us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let (count, elapsed) = if full {
            (&self.full_wait_count, &self.full_wait_us)
        } else {
            (&self.empty_wait_count, &self.empty_wait_us)
        };
        elapsed.fetch_add(us, Ordering::Relaxed);
        count.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SendResult {
    Sent,
    Stopped,
    Closed,
}

// 原生调用与同一结果分派分开，局部测试可以注入111而不伪造真实Wintun证据。
fn record_wintun_write(
    result: Result<(), exv_vpn_win32_resource::native_error::NativeError>,
    len: usize,
    counter: &AtomicU64,
    business_stats: Option<&crate::stats::StatsRegistry>,
    observation: &RawPacketStats,
) -> Result<(), exv_vpn_win32_resource::native_error::NativeError> {
    match result {
        Ok(()) => count_writer_download(counter, business_stats, len as u64),
        Err(error) if observation.record_send_error(error.code, len) => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

// 专用同步线程不能 blocking_send 后失去停止机会。满队列只持有当前一个包，
// 等待期间检查停止，不扩展无界缓冲，也不在持有 Wintun 锁时等待。
fn send_packet_observed(
    tx: &mpsc::Sender<Vec<u8>>,
    mut packet: Vec<u8>,
    stop: &AtomicBool,
    stats: &RawPacketStats,
) -> SendResult {
    loop {
        if stop.load(Ordering::Acquire) {
            return SendResult::Stopped;
        }
        match tx.try_send(packet) {
            Ok(()) => return SendResult::Sent,
            Err(mpsc::error::TrySendError::Closed(_)) => return SendResult::Closed,
            Err(mpsc::error::TrySendError::Full(pending)) => {
                packet = pending;
                stats.sleep(true);
            }
        }
    }
}

impl EngineDataPlane {
    /// 将 Wintun 原始包接入协议自己的有界收发队列；不封装 CSTP，不发送 CSTP 保活。
    /// 返回线程初始未激活，owner 安装会话后调用 activate；停止仍复用 stop_and_join。
    /// 协议任务负责过滤包、授权和心跳，并在退出时关闭入站队列。
    /// aux 中的 CSTP control_rx 与 latency 不用于此通道。
    #[must_use]
    pub fn spawn_raw_packet_data_plane(
        &self,
        outgoing: mpsc::Sender<Vec<u8>>,
        mut incoming: mpsc::Receiver<Vec<u8>>,
        log: Option<Arc<crate::log_sink::LogSink>>,
        aux: DataPlaneAux,
    ) -> Result<EngineDataPlaneThreads, DataPlaneSpawnError> {
        let reader_stop = Arc::new(AtomicBool::new(false));
        let writer_stop = Arc::new(AtomicBool::new(false));
        let activated = Arc::new(AtomicBool::new(false));
        let failure_reported = Arc::new(AtomicBool::new(false));
        let active_operation_id = Arc::new(Mutex::new(aux.operation_id.clone()));
        let ring_received_bytes = Arc::new(AtomicU64::new(0));
        let ring_sent_bytes = Arc::new(AtomicU64::new(0));
        let raw_packet_stats = Arc::new(RawPacketStats::default());
        let make_guard = |worker, stop: &Arc<AtomicBool>| DataPlaneWorkerGuard {
            raw_packets: true,
            stop: stop.clone(),
            reported: failure_reported.clone(),
            operation_id: active_operation_id.clone(),
            worker,
            aux: aux.clone(),
            log: log.clone(),
        };
        // 首个线程启动前就有同一组 owner，部分成功也不 detach。
        let mut threads = EngineDataPlaneThreads {
            activated: activated.clone(),
            failure_reported: failure_reported.clone(),
            active_operation_id: active_operation_id.clone(),
            reader_stop: reader_stop.clone(),
            writer_stop: writer_stop.clone(),
            reader: None,
            writer: None,
            ring_received_bytes: ring_received_bytes.clone(),
            ring_sent_bytes: ring_sent_bytes.clone(),
            raw_packet_stats: Some(raw_packet_stats.clone()),
        };
        let reader_guard = make_guard("wintun_raw_reader", &reader_stop);
        let writer_guard = make_guard("wintun_raw_writer", &writer_stop);
        let reader_session = self.session.clone();
        let reader_gate = activated.clone();
        let reader_flag = reader_stop.clone();
        let reader_counter = ring_received_bytes.clone();
        let reader_stats = aux.stats.clone();
        let reader_observation = raw_packet_stats.clone();
        if let Err(error) = threads.spawn_worker(true, "wintun_raw_reader", move || {
            let guard = reader_guard;
            if !wait_for_data_plane_activation(&reader_gate, &reader_flag) {
                return;
            }
            'reader: while !reader_flag.load(Ordering::Acquire) {
                if outgoing.is_closed() {
                    guard.report("raw_protocol_closed", None, None);
                    break;
                }
                let event = reader_session.lock().expect("session 锁").read_wait_event();
                // SAFETY: 线程持有 session，事件句柄由 Wintun 管理，join 后才释放 session。
                if unsafe { WaitForSingleObject(event, 50) } == WAIT_FAILED {
                    let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
                    guard.report("wintun_wait_failed", Some(code), None);
                    break;
                }
                while !reader_flag.load(Ordering::Acquire) {
                    let packet = {
                        let mut session = reader_session.lock().expect("session 锁");
                        match session.receive() {
                            Ok(Some(packet)) => Some(packet.as_slice().to_vec()),
                            Ok(None) => None,
                            Err(error) => {
                                drop(session);
                                guard.report("wintun_receive_failed", Some(error.code), None);
                                break 'reader;
                            }
                        }
                    };
                    let Some(packet) = packet else {
                        break;
                    };
                    count_reader_upload(
                        &reader_counter,
                        reader_stats.as_deref(),
                        packet.len() as u64,
                    );
                    match send_packet_observed(&outgoing, packet, &reader_flag, &reader_observation)
                    {
                        SendResult::Sent => {}
                        SendResult::Stopped => break 'reader,
                        SendResult::Closed => {
                            guard.report("raw_protocol_closed", None, None);
                            break 'reader;
                        }
                    }
                }
            }
        }) {
            return Err(DataPlaneSpawnError {
                message: error,
                threads,
            });
        }
        let writer_session = self.session.clone();
        let writer_gate = activated.clone();
        let writer_flag = writer_stop.clone();
        let writer_counter = ring_sent_bytes.clone();
        let writer_stats = aux.stats;
        let writer_observation = raw_packet_stats;
        if let Err(error) = threads.spawn_worker(false, "wintun_raw_writer", move || {
            let guard = writer_guard;
            if !wait_for_data_plane_activation(&writer_gate, &writer_flag) {
                return;
            }
            while !writer_flag.load(Ordering::Acquire) {
                match incoming.try_recv() {
                    Ok(packet) => {
                        let result = writer_session.lock().expect("session 锁").send(&packet);
                        if let Err(error) = record_wintun_write(
                            result,
                            packet.len(),
                            &writer_counter,
                            writer_stats.as_deref(),
                            &writer_observation,
                        ) {
                            guard.report("wintun_send_failed", Some(error.code), None);
                            break;
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => writer_observation.sleep(false),
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        guard.report("raw_protocol_closed", None, None);
                        break;
                    }
                }
            }
        }) {
            return Err(DataPlaneSpawnError {
                message: error,
                threads,
            });
        }
        Ok(threads)
    }
}

// 接线状态：已由产品 assemble 的 aTrust 分支调用；真实宿主流尚待验收。
// 接线与验收边界见 docs/research/atrust/t07-runtime-assembly.md。
