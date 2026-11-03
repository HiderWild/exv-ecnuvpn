// 原始 IP 通道由产品 assemble 的 aTrust 分支调用；Cisco 仍用 spawn_data_plane。
// 接线状态见 docs/research/atrust/t07-runtime-assembly.md；真实宿主流尚待验收。

use super::{
    Arc, AtomicBool, AtomicU64, DataPlaneAux, DataPlaneWorkerGuard, Duration, EngineDataPlane,
    EngineDataPlaneThreads, Mutex, Ordering, WAIT_FAILED, WaitForSingleObject, count_reader_upload,
    count_writer_download, mpsc, wait_for_data_plane_activation,
};

#[derive(Debug, PartialEq, Eq)]
enum SendResult {
    Sent,
    Stopped,
    Closed,
}

// 专用同步线程不能 blocking_send 后失去停止机会。满队列只持有当前一个包，
// 等待期间检查停止，不扩展无界缓冲，也不在持有 Wintun 锁时等待。
fn send_packet(tx: &mpsc::Sender<Vec<u8>>, mut packet: Vec<u8>, stop: &AtomicBool) -> SendResult {
    loop {
        if stop.load(Ordering::Acquire) {
            return SendResult::Stopped;
        }
        match tx.try_send(packet) {
            Ok(()) => return SendResult::Sent,
            Err(mpsc::error::TrySendError::Closed(_)) => return SendResult::Closed,
            Err(mpsc::error::TrySendError::Full(pending)) => {
                packet = pending;
                std::thread::sleep(Duration::from_millis(1));
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
    ) -> EngineDataPlaneThreads {
        let reader_stop = Arc::new(AtomicBool::new(false));
        let writer_stop = Arc::new(AtomicBool::new(false));
        let activated = Arc::new(AtomicBool::new(false));
        let failure_reported = Arc::new(AtomicBool::new(false));
        let active_operation_id = Arc::new(Mutex::new(aux.operation_id.clone()));
        let ring_received_bytes = Arc::new(AtomicU64::new(0));
        let ring_sent_bytes = Arc::new(AtomicU64::new(0));
        let make_guard = |worker, stop: &Arc<AtomicBool>| DataPlaneWorkerGuard {
            raw_packets: true,
            stop: stop.clone(),
            reported: failure_reported.clone(),
            operation_id: active_operation_id.clone(),
            worker,
            aux: aux.clone(),
            log: log.clone(),
        };
        let reader_guard = make_guard("wintun_raw_reader", &reader_stop);
        let writer_guard = make_guard("wintun_raw_writer", &writer_stop);
        let reader_session = self.session.clone();
        let reader_gate = activated.clone();
        let reader_flag = reader_stop.clone();
        let reader_counter = ring_received_bytes.clone();
        let reader_stats = aux.stats.clone();
        let reader = std::thread::spawn(move || {
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
                    match send_packet(&outgoing, packet, &reader_flag) {
                        SendResult::Sent => {}
                        SendResult::Stopped => break 'reader,
                        SendResult::Closed => {
                            guard.report("raw_protocol_closed", None, None);
                            break 'reader;
                        }
                    }
                }
            }
        });
        let writer_session = self.session.clone();
        let writer_gate = activated.clone();
        let writer_flag = writer_stop.clone();
        let writer_counter = ring_sent_bytes.clone();
        let writer_stats = aux.stats;
        let writer = std::thread::spawn(move || {
            let guard = writer_guard;
            if !wait_for_data_plane_activation(&writer_gate, &writer_flag) {
                return;
            }
            while !writer_flag.load(Ordering::Acquire) {
                match incoming.try_recv() {
                    Ok(packet) => {
                        let result = writer_session.lock().expect("session 锁").send(&packet);
                        match result {
                            Ok(()) => count_writer_download(
                                &writer_counter,
                                writer_stats.as_deref(),
                                packet.len() as u64,
                            ),
                            Err(error)
                                if error.code
                                    == windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW.0 => {}
                            Err(error) => {
                                guard.report("wintun_send_failed", Some(error.code), None);
                                break;
                            }
                        }
                    }
                    Err(mpsc::error::TryRecvError::Empty) => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        guard.report("raw_protocol_closed", None, None);
                        break;
                    }
                }
            }
        });
        EngineDataPlaneThreads {
            activated,
            failure_reported,
            active_operation_id,
            reader_stop,
            writer_stop,
            reader: Some(reader),
            writer: Some(writer),
            ring_received_bytes,
            ring_sent_bytes,
        }
    }
}

// 接线状态：已由产品 assemble 的 aTrust 分支调用；真实宿主流尚待验收。
// 接线与验收边界见 docs/research/atrust/t07-runtime-assembly.md。
