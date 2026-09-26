

//! Single-slot packet attach guard (spec TG-06).

use exv_vpn_domain::error::{
    EffectCertainty, ErrorCode, ErrorStage, ErrorSubject, RetryAdvice, VpnError,
};
use exv_vpn_domain::identity::RuntimeEpoch;

/// Guards the single packet-boundary attach slot for a runtime epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketAttachGuard {
    runtime_epoch: RuntimeEpoch,
    attached: bool,
}

impl PacketAttachGuard {
    /// Create an unattached guard for the given runtime epoch.
    #[must_use]
    pub const fn new(runtime_epoch: RuntimeEpoch) -> Self {
        Self {
            runtime_epoch,
            attached: false,
        }
    }

    /// Acquire the attach slot; a second call on an already-attached guard is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`exv_vpn_domain::error::ErrorCode::PacketLeaseAlreadyAttached`] when the
    /// slot is already held.
    pub fn try_attach(&mut self) -> Result<(), VpnError> {
        if self.attached {
            return Err(already_attached(self.runtime_epoch.clone()));
        }
        self.attached = true;
        Ok(())
    }

    /// Release the attach slot.
    pub const fn detach(&mut self) {
        self.attached = false;
    }

    /// Whether the slot is currently held.
    #[must_use]
    pub const fn is_attached(&self) -> bool {
        self.attached
    }
}

/// Error for a second attach on an already-held slot.
fn already_attached(epoch: RuntimeEpoch) -> VpnError {
    VpnError::try_from((
        ErrorCode::PacketLeaseAlreadyAttached,
        ErrorStage::AttachingPacketBoundary,
        EffectCertainty::NoEffect,
        RetryAdvice::DoNotRetry,
        ErrorSubject::Runtime(epoch),
        None,
        None,
    ))
    .expect("valid error tuple")
}