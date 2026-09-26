
/// The pre-authentication resource limits for one peer plane (WSP1 §7, frozen).
///
/// These values are measured on the Win32 acceptance host and must not drift from the frozen
/// facts: `stream=1`, `message=64 KiB (65536 B)`, `buffer=64 KiB (65536 B)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneLimits {
    /// Maximum number of concurrent streams a plane may dispatch before authentication.
    pub preauth_max_streams: usize,
    /// Maximum size of a single message/frame admitted before authentication (64 KiB).
    pub preauth_max_message_bytes: usize,
    /// Maximum cumulative buffered bytes a plane may hold before authentication (64 KiB).
    pub preauth_max_buffer_bytes: usize,
    /// Maximum size of a packet-message carried by the data plane.
    pub max_packet_message_bytes: usize,
}

impl PlaneLimits {
    /// The MVP's pre-auth limits, pinned to the frozen WSP1 values.
    #[must_use]
    pub const fn mvp() -> Self {
        Self {
            preauth_max_streams: 1,
            preauth_max_message_bytes: 65536,
            preauth_max_buffer_bytes: 65536,
            max_packet_message_bytes: 65536,
        }
    }
}

