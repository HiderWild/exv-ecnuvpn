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
};

// ProtocolTransport is the narrow boundary between the deterministic protocol
// session state machine and an eventual production TLS/CSTP transport.
class ProtocolTransport {
public:
  virtual ~ProtocolTransport() = default;

  virtual AuthResult authenticate(const ProtocolSessionOptions &options) = 0;
  virtual ValidationResult connect_cstp(const std::string &cookie,
                                        TunnelMetadata *metadata) = 0;
  virtual ValidationResult
  exchange_packet(const std::vector<std::uint8_t> &packet,
                  std::vector<std::uint8_t> *response_packet) = 0;

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
  ValidationResult reconnect(PacketDevice *device, EventSink *events,
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
