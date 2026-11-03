//! aTrust 数据面保持与协议 owner 观察。
//! Cisco 保持既有 bootstrap_runtime 的 CstpTasks 外层持有路径；本模块不接管它。

use std::sync::Arc;

use crate::atrust_tunnel::{AtrustCloseObservation, AtrustPacketTunnel};
use crate::log_sink::LogSink;

/// 一次保持环服务的观察结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HoldObservation {
    /// 会话仍在（无事发生或已处理完一个事件/节拍）。
    Alive,
    /// 会话失联（控制通道关闭/协议 worker 退出等）。
    Lost,
}

/// 供保持环的协议无关面（设计 §6.1 修订版）。
#[async_trait::async_trait]
pub(crate) trait TunnelHold: Send {
    /// 周期保活帧（仅 CSTP：keepalive）。aTrust 恒 [`None`]——L3 心跳由 Common
    /// L3Connection 内部 worker 独占，宿主不发帧。
    fn keepalive_frame(&mut self) -> Option<Vec<u8>>;

    /// 会话侧事件分派（CSTP：DPD/控制帧分派+周期 keepalive 发送+延迟探测节拍；
    /// aTrust：空实现——仅观察协议 worker 存活，无事可做）。
    async fn handle_control(&mut self) -> HoldObservation;

    /// 失联归类：CSTP 按泵/控制观察（既有语义：恒 "session_lost"——Cisco 终态
    /// trace/事件不因重构改变）；aTrust 透出 run_packets 终止类别的稳定 reason。
    fn session_lost_reason(&self) -> &'static str;

    /// 泵停止前的协议收尾（CSTP：控制通道有界收取+会话诊断汇总；aTrust：标记
    /// 宿主停机意图，避免泵线程退出被误报为协议掉线）。
    async fn settle(&mut self, session_lost: bool);

    /// 泵停止后的资源释放（CSTP：无（通道随本值 Drop 关闭）；aTrust：关闭
    /// `run_packets` owner 并记录终止类别）。
    async fn release(&mut self);
}

/// aTrust协议任务的共享观察器；join回执归外层管线owner。
pub(crate) struct AtrustHold {
    owner: Arc<tokio::sync::Mutex<Option<AtrustPacketTunnel>>>,
    logs: Arc<LogSink>,
    operation_id: Vec<u8>,
    /// worker 终止类别（观测到失联或 release 时取回）。
    lost_reason: Option<&'static str>,
    close_observation: Option<AtrustCloseObservation>,
}

impl AtrustHold {
    pub(crate) fn new(
        owner: AtrustPacketTunnel,
        logs: Arc<LogSink>,
        operation_id: Vec<u8>,
    ) -> Self {
        Self {
            owner: Arc::new(tokio::sync::Mutex::new(Some(owner))),
            logs,
            operation_id,
            lost_reason: None,
            close_observation: None,
        }
    }
    pub(crate) fn new_shared(
        owner: Arc<tokio::sync::Mutex<Option<AtrustPacketTunnel>>>,
        logs: Arc<LogSink>,
        operation_id: Vec<u8>,
    ) -> Self {
        Self {
            owner,
            logs,
            operation_id,
            lost_reason: None,
            close_observation: None,
        }
    }
    /// 只取 handle_control/release 已消费的原返回，不查询或关闭 owner。
    pub(crate) fn take_close_observation(&mut self) -> Option<AtrustCloseObservation> {
        self.close_observation.take()
    }
}

#[async_trait::async_trait]
impl TunnelHold for AtrustHold {
    fn keepalive_frame(&mut self) -> Option<Vec<u8>> {
        // L3 心跳由 Common 内部定时发送并消费应答，缺响应不主动终结；宿主不发帧。
        None
    }

    async fn handle_control(&mut self) -> HoldObservation {
        // aTrust 无控制通道可分派（空实现）：以 CSTP 环同款 1s 节拍观察协议
        // worker 存活；取消检查由管线保持环的环顶执行（与 CSTP 同 cadence）。
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let mut slot = self.owner.lock().await;
        let Some(owner) = slot.as_mut() else {
            return HoldObservation::Lost;
        };
        if owner.is_running() {
            return HoldObservation::Alive;
        }
        // worker 已退出：立即取回原终止类别；后续 close 重放同一完成结果。
        let observed = owner.close_observed().await;
        if let Err(category) = observed.result() {
            let already_reported = self.lost_reason == Some(category);
            self.lost_reason = Some(category);
            if !already_reported {
                self.logs.publish(
                    "error",
                    "packet",
                    "DARWIN_ATRUST_SESSION_LOST",
                    "aTrust packet session ended while tunnel held",
                    &observed.log_fields(),
                );
            }
        } else {
            self.lost_reason = Some("atrust_session_closed");
        }
        self.close_observation = Some(observed);
        HoldObservation::Lost
    }

    fn session_lost_reason(&self) -> &'static str {
        self.lost_reason.unwrap_or("atrust_connection_error")
    }

    async fn settle(&mut self, _session_lost: bool) {
        // 标记宿主停机意图：泵线程随后退出（宿主通道关闭）不被误报为协议掉线。
        let slot = self.owner.lock().await;
        let Some(owner) = slot.as_ref() else {
            return;
        };
        owner.begin_host_shutdown();
        // 会话诊断收尾（对齐 CSTP diagnostics.emit 的非秘密计数语义）：累计
        // 拒绝流/丢弃包计数 + 最后一项原因（PacketEvent 的 Debug 只含流五元组
        // 与原因类别，无 token/SID/载荷）。
        let (rejected, dropped) = owner.notice_counts();
        if rejected != 0 || dropped != 0 {
            self.logs.publish(
                "info",
                "packet",
                "DARWIN_ATRUST_SESSION_DIAGNOSTICS",
                "aTrust 会话结束诊断（非秘密计数）",
                &[
                    ("rejected_flows", rejected.to_string()),
                    ("dropped_packets", dropped.to_string()),
                    ("operation_len", self.operation_id.len().to_string()),
                    (
                        "last_notice",
                        owner
                            .take_last_notice()
                            .map_or_else(|| "none".to_owned(), |notice| format!("{notice:?}")),
                    ),
                ],
            );
        }
    }

    async fn release(&mut self) {
        // 消费同一 owner 的原完成观察；外层 close 只重放缓存。
        let mut slot = self.owner.lock().await;
        let Some(owner) = slot.as_mut() else {
            return;
        };
        let observed = owner.close_observed().await;
        if let Err(category) = observed.result() {
            let already_reported = self.lost_reason == Some(category);
            self.lost_reason = Some(category);
            if !already_reported {
                self.logs.publish(
                    "error",
                    "packet",
                    "DARWIN_ATRUST_SESSION_LOST",
                    "aTrust packet session ended during release",
                    &observed.log_fields(),
                );
            }
        }
        self.close_observation = Some(observed);
    }
}
