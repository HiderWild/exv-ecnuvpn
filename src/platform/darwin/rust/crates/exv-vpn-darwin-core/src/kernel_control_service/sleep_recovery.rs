//! 休眠前记住正在使用的连接，唤醒后仅恢复该连接；手动操作优先。

use super::*;
use crate::power::{PowerEvent, PowerMonitor};

#[derive(Clone, Debug)]
pub(super) struct SleepConnection {
    pub(super) generation: u64,
    pub(super) operation_id: Vec<u8>,
}

impl LiveConnectionProjection {
    fn session_reconnect_enabled(&self) -> bool {
        lock_recover(&self.reconnect_session_policy)
            .is_some_and(|policy| policy.enabled)
    }

    pub(super) fn sleep_connection(&self) -> Option<SleepConnection> {
        let snapshot = self.events.current_snapshot()?;
        if !self.session_reconnect_enabled()
            || !matches!(snapshot.state, Some(runtime_snapshot::State::Connected(_)))
        {
            return None;
        }
        Some(SleepConnection {
            generation: self.reconnect_generation.load(Ordering::Acquire),
            operation_id: snapshot.operation_id,
        })
    }

    pub(crate) fn observe_power(self: &Arc<Self>) -> Option<PowerMonitor> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let weak = Arc::downgrade(self);
        let mut pending = None;
        let monitor = crate::power::observe(move |event| match event {
            PowerEvent::WillSleep => {
                pending = weak.upgrade().and_then(|engine| engine.sleep_connection());
            }
            PowerEvent::DidWake => {
                if let Some(connection) = pending.take() {
                    let _ = tx.send(connection);
                }
            }
        });
        let monitor = match monitor {
            Ok(monitor) => monitor,
            Err(error) => {
                self.logs.aggregator().append_core(
                    "warn",
                    "core",
                    "POWER_OBSERVER_FAILED",
                    &error,
                    &[],
                );
                return None;
            }
        };
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(connection) = rx.recv().await {
                // 系统已唤醒不等于网络接口已经就绪，等待实际外部接口地址恢复。
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(engine) = weak.upgrade() else {
                    break;
                };
                // 接口恢复晚于 30 秒也不应贸然拆掉旧隧道并消耗重连预算。
                // 等待期间用户的新连接/断开撤销本次恢复；保存设置只影响下次连接。
                loop {
                    if connection.generation != engine.reconnect_generation.load(Ordering::Acquire)
                        || !engine.session_reconnect_enabled()
                    {
                        break;
                    }
                    if crate::power::network_available() {
                        engine.resume_after_sleep(connection).await;
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        });
        Some(monitor)
    }

    pub(super) async fn resume_after_sleep(&self, connection: SleepConnection) {
        if connection.generation != self.reconnect_generation.load(Ordering::Acquire)
            || !self.session_reconnect_enabled()
            || self.reconnect_active.load(Ordering::Acquire)
        {
            return;
        }
        let Some(snapshot) = self.events.current_snapshot() else {
            return;
        };
        if snapshot.operation_id != connection.operation_id {
            return;
        }
        match snapshot.state {
            Some(runtime_snapshot::State::Connected(_)) => {
                // 旧隧道仍显示连接但底层会话可能已过期，先按既有 Stop 收束再复用重连。
                let intent = reconnect_intent();
                let request = StopRequest {
                    intent: Some(wire::StopIntent {
                        lookup_key: intent.lookup_key,
                        request_digest: intent.request_digest,
                    }),
                };
                let next_generation = connection.generation.wrapping_add(1);
                if self.reconnect_generation.compare_exchange(
                    connection.generation, next_generation, Ordering::AcqRel, Ordering::Acquire,
                ).is_err() { return; }
                if let Err(error) = self.stop_in_generation(&request, next_generation, true).await {
                    self.logs.aggregator().append_core(
                        "warn",
                        "core",
                        "WAKE_STOP_FAILED",
                        error.message(),
                        &[],
                    );
                    return;
                }
                if self.reconnect_generation.load(Ordering::Acquire)
                    != connection.generation.wrapping_add(1)
                {
                    return;
                }
            }
            Some(runtime_snapshot::State::FailedClean(_)) => {}
            _ => return,
        }
        self.reconnect_ever_connected.store(true, Ordering::Release);
        self.logs.aggregator().append_core(
            "info",
            "core",
            "WAKE_RECONNECT",
            "系统休眠恢复，正在重新建立休眠前的连接",
            &[],
        );
        self.signal_reconnect();
    }
}
