
/// The logical traffic plane of a named-pipe connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    /// The control plane: signaling and RPC (its Stop path must stay live under packet backpressure).
    Control,
    /// The data plane: packet traffic.
    Packet,
}

/// A physical named-pipe connection bound to exactly one plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionBinding {
    /// The plane this physical connection serves.
    plane: Plane,
    /// The Win32 named-pipe name this connection is bound to.
    pipe_name: String,
}

impl ConnectionBinding {
    /// Binds `pipe_name` to `plane` as one physical connection.
    #[must_use]
    pub const fn new(plane: Plane, pipe_name: String) -> Self {
        Self { plane, pipe_name }
    }

    /// Returns the plane this connection serves.
    #[must_use]
    pub const fn plane(&self) -> Plane {
        self.plane
    }

    /// Returns the Win32 pipe name this connection is bound to.
    #[must_use]
    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }
}

