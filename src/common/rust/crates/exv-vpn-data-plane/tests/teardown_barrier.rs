
use exv_vpn_data_plane::teardown::{EofSignal, JoinOutcome, TeardownBarrier, TeardownSide};

// ---- release ordering: aggregate only after BOTH sides join ----

#[test]
fn join_both_sides_releases_aggregate() {
    let mut b = TeardownBarrier::new();
    assert_eq!(b.join(TeardownSide::ProtocolControl), JoinOutcome::Pending);
    assert!(!b.is_released());
    assert_eq!(b.join(TeardownSide::PacketData), JoinOutcome::Released);
    assert!(b.is_released());
    assert!(!b.is_fenced());
}

#[test]
fn single_side_join_is_pending_and_fenced() {
    let mut b = TeardownBarrier::new();
    assert_eq!(b.join(TeardownSide::PacketData), JoinOutcome::Pending);
    assert!(!b.is_released());
    assert!(b.is_fenced());
}

#[test]
fn never_joining_peer_blocks_teardown() {
    let mut b = TeardownBarrier::new();
    assert_eq!(b.join(TeardownSide::ProtocolControl), JoinOutcome::Pending);
    // The packet child never joins; the aggregate must stay fenced/blocked.
    assert!(!b.is_released());
    assert!(b.is_fenced());
}

// ---- EOF: one side cancels the peer, but both joins are still required ----

#[test]
fn eof_on_protocol_cancels_packet_peer() {
    let mut b = TeardownBarrier::new();
    assert_eq!(
        b.signal_eof(TeardownSide::ProtocolControl),
        EofSignal::PeerCancelled
    );
    assert!(b.is_cancelled(TeardownSide::PacketData));
}

#[test]
fn eof_on_packet_cancels_protocol_peer() {
    let mut b = TeardownBarrier::new();
    assert_eq!(
        b.signal_eof(TeardownSide::PacketData),
        EofSignal::PeerCancelled
    );
    assert!(b.is_cancelled(TeardownSide::ProtocolControl));
}

#[test]
fn eof_alone_does_not_release_aggregate() {
    let mut b = TeardownBarrier::new();
    b.signal_eof(TeardownSide::ProtocolControl);
    b.signal_eof(TeardownSide::PacketData);
    assert!(b.is_cancelled(TeardownSide::ProtocolControl));
    assert!(b.is_cancelled(TeardownSide::PacketData));
    // EOF/cancel alone must not release the aggregate; both sides must actually join.
    assert!(!b.is_released());
    assert!(b.is_fenced());
}

#[test]
fn eof_cancels_peer_but_peer_join_still_required() {
    let mut b = TeardownBarrier::new();
    assert_eq!(
        b.signal_eof(TeardownSide::ProtocolControl),
        EofSignal::PeerCancelled
    );
    assert!(b.is_cancelled(TeardownSide::PacketData));
    // Cancellation alone does not satisfy the barrier: the peer's join is still required.
    assert_eq!(b.join(TeardownSide::ProtocolControl), JoinOutcome::Pending);
    assert!(!b.is_released());
    assert!(b.is_fenced());
    assert_eq!(b.join(TeardownSide::PacketData), JoinOutcome::Released);
}

#[test]
fn eof_both_sides_then_join_releases() {
    let mut b = TeardownBarrier::new();
    assert_eq!(
        b.signal_eof(TeardownSide::ProtocolControl),
        EofSignal::PeerCancelled
    );
    assert_eq!(
        b.signal_eof(TeardownSide::PacketData),
        EofSignal::PeerAlreadyCancelled
    );
    assert_eq!(b.join(TeardownSide::ProtocolControl), JoinOutcome::Pending);
    assert_eq!(b.join(TeardownSide::PacketData), JoinOutcome::Released);
    assert!(b.is_released());
}

// ---- capability/attach gate: fenced while held, reissuable only after release ----

#[test]
fn new_capability_rejected_while_barrier_held() {
    let b = TeardownBarrier::new();
    assert!(b.is_fenced());
    let mut b2 = TeardownBarrier::new();
    b2.join(TeardownSide::ProtocolControl);
    assert!(b2.is_fenced());
}

#[test]
fn capability_reissuable_only_after_release() {
    let mut b = TeardownBarrier::new();
    assert!(b.is_fenced());
    b.join(TeardownSide::ProtocolControl);
    b.join(TeardownSide::PacketData);
    assert!(b.is_released());
    assert!(!b.is_fenced());
}

// ---- duplicate / stale joins must not double-count ----

#[test]
fn double_join_same_side_does_not_release() {
    let mut b = TeardownBarrier::new();
    assert_eq!(b.join(TeardownSide::ProtocolControl), JoinOutcome::Pending);
    assert_eq!(
        b.join(TeardownSide::ProtocolControl),
        JoinOutcome::Rejected
    );
    assert!(!b.is_released());
    assert!(b.is_fenced());
}

#[test]
fn stale_join_after_release_is_rejected() {
    let mut b = TeardownBarrier::new();
    b.join(TeardownSide::ProtocolControl);
    b.join(TeardownSide::PacketData);
    assert!(b.is_released());
    // A stale/old-ownership frame joining after release is rejected.
    assert_eq!(
        b.join(TeardownSide::ProtocolControl),
        JoinOutcome::Rejected
    );
}