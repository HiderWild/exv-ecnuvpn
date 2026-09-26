
use exv_vpn_domain::model::RuntimeState;

use crate::mailbox::BoundedMailbox;

/// A bounded snapshot/observer path. Publishing is latest-wins: when the buffer is full, the oldest
/// unobserved revision is dropped so a slow reader never stalls on stale revisions.
pub struct SnapshotObserver {
    revisions: BoundedMailbox<RuntimeState>,
}

impl SnapshotObserver {
    /// Create an observer with a bounded revision buffer of at most `capacity` entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            revisions: BoundedMailbox::new(capacity),
        }
    }

    /// Publish the latest committed state. If the buffer is full, the oldest revision is dropped.
    pub fn publish(&mut self, state: RuntimeState) {
        self.revisions.try_send_drop_oldest(state);
    }

    /// Observe the next available revision, if any.
    pub fn observe(&mut self) -> Option<RuntimeState> {
        self.revisions.try_recv()
    }

    /// Number of unobserved revisions currently buffered.
    pub fn len(&self) -> usize {
        self.revisions.len()
    }

    /// Whether there are no unobserved revisions buffered.
    pub fn is_empty(&self) -> bool {
        self.revisions.is_empty()
    }
}

