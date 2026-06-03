#pragma once

#include "vpn_engine/event_sink.hpp"
#include "vpn_engine/packet_device.hpp"
#include "vpn_engine/protocol/auth.hpp"
#include "vpn_engine/protocol/session.hpp"
#include "vpn_engine/session_state.hpp"

#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <mutex>
#include <string>
#include <vector>

namespace ecnuvpn {
namespace tests {
namespace support {

struct FakeAnyConnectCredentials {
  std::string username = "alice";
  std::string password = "test-mock-password-placeholder";
};

struct FakeAnyConnectServerOptions {
  FakeAnyConnectServerOptions();

  FakeAnyConnectCredentials expected_credentials;
  std::string session_cookie = "webvpn_session=FAKE_COOKIE";
  vpn_engine::TunnelMetadata tunnel_metadata;

  // If non-zero, the fake closes the transport before echoing this data frame
  // number on the current transport.
  std::size_t close_on_data_frame_number = 0;
  bool close_only_once = false;
};

class FakeAnyConnectServer {
public:
  explicit FakeAnyConnectServer(
      FakeAnyConnectServerOptions options = FakeAnyConnectServerOptions());

  vpn_engine::protocol::AuthResult
  password_authenticate(const FakeAnyConnectCredentials &credentials);

  vpn_engine::ValidationResult
  connect_cstp(const std::string &cookie,
               vpn_engine::TunnelMetadata *metadata);

  // Full-duplex data plane: the fake echoes each outbound data packet back as
  // an inbound data frame. send_packet enqueues the echo; receive_frame blocks
  // until an echo is available or the transport is closed.
  vpn_engine::ValidationResult
  send_packet(const std::vector<std::uint8_t> &packet);
  vpn_engine::ValidationResult
  send_control(vpn_engine::protocol::InboundFrameKind kind);
  vpn_engine::ValidationResult
  receive_frame(vpn_engine::protocol::InboundFrame *out);

  // Close the data-plane transport: unblocks a pending receive_frame with a
  // transport_closed result. Mirrors the production transport disconnect().
  void close_transport();

  void reset_transport();

  bool closed() const;
  int auth_attempts() const;
  int cstp_connects() const;
  int data_frames_received() const;

private:
  FakeAnyConnectServerOptions options_;
  mutable std::mutex mu_;
  std::condition_variable cv_;
  std::deque<std::vector<std::uint8_t>> echo_queue_;
  bool closed_ = false;
  bool close_triggered_ = false;
  int auth_attempts_ = 0;
  int cstp_connects_ = 0;
  int data_frames_received_ = 0;
  std::size_t data_frames_on_transport_ = 0;
};

class RecordingEventSink final : public vpn_engine::EventSink {
public:
  void emit(const vpn_engine::VpnEngineEvent &event) override;

  const std::vector<vpn_engine::VpnEngineEvent> &events() const;
  void clear();

private:
  std::vector<vpn_engine::VpnEngineEvent> events_;
};

class ScriptedPacketDevice final : public vpn_engine::PacketDevice {
public:
  explicit ScriptedPacketDevice(
      std::vector<std::vector<std::uint8_t>> packets = {});

  vpn_engine::ValidationResult
  open(const vpn_engine::TunnelMetadata &metadata) override;
  vpn_engine::ValidationResult
  read_packet(std::vector<std::uint8_t> *packet) override;
  vpn_engine::ValidationResult
  write_packet(const std::vector<std::uint8_t> &packet) override;
  void close() override;

  const std::vector<std::vector<std::uint8_t>> &written_packets() const;
  const vpn_engine::TunnelMetadata &last_open_metadata() const;
  int open_count() const;
  int close_count() const;
  bool is_open() const;

private:
  mutable std::mutex device_mu_;
  std::deque<std::vector<std::uint8_t>> packets_;
  std::vector<std::vector<std::uint8_t>> written_packets_;
  vpn_engine::TunnelMetadata last_open_metadata_;
  bool open_ = false;
  int open_count_ = 0;
  int close_count_ = 0;
};

struct FakeAnyConnectRunOptions {
  FakeAnyConnectCredentials credentials;
  bool auto_reconnect = false;
  int max_reconnects = 0;
};

struct FakeAnyConnectRunResult {
  vpn_engine::ValidationResult result;
  vpn_engine::SessionState state;
  int reconnects = 0;
};

FakeAnyConnectRunResult
run_fake_anyconnect_session(FakeAnyConnectServer &server,
                            vpn_engine::PacketDevice &device,
                            vpn_engine::EventSink &events,
                            const FakeAnyConnectRunOptions &options);

} // namespace support
} // namespace tests
} // namespace ecnuvpn
