//! 会话事实汇总：入队不等于 TLS 写完，写完不等于对端确认。
use crate::log_sink::LogSink;
use exv_vpn_cstp::session::{
    CstpControlEvent, IoDirection, SessionEndReason, SessionIoDiagnostics, SessionIoError,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) fn identity(operation: &[u8]) -> Vec<(&'static str, String)> {
    vec![
        (
            "operation_id_hex",
            operation.iter().map(|b| format!("{b:02x}")).collect(),
        ),
        (
            "event_observed_ms",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string(),
        ),
    ]
}
pub(crate) fn build_fields() -> Vec<(&'static str, String)> {
    let fields = crate::build_identity::fields();
    [
        "build.release_version",
        "build.git_commit",
        "build.git_dirty",
        "build.git_dirty_scope",
        "build.profile",
        "build.profile_scope",
        "build.target",
        "build.opt_level",
        "build.identity_source",
        "build.override_fields",
    ]
    .into_iter()
    .filter_map(|key| fields.get(key).map(|value| (key, value.clone())))
    .collect()
}
pub(crate) fn io_fields(error: &SessionIoError) -> Vec<(&'static str, String)> {
    vec![
        ("io_direction", format!("{:?}", error.direction)),
        ("io_kind", format!("{:?}", error.kind)),
        (
            "raw_os_error",
            error
                .raw_os_error
                .map_or_else(|| "none".into(), |v| v.to_string()),
        ),
        ("io_detail", error.detail.clone()),
        ("error_observed_ms", error.observed_ms.to_string()),
    ]
}
pub(crate) struct SessionDiagnostics {
    started: Instant,
    last_summary: Instant,
    read: Option<SessionIoDiagnostics>,
    write: Option<SessionIoDiagnostics>,
    pub keepalive_queued: u64,
    pub dpd_reply_queued: u64,
    pub queue_failures: u64,
    keepalive_received: u64,
    dpd_requests_received: u64,
    dpd_responses_received: u64,
}
impl SessionDiagnostics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            last_summary: Instant::now(),
            read: None,
            write: None,
            keepalive_queued: 0,
            dpd_reply_queued: 0,
            queue_failures: 0,
            keepalive_received: 0,
            dpd_requests_received: 0,
            dpd_responses_received: 0,
        }
    }
    pub fn observe(&mut self, event: &CstpControlEvent, logs: &LogSink, operation: &[u8]) {
        match event {
            CstpControlEvent::IoDiagnostics(sample) => match sample.direction {
                IoDirection::Read => self.read = Some(sample.clone()),
                IoDirection::Write => self.write = Some(sample.clone()),
            },
            CstpControlEvent::Control { kind: 7, .. } => self.keepalive_received += 1,
            CstpControlEvent::Control { kind: 3, .. } => self.dpd_requests_received += 1,
            CstpControlEvent::DpdResponse => self.dpd_responses_received += 1,
            CstpControlEvent::WriteFailed(error) => {
                let mut fields = identity(operation);
                fields.extend(io_fields(error));
                logs.publish(
                    "warn",
                    "cstp",
                    "CSTP_WRITE_FAILED",
                    "TLS 写入失败；队列接收不能证明帧已写入",
                    &fields,
                );
            }
            CstpControlEvent::SessionEnded(reason) => {
                let mut fields = identity(operation);
                let reason = match reason {
                    SessionEndReason::ServerDisconnect { kind, code, reason, body_len } => {
                        fields.push(("server_packet_type", format!("{kind:#04x}")));
                        fields.push(("server_reason_code", format!("{code:?}")));
                        fields.push(("server_reason", reason.clone()));
                        fields.push(("server_body_len", body_len.to_string()));
                        "server_disconnect"
                    }
                    SessionEndReason::Io(error) => {
                        fields.extend(io_fields(error));
                        "io_error"
                    }
                    SessionEndReason::GatewayClosedStream => "gateway_closed_stream",
                    SessionEndReason::Codec(_) => "codec_error",
                    SessionEndReason::ConsumerDropped => "consumer_dropped",
                };
                fields.push(("reason", reason.into()));
                logs.publish(
                    "warn",
                    "cstp",
                    "CSTP_SESSION_ENDED",
                    "CSTP 会话结束",
                    &fields,
                );
            }
            _ => {}
        }
    }
    fn fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = vec![
            (
                "connected_for_ms",
                self.started.elapsed().as_millis().to_string(),
            ),
            ("keepalive_interval_secs", "15".into()),
            ("keepalive_queued", self.keepalive_queued.to_string()),
            ("dpd_reply_queued", self.dpd_reply_queued.to_string()),
            ("control_queue_failures", self.queue_failures.to_string()),
            ("keepalive_received", self.keepalive_received.to_string()),
            (
                "dpd_requests_received",
                self.dpd_requests_received.to_string(),
            ),
            (
                "dpd_responses_received",
                self.dpd_responses_received.to_string(),
            ),
        ];
        for (sample, bytes, operations, age, activity) in [
            (
                &self.read,
                "tls_read_bytes",
                "tls_read_operations",
                "read_sample_age_ms",
                "last_read_ago_ms",
            ),
            (
                &self.write,
                "tls_write_completed_bytes",
                "tls_write_completed_frames",
                "write_sample_age_ms",
                "last_write_ago_ms",
            ),
        ] {
            fields.push((
                bytes,
                sample
                    .as_ref()
                    .map_or_else(|| "unobserved".into(), |s| s.bytes.to_string()),
            ));
            fields.push((
                operations,
                sample
                    .as_ref()
                    .map_or_else(|| "unobserved".into(), |s| s.operations.to_string()),
            ));
            fields.push((
                age,
                sample.as_ref().map_or_else(
                    || "unobserved".into(),
                    |s| s.sampled_at.elapsed().as_millis().to_string(),
                ),
            ));
            fields.push((
                activity,
                sample.as_ref().and_then(|s| s.last_activity).map_or_else(
                    || "unobserved".into(),
                    |t| t.elapsed().as_millis().to_string(),
                ),
            ));
        }
        for (key, value) in [
            (
                "keepalive_written",
                self.write.as_ref().map(|s| s.keepalive_frames),
            ),
            (
                "dpd_request_written",
                self.write.as_ref().map(|s| s.dpd_request_frames),
            ),
            (
                "dpd_reply_written",
                self.write.as_ref().map(|s| s.dpd_response_frames),
            ),
        ] {
            fields.push((
                key,
                value.map_or_else(|| "unobserved".into(), |v| v.to_string()),
            ));
        }
        fields
    }
    pub fn periodic(&mut self, logs: &LogSink, operation: &[u8]) {
        if self.last_summary.elapsed() >= Duration::from_secs(30) {
            self.emit(logs, operation, "periodic");
        }
    }
    pub fn emit(&mut self, logs: &LogSink, operation: &[u8], trigger: &'static str) {
        let mut fields = identity(operation);
        fields.extend(self.fields());
        fields.push(("trigger", trigger.into()));
        logs.publish(
            if trigger == "periodic" {
                "debug"
            } else {
                "info"
            },
            "cstp",
            "CSTP_SESSION_SUMMARY",
            "会话流量统计",
            &fields,
        );
        self.last_summary = Instant::now();
    }
}
