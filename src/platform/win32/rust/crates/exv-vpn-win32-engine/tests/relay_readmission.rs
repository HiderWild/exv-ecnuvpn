
use exv_vpn_data_plane::budget::DataPlaneDirection;
use exv_vpn_domain::identity::{ResourceIdentityDigest, RuntimeEpoch};
use exv_vpn_domain::model::PacketLeaseRef;
use exv_engine::packet_relay::{
    AttachedPacketRelay, RelayDirection, RelayLegState, RelayTerminalSource,
};
use exv_vpn_win32_ipc::packet_channel::PacketChannel;
use exv_vpn_win32_ipc::packet_limits::PacketLimits;
use exv_vpn_win32_resource::packet_capability::PacketCapability;

use uuid::Uuid;

/// 确定性非 nil lease（digest 以 'L' 前缀 + 序号填充，跨测试互不冲突）。
fn lease(n: u8) -> PacketLeaseRef {
    let mut digest = [0u8; 32];
    digest[0] = 0x4C; // 'L'
    digest[1] = n;
    PacketLeaseRef::try_from(ResourceIdentityDigest::try_from(digest).expect("digest"))
        .expect("lease")
}

/// 确定性非 nil runtime epoch（owner_lease 同款构造）。
fn epoch(n: u128) -> RuntimeEpoch {
    RuntimeEpoch::try_from(Uuid::from_u128(n)).expect("epoch")
}

/// 签发确定性一次性 packet capability（绑定 lease(n) + epoch(n)）。
fn issue_capability(n: u8) -> PacketCapability {
    PacketCapability::issue(lease(n), epoch(u128::from(n)))
}

/// 构造一个已 attach 的 relay（connection_id = `conn`），组合 W23A channel。
fn attach_relay(conn: u64) -> AttachedPacketRelay {
    let mut capability = issue_capability(7);
    let attachment = AttachedPacketRelay::attach(&mut capability, conn)
        .expect("fresh capability 首次 attach 必须成功");
    let channel = PacketChannel::new(&PacketLimits::mvp(), DataPlaneDirection::ProtocolToPacket)
        .expect("mvp limits 构造 channel");
    AttachedPacketRelay::new(attachment, channel, PacketLimits::mvp())
}

/// 两腿启动并断言 running proof 成立。
fn start_both(relay: &mut AttachedPacketRelay) {
    relay.start_leg(RelayDirection::Receive);
    relay.start_leg(RelayDirection::Send);
    assert!(relay.running_proof(), "两腿运行 -> running proof 成立");
}

/// 两腿都断言为给定状态。
fn assert_legs(relay: &AttachedPacketRelay, want: RelayLegState, ctx: &str) {
    assert_eq!(
        relay.leg_state(RelayDirection::Receive),
        want,
        "{ctx}: receive 腿状态"
    );
    assert_eq!(
        relay.leg_state(RelayDirection::Send),
        want,
        "{ctx}: send 腿状态"
    );
}

// Kills 'stopped relay cannot be re-armed for the next connection'.
#[test]
fn reset_legs_rearms_a_stopped_relay_for_the_next_connection() {
    let mut relay = attach_relay(30);
    start_both(&mut relay);

    // Stop：两腿 Terminal，running proof 撤销。
    relay.stop();
    assert_legs(&relay, RelayLegState::Terminal, "Stop 后");
    assert!(!relay.running_proof(), "Stop 后 running proof 必须失效");

    // Re-arm：Terminal → Attached，但尚未运行（running proof 仍 false）。
    relay.reset_legs();
    assert_legs(&relay, RelayLegState::Attached, "reset_legs 后");
    assert!(
        !relay.running_proof(),
        "re-armed 但未 start 前 running proof 不成立"
    );

    // 二次连接：两腿重新启动 → running proof 回到 true。
    start_both(&mut relay);
}

// Kills 'reset disturbs a fresh or running relay'.
#[test]
fn reset_legs_leaves_non_terminal_legs_untouched() {
    // 全新 relay（两腿 Attached）：reset 是无操作。
    let mut relay = attach_relay(31);
    relay.reset_legs();
    assert_legs(&relay, RelayLegState::Attached, "fresh reset");

    // 运行中 relay（两腿 Running）：reset 不得打断 Running。
    start_both(&mut relay);
    relay.reset_legs();
    assert!(relay.running_proof(), "reset 不得打断运行中的 running proof");
    assert_legs(&relay, RelayLegState::Running, "running reset");
}

// Kills 'terminal via on_terminal cannot be re-armed'.
#[test]
fn reset_legs_rearms_after_any_terminal_source() {
    for (source, conn, label) in [
        (RelayTerminalSource::StreamEof, 41, "stream EOF"),
        (RelayTerminalSource::TaskPanic, 42, "relay task panic"),
        (RelayTerminalSource::NativeReadTerminal, 43, "native read terminal"),
        (RelayTerminalSource::Stop, 44, "explicit Stop"),
    ] {
        let mut relay = attach_relay(conn);
        start_both(&mut relay);
        relay.on_terminal(source);
        assert_legs(&relay, RelayLegState::Terminal, label);
        assert!(!relay.running_proof(), "{label} 必须使 running proof 失效");

        relay.reset_legs();
        start_both(&mut relay);
    }
}

