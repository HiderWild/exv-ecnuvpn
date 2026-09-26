//! Engine 结构化日志 sink（MAC-OBS-13 S1）：pipeline/泵诊断 → wire `LogEvent` 推送。
//!
//! 镜像 [`crate::stats::StatsPublisher`] 的接缝形态（有界通道 + last-writer-wins +
//! 重连重新 open），差异只在驱动方式：统计是定时采样 task，日志是事件驱动直推——
//! `open_stream` 只挂接通道，不 spawn 任务。
//!
//! ## 推送语义（选型）
//!
//! * **无订阅者（Core 未挂 `StreamLogs`）→ 直接丢弃**：诊断日志低价值高频，Core
//!   侧历史由聚合落盘负责（MAC-OBS-13 计划 §0.6），Engine 不做离线缓冲补拉
//!   （wire `StreamLogsRequest.resume_tick` 契约为 0 = 从当前流位置开始）。
//! * **有订阅者但通道满 → 丢弃该条（`try_send`）**：日志绝不阻塞 pipeline/泵业务
//!   路径；泵线程为阻塞 I/O 的 OS 线程，`try_send` 无需 runtime、可安全调用。
//! * **订阅端断线（receiver drop）→ 后续 publish 静默丢弃**；Core 重连重新
//!   `open_stream` 即恢复推送（替换旧通道，旧通道随即关闭）。
//!
//! ## 与既有诊断的关系
//!
//! sink 记录为**并列增量**：既有 `println!`（pipeline）与 `pump::diag` 落盘诊断
//! 一律保留不删（计划 §S1 明确不重构、不删除）；stderr 被服务代理置空后，结构化
//! 事件经认证 UDS 到达 Core 是唯一面向产品的日志通道。
//!
//! ## 脱敏
//!
//! 调用点只允许经 `fields` 白名单键（`step`/`errno`/`substep`/`pid` 等结构化诊断
//! 元数据）附带上下文；凭据、密文 envelope 与证书材料永不进入 `message`/`fields`
//! （对齐 proto `LogEvent` 安全注解；Core 入库侧另有 redact 防御层）。

use std::sync::Mutex;

use exv_vpn_wire::generated as wire;
use tokio::sync::mpsc;
use tonic::Status;

/// 推送通道容量（控制面诊断事件低频；满即丢新，绝不阻塞业务路径）。
const SINK_CHANNEL_CAPACITY: usize = 256;

/// Engine 结构化日志推送端点。
///
/// `open_stream` 挂接唯一推送通道（last-writer-wins：替换即关闭旧通道）；业务路径
/// 经 [`LogSink::publish`] 投递事件（非阻塞、可从任意线程调用）。
#[derive(Debug, Default)]
pub struct LogSink {
    /// 当前挂接的推送通道；`None` = 无订阅者（publish 直接丢弃）。
    push: Mutex<Option<mpsc::Sender<Result<wire::LogEvent, Status>>>>,
}

impl LogSink {
    /// 建一个无订阅者的 sink（未挂接流；publish 丢弃）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 挂接一条 `StreamLogs` 推送流，返回接收端（gRPC server 转成 receiver stream）。
    ///
    /// 注册为当前唯一推送通道（替换任何残留旧通道——旧通道的接收端随上一次流关闭）。
    #[must_use]
    pub fn open_stream(&self) -> mpsc::Receiver<Result<wire::LogEvent, Status>> {
        let (tx, rx) = mpsc::channel(SINK_CHANNEL_CAPACITY);
        if let Ok(mut push) = self.push.lock() {
            *push = Some(tx);
        }
        rx
    }

    /// 投递一条结构化诊断事件（非阻塞；无订阅者/通道满/通道关闭均静默丢弃）。
    ///
    /// `level` 取 `info` | `warn` | `error`（wire 注释约定）；`fields` 为白名单
    /// 结构化键值（本函数内收集为 wire map）。
    pub fn publish(
        &self,
        level: &str,
        component: &str,
        code: &str,
        message: impl Into<String>,
        fields: &[(&'static str, String)],
    ) {
        let sender = self
            .push
            .lock()
            .ok()
            .and_then(|push| push.as_ref().cloned());
        let Some(sender) = sender else {
            return; // 无订阅者：丢弃（历史由 Core 聚合负责）。
        };
        let event = wire::LogEvent {
            level: level.to_owned(),
            component: component.to_owned(),
            code: code.to_owned(),
            message: message.into(),
            fields: fields
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            timestamp_ms: wall_clock_ms(),
        };
        // 通道满或接收端已 drop：丢弃该条，绝不阻塞业务路径。
        let _ = sender.try_send(Ok(event));
    }
}

/// 当前 wall-clock epoch 毫秒（UTC；proto 约定 0 = unknown，本实现恒填真实值）。
///
/// 与 [`crate::stats`] 的同名 helper 同义（统计采样与日志投递共用同一时间口径）。
fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// ---------------------------------------------------------------------------
// 单元测试：结构化字段输出、无订阅者丢弃、断线丢弃、通道满丢弃不阻塞。
// ---------------------------------------------------------------------------
