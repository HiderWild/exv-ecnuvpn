#pragma once

#include "vpn_engine/engine.hpp"
#include "vpn_engine/event_sink.hpp"
#include "vpn_engine/packet_device.hpp"
#include "vpn_engine/protocol/auth.hpp"
#include "vpn_engine/protocol/url.hpp"
#include "vpn_engine/session_state.hpp"

#include <atomic>
#include <cstdint>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

class CancellationToken {
public:
  virtual ~CancellationToken() = default;
  virtual bool is_cancelled() const = 0;
};

struct ProtocolSessionOptions {
  ParsedVpnUrl server;
  std::string username;
  std::string password;
  std::string useragent;
  bool disable_dtls = true;
  bool auto_reconnect = false;
  int max_reconnects = 1;
  int packet_loop_no_data_poll_limit = 1000;

  // Safe tunnel MTU applied when the gateway-negotiated MTU is missing or out
  // of the supported [576, 1500] range. Defaults to the AnyConnect-typical
  // 1290; the engine overrides it from configuration.
  int mtu_fallback = 1290;

  // Liveness servicing (DPD / keepalive). These are expressed in consecutive
  // idle outbound-poll counts (one idle poll is ~1ms). 0 disables that timer.
  // Responding to an inbound DPD request is always on and not gated by these.
  int keepalive_idle_poll_interval = 0;
  int dpd_idle_poll_interval = 0;
  int dead_peer_poll_budget = 0;
};

// Classification of a single CSTP frame received from the gateway. This is the
// public CSTP/AnyConnect record taxonomy used by the full-duplex data plane.
enum class InboundFrameKind {
  none,
  data,
  dpd_request,
  dpd_response,
  keepalive,
  disconnect,
};

struct InboundFrame {
  InboundFrameKind kind = InboundFrameKind::none;
  std::vector<std::uint8_t> payload;
};

// ProtocolTransport is the narrow boundary between the deterministic protocol
// session state machine and the production TLS/CSTP transport. The data plane
// is full-duplex: outbound IP packets are framed and written independently of
// inbound CSTP frames, which are read and classified independently.
class ProtocolTransport {
public:
  virtual ~ProtocolTransport() = default;

  virtual AuthResult authenticate(const ProtocolSessionOptions &options) = 0;
  virtual ValidationResult connect_cstp(const std::string &cookie,
                                        TunnelMetadata *metadata) = 0;

  // Outbound data path: frame a single IP packet as a CSTP data record and
  // write it to the stream. Must be safe to call concurrently with
  // receive_frame (which only reads from the stream).
  virtual ValidationResult
  send_packet(const std::vector<std::uint8_t> &packet) = 0;

  // Outbound control path: send a control record (e.g. dpd_response,
  // keepalive). Must be safe to call concurrently with receive_frame.
  virtual ValidationResult send_control(InboundFrameKind kind) = 0;

  // Inbound path: block until the next complete CSTP frame is available,
  // decode and classify it. Returns a transport_closed error on peer EOF.
  virtual ValidationResult receive_frame(InboundFrame *out) = 0;

  virtual void disconnect() = 0;
  virtual void reset_for_reconnect() = 0;
};

class ProtocolSession {
public:
  ProtocolSession(ProtocolSessionOptions options, ProtocolTransport *transport);

  ValidationResult authenticate();
  ValidationResult connect_cstp(TunnelMetadata *metadata);
  ValidationResult run_packet_loop(PacketDevice *device, EventSink *events,
                                   CancellationToken *cancel);
  void disconnect();

  const SessionState &state() const;
  int reconnect_attempts() const;

private:
  // Outcome of a single full-duplex forwarding session (between connect and a
  // disconnect/cancel/error). Distinguishes a clean cancel/stop from a fatal
  // transport/device error that may warrant a reconnect.
  struct ForwardingOutcome {
    ValidationResult result;
    bool cancelled = false;
  };

  ValidationResult reconnect(PacketDevice *device, EventSink *events,
                             CancellationToken *cancel);
  ForwardingOutcome run_forwarding(PacketDevice *device, EventSink *events,
                                   CancellationToken *cancel);
  ValidationResult stop_cancelled(PacketDevice *device, EventSink *events);
  bool cancellation_requested(const CancellationToken *cancel) const;

  ProtocolSessionOptions options_;
  ProtocolTransport *transport_ = nullptr;
  SessionState state_;
  std::string cookie_;
  TunnelMetadata metadata_;
  bool authenticated_ = false;
  bool cstp_connected_ = false;
  int reconnect_attempts_ = 0;
  std::atomic<bool> disconnect_requested_{false};
  PacketDevice *current_device_ = nullptr;
};

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
