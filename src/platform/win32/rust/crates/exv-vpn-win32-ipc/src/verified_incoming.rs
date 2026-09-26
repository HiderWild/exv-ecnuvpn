
use crate::peer_auth::PeerAuthError;

/// The disposition of an incoming connection at the dispatch seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingVerdict {
    /// The connection passed authentication and was dispatched.
    Dispatched,
    /// Authentication failed; the peer must not be dispatched.
    AuthFailed(PeerAuthError),
}

